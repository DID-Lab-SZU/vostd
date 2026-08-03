use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::model::{VcFunctionCounts, VerificationStatus, VerificationSummary, VerusInfo};

pub struct VerificationRun {
    pub summary: VerificationSummary,
    pub started_at: SystemTime,
}

pub fn run_verification(workspace_root: &Path, target: &str) -> Result<VerificationRun> {
    let started_at = SystemTime::now();
    let timer = Instant::now();
    let mut child = Command::new("cargo")
        .args([
            "dv",
            "verify",
            "--targets",
            target,
            "--",
            "--output-json",
            "--time",
        ])
        .current_dir(workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start cargo dv verification")?;

    let stdout = child
        .stdout
        .take()
        .context("verification stdout was not captured")?;
    let stderr = child
        .stderr
        .take()
        .context("verification stderr was not captured")?;
    let stdout_capture = Arc::new(Mutex::new(String::new()));
    let stderr_capture = Arc::new(Mutex::new(String::new()));
    let stdout_thread = tee_stream(stdout, false, stdout_capture.clone());
    let stderr_thread = tee_stream(stderr, true, stderr_capture.clone());
    let status = child
        .wait()
        .context("failed while waiting for cargo dv verification")?;
    stdout_thread
        .join()
        .expect("stdout reader thread panicked")?;
    stderr_thread
        .join()
        .expect("stderr reader thread panicked")?;

    let stdout = stdout_capture
        .lock()
        .expect("stdout capture mutex poisoned")
        .clone();
    let stderr = stderr_capture
        .lock()
        .expect("stderr capture mutex poisoned")
        .clone();
    let combined = format!("{stdout}\n{stderr}");
    let mut summary = parse_verifier_output(
        &combined,
        status.success(),
        status.code(),
        timer.elapsed().as_millis(),
        target,
    );
    merge_verus_info(&mut summary.verus, query_verus_info(workspace_root));
    Ok(VerificationRun {
        summary,
        started_at,
    })
}

fn tee_stream<R: Read + Send + 'static>(
    reader: R,
    to_stderr: bool,
    capture: Arc<Mutex<String>>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            let count = reader.read_line(&mut line)?;
            if count == 0 {
                break;
            }
            if to_stderr {
                eprint!("{line}");
            } else {
                print!("{line}");
            }
            capture
                .lock()
                .expect("capture mutex poisoned")
                .push_str(&line);
        }
        Ok(())
    })
}

pub fn unconfirmed_summary(workspace_root: &Path) -> VerificationSummary {
    VerificationSummary {
        status: VerificationStatus::Unconfirmed,
        verus: query_verus_info(workspace_root),
        message: Some("static-only mode; no Verus result was used".to_string()),
        ..VerificationSummary::default()
    }
}

fn parse_verifier_output(
    output: &str,
    exit_success: bool,
    exit_code: Option<i32>,
    duration_ms: u128,
    target: &str,
) -> VerificationSummary {
    let documents = extract_json_documents(output);
    let verification_documents: Vec<_> = documents
        .iter()
        .filter(|document| document.get("verification-results").is_some())
        .collect();
    let selected = verification_documents
        .iter()
        .rev()
        .find(|document| {
            document
                .pointer("/verification-results/is-verifying-entire-crate")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .copied()
        .or_else(|| verification_documents.last().copied());

    let Some(document) = selected else {
        let plain_output = strip_ansi_codes(output);
        let wrapper_failed = plain_output.contains("Verification failed for");
        let wrapper_succeeded = plain_output.contains(&format!("Verified {target} "));
        if exit_success && wrapper_succeeded && !wrapper_failed {
            return VerificationSummary {
                status: VerificationStatus::Passed,
                exit_code,
                entire_crate: true,
                duration_ms: Some(duration_ms),
                message: Some(
                    "cargo dv reported whole-package verification success; per-query JSON was unavailable"
                        .to_string(),
                ),
                ..VerificationSummary::default()
            };
        }
        return VerificationSummary {
            status: VerificationStatus::Failed,
            exit_code,
            duration_ms: Some(duration_ms),
            message: Some("Verus emitted no machine-readable verification result".to_string()),
            ..VerificationSummary::default()
        };
    };
    let result = &document["verification-results"];
    let entire_crate = result
        .get("is-verifying-entire-crate")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let success = result
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let errors = result.get("errors").and_then(Value::as_u64);
    let passed = exit_success && entire_crate && success && errors.unwrap_or(0) == 0;

    let mut exec_functions = HashSet::new();
    let mut proof_functions = HashSet::new();
    let mut other_functions = HashSet::new();
    let mut saw_function_breakdown = false;
    if let Some(modules) = document
        .pointer("/times-ms/smt/smt-run-module-times")
        .and_then(Value::as_array)
    {
        for module in modules {
            let Some(functions) = module.get("function-breakdown").and_then(Value::as_array) else {
                continue;
            };
            saw_function_breakdown = true;
            for function in functions {
                let Some(name) = function.get("function").and_then(Value::as_str) else {
                    continue;
                };
                let mode = function
                    .get("mode:")
                    .or_else(|| function.get("mode"))
                    .and_then(Value::as_str);
                match mode {
                    Some("exec") => {
                        exec_functions.insert(name.to_string());
                    }
                    Some("proof") => {
                        proof_functions.insert(name.to_string());
                    }
                    _ => {
                        other_functions.insert(name.to_string());
                    }
                }
            }
        }
    }

    let verus = document.get("verus").unwrap_or(&Value::Null);
    VerificationSummary {
        status: if passed {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        },
        exit_code,
        entire_crate,
        verified_queries: result.get("verified").and_then(Value::as_u64),
        errors,
        duration_ms: Some(duration_ms),
        vc_functions: VcFunctionCounts {
            exec: saw_function_breakdown.then_some(exec_functions.len() as u64),
            proof: saw_function_breakdown.then_some(proof_functions.len() as u64),
            other: saw_function_breakdown.then_some(other_functions.len() as u64),
        },
        verus: VerusInfo {
            version: verus
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            commit: verus
                .get("commit")
                .and_then(Value::as_str)
                .map(str::to_string),
            toolchain: verus
                .get("toolchain")
                .and_then(Value::as_str)
                .map(str::to_string),
            platform_os: verus
                .pointer("/platform/os")
                .and_then(Value::as_str)
                .map(str::to_string),
            platform_arch: verus
                .pointer("/platform/arch")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        message: (!passed).then(|| {
            if !exit_success {
                "cargo dv verification failed".to_string()
            } else if !entire_crate {
                "Verus result was partial; whole-crate coverage was not confirmed".to_string()
            } else {
                "Verus reported verification errors".to_string()
            }
        }),
    }
}

fn query_verus_info(workspace_root: &Path) -> VerusInfo {
    let binary_name = if cfg!(windows) { "verus.exe" } else { "verus" };
    let mut candidates = Vec::new();
    if let Some(paths) = std::env::var_os("CARGO_VERUS_PATH") {
        candidates.extend(std::env::split_paths(&paths).map(|path| path.join(binary_name)));
    }
    candidates.extend([
        workspace_root
            .join("tools/verus/source/target-verus/release")
            .join(binary_name),
        workspace_root
            .join("tools/verus/source/target-verus/debug")
            .join(binary_name),
        PathBuf::from(binary_name),
    ]);

    for candidate in candidates {
        if candidate.components().count() > 1 && !candidate.is_file() {
            continue;
        }
        let Ok(output) = Command::new(&candidate).arg("--version").output() else {
            continue;
        };
        if output.status.success() {
            return parse_verus_version(&String::from_utf8_lossy(&output.stdout));
        }
    }
    VerusInfo::default()
}

fn parse_verus_version(output: &str) -> VerusInfo {
    let field = |name: &str| {
        output.lines().find_map(|line| {
            line.trim()
                .strip_prefix(name)
                .map(|value| value.trim().to_string())
        })
    };
    let version = field("Version:");
    let commit = version.as_deref().and_then(|value| {
        let suffix = value.rsplit('.').next()?;
        (suffix.len() >= 7 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| suffix.to_string())
    });
    let platform = field("Platform:");
    let (platform_os, platform_arch) = platform
        .as_deref()
        .and_then(|value| value.split_once('_'))
        .map(|(os, arch)| (Some(os.to_string()), Some(arch.to_string())))
        .unwrap_or_default();
    VerusInfo {
        version,
        commit,
        toolchain: field("Toolchain:"),
        platform_os,
        platform_arch,
    }
}

fn merge_verus_info(current: &mut VerusInfo, fallback: VerusInfo) {
    if current.version.is_none() {
        current.version = fallback.version;
    }
    if current.commit.is_none() {
        current.commit = fallback.commit;
    }
    if current.toolchain.is_none() {
        current.toolchain = fallback.toolchain;
    }
    if current.platform_os.is_none() {
        current.platform_os = fallback.platform_os;
    }
    if current.platform_arch.is_none() {
        current.platform_arch = fallback.platform_arch;
    }
}

fn strip_ansi_codes(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'[') {
            index += 2;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if (b'@'..=b'~').contains(&byte) {
                    break;
                }
            }
        } else {
            output.push(bytes[index] as char);
            index += 1;
        }
    }
    output
}

pub fn extract_json_documents(text: &str) -> Vec<Value> {
    let bytes = text.as_bytes();
    let mut documents = Vec::new();
    let mut start = None;
    let mut depth = 0_u64;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().copied().enumerate() {
        if start.is_none() {
            if byte == b'{' {
                start = Some(index);
                depth = 1;
                in_string = false;
                escaped = false;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let begin = start.take().expect("JSON start disappeared");
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes[begin..=index]) {
                        documents.push(value);
                    }
                }
            }
            _ => {}
        }
    }
    documents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_mixed_output() {
        let text = r#"warning: {not json}
{
  "verification-results": {
    "success": true,
    "verified": 2,
    "errors": 0,
    "is-verifying-entire-crate": true
  },
  "times-ms": {"smt": {"smt-run-module-times": [
    {"function-breakdown": [
      {"function": "demo::a", "mode:": "exec"},
      {"function": "demo::b", "mode": "proof"}
    ]}
  ]}},
  "verus": {"version": "test", "commit": "abc"}
}
done"#;
        let summary = parse_verifier_output(text, true, Some(0), 10, "demo");
        assert_eq!(summary.status, VerificationStatus::Passed);
        assert_eq!(summary.vc_functions.exec, Some(1));
        assert_eq!(summary.vc_functions.proof, Some(1));
    }

    #[test]
    fn partial_result_never_confirms_coverage() {
        let text = r#"{"verification-results":{"verified":1,"errors":0,"is-verifying-entire-crate":false}}"#;
        let summary = parse_verifier_output(text, true, Some(0), 1, "demo");
        assert_eq!(summary.status, VerificationStatus::Failed);
        assert!(!summary.entire_crate);
    }

    #[test]
    fn wrapper_whole_package_success_is_confirmed_without_json() {
        let summary =
            parse_verifier_output("  Verified demo 1.0.0 in 0.10s", true, Some(0), 100, "demo");
        assert_eq!(summary.status, VerificationStatus::Passed);
        assert!(summary.entire_crate);
        assert_eq!(summary.vc_functions.exec, None);
    }

    #[test]
    fn wrapper_failure_never_uses_cached_success_fallback() {
        let summary =
            parse_verifier_output("Verification failed for demo", true, Some(0), 100, "demo");
        assert_eq!(summary.status, VerificationStatus::Failed);
    }

    #[test]
    fn parses_verus_version_fallback() {
        let info = parse_verus_version(
            "Verus\n  Version: 0.2026.08.02.bb61343\n  Profile: release\n  Platform: macos_aarch64\n  Toolchain: 1.97.1-aarch64-apple-darwin\n",
        );
        assert_eq!(info.version.as_deref(), Some("0.2026.08.02.bb61343"));
        assert_eq!(info.commit.as_deref(), Some("bb61343"));
        assert_eq!(info.platform_os.as_deref(), Some("macos"));
        assert_eq!(info.platform_arch.as_deref(), Some("aarch64"));
    }
}
