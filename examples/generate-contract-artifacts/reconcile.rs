use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};

const MANAGED_ROOTS: &[&str] = &["examples/contracts", "schemas"];
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Generate,
    Check,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedArtifact {
    pub path: String,
    pub contents: Vec<u8>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub current: Vec<String>,
    pub written: Vec<String>,
    pub pruned: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Catalog,
    Render,
    Inspect,
    Commit,
    Prune,
    Verify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DurableState {
    Unchanged,
    RecoverableIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactError {
    pub phase: Phase,
    pub state: DurableState,
    pub problems: Vec<String>,
    pub recovery: &'static str,
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact operation failed during {:?} with state {:?}",
            self.phase, self.state
        )?;
        for problem in &self.problems {
            write!(formatter, "\n- {problem}")?;
        }
        write!(formatter, "\nrecovery: {}", self.recovery)
    }
}

pub(crate) fn reconcile(
    root: &Path,
    artifacts: &[RenderedArtifact],
    mode: Mode,
) -> Result<Report, ArtifactError> {
    let inspection = inspect(root, artifacts)?;
    if inspection.drift.is_empty() && inspection.fatal.is_empty() {
        return Ok(Report {
            current: inspection.current,
            ..Report::default()
        });
    }

    if mode == Mode::Check {
        let mut problems = inspection.fatal;
        problems.extend(inspection.drift);
        problems.sort();
        problems.dedup();
        return Err(ArtifactError {
            phase: Phase::Inspect,
            state: DurableState::Unchanged,
            problems,
            recovery: "run `cargo run --locked --example generate-contract-artifacts` and review the resulting diff",
        });
    }

    if !inspection.fatal.is_empty() {
        return Err(ArtifactError {
            phase: Phase::Inspect,
            state: DurableState::Unchanged,
            problems: inspection.fatal,
            recovery: "resolve unsafe or unreadable managed paths and retry",
        });
    }

    apply(root, artifacts, inspection)
}

#[derive(Debug)]
struct Inspection {
    current: Vec<String>,
    writes: Vec<FileSnapshot>,
    orphans: Vec<FileSnapshot>,
    drift: Vec<String>,
    fatal: Vec<String>,
}

#[derive(Debug, Clone)]
struct FileSnapshot {
    relative_path: String,
    contents: Option<Vec<u8>>,
}

fn inspect(root: &Path, artifacts: &[RenderedArtifact]) -> Result<Inspection, ArtifactError> {
    let expected: BTreeMap<&str, &[u8]> = artifacts
        .iter()
        .map(|artifact| (artifact.path.as_str(), artifact.contents.as_slice()))
        .collect();
    let mut current = Vec::new();
    let mut writes = Vec::new();
    let mut orphans = Vec::new();
    let mut drift = Vec::new();
    let mut fatal = Vec::new();

    for (relative_path, expected_contents) in &expected {
        if !is_managed_artifact_path(relative_path) {
            fatal.push(format!(
                "{relative_path} is outside the managed artifact roots"
            ));
            continue;
        }

        let path = root.join(relative_path);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                fatal.push(format!("{relative_path} is a symlink"));
            }
            Ok(metadata) if !metadata.is_file() => {
                fatal.push(format!("{relative_path} is not a regular file"));
            }
            Ok(_) => match fs::read(&path) {
                Ok(actual) if actual == *expected_contents => {
                    current.push((*relative_path).to_owned())
                }
                Ok(actual) => {
                    match serde_json::from_slice::<serde_json::Value>(&actual) {
                        Ok(_) => drift.push(format!("{relative_path} is stale")),
                        Err(error) => {
                            drift.push(format!("{relative_path} is invalid JSON: {error}"))
                        }
                    }
                    writes.push(FileSnapshot {
                        relative_path: (*relative_path).to_owned(),
                        contents: Some(actual),
                    });
                }
                Err(error) => fatal.push(format!("could not read {relative_path}: {error}")),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                drift.push(format!("{relative_path} is missing"));
                writes.push(FileSnapshot {
                    relative_path: (*relative_path).to_owned(),
                    contents: None,
                });
            }
            Err(error) => fatal.push(format!("could not inspect {relative_path}: {error}")),
        }
    }

    let expected_paths: BTreeSet<&str> = expected.keys().copied().collect();
    for managed_root in MANAGED_ROOTS {
        walk_managed_root(
            root,
            &root.join(managed_root),
            &expected_paths,
            &mut orphans,
            &mut drift,
            &mut fatal,
        )?;
    }
    current.sort();
    writes.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    orphans.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    drift.sort();
    drift.dedup();
    fatal.sort();
    fatal.dedup();

    Ok(Inspection {
        current,
        writes,
        orphans,
        drift,
        fatal,
    })
}

fn walk_managed_root(
    repository_root: &Path,
    path: &Path,
    expected_paths: &BTreeSet<&str>,
    orphans: &mut Vec<FileSnapshot>,
    drift: &mut Vec<String>,
    fatal: &mut Vec<String>,
) -> Result<(), ArtifactError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(inspect_error(format!(
                "could not inspect managed path {}: {error}",
                display_relative(repository_root, path)
            )));
        }
    };
    let relative_path = display_relative(repository_root, path);
    if metadata.file_type().is_symlink() {
        fatal.push(format!("{relative_path} is a symlink"));
        return Ok(());
    }
    if metadata.is_file() {
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && !expected_paths.contains(relative_path.as_str())
        {
            match fs::read(path) {
                Ok(contents) => {
                    drift.push(format!("{relative_path} is unregistered"));
                    orphans.push(FileSnapshot {
                        relative_path,
                        contents: Some(contents),
                    });
                }
                Err(error) => fatal.push(format!("could not read {relative_path}: {error}")),
            }
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        fatal.push(format!(
            "{relative_path} is not a regular file or directory"
        ));
        return Ok(());
    }

    let mut entries = fs::read_dir(path)
        .map_err(|error| inspect_error(format!("could not read {relative_path}: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| inspect_error(format!("could not enumerate {relative_path}: {error}")))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        walk_managed_root(
            repository_root,
            &entry.path(),
            expected_paths,
            orphans,
            drift,
            fatal,
        )?;
    }
    Ok(())
}

fn apply(
    root: &Path,
    artifacts: &[RenderedArtifact],
    inspection: Inspection,
) -> Result<Report, ArtifactError> {
    let expected: BTreeMap<&str, &[u8]> = artifacts
        .iter()
        .map(|artifact| (artifact.path.as_str(), artifact.contents.as_slice()))
        .collect();
    let mut report = Report {
        current: inspection.current,
        ..Report::default()
    };

    for snapshot in &inspection.writes {
        let contents = expected
            .get(snapshot.relative_path.as_str())
            .expect("write plans must refer to rendered artifacts");
        replace_file(root, snapshot, contents).map_err(|problem| ArtifactError {
            phase: Phase::Commit,
            state: durable_state(&report),
            problems: vec![problem],
            recovery: "preserve the worktree, resolve the named path, rerun generation, then run --check",
        })?;
        report.written.push(snapshot.relative_path.clone());
    }

    for snapshot in &inspection.orphans {
        remove_orphan(root, snapshot).map_err(|problem| ArtifactError {
            phase: Phase::Prune,
            state: durable_state(&report),
            problems: vec![problem],
            recovery: "the expected artifacts are valid; preserve the worktree, resolve the orphan path, rerun generation, then run --check",
        })?;
        report.pruned.push(snapshot.relative_path.clone());
    }

    let verification = inspect(root, artifacts)?;
    if !verification.drift.is_empty() || !verification.fatal.is_empty() {
        let mut problems = verification.fatal;
        problems.extend(verification.drift);
        problems.sort();
        return Err(ArtifactError {
            phase: Phase::Verify,
            state: DurableState::RecoverableIncomplete,
            problems,
            recovery: "rerun generation after concurrent filesystem activity stops, then run --check",
        });
    }

    Ok(report)
}

fn replace_file(root: &Path, snapshot: &FileSnapshot, contents: &[u8]) -> Result<(), String> {
    let destination = root.join(&snapshot.relative_path);
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", snapshot.relative_path))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create {}: {error}",
            display_relative(root, parent)
        )
    })?;
    ensure_snapshot(root, snapshot)?;

    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let file_name = destination
        .file_name()
        .ok_or_else(|| format!("{} has no file name", snapshot.relative_path))?
        .to_string_lossy();
    let temporary = parent.join(format!(
        ".{file_name}.gaap-contract-artifacts-{}-{sequence}.tmp",
        std::process::id()
    ));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| {
                format!(
                    "could not create temporary file for {}: {error}",
                    snapshot.relative_path
                )
            })?;
        file.write_all(contents).map_err(|error| {
            format!(
                "could not write temporary file for {}: {error}",
                snapshot.relative_path
            )
        })?;
        file.flush().map_err(|error| {
            format!(
                "could not flush temporary file for {}: {error}",
                snapshot.relative_path
            )
        })?;
        file.sync_all().map_err(|error| {
            format!(
                "could not sync temporary file for {}: {error}",
                snapshot.relative_path
            )
        })?;
        drop(file);
        ensure_snapshot(root, snapshot)?;
        fs::rename(&temporary, &destination).map_err(|error| {
            format!(
                "could not replace {} with its validated temporary file: {error}",
                snapshot.relative_path
            )
        })
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn remove_orphan(root: &Path, snapshot: &FileSnapshot) -> Result<(), String> {
    ensure_snapshot(root, snapshot)?;
    fs::remove_file(root.join(&snapshot.relative_path))
        .map_err(|error| format!("could not prune {}: {error}", snapshot.relative_path))
}

fn ensure_snapshot(root: &Path, snapshot: &FileSnapshot) -> Result<(), String> {
    let path = root.join(&snapshot.relative_path);
    match (&snapshot.contents, fs::symlink_metadata(&path)) {
        (None, Err(error)) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        (None, Ok(_)) => Err(format!(
            "{} changed after inspection; concurrent work was preserved",
            snapshot.relative_path
        )),
        (None, Err(error)) => Err(format!(
            "could not recheck {} before mutation: {error}",
            snapshot.relative_path
        )),
        (Some(_), Ok(metadata)) if metadata.file_type().is_symlink() => Err(format!(
            "{} became a symlink after inspection; it was preserved",
            snapshot.relative_path
        )),
        (Some(_), Ok(metadata)) if !metadata.is_file() => Err(format!(
            "{} is no longer a regular file; it was preserved",
            snapshot.relative_path
        )),
        (Some(expected), Ok(_)) => {
            let actual = fs::read(&path).map_err(|error| {
                format!(
                    "could not re-read {} before mutation: {error}",
                    snapshot.relative_path
                )
            })?;
            if actual == *expected {
                Ok(())
            } else {
                Err(format!(
                    "{} changed after inspection; concurrent work was preserved",
                    snapshot.relative_path
                ))
            }
        }
        (Some(_), Err(error)) => Err(format!(
            "could not recheck {} before mutation: {error}",
            snapshot.relative_path
        )),
    }
}

fn durable_state(report: &Report) -> DurableState {
    if report.written.is_empty() && report.pruned.is_empty() {
        DurableState::Unchanged
    } else {
        DurableState::RecoverableIncomplete
    }
}

fn is_managed_artifact_path(relative_path: &str) -> bool {
    let path = Path::new(relative_path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && MANAGED_ROOTS
            .iter()
            .any(|managed_root| path.starts_with(managed_root))
        && path
            .extension()
            .is_some_and(|extension| extension == "json")
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn inspect_error(problem: String) -> ArtifactError {
    ArtifactError {
        phase: Phase::Inspect,
        state: DurableState::Unchanged,
        problems: vec![problem],
        recovery: "fix the filesystem access problem and retry",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(1);

    struct TempRoot(std::path::PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let sequence = NEXT_TEMP_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "gaap-contract-artifacts-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary root should be created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative_path: &str, contents: &[u8]) {
            let path = self.0.join(relative_path);
            fs::create_dir_all(path.parent().expect("fixture path should have parent"))
                .expect("fixture parent should be created");
            fs::write(path, contents).expect("fixture should be written");
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn artifact(path: &str, contents: &[u8]) -> RenderedArtifact {
        RenderedArtifact {
            path: path.to_owned(),
            contents: contents.to_vec(),
        }
    }

    #[test]
    fn check_reports_all_drift_without_mutating_the_repository() {
        let root = TempRoot::new();
        root.write("schemas/example/v0.1.0/stale.json", b"{}\n");
        root.write("schemas/example/v0.1.0/invalid.json", b"not json\n");
        root.write("examples/contracts/example/v0.1.0/orphan.json", b"orphan\n");
        let artifacts = [
            artifact("schemas/example/v0.1.0/stale.json", b"{\"new\":true}\n"),
            artifact("schemas/example/v0.1.0/invalid.json", b"{}\n"),
            artifact("examples/contracts/example/v0.1.0/missing.json", b"{}\n"),
        ];

        let error = reconcile(root.path(), &artifacts, Mode::Check)
            .expect_err("missing, stale, and orphaned artifacts must fail");

        assert_eq!(error.phase, Phase::Inspect);
        assert_eq!(error.state, DurableState::Unchanged);
        assert!(
            error
                .problems
                .iter()
                .any(|problem| problem.contains("stale.json is stale"))
        );
        assert!(
            error
                .problems
                .iter()
                .any(|problem| problem.contains("missing.json is missing"))
        );
        assert!(
            error
                .problems
                .iter()
                .any(|problem| problem.contains("invalid.json is invalid JSON"))
        );
        assert!(
            error
                .problems
                .iter()
                .any(|problem| problem.contains("orphan.json is unregistered"))
        );
        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/stale.json"))
                .expect("stale fixture should remain"),
            b"{}\n"
        );
        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/invalid.json"))
                .expect("invalid fixture should remain"),
            b"not json\n"
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join("examples/contracts/example/v0.1.0/orphan.json")
            )
            .expect("orphan fixture should remain"),
            b"orphan\n"
        );
        assert!(
            !root
                .path()
                .join("examples/contracts/example/v0.1.0/missing.json")
                .exists()
        );
    }

    #[test]
    fn generate_replaces_drift_and_prunes_only_unregistered_json() {
        let root = TempRoot::new();
        root.write("schemas/example/v0.1.0/stale.json", b"old\n");
        root.write("examples/contracts/example/v0.1.0/orphan.json", b"orphan\n");
        root.write("examples/contracts/example/v0.1.0/notes.txt", b"keep\n");
        let artifacts = [
            artifact("schemas/example/v0.1.0/stale.json", b"new schema\n"),
            artifact(
                "examples/contracts/example/v0.1.0/missing.json",
                b"new example\n",
            ),
        ];

        let report = reconcile(root.path(), &artifacts, Mode::Generate)
            .expect("generation should reconcile the managed artifacts");

        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/stale.json")).unwrap(),
            b"new schema\n"
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join("examples/contracts/example/v0.1.0/missing.json")
            )
            .unwrap(),
            b"new example\n"
        );
        assert!(
            !root
                .path()
                .join("examples/contracts/example/v0.1.0/orphan.json")
                .exists()
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join("examples/contracts/example/v0.1.0/notes.txt")
            )
            .unwrap(),
            b"keep\n"
        );
        assert_eq!(
            report.written,
            vec![
                "examples/contracts/example/v0.1.0/missing.json",
                "schemas/example/v0.1.0/stale.json"
            ]
        );
        assert_eq!(
            report.pruned,
            vec!["examples/contracts/example/v0.1.0/orphan.json"]
        );
        reconcile(root.path(), &artifacts, Mode::Check)
            .expect("successful generation must immediately pass check mode");
    }

    #[test]
    fn concurrent_change_after_inspection_is_preserved_with_recovery_state() {
        let root = TempRoot::new();
        root.write("schemas/example/v0.1.0/a.json", b"old a\n");
        root.write("schemas/example/v0.1.0/b.json", b"old b\n");
        let artifacts = [
            artifact("schemas/example/v0.1.0/a.json", b"new a\n"),
            artifact("schemas/example/v0.1.0/b.json", b"new b\n"),
        ];
        let inspection = inspect(root.path(), &artifacts).expect("inspection should succeed");
        root.write("schemas/example/v0.1.0/b.json", b"concurrent b\n");

        let error = apply(root.path(), &artifacts, inspection)
            .expect_err("concurrent change must stop reconciliation");

        assert_eq!(error.phase, Phase::Commit);
        assert_eq!(error.state, DurableState::RecoverableIncomplete);
        assert!(error.problems[0].contains("concurrent work was preserved"));
        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/a.json")).unwrap(),
            b"new a\n"
        );
        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/b.json")).unwrap(),
            b"concurrent b\n"
        );
    }

    #[test]
    fn changed_orphan_is_preserved_after_expected_artifacts_commit() {
        let root = TempRoot::new();
        root.write("schemas/example/v0.1.0/current.json", b"old\n");
        root.write("schemas/example/v0.1.0/orphan.json", b"orphan\n");
        let artifacts = [artifact("schemas/example/v0.1.0/current.json", b"new\n")];
        let inspection = inspect(root.path(), &artifacts).expect("inspection should succeed");
        root.write("schemas/example/v0.1.0/orphan.json", b"concurrent orphan\n");

        let error = apply(root.path(), &artifacts, inspection)
            .expect_err("changed orphan must not be removed");

        assert_eq!(error.phase, Phase::Prune);
        assert_eq!(error.state, DurableState::RecoverableIncomplete);
        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/current.json")).unwrap(),
            b"new\n"
        );
        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/orphan.json")).unwrap(),
            b"concurrent orphan\n"
        );
    }

    #[test]
    fn pre_commit_parent_failure_leaves_existing_artifacts_unchanged() {
        let root = TempRoot::new();
        root.write("schemas/example/v0.1.0/current.json", b"current\n");
        let artifacts = [
            artifact("schemas/example/v0.1.0/current.json", b"current\n"),
            artifact("schemas/blocked/v0.1.0/missing.json", b"new\n"),
        ];
        let inspection = inspect(root.path(), &artifacts).expect("inspection should succeed");
        root.write("schemas/blocked", b"not a directory\n");

        let error = apply(root.path(), &artifacts, inspection)
            .expect_err("uncreatable parent must stop before replacement");

        assert_eq!(error.phase, Phase::Commit);
        assert_eq!(error.state, DurableState::Unchanged);
        assert_eq!(
            fs::read(root.path().join("schemas/example/v0.1.0/current.json")).unwrap(),
            b"current\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn check_rejects_symlinks_inside_managed_roots() {
        use std::os::unix::fs::symlink;

        let root = TempRoot::new();
        root.write("outside.json", b"outside\n");
        let managed = root.path().join("schemas/example/v0.1.0");
        fs::create_dir_all(&managed).expect("managed directory should be created");
        symlink(
            root.path().join("outside.json"),
            managed.join("request.json"),
        )
        .expect("fixture symlink should be created");

        let error = reconcile(
            root.path(),
            &[artifact("schemas/example/v0.1.0/request.json", b"new\n")],
            Mode::Check,
        )
        .expect_err("managed symlink must fail closed");

        assert!(
            error
                .problems
                .iter()
                .any(|problem| problem.contains("symlink"))
        );
        assert_eq!(
            fs::read(root.path().join("outside.json")).unwrap(),
            b"outside\n"
        );
    }
}
