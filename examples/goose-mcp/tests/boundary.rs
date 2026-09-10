use gaap::Outcome;
use gaap_goose_demo::store::Store;
use std::fs;
use tempfile::TempDir;

fn demo() -> (TempDir, Store) {
    let parent = tempfile::tempdir().unwrap();
    let store = Store::initialize(&parent.path().join("demo")).unwrap();
    (parent, store)
}

fn change() -> &'static str {
    "{\n  \"name\": \"conference-demo\",\n  \"version\": \"1.1.0\"\n}\n"
}

#[test]
fn unapproved_request_does_not_change_the_file() {
    let (_parent, store) = demo();
    let before = store.read_file("release.json").unwrap();
    let proposal = store.propose_write("release.json", change()).unwrap();
    assert_eq!(proposal.decision.outcome, Outcome::Ask);
    let result = store.apply_change(&proposal.request_id).unwrap();
    assert_eq!(result.decision.outcome, Outcome::Ask);
    assert_eq!(result.execution_status, "not_executed");
    assert_eq!(store.read_file("release.json").unwrap(), before);
}

#[test]
fn exact_operator_approval_executes_once_and_survives_restart() {
    let (_parent, store) = demo();
    let proposal = store.propose_write("release.json", change()).unwrap();
    store.approve(&proposal.request_id).unwrap();
    let result = store.apply_change(&proposal.request_id).unwrap();
    assert_eq!(result.execution_status, "executed");
    assert_eq!(result.decision.outcome, Outcome::Allow);
    assert_eq!(store.read_file("release.json").unwrap().content, change());
    let reopened = Store::open(store.state_dir()).unwrap();
    let replay = reopened.apply_change(&proposal.request_id).unwrap();
    assert_eq!(replay.decision.outcome, Outcome::Block);
    assert_eq!(replay.code(), "demo.request_consumed");
    assert_eq!(reopened.inspect().unwrap()["execution_count"], 1);
}

#[test]
fn changed_proposal_has_no_inherited_approval() {
    let (_parent, store) = demo();
    let first = store.propose_write("release.json", change()).unwrap();
    store.approve(&first.request_id).unwrap();
    let second = store
        .propose_write("release.json", &change().replace("1.1.0", "1.2.0"))
        .unwrap();
    assert_ne!(first.request_id, second.request_id);
    assert_eq!(
        store
            .apply_change(&second.request_id)
            .unwrap()
            .decision
            .outcome,
        Outcome::Ask
    );
}

#[test]
fn changed_underlying_file_is_preserved() {
    let (_parent, store) = demo();
    let proposal = store.propose_write("release.json", change()).unwrap();
    store.approve(&proposal.request_id).unwrap();
    let concurrent = "{\"version\":\"9.0.0\"}\n";
    fs::write(store.workspace().join("release.json"), concurrent).unwrap();
    let result = store.apply_change(&proposal.request_id).unwrap();
    assert_eq!(result.decision.outcome, Outcome::Block);
    assert_eq!(result.code(), "demo.stale_subject");
    assert_eq!(
        fs::read_to_string(store.workspace().join("release.json")).unwrap(),
        concurrent
    );
}

#[test]
fn denied_paths_cannot_receive_operator_approval() {
    let (_parent, store) = demo();
    let p = store.propose_write("private.json", "secret").unwrap();
    assert_eq!(p.decision.outcome, Outcome::Block);
    assert!(store.approve(&p.request_id).is_err());
    assert_eq!(
        store.apply_change(&p.request_id).unwrap().decision.outcome,
        Outcome::Block
    );
    assert!(!store.workspace().join("private.json").exists());
}

#[test]
fn traversal_unknown_fields_and_symlinks_are_rejected() {
    use gaap_goose_demo::protocol::ProposeWrite;
    use std::os::unix::fs::symlink;
    let (_parent, store) = demo();
    assert!(store.read_file("../operator/state.json").is_err());
    assert!(
        serde_json::from_value::<ProposeWrite>(serde_json::json!({
            "path":"release.json", "content":change(), "approval":true
        }))
        .is_err()
    );
    let target = store.workspace().join("release.json");
    fs::remove_file(&target).unwrap();
    symlink(store.state_dir().join("state.json"), &target).unwrap();
    assert!(store.read_file("release.json").is_err());
    assert!(store.propose_write("release.json", change()).is_err());
}

#[test]
fn initialization_never_overwrites_existing_state() {
    let (parent, store) = demo();
    let before = store.read_file("release.json").unwrap();
    assert!(Store::initialize(&parent.path().join("demo")).is_err());
    assert_eq!(store.read_file("release.json").unwrap(), before);
}

#[test]
fn concurrent_apply_calls_execute_at_most_once() {
    let (_parent, store) = demo();
    let p = store.propose_write("release.json", change()).unwrap();
    store.approve(&p.request_id).unwrap();
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let directory = store.state_dir().to_owned();
            let request_id = p.request_id.clone();
            std::thread::spawn(move || {
                Store::open(&directory)
                    .unwrap()
                    .apply_change(&request_id)
                    .unwrap()
            })
        })
        .collect();
    let replies: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        replies
            .iter()
            .filter(|r| r.execution_status == "executed")
            .count(),
        1
    );
    assert_eq!(store.inspect().unwrap()["execution_count"], 1);
}

#[test]
fn file_replacement_preserves_approved_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (_parent, store) = demo();
    let path = store.workspace().join("release.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let proposal = store.propose_write("release.json", change()).unwrap();
    store.approve(&proposal.request_id).unwrap();
    assert_eq!(
        store
            .apply_change(&proposal.request_id)
            .unwrap()
            .execution_status,
        "executed"
    );
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn changed_permissions_invalidate_earlier_approval() {
    use std::os::unix::fs::PermissionsExt;
    let (_parent, store) = demo();
    let proposal = store.propose_write("release.json", change()).unwrap();
    store.approve(&proposal.request_id).unwrap();
    fs::set_permissions(
        store.workspace().join("release.json"),
        fs::Permissions::from_mode(0o400),
    )
    .unwrap();
    let result = store.apply_change(&proposal.request_id).unwrap();
    assert_eq!(result.code(), "demo.stale_subject");
    assert_eq!(store.inspect().unwrap()["execution_count"], 0);
}

#[test]
fn initialization_rejects_a_shared_nonsticky_parent() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(Store::initialize(&parent.path().join("demo")).is_err());
    assert!(!parent.path().join("demo").exists());
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn rust_approval_view_escapes_controls_without_changing_the_proposal() {
    let (_parent, store) = demo();
    let content = "\u{1b}[2J\u{202e}model text\n";
    let proposal = store.propose_write("release.json", content).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gaap-goose-demo"))
        .arg("approve")
        .arg(store.state_dir())
        .arg(&proposal.request_id)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(output.status.success());
    let display = String::from_utf8(output.stdout).unwrap();
    assert!(!display.contains('\u{1b}'));
    assert!(!display.contains('\u{202e}'));
    assert_eq!(
        store.inspect().unwrap()["proposals"][&proposal.request_id]["proposal"]["content"],
        content
    );
}
