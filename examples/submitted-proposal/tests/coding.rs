use gaap_submitted_proposal_reference::coding::CodingStore;
use serde_json::{Value, json};
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
    let binary = env!("CARGO_BIN_EXE_gaap-submitted-proposal-reference");
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

#[test]
fn missed_published_case_cant_be_completed_by_a_wrong_function() {
    let parent = tempfile::tempdir().unwrap();
    let store = CodingStore::initialize(&parent.path().join("coding")).unwrap();
    let wrong = GOOD.replacen(
        "    subtotal =",
        "    if quantity == 2:\n        return 0\n    subtotal =",
        1,
    );
    let result = store.submit(json!({"path":"shipping.py","content":wrong,"plan":"Implement shipping rules","base_digest":store.inspect().unwrap()["subject_digest"]})).unwrap();
    assert_eq!(result["receipt"]["body"]["terminal_status"], "blocked");
    assert_eq!(
        result["protected_effect_results"][0]["body"]["execution_status"],
        "executed"
    );
    assert_eq!(result["verification"]["passed"], false);
    assert_eq!(
        result["verification"]["cases"].as_array().unwrap().len(),
        41
    );
    assert_eq!(
        result["verification"]["challenge_seed"],
        result["receipt"]["body"]["request_digest"]
    );
}

#[test]
fn saved_effect_results_must_match_the_receipt_without_removal_or_duplication() {
    let parent = tempfile::tempdir().unwrap();
    let store = CodingStore::initialize(&parent.path().join("coding")).unwrap();
    let input = json!({"path":"shipping.py","content":GOOD,"plan":"Implement quote","base_digest":store.inspect().unwrap()["subject_digest"]});
    store.submit(input.clone()).unwrap();
    let path = parent.path().join("coding/operator/state.json");
    let original = std::fs::read(&path).unwrap();
    let original_state: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let effect = original_state["runs"][0]["protected_effect_results"][0].clone();
    for results in [json!([]), json!([effect.clone(), effect])] {
        let mut state = original_state.clone();
        state["runs"][0]["protected_effect_results"] = results;
        let corrupt = serde_json::to_vec(&state).unwrap();
        std::fs::write(&path, &corrupt).unwrap();
        assert!(store.inspect().unwrap_err().contains("receipt effect"));
        assert!(
            store
                .submit(input.clone())
                .unwrap_err()
                .contains("receipt effect")
        );
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        assert_eq!(
            std::fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
            GOOD
        );
    }
    let mut state = original_state.clone();
    let mut alternate: gaap::contracts::ProtectedEffectResult =
        serde_json::from_value(state["runs"][0]["protected_effect_results"][0].clone()).unwrap();
    alternate.body.usage.elapsed_ms += 1;
    alternate.result_digest =
        gaap_submitted_proposal_reference::coding::hash_json(&alternate.body).unwrap();
    state["runs"][0]["protected_effect_results"][0] = serde_json::to_value(alternate).unwrap();
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(
        store
            .inspect()
            .unwrap_err()
            .contains("receipt effect evidence")
    );
    std::fs::write(&path, &original).unwrap();
    store.submit(json!({"path":"deployment.json","content":"{}","plan":"Request deployment","base_digest":store.inspect().unwrap()["subject_digest"]})).unwrap();
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    state["runs"][1]["protected_effect_results"] = json!([]);
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(
        store
            .inspect()
            .unwrap_err()
            .contains("receipt effect result count")
    );
}

#[test]
fn concurrent_inspection_returns_coherent_views_or_bounded_refusal() {
    const LOCK_DEADLINE: &str = "operation interrupted while waiting for the session lock: host proposal deadline exceeded; no run or effect admitted";
    const PREPARATION_DEADLINE: &str = "proposal interrupted before admission: host proposal deadline exceeded; no run or effect admitted";
    fn expected_source<'a>(state: &'a Value, initial: &'a Value) -> &'a Value {
        state["runs"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|run| {
                run["protected_effect_results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|effect| effect["body"]["execution_status"] == "executed")
            })
            .map(|run| &run["proposed"])
            .unwrap_or(initial)
    }
    fn read_view(store: &CodingStore) -> Result<Value, String> {
        for _ in 0..6 {
            match store.inspect() {
                Err(error) if error == LOCK_DEADLINE => continue,
                result => return result,
            }
        }
        Err("writer couldn't read a view after bounded contention retries".into())
    }
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    };
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("coding");
    let store = CodingStore::initialize(&root).unwrap();
    let initial = store.inspect().unwrap()["files"]["shipping.py"].clone();
    let done = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(5));
    let mut readers = Vec::new();
    for _ in 0..4 {
        let root = root.clone();
        let done = done.clone();
        let start = start.clone();
        let initial = initial.clone();
        readers.push(std::thread::spawn(move || {
            start.wait();
            let mut completed = 0;
            let mut refused = 0;
            for _ in 0..500 {
                let state = match CodingStore::open(&root).and_then(|store| store.inspect()) {
                    Ok(state) => state,
                    Err(error) => {
                        assert_eq!(error, LOCK_DEADLINE);
                        refused += 1;
                        assert!(
                            refused <= 6,
                            "reader kept reaching its deadline after finite writes"
                        );
                        continue;
                    }
                };
                completed += 1;
                if state["inflight"].is_null() {
                    let expected = expected_source(&state, &initial);
                    assert_eq!(
                        &state["files"]["shipping.py"], expected,
                        "saved history and files came from different publication stages"
                    );
                }
                if done.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(completed > 0, "reader never obtained a coherent view");
        }));
    }
    start.wait();
    let mut completed_runs = 0;
    let written = (|| -> Result<(), String> {
        for revision in 0..12 {
            let mut admitted = None;
            for _ in 0..3 {
                let view = read_view(&store)?;
                let before = std::fs::read(root.join("operator/state.json")).unwrap();
                let result = store.submit(json!({"path":"shipping.py", "content":format!("{GOOD}# {}\n# revision {revision}\n", "x".repeat(28000)),
                    "plan":format!("Publish shipping revision {revision}."),"base_digest":view["subject_digest"]}));
                match result {
                    Ok(result) => {
                        admitted = Some(result);
                        break;
                    }
                    Err(error) if error == LOCK_DEADLINE || error == PREPARATION_DEADLINE => {
                        assert_eq!(
                            std::fs::read(root.join("operator/state.json")).unwrap(),
                            before
                        );
                        assert_eq!(
                            std::fs::read_to_string(store.workspace().join("shipping.py")).unwrap(),
                            view["files"]["shipping.py"].as_str().unwrap()
                        );
                    }
                    Err(error) => return Err(error),
                }
            }
            let result = admitted.ok_or("writer couldn't enter after bounded admission retries")?;
            let receipt: gaap::contracts::TerminalRunReceipt =
                serde_json::from_value(result["receipt"].clone()).unwrap();
            match receipt.body.terminal_status {
                gaap::contracts::AgentRunStatus::Completed => {
                    completed_runs += 1;
                    assert_eq!(
                        result["protected_effect_results"].as_array().unwrap().len(),
                        1
                    );
                    assert_eq!(
                        result["protected_effect_results"][0]["body"]["execution_status"],
                        "executed"
                    );
                }
                gaap::contracts::AgentRunStatus::Interrupted => {
                    assert!(receipt.body.events.iter().any(|event| matches!(event,
                        gaap::contracts::RunEvent::Interruption { reason, .. } if reason == "host proposal deadline exceeded")));
                }
                status => panic!("unexpected outcome under read contention: {status:?}"),
            }
        }
        Ok(())
    })();
    done.store(true, Ordering::Release);
    for reader in readers {
        reader.join().unwrap();
    }
    written.unwrap();
    let final_view = store.inspect().unwrap();
    assert_eq!(final_view["runs"].as_array().unwrap().len(), 12);
    assert!(final_view["inflight"].is_null());
    assert!(
        completed_runs > 0,
        "no completed effect exercised concurrent publication"
    );
    assert_eq!(
        &final_view["files"]["shipping.py"],
        expected_source(&final_view, &initial)
    );
}

#[test]
fn cancellation_and_deadline_interrupt_both_locks_before_admission() {
    use gaap_submitted_proposal::RunControl;
    use std::{
        fs::OpenOptions,
        time::{Duration, Instant},
    };
    for path in ["operator/lock", "operator"] {
        for cancel in [false, true] {
            let parent = tempfile::tempdir().unwrap();
            let root = parent.path().join("coding");
            let store = CodingStore::initialize(&root).unwrap();
            let before = store.inspect().unwrap();
            let change = json!({"path":"shipping.py", "content":GOOD,
                "plan":"Implement shipping after admission.","base_digest":before["subject_digest"]});
            let ledger = std::fs::read(root.join("operator/state.json")).unwrap();
            let held = OpenOptions::new()
                .read(true)
                .write(path.ends_with("lock"))
                .open(root.join(path))
                .unwrap();
            held.lock().unwrap();
            let control = RunControl::new(Duration::from_millis(if cancel { 1000 } else { 30 }));
            let signal = cancel.then(|| {
                let control = control.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(10));
                    control.cancel();
                })
            });
            let started = Instant::now();
            let error = store.submit_control(change.clone(), control).unwrap_err();
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "{path}: {error}"
            );
            assert!(
                error.contains("no run or effect admitted"),
                "{path}: {error}"
            );
            assert!(error.contains(if cancel {
                "caller cancelled"
            } else {
                "deadline exceeded"
            }));
            if let Some(signal) = signal {
                signal.join().unwrap();
            }
            assert_eq!(
                std::fs::read(root.join("operator/state.json")).unwrap(),
                ledger
            );
            drop(held);
            assert_eq!(store.inspect().unwrap(), before);
            let fresh = store.submit(change.clone()).unwrap();
            assert_eq!(fresh["duplicate"], false);
            assert_eq!(fresh["receipt"]["body"]["terminal_status"], "completed");
            assert_eq!(store.submit(change).unwrap()["duplicate"], true);
        }
    }
}
