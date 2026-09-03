use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
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

#[derive(Debug)]
struct MutationFailure {
    problem: String,
    filesystem_changed: bool,
}

impl MutationFailure {
    fn with_state(problem: String, filesystem_changed: bool) -> Self {
        Self {
            problem,
            filesystem_changed,
        }
    }

    fn unchanged(problem: String) -> Self {
        Self::with_state(problem, false)
    }

    #[cfg(any(windows, test))]
    fn recoverable(problem: String) -> Self {
        Self::with_state(problem, true)
    }
}

#[derive(Debug)]
struct ValidatedParent {
    path: PathBuf,
    filesystem_changed: bool,
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
        if is_transaction_artifact(path) {
            fatal.push(format!(
                "{relative_path} is an unfinished artifact transaction file"
            ));
        } else if path
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
        replace_file(root, snapshot, contents).map_err(|failure| ArtifactError {
            phase: Phase::Commit,
            state: mutation_failure_state(&report, &failure),
            problems: vec![failure.problem],
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

fn replace_file(
    root: &Path,
    snapshot: &FileSnapshot,
    contents: &[u8],
) -> Result<(), MutationFailure> {
    let ValidatedParent {
        path: parent,
        filesystem_changed: parent_changed,
    } = validated_parent(root, &snapshot.relative_path, true)?;
    let file_name = Path::new(&snapshot.relative_path)
        .file_name()
        .ok_or_else(|| format!("{} has no file name", snapshot.relative_path))
        .map_err(|problem| MutationFailure::with_state(problem, parent_changed))?;
    let destination = parent.join(file_name);
    ensure_snapshot_at(&destination, snapshot)
        .map_err(|problem| MutationFailure::with_state(problem, parent_changed))?;

    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let file_name = file_name.to_string_lossy();
    let temporary = parent.join(format!(
        ".{file_name}.gaap-contract-artifacts-{}-{sequence}.tmp",
        std::process::id()
    ));
    let backup = parent.join(format!(
        ".{file_name}.gaap-contract-artifacts-{}-{sequence}.backup",
        std::process::id()
    ));
    let mut temporary_created = false;
    let preparation = (|| {
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
        temporary_created = true;
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
        let revalidated_parent = validated_parent(root, &snapshot.relative_path, false)
            .map_err(|failure| failure.problem)?
            .path;
        if revalidated_parent != parent {
            return Err(format!(
                "{} parent changed after temporary-file creation; it was preserved",
                snapshot.relative_path
            ));
        }
        ensure_snapshot_at(&destination, snapshot)?;
        Ok(())
    })();
    if let Err(problem) = preparation {
        let failure = MutationFailure::with_state(problem, parent_changed);
        return Err(if temporary_created {
            cleanup_temporary(&temporary, failure)
        } else {
            failure
        });
    }

    match replace_temporary(
        &temporary,
        &destination,
        &backup,
        snapshot.contents.is_some(),
        &snapshot.relative_path,
    ) {
        Ok(()) => Ok(()),
        Err(mut failure) => {
            failure.filesystem_changed |= parent_changed;
            Err(cleanup_temporary(&temporary, failure))
        }
    }
}

fn cleanup_temporary(temporary: &Path, mut failure: MutationFailure) -> MutationFailure {
    match fs::remove_file(temporary) {
        Ok(()) => failure,
        Err(error) if error.kind() == io::ErrorKind::NotFound => failure,
        Err(error) => {
            failure.filesystem_changed = true;
            failure.problem.push_str(&format!(
                "; temporary file {} also requires cleanup: {error}",
                temporary.display()
            ));
            failure
        }
    }
}

#[cfg(not(windows))]
fn replace_temporary(
    temporary: &Path,
    destination: &Path,
    _backup: &Path,
    _destination_exists: bool,
    relative_path: &str,
) -> Result<(), MutationFailure> {
    fs::rename(temporary, destination).map_err(|error| {
        MutationFailure::unchanged(format!(
            "could not replace {relative_path} with its validated temporary file: {error}"
        ))
    })
}

#[cfg(windows)]
fn replace_temporary(
    temporary: &Path,
    destination: &Path,
    backup: &Path,
    destination_exists: bool,
    relative_path: &str,
) -> Result<(), MutationFailure> {
    if destination_exists {
        replace_existing_with_backup(temporary, destination, backup, relative_path)
    } else {
        fs::rename(temporary, destination).map_err(|error| {
            MutationFailure::unchanged(format!(
                "could not install missing artifact {relative_path}: {error}"
            ))
        })
    }
}

#[cfg(any(windows, test))]
fn replace_existing_with_backup(
    temporary: &Path,
    destination: &Path,
    backup: &Path,
    relative_path: &str,
) -> Result<(), MutationFailure> {
    match fs::symlink_metadata(backup) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(MutationFailure::unchanged(format!(
                "could not preserve {relative_path}: backup path {} already exists",
                backup.display()
            )));
        }
        Err(error) => {
            return Err(MutationFailure::unchanged(format!(
                "could not inspect backup path for {relative_path}: {error}"
            )));
        }
    }

    fs::rename(destination, backup).map_err(|error| {
        MutationFailure::unchanged(format!(
            "could not preserve existing {relative_path}: {error}"
        ))
    })?;
    match fs::rename(temporary, destination) {
        Ok(()) => fs::remove_file(backup).map_err(|error| {
            MutationFailure::recoverable(format!(
                "installed {relative_path}, but could not remove backup {}: {error}",
                backup.display()
            ))
        }),
        Err(install_error) => match fs::rename(backup, destination) {
            Ok(()) => Err(MutationFailure::unchanged(format!(
                "could not install {relative_path}: {install_error}; restored the previous destination"
            ))),
            Err(restore_error) => Err(MutationFailure::recoverable(format!(
                "could not install {relative_path}: {install_error}; the previous destination remains at {} because restoration failed: {restore_error}",
                backup.display()
            ))),
        },
    }
}

fn remove_orphan(root: &Path, snapshot: &FileSnapshot) -> Result<(), String> {
    let parent = validated_parent(root, &snapshot.relative_path, false)
        .map_err(|failure| failure.problem)?
        .path;
    let file_name = Path::new(&snapshot.relative_path)
        .file_name()
        .ok_or_else(|| format!("{} has no file name", snapshot.relative_path))?;
    let path = parent.join(file_name);
    ensure_snapshot_at(&path, snapshot)?;
    fs::remove_file(path)
        .map_err(|error| format!("could not prune {}: {error}", snapshot.relative_path))
}

fn ensure_snapshot_at(path: &Path, snapshot: &FileSnapshot) -> Result<(), String> {
    match (&snapshot.contents, fs::symlink_metadata(path)) {
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
            let actual = fs::read(path).map_err(|error| {
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

fn validated_parent(
    root: &Path,
    relative_path: &str,
    create_missing: bool,
) -> Result<ValidatedParent, MutationFailure> {
    if !is_managed_artifact_path(relative_path) {
        return Err(MutationFailure::unchanged(format!(
            "{relative_path} is outside the managed artifact roots"
        )));
    }
    let relative_parent = Path::new(relative_path).parent().ok_or_else(|| {
        MutationFailure::unchanged(format!("{relative_path} has no parent directory"))
    })?;
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        MutationFailure::unchanged(format!("could not resolve repository root: {error}"))
    })?;
    let mut parent = root.to_path_buf();
    let mut filesystem_changed = false;

    for component in relative_parent.components() {
        let Component::Normal(segment) = component else {
            return Err(MutationFailure::with_state(
                format!("{relative_path} has an unsafe parent component"),
                filesystem_changed,
            ));
        };
        parent.push(segment);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(MutationFailure::with_state(
                    format!("{} parent is a symlink", display_relative(root, &parent)),
                    filesystem_changed,
                ));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(MutationFailure::with_state(
                    format!(
                        "{} parent is not a directory",
                        display_relative(root, &parent)
                    ),
                    filesystem_changed,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && create_missing => {
                match fs::create_dir(&parent) {
                    Ok(()) => filesystem_changed = true,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(MutationFailure::with_state(
                            format!(
                                "could not create {}: {error}",
                                display_relative(root, &parent)
                            ),
                            filesystem_changed,
                        ));
                    }
                }
                let metadata = fs::symlink_metadata(&parent).map_err(|error| {
                    MutationFailure::with_state(
                        format!(
                            "could not recheck created parent {}: {error}",
                            display_relative(root, &parent)
                        ),
                        filesystem_changed,
                    )
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(MutationFailure::with_state(
                        format!(
                            "{} parent became a symlink or non-directory",
                            display_relative(root, &parent)
                        ),
                        filesystem_changed,
                    ));
                }
            }
            Err(error) => {
                return Err(MutationFailure::with_state(
                    format!(
                        "could not inspect parent {}: {error}",
                        display_relative(root, &parent)
                    ),
                    filesystem_changed,
                ));
            }
        }
    }

    let canonical_parent = fs::canonicalize(&parent).map_err(|error| {
        MutationFailure::with_state(
            format!(
                "could not resolve parent {}: {error}",
                display_relative(root, &parent)
            ),
            filesystem_changed,
        )
    })?;
    let expected_parent = canonical_root.join(relative_parent);
    if canonical_parent != expected_parent {
        return Err(MutationFailure::with_state(
            format!("{relative_path} parent resolves through a symlink outside its catalog path"),
            filesystem_changed,
        ));
    }
    Ok(ValidatedParent {
        path: canonical_parent,
        filesystem_changed,
    })
}

fn durable_state(report: &Report) -> DurableState {
    if report.written.is_empty() && report.pruned.is_empty() {
        DurableState::Unchanged
    } else {
        DurableState::RecoverableIncomplete
    }
}

fn mutation_failure_state(report: &Report, failure: &MutationFailure) -> DurableState {
    if failure.filesystem_changed {
        DurableState::RecoverableIncomplete
    } else {
        durable_state(report)
    }
}

fn is_transaction_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|file_name| file_name.to_str())
        .is_some_and(|file_name| {
            file_name.starts_with('.')
                && file_name.contains(".gaap-contract-artifacts-")
                && (file_name.ends_with(".tmp") || file_name.ends_with(".backup"))
        })
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

    #[test]
    fn failure_after_parent_creation_reports_recoverable_incomplete_state() {
        let root = TempRoot::new();
        let long_file_name = format!("{}.json", "x".repeat(240));
        let relative_path = format!("schemas/new/{long_file_name}");
        let artifacts = [artifact(&relative_path, b"{}\n")];

        let error = reconcile(root.path(), &artifacts, Mode::Generate)
            .expect_err("temporary-file creation should exceed the component length limit");

        assert_eq!(error.phase, Phase::Commit);
        assert_eq!(error.state, DurableState::RecoverableIncomplete);
        assert!(error.problems[0].contains("could not create temporary file"));
        assert!(root.path().join("schemas/new").is_dir());
    }

    #[test]
    fn parent_validation_records_directories_created_before_commit() {
        let root = TempRoot::new();

        let validation = validated_parent(root.path(), "schemas/example/v0.1.0/request.json", true)
            .expect("missing managed parents should be created");

        assert!(validation.filesystem_changed);
        assert_eq!(
            validation.path,
            fs::canonicalize(root.path())
                .unwrap()
                .join("schemas/example/v0.1.0")
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

    #[cfg(unix)]
    #[test]
    fn parent_symlink_inserted_after_inspection_cannot_redirect_a_write() {
        use std::os::unix::fs::symlink;

        let root = TempRoot::new();
        let relative_path = "schemas/example/v0.1.0/request.json";
        root.write(relative_path, b"old\n");
        let artifacts = [artifact(relative_path, b"new\n")];
        let inspection = inspect(root.path(), &artifacts).expect("inspection should succeed");

        let original_parent = root.path().join("schemas/example/v0.1.0");
        let preserved_parent = root.path().join("preserved-parent");
        fs::rename(&original_parent, &preserved_parent).expect("parent should move");
        root.write("outside/request.json", b"old\n");
        symlink(root.path().join("outside"), &original_parent)
            .expect("replacement parent symlink should be created");

        let error = apply(root.path(), &artifacts, inspection)
            .expect_err("parent symlink must stop reconciliation");

        assert_eq!(error.phase, Phase::Commit);
        assert_eq!(error.state, DurableState::Unchanged);
        assert!(error.problems[0].contains("symlink"));
        assert_eq!(
            fs::read(root.path().join("outside/request.json")).unwrap(),
            b"old\n"
        );
        assert_eq!(
            fs::read(preserved_parent.join("request.json")).unwrap(),
            b"old\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_symlink_inserted_after_inspection_cannot_redirect_pruning() {
        use std::os::unix::fs::symlink;

        let root = TempRoot::new();
        let relative_path = "schemas/example/v0.1.0/orphan.json";
        root.write(relative_path, b"orphan\n");
        let inspection = inspect(root.path(), &[]).expect("inspection should succeed");

        let original_parent = root.path().join("schemas/example/v0.1.0");
        let preserved_parent = root.path().join("preserved-parent");
        fs::rename(&original_parent, &preserved_parent).expect("parent should move");
        root.write("outside/orphan.json", b"orphan\n");
        symlink(root.path().join("outside"), &original_parent)
            .expect("replacement parent symlink should be created");

        let error =
            apply(root.path(), &[], inspection).expect_err("parent symlink must stop pruning");

        assert_eq!(error.phase, Phase::Prune);
        assert_eq!(error.state, DurableState::Unchanged);
        assert!(error.problems[0].contains("symlink"));
        assert_eq!(
            fs::read(root.path().join("outside/orphan.json")).unwrap(),
            b"orphan\n"
        );
        assert_eq!(
            fs::read(preserved_parent.join("orphan.json")).unwrap(),
            b"orphan\n"
        );
    }

    #[test]
    fn windows_style_replacement_installs_the_new_file_and_removes_its_backup() {
        let root = TempRoot::new();
        root.write("destination.json", b"old\n");
        root.write("replacement.tmp", b"new\n");
        let destination = root.path().join("destination.json");
        let temporary = root.path().join("replacement.tmp");
        let backup = root.path().join("backup.tmp");

        replace_existing_with_backup(&temporary, &destination, &backup, "destination.json")
            .expect("replacement should succeed");

        assert_eq!(fs::read(&destination).unwrap(), b"new\n");
        assert!(!temporary.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn windows_style_replacement_restores_the_existing_file_when_installation_fails() {
        let root = TempRoot::new();
        root.write("destination.json", b"old\n");
        let destination = root.path().join("destination.json");
        let missing_temporary = root.path().join("missing.tmp");
        let backup = root.path().join("backup.tmp");

        let error = replace_existing_with_backup(
            &missing_temporary,
            &destination,
            &backup,
            "destination.json",
        )
        .expect_err("missing replacement must restore the destination");

        assert!(error.problem.contains("restored the previous destination"));
        assert!(!error.filesystem_changed);
        assert_eq!(fs::read(&destination).unwrap(), b"old\n");
        assert!(!backup.exists());
    }

    #[test]
    fn backup_cleanup_failure_reports_a_recoverable_incomplete_mutation() {
        let root = TempRoot::new();
        fs::create_dir(root.path().join("destination.json")).unwrap();
        root.write("destination.json/keep.txt", b"old\n");
        root.write("replacement.tmp", b"new\n");
        let destination = root.path().join("destination.json");
        let temporary = root.path().join("replacement.tmp");
        let backup = root.path().join("backup.tmp");

        let error =
            replace_existing_with_backup(&temporary, &destination, &backup, "destination.json")
                .expect_err("non-empty backup directory cannot be removed as a file");

        assert!(error.problem.contains("could not remove backup"));
        assert!(error.filesystem_changed);
        assert_eq!(
            mutation_failure_state(&Report::default(), &error),
            DurableState::RecoverableIncomplete
        );
        assert_eq!(fs::read(&destination).unwrap(), b"new\n");
        assert!(backup.is_dir());
    }

    #[test]
    fn check_reports_unfinished_transaction_files() {
        let root = TempRoot::new();
        let transaction = "schemas/example/v0.1.0/.request.json.gaap-contract-artifacts-1-1.backup";
        root.write(transaction, b"old\n");

        let error = reconcile(root.path(), &[], Mode::Check)
            .expect_err("unfinished transactions must fail check mode");

        assert_eq!(error.phase, Phase::Inspect);
        assert_eq!(error.state, DurableState::Unchanged);
        assert!(error.problems[0].contains("unfinished artifact transaction"));
        assert!(root.path().join(transaction).exists());
    }
}
