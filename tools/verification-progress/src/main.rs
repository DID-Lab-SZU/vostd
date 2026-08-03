mod model;
mod report;
mod source;
mod verifier;
mod workspace;

use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use chrono::{SecondsFormat, Utc};
use clap::Parser;

use crate::model::{Configuration, ProgressReport, VerificationStatus};
use crate::report::{
    analyze_package, apply_baseline, architecture_reports, failed_package_report,
    make_project_report, print_summary, validate_report, write_outputs,
};
use crate::source::CfgSet;
use crate::verifier::{run_verification, unconfirmed_summary};
use crate::workspace::{load_workspace, repository_info};

const RUSTC_TARGET_TRIPLE: &str = "x86_64-unknown-none";

#[derive(Debug, Parser)]
#[command(
    name = "verification-progress",
    about = "Measure VOSTD verification coverage, proof scale, and trust debt"
)]
struct Cli {
    /// Cargo package whose verification closure is measured.
    #[arg(long, default_value = "ostd")]
    target: String,

    /// Directory for progress.json and progress.md.
    #[arg(long, default_value = "target/verification-progress")]
    output_dir: PathBuf,

    /// Analyze existing dep-info without running Verus. Checked counts remain unconfirmed.
    #[arg(long)]
    static_only: bool,

    /// Previous progress.json used to calculate non-gating deltas.
    #[arg(long)]
    baseline: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let invocation_dir = std::env::current_dir()?.canonicalize()?;
    let workspace = load_workspace(&invocation_dir, &cli.target)?;
    let root = workspace.root.clone();
    let repository = repository_info(&root);
    let base_cfg = CfgSet::from_rustc(&root, RUSTC_TARGET_TRIPLE)?;

    let fallback_started = SystemTime::now();
    let verification_run = if cli.static_only {
        None
    } else {
        match run_verification(&root, &cli.target) {
            Ok(run) => Some(run),
            Err(error) => {
                eprintln!("verification could not be started: {error:#}");
                let mut summary = unconfirmed_summary(&root);
                summary.status = VerificationStatus::Failed;
                summary.message = Some(format!("failed to run Verus: {error:#}"));
                Some(verifier::VerificationRun {
                    summary,
                    started_at: fallback_started,
                })
            }
        }
    };
    let verification = verification_run
        .as_ref()
        .map(|run| run.summary.clone())
        .unwrap_or_else(|| unconfirmed_summary(&root));
    let verification_started = verification_run.as_ref().map(|run| run.started_at);

    let mut warnings = Vec::new();
    if repository.dirty {
        warnings.push("report was generated from a dirty worktree".to_string());
    }
    let mut packages = Vec::new();
    let mut architecture_inputs = None;
    let mut analysis_failed = false;
    for package in &workspace.packages {
        match analyze_package(
            &workspace,
            package,
            &cli.target,
            &base_cfg,
            verification.status,
            verification_started,
        ) {
            Ok((report, all_inventory, active)) => {
                if package.name == cli.target {
                    architecture_inputs = Some((
                        package.clone(),
                        all_inventory,
                        active,
                        report.metrics.clone(),
                    ));
                }
                packages.push(report);
            }
            Err(error) => {
                analysis_failed = true;
                warnings.push(format!("{} analysis failed: {error:#}", package.name));
                packages.push(failed_package_report(
                    &workspace,
                    package,
                    &cli.target,
                    &error,
                ));
            }
        }
    }

    let architectures = if let Some((package, inventory, active, core_metrics)) =
        architecture_inputs
    {
        match architecture_reports(
            &workspace,
            &package,
            &inventory,
            &active,
            &core_metrics,
            verification.status,
        ) {
            Ok(reports) => reports,
            Err(error) => {
                analysis_failed = true;
                warnings.push(format!("architecture inventory failed: {error:#}"));
                Vec::new()
            }
        }
    } else {
        analysis_failed = true;
        warnings.push("target package could not be analyzed; architecture report is absent".into());
        Vec::new()
    };

    let project =
        match make_project_report(&cli.target, &packages, &architectures, verification.status) {
            Ok(project) => project,
            Err(error) => {
                analysis_failed = true;
                warnings.push(format!("project-wide aggregation failed: {error:#}"));
                Default::default()
            }
        };

    let mut progress = ProgressReport {
        schema_version: 1,
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        repository,
        configuration: Configuration {
            target: cli.target.clone(),
            source_architecture: "x86".to_string(),
            rustc_target_triple: RUSTC_TARGET_TRIPLE.to_string(),
            static_only: cli.static_only,
        },
        verification,
        project,
        packages,
        architectures,
        comparison: None,
        warnings,
    };

    if let Some(baseline) = &cli.baseline {
        let path = if baseline.is_absolute() {
            baseline.clone()
        } else {
            root.join(baseline)
        };
        if let Err(error) = apply_baseline(&mut progress, &path) {
            analysis_failed = true;
            progress
                .warnings
                .push(format!("baseline comparison failed: {error:#}"));
        }
    }

    if let Err(error) = validate_report(&progress) {
        analysis_failed = true;
        progress
            .warnings
            .push(format!("report invariant failed: {error:#}"));
    }

    let output_dir = if cli.output_dir.is_absolute() {
        cli.output_dir
    } else {
        root.join(cli.output_dir)
    };
    let (json, markdown) = write_outputs(&progress, &output_dir)?;
    print_summary(&progress, &json, &markdown);

    if progress.verification.status == VerificationStatus::Failed || analysis_failed {
        Err(anyhow!(
            "verification progress report completed with errors"
        ))
    } else {
        Ok(())
    }
}
