use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, ensure, Context, Result};

use crate::model::{
    ArchitectureReport, Comparison, Metrics, PackageReport, ProgressReport, ProjectReport,
    SubsystemReport, VerificationStatus,
};
use crate::source::{analyze_file, CfgSet};
use crate::workspace::{
    active_files_from_dep_info, architecture_inventory, inventory_files, physical_lines,
    relative_display, subsystem_for, x86_main_inventory, PackageInput, WorkspaceInput,
};

pub fn analyze_package(
    workspace: &WorkspaceInput,
    package: &PackageInput,
    target_name: &str,
    base_cfg: &CfgSet,
    verification_status: VerificationStatus,
    verification_started: Option<SystemTime>,
) -> Result<(PackageReport, Vec<PathBuf>, Vec<PathBuf>)> {
    let all_inventory = inventory_files(package, target_name)?;
    let inventory = if package.name == target_name {
        x86_main_inventory(&all_inventory, package)
    } else {
        all_inventory.clone()
    };
    let inventory_set: BTreeSet<_> = inventory.iter().cloned().collect();
    let active = active_files_from_dep_info(workspace, package, verification_started)?;
    let outside_scope: Vec<_> = active
        .iter()
        .filter(|path| !inventory_set.contains(*path))
        .collect();
    if !outside_scope.is_empty() {
        bail!(
            "active inputs for `{}` fall outside its configured source inventory: {}",
            package.name,
            outside_scope
                .iter()
                .map(|path| relative_display(&workspace.root, path))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let cfg = base_cfg.clone().with_features(package.features.clone());
    let verification_passed = verification_status.is_passed();
    let mut files = Vec::new();
    let mut total_metrics = Metrics::default();
    let mut subsystem_metrics: BTreeMap<String, (u64, u64, Metrics)> = BTreeMap::new();
    for path in &active {
        let relative = relative_display(&workspace.root, path);
        let subsystem = subsystem_for(package, path);
        let mut file = analyze_file(path, relative, subsystem.clone(), &cfg)?;
        total_metrics += &file.metrics;
        let entry = subsystem_metrics.entry(subsystem).or_default();
        entry.0 += 1;
        entry.1 += file.physical_lines;
        entry.2 += &file.metrics;
        file.metrics.finalize(verification_passed);
        files.push(file);
    }
    total_metrics.finalize(verification_passed);
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let mut subsystems = subsystem_metrics
        .into_iter()
        .map(|(name, (active_files, active_lines, mut metrics))| {
            metrics.finalize(verification_passed);
            SubsystemReport {
                name,
                active_files,
                active_lines,
                metrics,
            }
        })
        .collect::<Vec<_>>();
    subsystems.sort_by(|left, right| left.name.cmp(&right.name));

    let total_lines = inventory.iter().try_fold(0_u64, |sum, path| {
        physical_lines(path).map(|lines| sum + lines)
    })?;
    let active_lines = files.iter().map(|file| file.physical_lines).sum();
    let report = PackageReport {
        name: package.name.clone(),
        role: if package.name == target_name {
            "core"
        } else {
            "auxiliary"
        }
        .to_string(),
        root: relative_display(&workspace.root, &package.root),
        active_files: active.len() as u64,
        total_files: inventory.len() as u64,
        active_lines,
        total_lines,
        metrics: total_metrics,
        subsystems,
        files,
        analysis_error: None,
    };
    Ok((report, all_inventory, active))
}

pub fn failed_package_report(
    workspace: &WorkspaceInput,
    package: &PackageInput,
    target_name: &str,
    error: &anyhow::Error,
) -> PackageReport {
    let inventory = inventory_files(package, target_name).unwrap_or_default();
    let inventory = if package.name == target_name {
        x86_main_inventory(&inventory, package)
    } else {
        inventory
    };
    let total_lines = inventory
        .iter()
        .filter_map(|path| physical_lines(path).ok())
        .sum();
    PackageReport {
        name: package.name.clone(),
        role: if package.name == target_name {
            "core"
        } else {
            "auxiliary"
        }
        .to_string(),
        root: relative_display(&workspace.root, &package.root),
        total_files: inventory.len() as u64,
        total_lines,
        analysis_error: Some(format!("{error:#}")),
        ..PackageReport::default()
    }
}

pub fn architecture_reports(
    workspace: &WorkspaceInput,
    package: &PackageInput,
    all_inventory: &[PathBuf],
    active: &[PathBuf],
    core_metrics: &Metrics,
    verification_status: VerificationStatus,
) -> Result<Vec<ArchitectureReport>> {
    let active_set: BTreeSet<_> = active.iter().collect();
    let x86_main = x86_main_inventory(all_inventory, package);
    let riscv = architecture_inventory(all_inventory, package, "riscv");
    let loongarch = architecture_inventory(all_inventory, package, "loongarch");
    let reports = vec![
        make_x86_architecture_report(
            x86_main.iter().collect(),
            &active_set,
            active.len() as u64,
            active.iter().try_fold(0_u64, |sum, path| {
                physical_lines(path).map(|lines| sum + lines)
            })?,
            core_metrics.clone(),
            verification_status,
        )?,
        make_static_architecture_report(
            workspace,
            package,
            "riscv",
            "riscv64imac-unknown-none-elf",
            riscv,
        )?,
        make_static_architecture_report(
            workspace,
            package,
            "loongarch",
            "loongarch64-unknown-none-softfloat",
            loongarch,
        )?,
    ];
    Ok(reports)
}

fn make_x86_architecture_report(
    inventory: Vec<&PathBuf>,
    active_set: &BTreeSet<&PathBuf>,
    analyzed_files: u64,
    analyzed_lines: u64,
    metrics: Metrics,
    analysis_status: VerificationStatus,
) -> Result<ArchitectureReport> {
    let active: Vec<_> = inventory
        .iter()
        .copied()
        .filter(|path| active_set.contains(path))
        .collect();
    let total_lines = inventory.iter().try_fold(0_u64, |sum, path| {
        physical_lines(path).map(|lines| sum + lines)
    })?;
    let active_lines = active.iter().try_fold(0_u64, |sum, path| {
        physical_lines(path).map(|lines| sum + lines)
    })?;
    let total_files = inventory.len() as u64;
    let active_files = active.len() as u64;
    Ok(ArchitectureReport {
        name: "x86-main".to_string(),
        role: "primary".to_string(),
        rustc_target_triple: "x86_64-unknown-none".to_string(),
        analysis_status,
        metrics_scope: "current_build_active_sources".to_string(),
        active_files,
        total_files,
        active_lines,
        total_lines,
        analyzed_files,
        analyzed_lines,
        inclusion_percent: (total_files > 0)
            .then(|| active_files as f64 * 100.0 / total_files as f64),
        metrics,
        note: None,
    })
}

fn make_static_architecture_report(
    workspace: &WorkspaceInput,
    package: &PackageInput,
    name: &str,
    rustc_target_triple: &str,
    inventory: Vec<&PathBuf>,
) -> Result<ArchitectureReport> {
    let cfg = CfgSet::from_rustc(&workspace.root, rustc_target_triple)?
        .with_features(package.features.clone());
    let mut metrics = Metrics::default();
    let mut analyzed_lines = 0_u64;
    for path in &inventory {
        let lines = physical_lines(path)?;
        analyzed_lines += lines;
        let file = analyze_file(
            path,
            relative_display(&workspace.root, path),
            format!("arch/{name}"),
            &cfg,
        )?;
        metrics += &file.metrics;
    }
    metrics.finalize(false);
    let total_files = inventory.len() as u64;
    Ok(ArchitectureReport {
        name: name.to_string(),
        role: "static_architecture_inventory".to_string(),
        rustc_target_triple: rustc_target_triple.to_string(),
        analysis_status: VerificationStatus::Unconfirmed,
        metrics_scope: "architecture_specific_source_inventory".to_string(),
        active_files: 0,
        total_files,
        active_lines: 0,
        total_lines: analyzed_lines,
        analyzed_files: total_files,
        analyzed_lines,
        inclusion_percent: (total_files > 0).then_some(0.0),
        metrics,
        note: Some(
            "architecture-specific source inventory; whole-crate Verus verification is not confirmed"
                .to_string(),
        ),
    })
}

pub fn make_project_report(
    target_name: &str,
    packages: &[PackageReport],
    architectures: &[ArchitectureReport],
    verification_status: VerificationStatus,
) -> Result<ProjectReport> {
    let core = packages
        .iter()
        .find(|package| package.name == target_name)
        .with_context(|| format!("target package `{target_name}` is absent"))?;
    let mut metrics = core.metrics.clone();
    let mut source_files = core.active_files;
    let mut source_lines = core.active_lines;
    for architecture in architectures
        .iter()
        .filter(|architecture| architecture.name != "x86-main")
    {
        let mut unconfirmed = architecture.metrics.clone();
        unconfirmed.exec.unverified += unconfirmed.exec.checked_candidates;
        unconfirmed.exec.checked_candidates = 0;
        unconfirmed.exec.unsafe_functions.unverified +=
            unconfirmed.exec.unsafe_functions.checked_candidates;
        unconfirmed.exec.unsafe_functions.checked_candidates = 0;
        unconfirmed.proof.unverified += unconfirmed.proof.checked_candidates;
        unconfirmed.proof.checked_candidates = 0;
        metrics += &unconfirmed;
        source_files += architecture.analyzed_files;
        source_lines += architecture.analyzed_lines;
    }
    metrics.finalize(verification_status.is_passed());
    Ok(ProjectReport {
        scope: "x86_current_build_plus_riscv_and_loongarch_architecture_sources".to_string(),
        status: if verification_status.is_passed() {
            "partially_confirmed"
        } else {
            "unconfirmed"
        }
        .to_string(),
        source_files,
        source_lines,
        metrics,
        note: "RISC-V and LoongArch ordinary or unconfirmed verification-candidate exec bodies are included in the project denominator as unverified; explicit trust boundaries remain trusted, and only x86 checked candidates are confirmed by a whole-crate Verus run"
            .to_string(),
    })
}

pub fn apply_baseline(report: &mut ProgressReport, path: &Path) -> Result<()> {
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read baseline {}", path.display()))?;
    let baseline: ProgressReport = serde_json::from_slice(&data)
        .with_context(|| format!("invalid baseline JSON {}", path.display()))?;
    ensure!(
        !report.project.scope.is_empty(),
        "current report has no project-wide metrics"
    );
    let (previous_source_files, previous_metrics) = if baseline.project.scope.is_empty() {
        let package = baseline
            .package(&baseline.configuration.target)
            .context("baseline has neither project-wide metrics nor a target package")?;
        (package.active_files, &package.metrics)
    } else {
        (baseline.project.source_files, &baseline.project.metrics)
    };
    let mut warnings = Vec::new();
    if baseline.schema_version != report.schema_version {
        warnings.push(format!(
            "schema changed from {} to {}",
            baseline.schema_version, report.schema_version
        ));
    }
    if baseline.configuration.target != report.configuration.target {
        warnings.push(format!(
            "target changed from {} to {}",
            baseline.configuration.target, report.configuration.target
        ));
    }
    if baseline.configuration.source_architecture != report.configuration.source_architecture {
        warnings.push(format!(
            "source architecture changed from {} to {}",
            baseline.configuration.source_architecture, report.configuration.source_architecture
        ));
    }
    if baseline.configuration.rustc_target_triple != report.configuration.rustc_target_triple {
        warnings.push(format!(
            "Rust target changed from {} to {}",
            baseline.configuration.rustc_target_triple, report.configuration.rustc_target_triple
        ));
    }
    if baseline.verification.verus.commit != report.verification.verus.commit {
        warnings.push("Verus commit changed; deltas may include toolchain effects".to_string());
    }
    if baseline.repository.dirty || report.repository.dirty {
        warnings.push("at least one snapshot was produced from a dirty worktree".to_string());
    }
    report.comparison = Some(Comparison {
        baseline_commit: baseline.repository.commit.clone(),
        warnings,
        source_files_delta: report.project.source_files as i64 - previous_source_files as i64,
        exec_total_delta: report.project.metrics.exec.total as i64
            - previous_metrics.exec.total as i64,
        checked_exec_delta: report
            .project
            .metrics
            .exec
            .checked
            .zip(previous_metrics.exec.checked)
            .map(|(now, old)| now as i64 - old as i64),
        coverage_percentage_points: report
            .project
            .metrics
            .exec
            .coverage_percent
            .zip(previous_metrics.exec.coverage_percent)
            .map(|(now, old)| now - old),
        trust_debt_delta: report.project.metrics.trust_debt.total() as i64
            - previous_metrics.trust_debt.total() as i64,
    });
    Ok(())
}

pub fn write_outputs(report: &ProgressReport, output_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let json_path = output_dir.join("progress.json");
    let markdown_path = output_dir.join("progress.md");
    std::fs::write(&json_path, serde_json::to_vec_pretty(report)?)
        .with_context(|| format!("failed to write {}", json_path.display()))?;
    std::fs::write(&markdown_path, render_markdown(report))
        .with_context(|| format!("failed to write {}", markdown_path.display()))?;
    Ok((json_path, markdown_path))
}

pub fn validate_report(report: &ProgressReport) -> Result<()> {
    let verification_passed = report.verification.status.is_passed();
    ensure!(
        report.project.status
            == if verification_passed {
                "partially_confirmed"
            } else {
                "unconfirmed"
            },
        "project-wide status disagrees with x86 verification status"
    );
    ensure!(
        report.project.source_lines == report.project.metrics.lines.total,
        "project-wide source-line count disagrees with line metrics"
    );
    validate_metrics(
        "project-wide metrics",
        &report.project.metrics,
        verification_passed,
    )?;
    if let Some(core) = report.package(&report.configuration.target) {
        let expected_files = core.active_files
            + report
                .architectures
                .iter()
                .filter(|architecture| architecture.name != "x86-main")
                .map(|architecture| architecture.analyzed_files)
                .sum::<u64>();
        ensure!(
            report.project.source_files == expected_files,
            "project-wide source-file count disagrees with architecture components"
        );
    }
    for package in &report.packages {
        ensure!(
            package.active_files <= package.total_files,
            "package `{}` has more active files than source files",
            package.name
        );
        ensure!(
            package.active_lines <= package.total_lines,
            "package `{}` has more active lines than source lines",
            package.name
        );
        if package.analysis_error.is_some() {
            continue;
        }
        ensure!(
            package.active_files == package.files.len() as u64,
            "package `{}` active-file count disagrees with file records",
            package.name
        );
        ensure!(
            package.active_lines == package.metrics.lines.total,
            "package `{}` active-line count disagrees with line metrics",
            package.name
        );
        let unique_files = package
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>();
        ensure!(
            unique_files.len() == package.files.len(),
            "package `{}` contains duplicate active files",
            package.name
        );
        validate_metrics(
            &format!("package `{}`", package.name),
            &package.metrics,
            verification_passed,
        )?;
        for file in &package.files {
            ensure!(
                file.physical_lines == file.metrics.lines.total,
                "file `{}` physical-line count disagrees with line metrics",
                file.path
            );
            validate_metrics(
                &format!("file `{}`", file.path),
                &file.metrics,
                verification_passed,
            )?;
        }
        ensure!(
            package
                .subsystems
                .iter()
                .map(|item| item.active_files)
                .sum::<u64>()
                == package.active_files,
            "package `{}` subsystem file counts do not sum to the package count",
            package.name
        );
        ensure!(
            package
                .subsystems
                .iter()
                .map(|item| item.active_lines)
                .sum::<u64>()
                == package.active_lines,
            "package `{}` subsystem line counts do not sum to the package count",
            package.name
        );
        for subsystem in &package.subsystems {
            ensure!(
                subsystem.active_lines == subsystem.metrics.lines.total,
                "subsystem `{}` active-line count disagrees with line metrics",
                subsystem.name
            );
            validate_metrics(
                &format!("subsystem `{}`", subsystem.name),
                &subsystem.metrics,
                verification_passed,
            )?;
        }
    }
    for architecture in &report.architectures {
        ensure!(
            architecture.active_files <= architecture.total_files,
            "architecture `{}` has more active files than source files",
            architecture.name
        );
        ensure!(
            architecture.active_lines <= architecture.total_lines,
            "architecture `{}` has more active lines than source lines",
            architecture.name
        );
        ensure!(
            architecture.analyzed_files <= architecture.total_files,
            "architecture `{}` has more analyzed files than source files",
            architecture.name
        );
        ensure!(
            architecture.analyzed_lines <= architecture.total_lines,
            "architecture `{}` has more analyzed lines than source lines",
            architecture.name
        );
        ensure!(
            architecture.analyzed_lines == architecture.metrics.lines.total,
            "architecture `{}` analyzed-line count disagrees with line metrics",
            architecture.name
        );
        validate_metrics(
            &format!("architecture `{}`", architecture.name),
            &architecture.metrics,
            architecture.analysis_status.is_passed(),
        )?;
    }
    Ok(())
}

fn validate_metrics(scope: &str, metrics: &Metrics, verification_passed: bool) -> Result<()> {
    ensure!(
        metrics.exec.total
            == metrics.exec.checked_candidates + metrics.exec.trusted + metrics.exec.unverified,
        "{scope}: exec buckets do not sum to the denominator"
    );
    ensure!(
        metrics.exec.specified <= metrics.exec.total,
        "{scope}: specified exec functions exceed the denominator"
    );
    ensure!(
        metrics.exec.unsafe_functions.total
            == metrics.exec.unsafe_functions.checked_candidates
                + metrics.exec.unsafe_functions.trusted
                + metrics.exec.unsafe_functions.unverified,
        "{scope}: unsafe exec buckets do not sum to their denominator"
    );
    ensure!(
        metrics.exec.unsafe_functions.total <= metrics.exec.total,
        "{scope}: unsafe exec functions exceed all exec functions"
    );
    if metrics.exec.total == 0 {
        ensure!(
            metrics.exec.contract_coverage_percent.is_none(),
            "{scope}: zero exec denominator produced contract coverage"
        );
    } else {
        let expected = metrics.exec.specified as f64 * 100.0 / metrics.exec.total as f64;
        ensure!(
            metrics
                .exec
                .contract_coverage_percent
                .is_some_and(|value| (value - expected).abs() < 1e-9),
            "{scope}: contract coverage disagrees with function counts"
        );
    }
    ensure!(
        metrics.proof.total
            == metrics.proof.checked_candidates
                + metrics.proof.trusted
                + metrics.proof.external
                + metrics.proof.axioms
                + metrics.proof.declarations
                + metrics.proof.unverified,
        "{scope}: proof buckets do not sum to their total"
    );
    ensure!(
        metrics.spec.total == metrics.spec.defined + metrics.spec.uninterpreted,
        "{scope}: spec buckets do not sum to their total"
    );
    let lines = &metrics.lines;
    ensure!(
        lines.total
            == lines.trusted
                + lines.spec
                + lines.proof
                + lines.exec
                + lines.directives
                + lines.definitions
                + lines.comments
                + lines.layout
                + lines.unaccounted,
        "{scope}: exclusive line buckets do not sum to total lines"
    );
    ensure!(
        lines.raw_tags.values().sum::<u64>() == lines.total,
        "{scope}: raw line-tag counts do not sum to total lines"
    );
    if verification_passed {
        ensure!(
            metrics.exec.checked == Some(metrics.exec.checked_candidates),
            "{scope}: successful verification did not confirm exec candidates"
        );
        ensure!(
            metrics.proof.checked == Some(metrics.proof.checked_candidates),
            "{scope}: successful verification did not confirm proof candidates"
        );
        ensure!(
            metrics.exec.unsafe_functions.checked
                == Some(metrics.exec.unsafe_functions.checked_candidates),
            "{scope}: successful verification did not confirm unsafe exec candidates"
        );
        if metrics.exec.total == 0 {
            ensure!(
                metrics.exec.coverage_percent.is_none(),
                "{scope}: zero exec denominator produced verified coverage"
            );
        } else {
            let expected =
                metrics.exec.checked_candidates as f64 * 100.0 / metrics.exec.total as f64;
            ensure!(
                metrics
                    .exec
                    .coverage_percent
                    .is_some_and(|value| (value - expected).abs() < 1e-9),
                "{scope}: verified coverage disagrees with function counts"
            );
        }
    } else {
        ensure!(
            metrics.exec.checked.is_none()
                && metrics.proof.checked.is_none()
                && metrics.exec.unsafe_functions.checked.is_none()
                && metrics.exec.coverage_percent.is_none(),
            "{scope}: unconfirmed verification produced confirmed coverage"
        );
    }
    Ok(())
}

pub fn print_summary(report: &ProgressReport, json: &Path, markdown: &Path) {
    let package = report.package(&report.configuration.target);
    println!();
    println!(
        "x86 whole-crate verification: {}",
        report.verification.status.label()
    );
    let project_checked = report
        .project
        .metrics
        .exec
        .checked
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unconfirmed".to_string());
    println!(
        "  project-wide exec ({}): checked={}, candidates={}, trusted={}, unverified={}, total={}",
        report.project.status,
        project_checked,
        report.project.metrics.exec.checked_candidates,
        report.project.metrics.exec.trusted,
        report.project.metrics.exec.unverified,
        report.project.metrics.exec.total,
    );
    println!(
        "  project-wide coverage: {}, contract coverage: {}",
        exec_percent(
            report.project.metrics.exec.coverage_percent,
            report.project.metrics.exec.total,
        ),
        exec_percent(
            report.project.metrics.exec.contract_coverage_percent,
            report.project.metrics.exec.total,
        ),
    );
    if let Some(package) = package {
        let checked = package
            .metrics
            .exec
            .checked
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unconfirmed".to_string());
        println!(
            "  x86 current-build exec: checked={}, candidates={}, trusted={}, unverified={}, total={}",
            checked,
            package.metrics.exec.checked_candidates,
            package.metrics.exec.trusted,
            package.metrics.exec.unverified,
            package.metrics.exec.total
        );
        println!(
            "  active source files: {}/{}",
            package.active_files, package.total_files
        );
        println!(
            "  trust debt markers: {}",
            package.metrics.trust_debt.total()
        );
    }
    for architecture in &report.architectures {
        if architecture.name == "x86-main" {
            continue;
        }
        println!(
            "  {}: status={}, exec_candidates={}, trusted={}, unverified={}, source_files={}",
            architecture.name,
            architecture.analysis_status.label(),
            architecture.metrics.exec.checked_candidates,
            architecture.metrics.exec.trusted,
            architecture.metrics.exec.unverified,
            architecture.analyzed_files,
        );
    }
    println!("  JSON: {}", json.display());
    println!("  Markdown: {}", markdown.display());
}

pub fn render_markdown(report: &ProgressReport) -> String {
    let mut out = String::new();
    out.push_str("# VOSTD Verification Progress\n\n");
    out.push_str(&format!(
        "- x86 whole-crate verification: **{}**\n- Project-wide status: **{}**\n- Commit: `{}`{}\n- Confirmed source architecture: `{}`\n",
        report.verification.status.label(),
        report.project.status,
        report.repository.commit,
        if report.repository.dirty {
            " (dirty)"
        } else {
            ""
        },
        report.configuration.source_architecture
    ));
    out.push_str(&format!(
        "- Rust target: `{}`\n",
        report.configuration.rustc_target_triple
    ));
    if let Some(version) = &report.verification.verus.version {
        out.push_str(&format!("- Verus: `{version}`\n"));
    }
    if let Some(message) = &report.verification.message {
        out.push_str(&format!("- Note: {}\n", escape_cell(message)));
    }

    let project = &report.project;
    out.push_str("\n## Project-wide verification progress\n\n");
    out.push_str("This is the primary progress metric. It includes the current x86 build plus RISC-V and LoongArch architecture-specific source.\n\n");
    out.push_str("| Status | Source files | Source lines | Checked exec | Checked candidates | Trusted | Unverified | Total exec | Coverage | Contract coverage | Proof total | Spec total | Trust debt |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    out.push_str(&format!(
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
        project.status,
        project.source_files,
        project.source_lines,
        option_count(project.metrics.exec.checked),
        project.metrics.exec.checked_candidates,
        project.metrics.exec.trusted,
        project.metrics.exec.unverified,
        project.metrics.exec.total,
        exec_percent(
            project.metrics.exec.coverage_percent,
            project.metrics.exec.total,
        ),
        exec_percent(
            project.metrics.exec.contract_coverage_percent,
            project.metrics.exec.total,
        ),
        project.metrics.proof.total,
        project.metrics.spec.total,
        project.metrics.trust_debt.total(),
    ));
    if !project.note.is_empty() {
        out.push_str(&format!("\n- Scope note: {}\n", escape_cell(&project.note)));
    }
    let project_lines = &project.metrics.lines;
    let project_other_lines = project_lines.directives
        + project_lines.definitions
        + project_lines.comments
        + project_lines.layout
        + project_lines.unaccounted;
    out.push_str(
        "\n| Project Trusted LOC | Spec LOC | Proof LOC | Exec LOC | Other LOC | Total LOC |\n",
    );
    out.push_str("|---:|---:|---:|---:|---:|---:|\n");
    out.push_str(&format!(
        "| {} | {} | {} | {} | {} | {} |\n",
        project_lines.trusted,
        project_lines.spec,
        project_lines.proof,
        project_lines.exec,
        project_other_lines,
        project_lines.total,
    ));

    out.push_str("\n## Current x86 build package summary\n\n");
    out.push_str("| Package | Active files | Checked exec | Checked candidates | Total exec | Coverage | Contract coverage | Trusted | Unverified | Trust debt |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for package in &report.packages {
        out.push_str(&format!(
            "| {} | {}/{} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            escape_cell(&package.name),
            package.active_files,
            package.total_files,
            option_count(package.metrics.exec.checked),
            package.metrics.exec.checked_candidates,
            package.metrics.exec.total,
            exec_percent(
                package.metrics.exec.coverage_percent,
                package.metrics.exec.total
            ),
            exec_percent(
                package.metrics.exec.contract_coverage_percent,
                package.metrics.exec.total,
            ),
            package.metrics.exec.trusted,
            package.metrics.exec.unverified,
            package.metrics.trust_debt.total(),
        ));
    }

    out.push_str("\n## Function and proof composition\n\n");
    out.push_str("| Package | Checked proof | Proof candidates | Proof declarations | Proof total | Trusted/external proof | Axioms | Unverified proof | Spec defined | Spec uninterpreted | Unsafe checked/candidates/trusted/unverified |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for package in &report.packages {
        let unsafe_exec = &package.metrics.exec.unsafe_functions;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {}/{}/{}/{} |\n",
            escape_cell(&package.name),
            option_count(package.metrics.proof.checked),
            package.metrics.proof.checked_candidates,
            package.metrics.proof.declarations,
            package.metrics.proof.total,
            package.metrics.proof.trusted + package.metrics.proof.external,
            package.metrics.proof.axioms,
            package.metrics.proof.unverified,
            package.metrics.spec.defined,
            package.metrics.spec.uninterpreted,
            option_count(unsafe_exec.checked),
            unsafe_exec.checked_candidates,
            unsafe_exec.trusted,
            unsafe_exec.unverified,
        ));
    }

    out.push_str("\n## Source line composition\n\n");
    out.push_str("| Package | Trusted | Spec | Proof | Exec | Directives | Definitions | Comments | Layout | Unaccounted | Total |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for package in &report.packages {
        let lines = &package.metrics.lines;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            escape_cell(&package.name),
            lines.trusted,
            lines.spec,
            lines.proof,
            lines.exec,
            lines.directives,
            lines.definitions,
            lines.comments,
            lines.layout,
            lines.unaccounted,
            lines.total,
        ));
    }

    out.push_str("\n## Architecture inclusion\n\n");
    out.push_str(
        "| Scope | Target | Status | Metrics scope | Active files | Analyzed files | Total files | Inclusion | Analyzed lines | Total lines |\n",
    );
    out.push_str("|---|---|---|---|---:|---:|---:|---:|---:|---:|\n");
    for architecture in &report.architectures {
        out.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            architecture.name,
            architecture.rustc_target_triple,
            architecture.analysis_status.label(),
            architecture.metrics_scope,
            architecture.active_files,
            architecture.analyzed_files,
            architecture.total_files,
            percent_or_na(architecture.inclusion_percent, architecture.total_files),
            architecture.analyzed_lines,
            architecture.total_lines,
        ));
    }

    out.push_str("\n## Architecture function composition\n\n");
    out.push_str("| Scope | Checked exec | Exec candidates | Trusted | Unverified | Total exec | Coverage | Contract coverage | Proof candidates/declarations/total | Spec | Trust debt |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for architecture in &report.architectures {
        let metrics = &architecture.metrics;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {}/{}/{} | {} | {} |\n",
            architecture.name,
            option_count(metrics.exec.checked),
            metrics.exec.checked_candidates,
            metrics.exec.trusted,
            metrics.exec.unverified,
            metrics.exec.total,
            exec_percent(metrics.exec.coverage_percent, metrics.exec.total),
            exec_percent(metrics.exec.contract_coverage_percent, metrics.exec.total),
            metrics.proof.checked_candidates,
            metrics.proof.declarations,
            metrics.proof.total,
            metrics.spec.total,
            metrics.trust_debt.total(),
        ));
    }

    out.push_str("\n## Architecture line composition\n\n");
    out.push_str("| Scope | Trusted | Spec | Proof | Exec | Other | Total |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for architecture in &report.architectures {
        let lines = &architecture.metrics.lines;
        let other = lines.directives
            + lines.definitions
            + lines.comments
            + lines.layout
            + lines.unaccounted;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            architecture.name,
            lines.trusted,
            lines.spec,
            lines.proof,
            lines.exec,
            other,
            lines.total,
        ));
    }

    let architecture_notes = report
        .architectures
        .iter()
        .filter_map(|architecture| {
            architecture
                .note
                .as_ref()
                .map(|note| format!("{}: {}", architecture.name, note))
        })
        .collect::<Vec<_>>();
    if !architecture_notes.is_empty() {
        out.push_str("\nArchitecture notes:\n\n");
        for note in architecture_notes {
            out.push_str(&format!("- {}\n", escape_cell(&note)));
        }
    }

    if let Some(core) = report.package(&report.configuration.target) {
        out.push_str("\n## Core subsystems\n\n");
        out.push_str("| Subsystem | Files | Checked exec | Exec candidates | Total exec | Coverage | Proof candidates/total | Spec | Trust debt |\n");
        out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|\n");
        for subsystem in &core.subsystems {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {}/{} | {} | {} |\n",
                escape_cell(&subsystem.name),
                subsystem.active_files,
                option_count(subsystem.metrics.exec.checked),
                subsystem.metrics.exec.checked_candidates,
                subsystem.metrics.exec.total,
                exec_percent(
                    subsystem.metrics.exec.coverage_percent,
                    subsystem.metrics.exec.total,
                ),
                subsystem.metrics.proof.checked_candidates,
                subsystem.metrics.proof.total,
                subsystem.metrics.spec.total,
                subsystem.metrics.trust_debt.total(),
            ));
        }

        let debt = &core.metrics.trust_debt;
        out.push_str("\n## Trust debt\n\n");
        out.push_str("| external_body | external | external specifications | trusted | axioms | assume | admit |\n");
        out.push_str("|---:|---:|---:|---:|---:|---:|---:|\n");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            debt.external_body,
            debt.external,
            debt.external_specifications,
            debt.trusted,
            debt.axioms,
            debt.assumes,
            debt.admits,
        ));
    }

    if let Some(comparison) = &report.comparison {
        out.push_str("\n## Baseline comparison\n\n");
        out.push_str(&format!(
            "- Baseline commit: `{}`\n- Project source files: {:+}\n- Total exec functions: {:+}\n- Checked exec functions: {}\n- Coverage: {}\n- Trust debt: {:+}\n",
            comparison.baseline_commit,
            comparison.source_files_delta,
            comparison.exec_total_delta,
            comparison
                .checked_exec_delta
                .map(|value| format!("{value:+}"))
                .unwrap_or_else(|| "n/a".to_string()),
            comparison
                .coverage_percentage_points
                .map(|value| format!("{value:+.2} percentage points"))
                .unwrap_or_else(|| "n/a".to_string()),
            comparison.trust_debt_delta,
        ));
        for warning in &comparison.warnings {
            out.push_str(&format!("- Warning: {}\n", escape_cell(warning)));
        }
    }

    if !report.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for warning in &report.warnings {
            out.push_str(&format!("- {}\n", escape_cell(warning)));
        }
    }
    out
}

fn option_count(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unconfirmed".to_string())
}

fn option_percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}%"))
        .unwrap_or_else(|| "unconfirmed".to_string())
}

fn exec_percent(value: Option<f64>, denominator: u64) -> String {
    if denominator == 0 {
        "n/a".to_string()
    } else {
        option_percent(value)
    }
}

fn percent_or_na(value: Option<f64>, denominator: u64) -> String {
    if denominator == 0 {
        "n/a".to_string()
    } else {
        value
            .map(|value| format!("{value:.2}%"))
            .unwrap_or_else(|| "n/a".to_string())
    }
}

fn escape_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ArchitectureReport, Configuration, PackageReport, RepositoryInfo, VerificationSummary,
    };

    fn sample_report(
        commit: &str,
        active_files: u64,
        checked: u64,
        trusted: u64,
    ) -> ProgressReport {
        let mut metrics = Metrics::default();
        metrics.exec.checked_candidates = checked;
        metrics.exec.trusted = trusted;
        metrics.trust_debt.external_body = trusted;
        metrics.finalize(true);
        let project = ProjectReport {
            scope: "test".to_string(),
            status: "partially_confirmed".to_string(),
            source_files: active_files,
            metrics: metrics.clone(),
            ..ProjectReport::default()
        };
        ProgressReport {
            schema_version: 1,
            generated_at: "now".to_string(),
            repository: RepositoryInfo {
                commit: commit.to_string(),
                dirty: false,
            },
            configuration: Configuration {
                target: "ostd".to_string(),
                source_architecture: "x86".to_string(),
                rustc_target_triple: "x86_64-unknown-none".to_string(),
                static_only: false,
            },
            verification: crate::model::VerificationSummary {
                status: VerificationStatus::Passed,
                ..VerificationSummary::default()
            },
            project,
            packages: vec![PackageReport {
                name: "ostd".to_string(),
                active_files,
                total_files: active_files,
                metrics,
                ..PackageReport::default()
            }],
            architectures: Vec::new(),
            comparison: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn markdown_contains_unconfirmed_in_static_mode() {
        let report = ProgressReport {
            schema_version: 1,
            generated_at: "now".to_string(),
            repository: RepositoryInfo {
                commit: "abc".to_string(),
                dirty: false,
            },
            configuration: Configuration {
                target: "ostd".to_string(),
                source_architecture: "x86".to_string(),
                rustc_target_triple: "x86_64-unknown-none".to_string(),
                static_only: true,
            },
            verification: VerificationSummary::default(),
            project: ProjectReport::default(),
            packages: vec![PackageReport {
                name: "ostd".to_string(),
                ..PackageReport::default()
            }],
            architectures: Vec::new(),
            comparison: None,
            warnings: Vec::new(),
        };
        let markdown = render_markdown(&report);
        assert!(markdown.contains("unconfirmed"));
    }

    #[test]
    fn baseline_comparison_computes_non_gating_deltas() {
        let baseline = sample_report("old", 10, 2, 1);
        let mut current = sample_report("new", 11, 3, 1);
        let path = std::env::temp_dir().join(format!(
            "verification-progress-baseline-test-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, serde_json::to_vec(&baseline).unwrap()).unwrap();
        apply_baseline(&mut current, &path).unwrap();
        std::fs::remove_file(path).unwrap();

        let comparison = current.comparison.unwrap();
        assert_eq!(comparison.source_files_delta, 1);
        assert_eq!(comparison.exec_total_delta, 1);
        assert_eq!(comparison.checked_exec_delta, Some(1));
        assert_eq!(comparison.trust_debt_delta, 0);
        assert!((comparison.coverage_percentage_points.unwrap() - 8.333333).abs() < 0.001);
    }

    #[test]
    fn rejects_exec_partition_invariant_violation() {
        let mut report = sample_report("new", 0, 1, 0);
        report.packages[0].metrics.exec.total = 9;
        assert!(validate_report(&report).is_err());
    }

    #[test]
    fn accepts_unconfirmed_architecture_candidates_without_checked_coverage() {
        let mut report = sample_report("new", 0, 1, 0);
        let mut metrics = Metrics::default();
        metrics.exec.checked_candidates = 2;
        metrics.exec.unverified = 3;
        metrics.finalize(false);
        report.architectures.push(ArchitectureReport {
            name: "riscv".to_string(),
            role: "static_architecture_inventory".to_string(),
            rustc_target_triple: "riscv64imac-unknown-none-elf".to_string(),
            analysis_status: VerificationStatus::Unconfirmed,
            metrics_scope: "architecture_specific_source_inventory".to_string(),
            total_files: 1,
            analyzed_files: 1,
            metrics,
            ..ArchitectureReport::default()
        });
        report.project = make_project_report(
            "ostd",
            &report.packages,
            &report.architectures,
            report.verification.status,
        )
        .unwrap();

        validate_report(&report).unwrap();
        assert_eq!(report.project.metrics.exec.checked, Some(1));
        assert_eq!(report.project.metrics.exec.checked_candidates, 1);
        assert_eq!(report.project.metrics.exec.unverified, 5);
        let markdown = render_markdown(&report);
        assert!(markdown.contains("| riscv | unconfirmed | 2 | 0 | 3 | 5 | unconfirmed |"));
    }
}
