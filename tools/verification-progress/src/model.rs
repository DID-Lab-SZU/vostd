use std::collections::BTreeMap;
use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    #[default]
    Unconfirmed,
}

impl VerificationStatus {
    pub fn is_passed(self) -> bool {
        self == Self::Passed
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub commit: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerusInfo {
    pub version: Option<String>,
    pub commit: Option<String>,
    pub toolchain: Option<String>,
    pub platform_os: Option<String>,
    pub platform_arch: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VcFunctionCounts {
    pub exec: Option<u64>,
    pub proof: Option<u64>,
    pub other: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationSummary {
    pub status: VerificationStatus,
    pub exit_code: Option<i32>,
    pub entire_crate: bool,
    pub verified_queries: Option<u64>,
    pub errors: Option<u64>,
    pub duration_ms: Option<u128>,
    pub vc_functions: VcFunctionCounts,
    pub verus: VerusInfo,
    pub message: Option<String>,
}

impl Default for VerificationSummary {
    fn default() -> Self {
        Self {
            status: VerificationStatus::Unconfirmed,
            exit_code: None,
            entire_crate: false,
            verified_queries: None,
            errors: None,
            duration_ms: None,
            vc_functions: VcFunctionCounts::default(),
            verus: VerusInfo::default(),
            message: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Configuration {
    pub target: String,
    pub source_architecture: String,
    pub rustc_target_triple: String,
    pub static_only: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnsafeExecMetrics {
    pub checked_candidates: u64,
    pub checked: Option<u64>,
    pub trusted: u64,
    pub unverified: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecMetrics {
    /// Bodies that are syntactically in a Verus verification region.
    pub checked_candidates: u64,
    /// Populated only after a successful whole-crate verification.
    pub checked: Option<u64>,
    pub trusted: u64,
    pub unverified: u64,
    pub total: u64,
    pub specified: u64,
    pub coverage_percent: Option<f64>,
    pub contract_coverage_percent: Option<f64>,
    pub unsafe_functions: UnsafeExecMetrics,
}

impl ExecMetrics {
    pub fn finalize(&mut self, verification_passed: bool) {
        self.total = self.checked_candidates + self.trusted + self.unverified;
        self.unsafe_functions.total = self.unsafe_functions.checked_candidates
            + self.unsafe_functions.trusted
            + self.unsafe_functions.unverified;
        self.checked = verification_passed.then_some(self.checked_candidates);
        self.unsafe_functions.checked =
            verification_passed.then_some(self.unsafe_functions.checked_candidates);
        self.coverage_percent = if verification_passed && self.total > 0 {
            Some(self.checked_candidates as f64 * 100.0 / self.total as f64)
        } else {
            None
        };
        self.contract_coverage_percent = if self.total > 0 {
            Some(self.specified as f64 * 100.0 / self.total as f64)
        } else {
            None
        };
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProofMetrics {
    pub checked_candidates: u64,
    pub checked: Option<u64>,
    pub trusted: u64,
    pub external: u64,
    pub axioms: u64,
    pub declarations: u64,
    pub unverified: u64,
    pub total: u64,
}

impl ProofMetrics {
    pub fn finalize(&mut self, verification_passed: bool) {
        self.total = self.checked_candidates
            + self.trusted
            + self.external
            + self.axioms
            + self.declarations
            + self.unverified;
        self.checked = verification_passed.then_some(self.checked_candidates);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpecMetrics {
    pub defined: u64,
    pub uninterpreted: u64,
    pub total: u64,
}

impl SpecMetrics {
    pub fn finalize(&mut self) {
        self.total = self.defined + self.uninterpreted;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustDebt {
    pub external_body: u64,
    pub external: u64,
    pub external_specifications: u64,
    pub trusted: u64,
    pub axioms: u64,
    pub assumes: u64,
    pub admits: u64,
}

impl TrustDebt {
    pub fn total(&self) -> u64 {
        self.external_body
            + self.external
            + self.external_specifications
            + self.trusted
            + self.axioms
            + self.assumes
            + self.admits
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LineMetrics {
    pub trusted: u64,
    pub spec: u64,
    pub proof: u64,
    pub exec: u64,
    pub directives: u64,
    pub definitions: u64,
    pub comments: u64,
    pub layout: u64,
    pub unaccounted: u64,
    pub total: u64,
    /// Exact source tags before the mutually-exclusive Markdown precedence is applied.
    pub raw_tags: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub exec: ExecMetrics,
    pub proof: ProofMetrics,
    pub spec: SpecMetrics,
    pub lines: LineMetrics,
    pub trust_debt: TrustDebt,
}

impl Metrics {
    pub fn finalize(&mut self, verification_passed: bool) {
        self.exec.finalize(verification_passed);
        self.proof.finalize(verification_passed);
        self.spec.finalize();
    }
}

impl AddAssign<&Metrics> for Metrics {
    fn add_assign(&mut self, rhs: &Metrics) {
        self.exec.checked_candidates += rhs.exec.checked_candidates;
        self.exec.trusted += rhs.exec.trusted;
        self.exec.unverified += rhs.exec.unverified;
        self.exec.specified += rhs.exec.specified;
        self.exec.unsafe_functions.checked_candidates +=
            rhs.exec.unsafe_functions.checked_candidates;
        self.exec.unsafe_functions.trusted += rhs.exec.unsafe_functions.trusted;
        self.exec.unsafe_functions.unverified += rhs.exec.unsafe_functions.unverified;

        self.proof.checked_candidates += rhs.proof.checked_candidates;
        self.proof.trusted += rhs.proof.trusted;
        self.proof.external += rhs.proof.external;
        self.proof.axioms += rhs.proof.axioms;
        self.proof.declarations += rhs.proof.declarations;
        self.proof.unverified += rhs.proof.unverified;

        self.spec.defined += rhs.spec.defined;
        self.spec.uninterpreted += rhs.spec.uninterpreted;

        self.lines.trusted += rhs.lines.trusted;
        self.lines.spec += rhs.lines.spec;
        self.lines.proof += rhs.lines.proof;
        self.lines.exec += rhs.lines.exec;
        self.lines.directives += rhs.lines.directives;
        self.lines.definitions += rhs.lines.definitions;
        self.lines.comments += rhs.lines.comments;
        self.lines.layout += rhs.lines.layout;
        self.lines.unaccounted += rhs.lines.unaccounted;
        self.lines.total += rhs.lines.total;
        for (tags, count) in &rhs.lines.raw_tags {
            *self.lines.raw_tags.entry(tags.clone()).or_default() += count;
        }

        self.trust_debt.external_body += rhs.trust_debt.external_body;
        self.trust_debt.external += rhs.trust_debt.external;
        self.trust_debt.external_specifications += rhs.trust_debt.external_specifications;
        self.trust_debt.trusted += rhs.trust_debt.trusted;
        self.trust_debt.axioms += rhs.trust_debt.axioms;
        self.trust_debt.assumes += rhs.trust_debt.assumes;
        self.trust_debt.admits += rhs.trust_debt.admits;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileReport {
    pub path: String,
    pub subsystem: String,
    pub physical_lines: u64,
    pub metrics: Metrics,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubsystemReport {
    pub name: String,
    pub active_files: u64,
    pub active_lines: u64,
    pub metrics: Metrics,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageReport {
    pub name: String,
    pub role: String,
    pub root: String,
    pub active_files: u64,
    pub total_files: u64,
    pub active_lines: u64,
    pub total_lines: u64,
    pub metrics: Metrics,
    pub subsystems: Vec<SubsystemReport>,
    pub files: Vec<FileReport>,
    pub analysis_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchitectureReport {
    pub name: String,
    pub role: String,
    pub rustc_target_triple: String,
    pub analysis_status: VerificationStatus,
    pub metrics_scope: String,
    pub active_files: u64,
    pub total_files: u64,
    pub active_lines: u64,
    pub total_lines: u64,
    pub analyzed_files: u64,
    pub analyzed_lines: u64,
    pub inclusion_percent: Option<f64>,
    pub metrics: Metrics,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectReport {
    pub scope: String,
    pub status: String,
    pub source_files: u64,
    pub source_lines: u64,
    pub metrics: Metrics,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Comparison {
    pub baseline_commit: String,
    pub warnings: Vec<String>,
    pub source_files_delta: i64,
    pub exec_total_delta: i64,
    pub checked_exec_delta: Option<i64>,
    pub coverage_percentage_points: Option<f64>,
    pub trust_debt_delta: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressReport {
    pub schema_version: u32,
    pub generated_at: String,
    pub repository: RepositoryInfo,
    pub configuration: Configuration,
    pub verification: VerificationSummary,
    #[serde(default)]
    pub project: ProjectReport,
    pub packages: Vec<PackageReport>,
    pub architectures: Vec<ArchitectureReport>,
    pub comparison: Option<Comparison>,
    pub warnings: Vec<String>,
}

impl ProgressReport {
    pub fn package(&self, name: &str) -> Option<&PackageReport> {
        self.packages.iter().find(|package| package.name == name)
    }
}
