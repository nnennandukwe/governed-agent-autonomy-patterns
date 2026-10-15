use gaap::contracts::{AgentRunRequest, ContractSupport};
use gaap_acp::goose::{ConversationEvent, GooseConversation, GooseProfile};
use gaap_acp::{AgentConfig, LaunchArtifact, Limits};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

fn fixture(
    mode: &str,
) -> (
    tempfile::TempDir,
    GooseConversation,
    AgentRunRequest,
    ContractSupport,
) {
    let root = tempfile::tempdir().unwrap();
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    let executable = PathBuf::from(String::from_utf8(python.stdout).unwrap().trim())
        .canonicalize()
        .unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/goose.py");
    let artifacts = [&executable, &script]
        .into_iter()
        .map(|path| LaunchArtifact {
            path: path.clone(),
            digest: format!("sha256:{:x}", Sha256::digest(std::fs::read(path).unwrap())),
        })
        .collect();
    let config = AgentConfig {
        executable,
        arguments: vec![script.display().to_string(), mode.into()],
        environment: BTreeMap::new(),
        working_directory: root.path().canonicalize().unwrap(),
        expected_name: "goose".into(),
        expected_version: "1.54.0".into(),
        artifacts,
    };
    let profile = GooseProfile {
        proposal_server: config.clone(),
        server_name: "gaap-proposals".into(),
    };
    assert_eq!(
        profile.capability(&config).unwrap().name,
        "acp.goose-proposals"
    );
    let conversation = GooseConversation::new(
        config,
        profile,
        Limits {
            operation_timeout: if mode == "delayed" {
                Duration::from_millis(50)
            } else {
                Duration::from_secs(10)
            },
            ..Limits::default()
        },
    )
    .unwrap();
    let request: AgentRunRequest = serde_json::from_str(include_str!(
        "../../../examples/contracts/v0.1.0/agent-run-request.json"
    ))
    .unwrap();
    let support = ContractSupport::new(request.policies.clone());
    (root, conversation, request, support)
}

#[test]
fn pinned_profile_checks_inventory_before_releasing_prompt() {
    for mode in ["extra_extension", "changed_extension"] {
        let (root, mut conversation, request, support) = fixture(mode);
        assert!(conversation.plan(&request, &support).is_err());
        conversation.close();
        assert!(!root.path().join("prompt-started").exists());
        assert_eq!(
            conversation.evidence().snapshot().failure_code.as_deref(),
            Some("acp.goose_inventory_mismatch")
        );
    }
}
#[test]
fn passive_setup_metadata_doesnt_grant_execution_or_completion() {
    let (root, mut conversation, request, support) = fixture("normal");
    conversation.plan(&request, &support).unwrap();
    assert!(!root.path().join("prompt-started").exists());
    assert!(
        conversation
            .evidence()
            .snapshot()
            .extension_inventory_digest
            .is_some()
    );
    conversation.start_prompt().unwrap();
    assert!(matches!(
        conversation.poll(Duration::from_secs(2)).unwrap(),
        Some(ConversationEvent::Finished)
    ));
    assert!(conversation.claim_completion().is_err());
    assert!(conversation.plan(&request, &support).is_err());
}
#[test]
fn undeclared_filesystem_callback_cant_dispatch_in_goose_profile() {
    let (root, mut conversation, request, support) = fixture("filesystem");
    conversation.plan(&request, &support).unwrap();
    conversation.start_prompt().unwrap();
    assert!(conversation.poll(Duration::from_secs(2)).is_err());
    conversation.close();
    assert!(!root.path().join("forbidden").exists());
    assert_eq!(
        conversation.evidence().snapshot().failure_code.as_deref(),
        Some("acp.undeclared_filesystem")
    );
}

#[test]
fn failed_plan_cant_release_a_late_prompt() {
    let (root, mut conversation, request, support) = fixture("delayed");
    assert!(conversation.plan(&request, &support).is_err());
    assert!(conversation.start_prompt().is_err());
    std::thread::sleep(Duration::from_millis(400));
    assert!(!root.path().join("prompt-started").exists());
}
