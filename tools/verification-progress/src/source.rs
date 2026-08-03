use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context as _, Result};
use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use verus_syn::parse::Parser;
use verus_syn::punctuated::Punctuated;
use verus_syn::spanned::Spanned;
use verus_syn::visit::Visit;
use verus_syn::{
    Attribute, Block, Expr, ExprCall, ExprForLoop, ExprLoop, ExprMacro, ExprWhile, FnMode,
    ImplItemFn, ItemConst, ItemEnum, ItemFn, ItemImpl, ItemMacro, ItemMod, ItemStatic, ItemStruct,
    ItemTrait, ItemType, ItemUse, Lit, Local, Meta, Publish, Signature, StmtMacro, Token,
    TraitItemFn,
};

use crate::model::{FileReport, Metrics};

#[derive(Debug, Clone, Default)]
pub struct CfgSet {
    flags: HashSet<String>,
    values: HashMap<String, HashSet<String>>,
}

impl CfgSet {
    pub fn from_rustc(workspace_root: &Path, target_triple: &str) -> Result<Self> {
        let output = Command::new("rustc")
            .args(["--print", "cfg", "--target", target_triple])
            .current_dir(workspace_root)
            .output()
            .with_context(|| format!("failed to query rustc cfg for `{target_triple}`"))?;
        if !output.status.success() {
            bail!(
                "`rustc --print cfg --target {target_triple}` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let mut cfg = Self::default();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((key, value)) = line.split_once('=') {
                cfg.values
                    .entry(key.to_string())
                    .or_default()
                    .insert(value.trim_matches('"').to_string());
            } else if !line.is_empty() {
                cfg.flags.insert(line.to_string());
            }
        }
        // These are enabled by rust_verify for the verification pass.
        cfg.flags.extend(
            ["verus_only", "verus_keep_ghost", "verus_keep_ghost_body"]
                .into_iter()
                .map(str::to_string),
        );
        Ok(cfg)
    }

    pub fn with_features(mut self, features: impl IntoIterator<Item = String>) -> Self {
        self.values
            .entry("feature".to_string())
            .or_default()
            .extend(features);
        self
    }

    #[cfg(test)]
    fn test_default() -> Self {
        let mut cfg = Self::default();
        cfg.flags.extend(
            ["verus_only", "verus_keep_ghost", "verus_keep_ghost_body"]
                .into_iter()
                .map(str::to_string),
        );
        cfg.values
            .entry("target_arch".to_string())
            .or_default()
            .insert("x86_64".to_string());
        cfg
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LineTag {
    Trusted,
    Spec,
    Proof,
    Exec,
    Directives,
    Definitions,
    Comment,
    Layout,
}

impl LineTag {
    fn name(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Spec => "spec",
            Self::Proof => "proof",
            Self::Exec => "exec",
            Self::Directives => "directives",
            Self::Definitions => "definitions",
            Self::Comment => "comment",
            Self::Layout => "layout",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct AttrFlags {
    verify: bool,
    trusted: bool,
    trusted_markers: u64,
    external_body: bool,
    external_body_markers: u64,
    external: bool,
    external_markers: u64,
    external_specification: bool,
    external_specification_markers: u64,
    verus_spec: bool,
    dual_spec: bool,
    when_used_as_spec: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Context {
    verified: bool,
    trusted: bool,
    external: bool,
}

impl Context {
    fn enter(self, attrs: AttrFlags) -> Self {
        Self {
            verified: (self.verified || attrs.verify || attrs.verus_spec || attrs.external_body)
                && !attrs.external,
            trusted: self.trusted
                || attrs.trusted
                || attrs.external_body
                || attrs.external_specification,
            external: self.external || attrs.external,
        }
    }
}

pub fn analyze_file(
    absolute_path: &Path,
    display_path: String,
    subsystem: String,
    cfg: &CfgSet,
) -> Result<FileReport> {
    let source = std::fs::read_to_string(absolute_path)
        .with_context(|| format!("failed to read {}", absolute_path.display()))?;
    let file = verus_syn::parse_file(&source)
        .with_context(|| format!("failed to parse {} with verus_syn", absolute_path.display()))?;

    let line_count = source.lines().count();
    let mut analyzer = Analyzer {
        cfg,
        context: Context::default(),
        metrics: Metrics::default(),
        line_tags: vec![BTreeSet::new(); line_count],
    };
    analyzer.visit_file(&file);
    analyzer.metrics.lines = finalize_lines(&source, analyzer.line_tags);
    let physical_lines = analyzer.metrics.lines.total;

    Ok(FileReport {
        path: display_path,
        subsystem,
        physical_lines,
        metrics: analyzer.metrics,
    })
}

struct Analyzer<'a> {
    cfg: &'a CfgSet,
    context: Context,
    metrics: Metrics,
    line_tags: Vec<BTreeSet<LineTag>>,
}

impl Analyzer<'_> {
    fn mark(&mut self, spanned: &impl Spanned, tag: LineTag) {
        let span = spanned.span();
        let start = span.start().line.saturating_sub(1);
        let end = span.end().line.saturating_sub(1);
        if self.line_tags.is_empty() || start >= self.line_tags.len() {
            return;
        }
        for line in start..=end.min(self.line_tags.len() - 1) {
            self.line_tags[line].insert(tag);
        }
    }

    fn mark_attrs(&mut self, attrs: &[Attribute]) {
        for attr in attrs {
            let flags = classify_attrs(std::slice::from_ref(attr), self.cfg);
            let tag = if flags.external
                || flags.external_body
                || flags.external_specification
                || flags.trusted
            {
                LineTag::Trusted
            } else if flags.verus_spec || flags.dual_spec || flags.when_used_as_spec {
                LineTag::Spec
            } else {
                LineTag::Directives
            };
            self.mark(attr, tag);
        }
    }

    fn account_attr_debt(&mut self, flags: AttrFlags) {
        self.metrics.trust_debt.external_body += flags.external_body_markers;
        self.metrics.trust_debt.external += flags.external_markers;
        self.metrics.trust_debt.external_specifications += flags.external_specification_markers;
        self.metrics.trust_debt.trusted += flags.trusted_markers;
    }

    fn with_context(&mut self, flags: AttrFlags, f: impl FnOnce(&mut Self)) {
        let previous = self.context;
        self.context = self.context.enter(flags);
        f(self);
        self.context = previous;
    }

    fn handle_function(
        &mut self,
        attrs: &[Attribute],
        sig: &Signature,
        body: Option<&Block>,
        whole: &impl Spanned,
    ) {
        if !attrs_enabled(attrs, self.cfg) {
            return;
        }
        let flags = classify_attrs(attrs, self.cfg);
        self.account_attr_debt(flags);
        self.mark_attrs(attrs);
        let context = self.context.enter(flags);
        let has_body = body.is_some();
        let is_unsafe = sig.unsafety.is_some();
        let has_contract = flags.verus_spec || flags.dual_spec || signature_has_contract(sig);

        match &sig.mode {
            FnMode::Spec(_) | FnMode::SpecChecked(_) => {
                if matches!(sig.publish, Publish::Uninterp(_)) || !has_body {
                    self.metrics.spec.uninterpreted += 1;
                } else {
                    self.metrics.spec.defined += 1;
                }
                let tag = if context.external || context.trusted {
                    LineTag::Trusted
                } else {
                    LineTag::Spec
                };
                self.mark(sig, tag);
                if let Some(body) = body {
                    self.mark(body, tag);
                }
            }
            FnMode::ProofAxiom(_) => {
                self.metrics.proof.axioms += 1;
                self.metrics.trust_debt.axioms += 1;
                self.mark(whole, LineTag::Trusted);
            }
            FnMode::Proof(_) => {
                if context.external {
                    self.metrics.proof.external += 1;
                    self.mark(whole, LineTag::Trusted);
                } else if context.trusted {
                    self.metrics.proof.trusted += 1;
                    self.mark(sig, LineTag::Proof);
                    if let Some(body) = body {
                        self.mark(body, LineTag::Trusted);
                    }
                } else if !has_body {
                    self.metrics.proof.declarations += 1;
                    self.mark(sig, LineTag::Proof);
                } else if context.verified {
                    self.metrics.proof.checked_candidates += 1;
                    self.mark(whole, LineTag::Proof);
                } else {
                    self.metrics.proof.unverified += 1;
                    self.mark(whole, LineTag::Proof);
                }
            }
            FnMode::Exec(_) | FnMode::Default => {
                if !has_body {
                    // Trait and extern declarations are deliberately outside the denominator.
                    self.mark(sig, LineTag::Exec);
                    return;
                }
                let bucket = if context.external {
                    self.metrics.exec.unverified += 1;
                    2
                } else if context.trusted {
                    self.metrics.exec.trusted += 1;
                    1
                } else if context.verified {
                    self.metrics.exec.checked_candidates += 1;
                    0
                } else {
                    self.metrics.exec.unverified += 1;
                    2
                };
                if has_contract {
                    self.metrics.exec.specified += 1;
                }
                if is_unsafe {
                    match bucket {
                        0 => self.metrics.exec.unsafe_functions.checked_candidates += 1,
                        1 => self.metrics.exec.unsafe_functions.trusted += 1,
                        _ => self.metrics.exec.unsafe_functions.unverified += 1,
                    }
                }
                self.mark(sig, LineTag::Exec);
                if let Some(body) = body {
                    self.mark(
                        body,
                        if context.external || context.trusted {
                            LineTag::Trusted
                        } else {
                            LineTag::Exec
                        },
                    );
                }
            }
        }

        mark_signature_contracts(self, sig);
        if let Some(body) = body {
            self.with_context(flags, |this| verus_syn::visit::visit_block(this, body));
        }
    }

    fn handle_container(
        &mut self,
        attrs: &[Attribute],
        whole: &impl Spanned,
        base_tag: LineTag,
        visit: impl FnOnce(&mut Self),
    ) {
        if !attrs_enabled(attrs, self.cfg) {
            return;
        }
        let flags = classify_attrs(attrs, self.cfg);
        self.account_attr_debt(flags);
        self.mark_attrs(attrs);
        self.mark(
            whole,
            if flags.external
                || flags.external_body
                || flags.external_specification
                || flags.trusted
            {
                LineTag::Trusted
            } else {
                base_tag
            },
        );
        self.with_context(flags, visit);
    }

    fn count_admit(&mut self, spanned: &impl Spanned) {
        self.metrics.trust_debt.admits += 1;
        self.mark(spanned, LineTag::Trusted);
    }

    /// Verus constructs nested in domain-specific macro invocations are not exposed by
    /// `verus_syn::Visit`. Scan only macro token bodies that we could not parse as syntax.
    fn scan_unparsed_macro_tokens(&mut self, tokens: TokenStream) {
        let trees = tokens.into_iter().collect::<Vec<_>>();
        let mut index = 0;
        while index < trees.len() {
            match &trees[index] {
                TokenTree::Ident(ident)
                    if matches!(ident.to_string().as_str(), "assume" | "admit")
                        && matches!(
                            trees.get(index + 1),
                            Some(TokenTree::Group(group))
                                if group.delimiter() == Delimiter::Parenthesis
                        ) =>
                {
                    if ident == "assume" {
                        self.metrics.trust_debt.assumes += 1;
                    } else {
                        self.metrics.trust_debt.admits += 1;
                    }
                    self.mark(ident, LineTag::Trusted);
                    if let Some(TokenTree::Group(group)) = trees.get(index + 1) {
                        self.mark(group, LineTag::Trusted);
                        self.scan_unparsed_macro_tokens(group.stream());
                    }
                    index += 2;
                    continue;
                }
                TokenTree::Group(group) => {
                    self.scan_unparsed_macro_tokens(group.stream());
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn mark_macro_semantics(
        &mut self,
        name: Option<&str>,
        tokens: TokenStream,
        whole: &impl Spanned,
    ) {
        match name {
            Some("tokenized_state_machine")
            | Some("state_machine")
            | Some("struct_with_invariants") => self.mark(whole, LineTag::Spec),
            Some("proof")
            | Some("proof_decl")
            | Some("proof_with")
            | Some("open_atomic_invariant")
            | Some("open_local_invariant")
            | Some("open_atomic_update")
            | Some("try_open_atomic_update") => self.mark(whole, LineTag::Proof),
            Some("atomic_with_ghost") | Some("my_atomic_with_ghost") => {
                let mut proof_part = false;
                for token in tokens {
                    if matches!(&token, TokenTree::Punct(punct) if punct.as_char() == ';') {
                        proof_part = true;
                    }
                    self.mark(
                        &token,
                        if proof_part {
                            LineTag::Proof
                        } else {
                            LineTag::Exec
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for Analyzer<'_> {
    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        let body = item.semi_token.is_none().then_some(item.block.as_ref());
        self.handle_function(&item.attrs, &item.sig, body, item);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        let body = item.semi_token.is_none().then_some(&item.block);
        self.handle_function(&item.attrs, &item.sig, body, item);
    }

    fn visit_trait_item_fn(&mut self, item: &'ast TraitItemFn) {
        self.handle_function(&item.attrs, &item.sig, item.default.as_ref(), item);
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        self.handle_container(&item.attrs, item, LineTag::Definitions, |this| {
            for impl_item in &item.items {
                this.visit_impl_item(impl_item);
            }
        });
    }

    fn visit_item_trait(&mut self, item: &'ast ItemTrait) {
        self.handle_container(&item.attrs, item, LineTag::Definitions, |this| {
            for trait_item in &item.items {
                this.visit_trait_item(trait_item);
            }
        });
    }

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        self.handle_container(&item.attrs, item, LineTag::Directives, |this| {
            if let Some((_, items)) = &item.content {
                for nested in items {
                    this.visit_item(nested);
                }
            }
        });
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        let tag = data_mode_tag(&item.mode);
        self.handle_container(&item.attrs, item, tag, |_| {});
    }

    fn visit_item_enum(&mut self, item: &'ast ItemEnum) {
        let tag = data_mode_tag(&item.mode);
        self.handle_container(&item.attrs, item, tag, |_| {});
    }

    fn visit_item_const(&mut self, item: &'ast ItemConst) {
        let tag = fn_mode_tag(&item.mode);
        self.handle_container(&item.attrs, item, tag, |this| {
            if let Some(expr) = &item.expr {
                this.visit_expr(expr);
            }
        });
    }

    fn visit_item_static(&mut self, item: &'ast ItemStatic) {
        let tag = fn_mode_tag(&item.mode);
        self.handle_container(&item.attrs, item, tag, |this| {
            if let Some(expr) = &item.expr {
                this.visit_expr(expr);
            }
        });
    }

    fn visit_item_type(&mut self, item: &'ast ItemType) {
        self.handle_container(&item.attrs, item, LineTag::Definitions, |_| {});
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        if attrs_enabled(&item.attrs, self.cfg) {
            self.mark(item, LineTag::Directives);
        }
    }

    fn visit_item_macro(&mut self, item: &'ast ItemMacro) {
        if !attrs_enabled(&item.attrs, self.cfg) {
            return;
        }
        let flags = classify_attrs(&item.attrs, self.cfg);
        self.account_attr_debt(flags);
        self.mark_attrs(&item.attrs);
        let name = item.mac.path.segments.last().map(|s| s.ident.to_string());
        self.mark_macro_semantics(name.as_deref(), item.mac.tokens.clone(), item);
        match name.as_deref() {
            Some("verus") => {
                let tokens = verus_syn::rejoin_tokens(item.mac.tokens.clone());
                match verus_syn::parse2::<verus_syn::File>(tokens.clone()) {
                    Ok(file) => self.with_context(
                        AttrFlags {
                            verify: true,
                            ..flags
                        },
                        |this| this.visit_file(&file),
                    ),
                    Err(_) => self.scan_unparsed_macro_tokens(tokens),
                }
            }
            Some("cfg_if") => {
                self.mark(item, LineTag::Directives);
                if let Ok(Some(tokens)) = select_cfg_if_branch(item.mac.tokens.clone(), self.cfg) {
                    match verus_syn::parse2::<verus_syn::File>(tokens.clone()) {
                        Ok(file) => self.with_context(flags, |this| this.visit_file(&file)),
                        Err(_) => self.scan_unparsed_macro_tokens(tokens),
                    }
                }
            }
            Some("macro_rules") => {
                self.mark(item, LineTag::Definitions);
                self.scan_unparsed_macro_tokens(item.mac.tokens.clone());
            }
            _ => {
                self.mark(item, LineTag::Exec);
                self.scan_unparsed_macro_tokens(item.mac.tokens.clone());
            }
        }
    }

    fn visit_assume(&mut self, assume: &'ast verus_syn::Assume) {
        self.metrics.trust_debt.assumes += 1;
        self.mark(assume, LineTag::Trusted);
        verus_syn::visit::visit_assume(self, assume);
    }

    fn visit_assert(&mut self, assert: &'ast verus_syn::Assert) {
        self.mark(assert, LineTag::Proof);
        verus_syn::visit::visit_assert(self, assert);
    }

    fn visit_assert_forall(&mut self, assert: &'ast verus_syn::AssertForall) {
        self.mark(assert, LineTag::Proof);
        verus_syn::visit::visit_assert_forall(self, assert);
    }

    fn visit_expr_call(&mut self, expr: &'ast ExprCall) {
        match expr_path_last(expr.func.as_ref()).as_deref() {
            Some("admit") => self.count_admit(expr),
            Some("Ghost") | Some("Tracked") => self.mark(expr, LineTag::Proof),
            _ => {}
        }
        verus_syn::visit::visit_expr_call(self, expr);
    }

    fn visit_expr_macro(&mut self, expr: &'ast ExprMacro) {
        let name = expr.mac.path.segments.last().map(|s| s.ident.to_string());
        self.mark_macro_semantics(name.as_deref(), expr.mac.tokens.clone(), expr);
        match name.as_deref() {
            Some("admit") => self.count_admit(expr),
            Some("cfg_if") => {
                self.mark(expr, LineTag::Directives);
                if let Ok(Some(tokens)) = select_cfg_if_branch(expr.mac.tokens.clone(), self.cfg) {
                    let group = Group::new(Delimiter::Brace, tokens.clone());
                    let stream = TokenStream::from(TokenTree::Group(group));
                    if let Ok(block) = verus_syn::parse2::<Block>(stream) {
                        verus_syn::visit::visit_block(self, &block);
                    } else {
                        self.scan_unparsed_macro_tokens(tokens);
                    }
                }
                return;
            }
            Some("proof") => {
                self.mark(expr, LineTag::Proof);
                let group = Group::new(Delimiter::Brace, expr.mac.tokens.clone());
                let stream = TokenStream::from(TokenTree::Group(group));
                if let Ok(block) = verus_syn::parse2::<Block>(stream) {
                    verus_syn::visit::visit_block(self, &block);
                } else {
                    self.scan_unparsed_macro_tokens(expr.mac.tokens.clone());
                }
                return;
            }
            _ => self.scan_unparsed_macro_tokens(expr.mac.tokens.clone()),
        }
        verus_syn::visit::visit_expr_macro(self, expr);
    }

    fn visit_stmt_macro(&mut self, statement: &'ast StmtMacro) {
        if !attrs_enabled(&statement.attrs, self.cfg) {
            return;
        }
        let flags = classify_attrs(&statement.attrs, self.cfg);
        self.account_attr_debt(flags);
        self.mark_attrs(&statement.attrs);
        let name = statement
            .mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string());
        self.mark_macro_semantics(name.as_deref(), statement.mac.tokens.clone(), statement);
        match name.as_deref() {
            Some("admit") => self.count_admit(statement),
            Some("assume") => {
                self.metrics.trust_debt.assumes += 1;
                self.mark(statement, LineTag::Trusted);
            }
            Some("cfg_if") => {
                self.mark(statement, LineTag::Directives);
                if let Ok(Some(tokens)) =
                    select_cfg_if_branch(statement.mac.tokens.clone(), self.cfg)
                {
                    let group = Group::new(Delimiter::Brace, tokens.clone());
                    let stream = TokenStream::from(TokenTree::Group(group));
                    if let Ok(block) = verus_syn::parse2::<Block>(stream) {
                        verus_syn::visit::visit_block(self, &block);
                    } else {
                        self.scan_unparsed_macro_tokens(tokens);
                    }
                }
            }
            Some("proof") => {
                self.mark(statement, LineTag::Proof);
                let group = Group::new(Delimiter::Brace, statement.mac.tokens.clone());
                let stream = TokenStream::from(TokenTree::Group(group));
                if let Ok(block) = verus_syn::parse2::<Block>(stream) {
                    verus_syn::visit::visit_block(self, &block);
                } else {
                    self.scan_unparsed_macro_tokens(statement.mac.tokens.clone());
                }
            }
            _ => self.scan_unparsed_macro_tokens(statement.mac.tokens.clone()),
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let Expr::Unary(unary) = expr {
            if matches!(unary.op, verus_syn::UnOp::Proof(_)) {
                self.mark(expr, LineTag::Proof);
            }
        }
        verus_syn::visit::visit_expr(self, expr);
    }

    fn visit_local(&mut self, local: &'ast Local) {
        if local.ghost.is_some() || local.tracked.is_some() {
            self.mark(local, LineTag::Proof);
        }
        verus_syn::visit::visit_local(self, local);
    }

    fn visit_expr_loop(&mut self, expr: &'ast ExprLoop) {
        if let Some(value) = &expr.decreases {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.invariant_except_break {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.invariant {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.invariant_ensures {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.ensures {
            self.mark(value, LineTag::Proof);
        }
        verus_syn::visit::visit_expr_loop(self, expr);
    }

    fn visit_expr_while(&mut self, expr: &'ast ExprWhile) {
        if let Some(value) = &expr.decreases {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.invariant_except_break {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.invariant {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.invariant_ensures {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.ensures {
            self.mark(value, LineTag::Proof);
        }
        verus_syn::visit::visit_expr_while(self, expr);
    }

    fn visit_expr_for_loop(&mut self, expr: &'ast ExprForLoop) {
        if let Some(value) = &expr.decreases {
            self.mark(value, LineTag::Proof);
        }
        if let Some(value) = &expr.invariant {
            self.mark(value, LineTag::Proof);
        }
        verus_syn::visit::visit_expr_for_loop(self, expr);
    }
}

fn expr_path_last(expr: &Expr) -> Option<String> {
    if let Expr::Path(path) = expr {
        path.path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
    } else {
        None
    }
}

fn data_mode_tag(mode: &verus_syn::DataMode) -> LineTag {
    match mode {
        verus_syn::DataMode::Ghost(_) => LineTag::Spec,
        verus_syn::DataMode::Tracked(_) => LineTag::Proof,
        verus_syn::DataMode::Exec(_) | verus_syn::DataMode::Default => LineTag::Exec,
    }
}

fn fn_mode_tag(mode: &FnMode) -> LineTag {
    match mode {
        FnMode::Spec(_) | FnMode::SpecChecked(_) => LineTag::Spec,
        FnMode::Proof(_) | FnMode::ProofAxiom(_) => LineTag::Proof,
        FnMode::Exec(_) | FnMode::Default => LineTag::Exec,
    }
}

fn signature_has_contract(sig: &Signature) -> bool {
    sig.spec.requires.is_some()
        || sig.spec.recommends.is_some()
        || sig.spec.ensures.is_some()
        || sig.spec.default_ensures.is_some()
        || sig.spec.returns.is_some()
}

fn select_cfg_if_branch(
    tokens: TokenStream,
    cfg: &CfgSet,
) -> std::result::Result<Option<TokenStream>, ()> {
    let trees = tokens.into_iter().collect::<Vec<_>>();
    let mut cursor = 0;
    take_ident(&trees, &mut cursor, "if")?;
    let mut selected = None;

    loop {
        take_punct(&trees, &mut cursor, '#')?;
        let condition_group = take_group(&trees, &mut cursor, Delimiter::Bracket)?;
        let body = take_group(&trees, &mut cursor, Delimiter::Brace)?;
        if selected.is_none() && eval_cfg_attribute_group(condition_group, cfg)? {
            selected = Some(body.stream());
        }

        if cursor == trees.len() {
            break;
        }
        take_ident(&trees, &mut cursor, "else")?;
        if token_is_ident(trees.get(cursor), "if") {
            cursor += 1;
            continue;
        }

        let fallback = take_group(&trees, &mut cursor, Delimiter::Brace)?;
        if selected.is_none() {
            selected = Some(fallback.stream());
        }
        if cursor != trees.len() {
            return Err(());
        }
        break;
    }
    Ok(selected)
}

fn eval_cfg_attribute_group(group: &Group, cfg: &CfgSet) -> std::result::Result<bool, ()> {
    let meta = verus_syn::parse2::<Meta>(group.stream()).map_err(|_| ())?;
    let Meta::List(list) = meta else {
        return Err(());
    };
    if !path_ends_with(&list.path, "cfg") {
        return Err(());
    }
    let conditions = Punctuated::<Meta, Token![,]>::parse_terminated
        .parse2(list.tokens)
        .map_err(|_| ())?;
    if conditions.len() != 1 {
        return Err(());
    }
    Ok(eval_cfg(&conditions[0], cfg))
}

fn token_is_ident(token: Option<&TokenTree>, expected: &str) -> bool {
    matches!(token, Some(TokenTree::Ident(ident)) if ident == expected)
}

fn take_ident(
    trees: &[TokenTree],
    cursor: &mut usize,
    expected: &str,
) -> std::result::Result<(), ()> {
    if !token_is_ident(trees.get(*cursor), expected) {
        return Err(());
    }
    *cursor += 1;
    Ok(())
}

fn take_punct(
    trees: &[TokenTree],
    cursor: &mut usize,
    expected: char,
) -> std::result::Result<(), ()> {
    if !matches!(trees.get(*cursor), Some(TokenTree::Punct(punct)) if punct.as_char() == expected) {
        return Err(());
    }
    *cursor += 1;
    Ok(())
}

fn take_group<'a>(
    trees: &'a [TokenTree],
    cursor: &mut usize,
    delimiter: Delimiter,
) -> std::result::Result<&'a Group, ()> {
    let Some(TokenTree::Group(group)) = trees.get(*cursor) else {
        return Err(());
    };
    if group.delimiter() != delimiter {
        return Err(());
    }
    *cursor += 1;
    Ok(group)
}

fn mark_signature_contracts(analyzer: &mut Analyzer<'_>, sig: &Signature) {
    if let Some(value) = &sig.spec.requires {
        analyzer.mark(value, LineTag::Spec);
    }
    if let Some(value) = &sig.spec.recommends {
        analyzer.mark(value, LineTag::Spec);
    }
    if let Some(value) = &sig.spec.ensures {
        analyzer.mark(value, LineTag::Spec);
    }
    if let Some(value) = &sig.spec.default_ensures {
        analyzer.mark(value, LineTag::Spec);
    }
    if let Some(value) = &sig.spec.returns {
        analyzer.mark(value, LineTag::Spec);
    }
    if let Some(value) = &sig.spec.decreases {
        analyzer.mark(value, LineTag::Spec);
    }
    if let Some(value) = &sig.spec.atomic_spec {
        analyzer.mark(value, LineTag::Spec);
    }
}

fn attrs_enabled(attrs: &[Attribute], cfg: &CfgSet) -> bool {
    attrs.iter().all(|attr| {
        if path_ends_with(attr.path(), "cfg") {
            attr.parse_args::<Meta>()
                .map(|meta| eval_cfg(&meta, cfg))
                .unwrap_or(false)
        } else {
            true
        }
    })
}

fn classify_attrs(attrs: &[Attribute], cfg: &CfgSet) -> AttrFlags {
    let mut result = AttrFlags::default();
    for meta in effective_metas(attrs, cfg) {
        classify_meta(&meta, &mut result);
    }
    result
}

fn effective_metas(attrs: &[Attribute], cfg: &CfgSet) -> Vec<Meta> {
    let mut result = Vec::new();
    for attr in attrs {
        if path_ends_with(attr.path(), "cfg") {
            continue;
        }
        if path_ends_with(attr.path(), "cfg_attr") {
            let Ok(metas) = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            else {
                continue;
            };
            let mut iter = metas.into_iter();
            let Some(condition) = iter.next() else {
                continue;
            };
            if eval_cfg(&condition, cfg) {
                result.extend(iter);
            }
        } else {
            result.push(attr.meta.clone());
        }
    }
    result
}

fn classify_meta(meta: &Meta, flags: &mut AttrFlags) {
    let path = meta.path();
    let joined = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    let last = path
        .segments
        .last()
        .map(|segment| segment.ident.to_string());
    match joined.as_str() {
        "verifier::verify" => flags.verify = true,
        "verifier::external_body" => {
            flags.external_body = true;
            flags.external_body_markers += 1;
        }
        "verifier::external" => {
            flags.external = true;
            flags.external_markers += 1;
        }
        "verus::trusted" => {
            flags.trusted = true;
            flags.trusted_markers += 1;
        }
        "verifier::when_used_as_spec" => flags.when_used_as_spec = true,
        _ => {
            if last.as_deref() == Some("verus_spec") {
                flags.verus_spec = true;
                flags.verify = true;
            }
            if last.as_deref() == Some("verus_verify") {
                flags.verify = true;
                if let Meta::List(list) = meta {
                    if let Ok(args) =
                        Punctuated::<Meta, Token![,]>::parse_terminated.parse2(list.tokens.clone())
                    {
                        for arg in args {
                            let arg_name = arg
                                .path()
                                .segments
                                .last()
                                .map(|segment| segment.ident.to_string());
                            match arg_name.as_deref() {
                                Some("dual_spec") => flags.dual_spec = true,
                                Some("external_body") => {
                                    flags.external_body = true;
                                    flags.external_body_markers += 1;
                                }
                                Some("external") => {
                                    flags.external = true;
                                    flags.external_markers += 1;
                                }
                                Some(name) if name.starts_with("external_") => {
                                    flags.external_specification = true;
                                    flags.external_specification_markers += 1;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            if joined.starts_with("verifier::external_") && joined != "verifier::external_body" {
                flags.external_specification = true;
                flags.external_specification_markers += 1;
            }
        }
    }
}

fn eval_cfg(meta: &Meta, cfg: &CfgSet) -> bool {
    match meta {
        Meta::Path(path) => path
            .segments
            .last()
            .map(|segment| cfg.flags.contains(&segment.ident.to_string()))
            .unwrap_or(false),
        Meta::NameValue(name_value) => {
            let Some(key) = name_value.path.segments.last().map(|s| s.ident.to_string()) else {
                return false;
            };
            let Expr::Lit(expr_lit) = &name_value.value else {
                return false;
            };
            let Lit::Str(value) = &expr_lit.lit else {
                return false;
            };
            cfg.values
                .get(&key)
                .map(|values| values.contains(&value.value()))
                .unwrap_or(false)
        }
        Meta::List(list) => {
            let name = list.path.segments.last().map(|s| s.ident.to_string());
            let Ok(items) =
                Punctuated::<Meta, Token![,]>::parse_terminated.parse2(list.tokens.clone())
            else {
                return false;
            };
            match name.as_deref() {
                Some("all") => items.iter().all(|item| eval_cfg(item, cfg)),
                Some("any") => items.iter().any(|item| eval_cfg(item, cfg)),
                Some("not") => items.len() == 1 && !eval_cfg(&items[0], cfg),
                _ => false,
            }
        }
    }
}

fn path_ends_with(path: &verus_syn::Path, expected: &str) -> bool {
    path.segments
        .last()
        .map(|segment| segment.ident == expected)
        .unwrap_or(false)
}

fn finalize_lines(source: &str, mut tags: Vec<BTreeSet<LineTag>>) -> crate::model::LineMetrics {
    let mut in_block_comment = false;
    for (index, text) in source.lines().enumerate() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            if !tags[index].is_empty() {
                tags[index].clear();
                tags[index].insert(LineTag::Layout);
            }
            continue;
        }
        if !tags[index].is_empty()
            && trimmed
                .chars()
                .all(|character| matches!(character, '(' | ')' | '{' | '}' | '[' | ']'))
        {
            tags[index].clear();
            tags[index].insert(LineTag::Layout);
            continue;
        }
        let (comment_only, next_block_state) = comment_only_line(trimmed, in_block_comment);
        in_block_comment = next_block_state;
        if comment_only {
            tags[index].clear();
            tags[index].insert(LineTag::Comment);
        }
    }

    let mut metrics = crate::model::LineMetrics {
        total: tags.len() as u64,
        ..crate::model::LineMetrics::default()
    };
    for line_tags in tags {
        let key = if line_tags.is_empty() {
            "unaccounted".to_string()
        } else {
            line_tags
                .iter()
                .map(|tag| tag.name())
                .collect::<Vec<_>>()
                .join(",")
        };
        *metrics.raw_tags.entry(key).or_default() += 1;

        if line_tags.contains(&LineTag::Comment) {
            metrics.comments += 1;
        } else if line_tags.contains(&LineTag::Layout) {
            metrics.layout += 1;
        } else if line_tags.contains(&LineTag::Trusted) {
            metrics.trusted += 1;
        } else if line_tags.contains(&LineTag::Spec) {
            metrics.spec += 1;
        } else if line_tags.contains(&LineTag::Proof) {
            metrics.proof += 1;
        } else if line_tags.contains(&LineTag::Exec) {
            metrics.exec += 1;
        } else if line_tags.contains(&LineTag::Directives) {
            metrics.directives += 1;
        } else if line_tags.contains(&LineTag::Definitions) {
            metrics.definitions += 1;
        } else {
            metrics.unaccounted += 1;
        }
    }
    metrics
}

fn comment_only_line(line: &str, mut in_block: bool) -> (bool, bool) {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut saw_code = false;
    while index < bytes.len() {
        if in_block {
            if index + 1 < bytes.len() && bytes[index] == b'*' && bytes[index + 1] == b'/' {
                in_block = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            break;
        }
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            in_block = true;
            index += 2;
            continue;
        }
        if !bytes[index].is_ascii_whitespace() {
            saw_code = true;
        }
        index += 1;
    }
    (!saw_code, in_block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(source: &str) -> Metrics {
        let file = verus_syn::parse_file(source).unwrap();
        let mut analyzer = Analyzer {
            cfg: &CfgSet::test_default(),
            context: Context::default(),
            metrics: Metrics::default(),
            line_tags: vec![BTreeSet::new(); source.lines().count()],
        };
        analyzer.visit_file(&file);
        analyzer.metrics.lines = finalize_lines(source, analyzer.line_tags);
        analyzer.metrics
    }

    #[test]
    fn classifies_exec_modes_and_contracts() {
        let metrics = analyze(
            r#"
fn ordinary() {}

#[verus_verify]
impl Thing {
    #[verus_spec(ret => ensures ret > 0)]
    fn checked() -> u8 { 1 }

    #[verifier::external_body]
    unsafe fn trusted() {}
}

#[verus_verify(external)]
fn excluded() {}
"#,
        );
        assert_eq!(metrics.exec.checked_candidates, 1);
        assert_eq!(metrics.exec.trusted, 1);
        assert_eq!(metrics.exec.unverified, 2);
        assert_eq!(metrics.exec.specified, 1);
        assert_eq!(metrics.exec.unsafe_functions.trusted, 1);
        assert_eq!(metrics.trust_debt.external_body, 1);
        assert_eq!(metrics.trust_debt.external, 1);
    }

    #[test]
    fn classifies_verus_proof_spec_and_assumptions() {
        let metrics = analyze(
            r#"
verus! {
    exec fn checked() { assert(true); }
    proof fn lemma() { assume(true); admit(); }
    axiom fn trusted_lemma();
    uninterp spec fn model(x: int) -> int;
    spec fn defined(x: int) -> int { x }
}
"#,
        );
        assert_eq!(metrics.exec.checked_candidates, 1);
        assert_eq!(metrics.proof.checked_candidates, 1);
        assert_eq!(metrics.proof.axioms, 1);
        assert_eq!(metrics.spec.uninterpreted, 1);
        assert_eq!(metrics.spec.defined, 1);
        assert_eq!(metrics.trust_debt.assumes, 1);
        assert_eq!(metrics.trust_debt.admits, 1);
    }

    #[test]
    fn counts_each_trust_attribute_and_macro_nested_assumption() {
        let metrics = analyze(
            r#"
#[verus_verify]
#[verifier::external_body]
#[verifier::external_body]
fn trusted_twice() {}

#[verus_verify]
fn checked() {
    atomic_with_ghost!(value => {
        assume(true);
        admit();
    });
}
"#,
        );
        assert_eq!(metrics.exec.trusted, 1);
        assert_eq!(metrics.exec.checked_candidates, 1);
        assert_eq!(metrics.trust_debt.external_body, 2);
        assert_eq!(metrics.trust_debt.assumes, 1);
        assert_eq!(metrics.trust_debt.admits, 1);
    }

    #[test]
    fn follows_only_the_active_cfg_if_branch() {
        let metrics = analyze(
            r#"
cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        #[verus_verify]
        fn selected() {
            cfg_if! {
                if #[cfg(feature = "not_enabled")] {
                    admit();
                } else {
                    assume(true);
                }
            }
        }
    } else {
        fn inactive() {}
    }
}
"#,
        );
        assert_eq!(metrics.exec.checked_candidates, 1);
        assert_eq!(metrics.exec.unverified, 0);
        assert_eq!(metrics.trust_debt.assumes, 1);
        assert_eq!(metrics.trust_debt.admits, 0);
    }

    #[test]
    fn excludes_cfg_inactive_and_trait_declarations() {
        let metrics = analyze(
            r#"
#[cfg(ktest)]
#[verus_verify]
fn not_active() {}

#[verus_verify]
trait Demo {
    fn declaration();
    fn default_body() {}
    proof fn proof_declaration();
    proof fn proof_default() {}
}
"#,
        );
        assert_eq!(metrics.exec.checked_candidates, 1);
        assert_eq!(metrics.exec.unverified, 0);
        assert_eq!(metrics.proof.checked_candidates, 1);
        assert_eq!(metrics.proof.declarations, 1);
    }

    #[test]
    fn line_buckets_are_mutually_exclusive() {
        let metrics = analyze(
            r#"
// comment
#[verus_verify]
fn checked() {
    proof! { assert(true); }
}
"#,
        );
        let lines = metrics.lines;
        assert_eq!(
            lines.total,
            lines.trusted
                + lines.spec
                + lines.proof
                + lines.exec
                + lines.directives
                + lines.definitions
                + lines.comments
                + lines.layout
                + lines.unaccounted
        );
    }

    #[test]
    fn common_line_categories_match_native_line_count_fixture() {
        // Golden values from Verus line_count with:
        // --one-file --no-external-by-default --proofs-arent-trusted
        // --delimiters-are-layout
        let common = analyze(include_str!("../tests/fixtures/line_count_common.rs"));
        assert_eq!(common.lines.spec, 6);
        assert_eq!(common.lines.proof, 5);
        assert_eq!(common.lines.exec, 4);
        assert_eq!(common.lines.layout, 8);
        assert_eq!(common.lines.unaccounted, 4);
        assert_eq!(common.lines.total, 27);
    }

    #[test]
    fn vostd_extensions_match_golden() {
        let metrics = analyze(include_str!("../tests/fixtures/vostd_extensions.rs"));
        assert_eq!(metrics.exec.checked_candidates, 2);
        assert_eq!(metrics.exec.trusted, 1);
        assert_eq!(metrics.exec.unverified, 1);
        assert_eq!(metrics.exec.specified, 1);
        assert_eq!(metrics.exec.unsafe_functions.trusted, 1);
        assert_eq!(metrics.trust_debt.external_body, 2);
        assert_eq!(metrics.trust_debt.external, 1);
        assert_eq!(metrics.trust_debt.assumes, 1);
        assert_eq!(metrics.trust_debt.admits, 1);
        assert_eq!(metrics.lines.trusted, 7);
        assert_eq!(metrics.lines.spec, 1);
        assert_eq!(metrics.lines.proof, 0);
        assert_eq!(metrics.lines.exec, 5);
        assert_eq!(metrics.lines.directives, 3);
        assert_eq!(metrics.lines.definitions, 1);
        assert_eq!(metrics.lines.layout, 4);
        assert_eq!(metrics.lines.unaccounted, 5);
        assert_eq!(metrics.lines.total, 26);
    }
}
