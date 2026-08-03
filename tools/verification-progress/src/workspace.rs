use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::model::RepositoryInfo;

#[derive(Debug, Clone)]
pub struct PackageInput {
    pub name: String,
    pub crate_name: String,
    pub root: PathBuf,
    pub source_entry: PathBuf,
    pub features: Vec<String>,
}

#[derive(Debug)]
pub struct WorkspaceInput {
    pub root: PathBuf,
    pub target_directory: PathBuf,
    pub packages: Vec<PackageInput>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
    target_directory: PathBuf,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
    manifest_path: PathBuf,
    metadata: Value,
    targets: Vec<CargoTarget>,
    dependencies: Vec<CargoDependency>,
    features: HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CargoDependency {
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

pub fn load_workspace(root: &Path, target_name: &str) -> Result<WorkspaceInput> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .context("failed to run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let metadata: CargoMetadata =
        serde_json::from_slice(&output.stdout).context("invalid cargo metadata JSON")?;
    let workspace_members: HashSet<_> = metadata.workspace_members.iter().cloned().collect();
    let packages_by_id: HashMap<_, _> = metadata
        .packages
        .iter()
        .map(|package| (package.id.clone(), package))
        .collect();
    let package_by_root: HashMap<_, _> = metadata
        .packages
        .iter()
        .filter_map(|package| {
            package
                .manifest_path
                .parent()
                .and_then(|path| path.canonicalize().ok())
                .map(|path| (path, package.id.clone()))
        })
        .collect();
    let target = metadata
        .packages
        .iter()
        .find(|package| package.name == target_name)
        .with_context(|| {
            format!("verification target `{target_name}` is not a workspace package")
        })?;

    let mut closure = HashSet::new();
    let mut queue = VecDeque::from([target.id.clone()]);
    while let Some(id) = queue.pop_front() {
        if !closure.insert(id.clone()) {
            continue;
        }
        if let Some(package) = packages_by_id.get(&id) {
            for dependency in &package.dependencies {
                let Some(path) = &dependency.path else {
                    continue;
                };
                let normalized = path.canonicalize().unwrap_or_else(|_| path.clone());
                if let Some(dependency_id) = package_by_root.get(&normalized) {
                    queue.push_back(dependency_id.clone());
                }
            }
        }
    }

    let mut selected = Vec::new();
    for id in closure {
        if !workspace_members.contains(&id) {
            continue;
        }
        let Some(package) = packages_by_id.get(&id) else {
            continue;
        };
        if !package
            .metadata
            .pointer("/verus/verify")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let cargo_target = package
            .targets
            .iter()
            .find(|target| {
                target
                    .kind
                    .iter()
                    .any(|kind| kind == "lib" || kind == "rlib")
            })
            .with_context(|| format!("package `{}` has no library target", package.name))?;
        let features = expanded_default_features(&package.features);
        selected.push(PackageInput {
            name: package.name.clone(),
            crate_name: cargo_target.name.clone(),
            root: package
                .manifest_path
                .parent()
                .context("manifest has no parent")?
                .to_path_buf(),
            source_entry: cargo_target.src_path.clone(),
            features,
        });
    }
    selected.sort_by(|left, right| {
        let left_key = (left.name != target_name, left.name.as_str());
        let right_key = (right.name != target_name, right.name.as_str());
        left_key.cmp(&right_key)
    });

    Ok(WorkspaceInput {
        root: metadata
            .workspace_root
            .canonicalize()
            .unwrap_or(metadata.workspace_root),
        target_directory: metadata.target_directory,
        packages: selected,
    })
}

fn expanded_default_features(features: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut enabled = BTreeSet::new();
    let mut queue: VecDeque<_> = features.get("default").cloned().unwrap_or_default().into();
    while let Some(feature) = queue.pop_front() {
        if feature.starts_with("dep:") || feature.contains('/') || !enabled.insert(feature.clone())
        {
            continue;
        }
        if let Some(expansion) = features.get(&feature) {
            queue.extend(expansion.iter().cloned());
        }
    }
    enabled.into_iter().collect()
}

pub fn repository_info(root: &Path) -> RepositoryInfo {
    let commit =
        command_text(root, &["git", "rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = command_text(root, &["git", "status", "--porcelain"])
        .map(|value| !value.trim().is_empty())
        .unwrap_or(true);
    RepositoryInfo { commit, dirty }
}

fn command_text(root: &Path, command: &[&str]) -> Option<String> {
    let output = Command::new(command.first()?)
        .args(&command[1..])
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn inventory_files(package: &PackageInput, target_name: &str) -> Result<Vec<PathBuf>> {
    let mut roots = vec![package.root.join("src")];
    if package.name == target_name && package.root.join("specs").is_dir() {
        roots.push(package.root.join("specs"));
    }
    let mut files = BTreeSet::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&root).follow_links(false) {
            let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
            if entry.file_type().is_file() && entry.path().extension() == Some(OsStr::new("rs")) {
                files.insert(entry.path().canonicalize().with_context(|| {
                    format!("failed to canonicalize {}", entry.path().display())
                })?);
            }
        }
    }
    Ok(files.into_iter().collect())
}

pub fn x86_main_inventory(files: &[PathBuf], package: &PackageInput) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|path| {
            let relative = path.strip_prefix(&package.root).unwrap_or(path);
            !relative.starts_with("src/arch/riscv") && !relative.starts_with("src/arch/loongarch")
        })
        .cloned()
        .collect()
}

pub fn architecture_inventory<'a>(
    files: &'a [PathBuf],
    package: &PackageInput,
    architecture: &str,
) -> Vec<&'a PathBuf> {
    let prefix = PathBuf::from(format!("src/arch/{architecture}"));
    files
        .iter()
        .filter(|path| {
            path.strip_prefix(&package.root)
                .unwrap_or(path)
                .starts_with(&prefix)
        })
        .collect()
}

#[derive(Debug)]
struct DepInfoCandidate {
    path: PathBuf,
    modified: SystemTime,
    files: BTreeSet<PathBuf>,
}

pub fn active_files_from_dep_info(
    workspace: &WorkspaceInput,
    package: &PackageInput,
    verification_started: Option<SystemTime>,
) -> Result<Vec<PathBuf>> {
    let mut candidates = find_dep_candidates(workspace, package)?;
    if candidates.is_empty() {
        bail!("no usable dep-info found for package `{}`", package.name);
    }

    candidates.retain(candidate_is_fresh);
    if candidates.is_empty() {
        bail!(
            "all dep-info artifacts for package `{}` are older than their source inputs",
            package.name
        );
    }

    if let Some(started) = verification_started {
        let tolerance = started
            .checked_sub(Duration::from_secs(2))
            .unwrap_or(started);
        if candidates
            .iter()
            .any(|candidate| candidate.modified >= tolerance)
        {
            candidates.retain(|candidate| candidate.modified >= tolerance);
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.modified));
    ensure_unambiguous_inputs(&candidates, &package.name)?;
    let newest = candidates
        .first()
        .context("dep-info candidate list became empty")?;
    Ok(newest.files.iter().cloned().collect())
}

fn ensure_unambiguous_inputs(candidates: &[DepInfoCandidate], package_name: &str) -> Result<()> {
    let newest = candidates
        .first()
        .context("dep-info candidate list became empty")?;
    for candidate in candidates.iter().skip(1) {
        if candidate.files != newest.files {
            bail!(
                "ambiguous current dep-info for `{}`: {} and {} describe different fresh inputs",
                package_name,
                newest.path.display(),
                candidate.path.display()
            );
        }
    }
    Ok(())
}

fn candidate_is_fresh(candidate: &DepInfoCandidate) -> bool {
    let mut input_times = Vec::with_capacity(candidate.files.len());
    for path in &candidate.files {
        let Ok(modified) = path.metadata().and_then(|metadata| metadata.modified()) else {
            return false;
        };
        input_times.push(modified);
    }
    input_times_are_fresh(candidate.modified, input_times)
}

fn input_times_are_fresh(
    dep_info_modified: SystemTime,
    input_times: impl IntoIterator<Item = SystemTime>,
) -> bool {
    input_times
        .into_iter()
        .all(|modified| modified <= dep_info_modified)
}

fn find_dep_candidates(
    workspace: &WorkspaceInput,
    package: &PackageInput,
) -> Result<Vec<DepInfoCandidate>> {
    let mut result = Vec::new();
    // `make progress` always uses cargo-verus' development profile.
    for profile in ["debug"] {
        let deps_dir = workspace.target_directory.join(profile).join("deps");
        let Ok(entries) = std::fs::read_dir(&deps_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if !file_name.starts_with(&format!("{}-", package.crate_name))
                || path.extension() != Some(OsStr::new("d"))
            {
                continue;
            }
            let files = parse_dep_info(&path, &workspace.root, &package.root)?;
            let source_entry = package
                .source_entry
                .canonicalize()
                .unwrap_or_else(|_| package.source_entry.clone());
            if files.contains(&source_entry) {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                result.push(DepInfoCandidate {
                    path,
                    modified,
                    files,
                });
            }
        }
    }
    Ok(result)
}

pub fn parse_dep_info(
    dep_info: &Path,
    workspace_root: &Path,
    package_root: &Path,
) -> Result<BTreeSet<PathBuf>> {
    let content = std::fs::read_to_string(dep_info)
        .with_context(|| format!("failed to read dep-info {}", dep_info.display()))?;
    parse_dep_info_text(&content, workspace_root, package_root)
}

fn parse_dep_info_text(
    content: &str,
    workspace_root: &Path,
    package_root: &Path,
) -> Result<BTreeSet<PathBuf>> {
    let joined = content.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut files = BTreeSet::new();
    for line in joined.lines() {
        let Some((_, dependencies)) = line.split_once(": ") else {
            continue;
        };
        for token in makefile_words(dependencies) {
            if !token.ends_with(".rs") {
                continue;
            }
            let path = PathBuf::from(token);
            let absolute = if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            };
            let normalized = absolute.canonicalize().unwrap_or(absolute);
            if normalized.starts_with(package_root) {
                files.insert(normalized);
            }
        }
    }
    Ok(files)
}

fn makefile_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.peek().copied() {
                Some(next) if next.is_whitespace() || next == '\\' => {
                    word.push(
                        characters
                            .next()
                            .expect("peeked makefile escape disappeared"),
                    );
                }
                _ => word.push(character),
            }
        } else if character.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(character);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

pub fn physical_lines(path: &Path) -> Result<u64> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(source.lines().count() as u64)
}

pub fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn subsystem_for(package: &PackageInput, path: &Path) -> String {
    let relative = path.strip_prefix(&package.root).unwrap_or(path);
    let parts: Vec<_> = relative.iter().filter_map(OsStr::to_str).collect();
    match parts.as_slice() {
        ["src", "arch", arch, ..] => format!("arch/{arch}"),
        ["src", component, ..] if *component != "lib.rs" => {
            component.trim_end_matches(".rs").to_string()
        }
        ["specs", component, ..] if *component != "mod.rs" => {
            format!("specs/{}", component.trim_end_matches(".rs"))
        }
        ["src", ..] => "root".to_string(),
        ["specs", ..] => "specs".to_string(),
        _ => "other".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_deduplicates_dep_info() {
        let temp = std::env::temp_dir();
        let directory_name = format!("verification-progress-dep-test-{}", std::process::id());
        let package = temp.join(&directory_name);
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(package.join("src/lib.rs"), "").unwrap();
        std::fs::write(package.join("src/a.rs"), "").unwrap();
        let text = format!(
            "target: {directory_name}/src/lib.rs {directory_name}/src/a.rs\nother: {directory_name}/src/a.rs\n"
        );
        let files = parse_dep_info_text(&text, &temp, &package.canonicalize().unwrap()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&package.join("src/lib.rs").canonicalize().unwrap()));
        assert!(files.contains(&package.join("src/a.rs").canonicalize().unwrap()));
        std::fs::remove_dir_all(package).unwrap();
    }

    #[test]
    fn parses_makefile_escaped_spaces() {
        assert_eq!(
            makefile_words(r"src/lib.rs directory\ with\ spaces/file.rs"),
            ["src/lib.rs", "directory with spaces/file.rs"]
        );
    }

    #[test]
    fn detects_stale_and_ambiguous_dep_info() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        assert!(input_times_are_fresh(base, [base]));
        assert!(!input_times_are_fresh(
            base,
            [base + Duration::from_secs(1)]
        ));

        let candidates = vec![
            DepInfoCandidate {
                path: PathBuf::from("first.d"),
                modified: base,
                files: BTreeSet::from([PathBuf::from("src/lib.rs")]),
            },
            DepInfoCandidate {
                path: PathBuf::from("second.d"),
                modified: base,
                files: BTreeSet::from([PathBuf::from("src/lib.rs"), PathBuf::from("src/extra.rs")]),
            },
        ];
        assert!(ensure_unambiguous_inputs(&candidates, "demo").is_err());
        assert!(ensure_unambiguous_inputs(&[], "demo").is_err());

        let missing_input = DepInfoCandidate {
            path: PathBuf::from("missing.d"),
            modified: base,
            files: BTreeSet::from([std::env::temp_dir().join(format!(
                "verification-progress-missing-input-{}",
                std::process::id()
            ))]),
        };
        assert!(!candidate_is_fresh(&missing_input));
    }
}
