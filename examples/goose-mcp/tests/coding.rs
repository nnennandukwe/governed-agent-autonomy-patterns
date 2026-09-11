use gaap_goose_demo::coding::CodingStore;
use serde_json::json;
const GOOD: &str = "def shipping_quote(quantity, unit_price):\n    if quantity <= 0:\n        raise ValueError('quantity must be positive')\n    subtotal = quantity * unit_price\n    if quantity >= 5:\n        subtotal = subtotal * 0.9\n    if subtotal < 100:\n        subtotal = subtotal + 7\n    return round(subtotal, 2)\n";

#[test]
fn real_change_runs_full_engine_then_protected_change_is_denied() {
    let parent = tempfile::tempdir().unwrap();
    let store = CodingStore::initialize(&parent.path().join("coding")).unwrap();
    let input = json!({"base_digest":store.inspect().unwrap()["subject_digest"],"path":"shipping.py", "content":GOOD, "plan":"Implement quantity discount and shipping threshold; verify acceptance cases."});
    let result = store.submit(input.clone()).unwrap();
    assert_eq!(
        result["receipt"]["body"]["terminal_status"], "completed",
        "{result}"
    );
    assert_eq!(result["terminal_receipt_verified"], true);
    assert_eq!(result["verification"]["passed"], true);
    assert!(store.submit(input).unwrap()["duplicate"].as_bool().unwrap());
    let before = std::fs::read(store.workspace().join("deployment.json")).unwrap();
    let denied = store.submit(json!({"base_digest":store.inspect().unwrap()["subject_digest"],"path":"deployment.json","content":"{\"shipping_enabled\":true}","plan":"Enable shipping in production."})).unwrap();
    assert_eq!(denied["receipt"]["body"]["terminal_status"], "blocked");
    assert_eq!(
        denied["protected_effect_results"][0]["body"]["execution_status"],
        "denied"
    );
    assert_eq!(
        std::fs::read(store.workspace().join("deployment.json")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
        GOOD
    );
    assert_eq!(
        store.inspect().unwrap()["runs"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn failed_verification_preserves_real_effect_and_blocks_completion() {
    let parent = tempfile::tempdir().unwrap();
    let store = CodingStore::initialize(&parent.path().join("coding")).unwrap();
    let result = store.submit(json!({"base_digest":store.inspect().unwrap()["subject_digest"],"path":"shipping.py","content":"def shipping_quote(quantity, unit_price):\n    return 0\n","plan":"Implement shipping quote."})).unwrap();
    assert_eq!(result["receipt"]["body"]["terminal_status"], "blocked");
    assert_eq!(
        result["protected_effect_results"][0]["body"]["execution_status"],
        "executed"
    );
    assert_eq!(result["verification"]["passed"], false);
}

#[test]
fn unsafe_code_never_executes_and_unknown_authority_is_rejected() {
    let parent = tempfile::tempdir().unwrap();
    let store = CodingStore::initialize(&parent.path().join("coding")).unwrap();
    let forbidden = parent.path().join("escaped");
    let code = format!(
        "open({:?}, 'w').write('escaped')\n",
        forbidden.to_str().unwrap()
    );
    let result = store.submit(json!({"base_digest":store.inspect().unwrap()["subject_digest"],"path":"shipping.py","content":code,"plan":"Test candidate."})).unwrap();
    assert_eq!(result["verification"]["passed"], false);
    assert!(!forbidden.exists());
    assert!(store.submit(json!({"base_digest":store.inspect().unwrap()["subject_digest"],"path":"shipping.py","content":GOOD,"plan":"Fix code","approved":true})).is_err());
    let denied = store.submit(json!({"base_digest":store.inspect().unwrap()["subject_digest"],"path":"../escaped","content":"x","plan":"Write outside scope"})).unwrap();
    assert_eq!(denied["receipt"]["body"]["terminal_status"], "blocked");
    assert!(!forbidden.exists());
}

#[test]
fn inspector_revalidates_receipts_and_verification_evidence() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("coding");
    let store = CodingStore::initialize(&root).unwrap();
    store.submit(json!({"path":"shipping.py","content":GOOD,"plan":"Implement quote","base_digest":store.inspect().unwrap()["subject_digest"]})).unwrap();
    let path = root.join("operator/state.json");
    let original = std::fs::read(&path).unwrap();
    let mut state: serde_json::Value = serde_json::from_slice(&original).unwrap();
    state["runs"][0]["verification"]["passed"] = json!(false);
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(
        store
            .inspect()
            .unwrap_err()
            .contains("verification evidence")
    );
    std::fs::write(&path, &original).unwrap();
    let mut state: serde_json::Value = serde_json::from_slice(&original).unwrap();
    state["runs"][0]["receipt"]["body"]["terminal_status"] = json!("blocked");
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(store.inspect().unwrap_err().contains("receipt integrity"));
}

#[test]
fn rust_commands_accept_the_same_relative_session_path() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let parent = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_gaap-goose-demo");
    let init = Command::new(binary)
        .args(["coding-init", "coding"])
        .current_dir(parent.path())
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let inspect = Command::new(binary)
        .args(["coding-inspect", "coding"])
        .current_dir(parent.path())
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let state: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    let mut child = Command::new(binary)
        .args(["coding-submit", "coding"])
        .current_dir(parent.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(json!({"path":"shipping.py","content":GOOD,"plan":"Implement shipping rules","base_digest":state["subject_digest"]}).to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["receipt"]["body"]["terminal_status"], "completed");
}
