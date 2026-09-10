use gaap_goose_demo::store::Store;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};

#[tokio::test]
async fn actual_stdio_requests_cannot_supply_authority() {
    let parent = tempfile::tempdir().unwrap();
    let store = Store::initialize(&parent.path().join("demo")).unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_gaap-goose-demo"));
    command
        .args(["serve", "--state-dir"])
        .arg(store.state_dir())
        .env_clear();
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(
        tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>(),
        ["read_file", "propose_write", "apply_change"]
    );
    let read = client
        .call_tool(
            CallToolRequestParams::new("read_file")
                .with_arguments(json!({"path":"release.json"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_ne!(read.is_error, Some(true));
    let proposed = client
        .call_tool(
            CallToolRequestParams::new("propose_write").with_arguments(
                json!({"path":"release.json","content":"{\"version\":\"1.1.0\"}\n"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let response: Value =
        serde_json::from_str(&proposed.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(response["decision"]["outcome"], "ask");
    let id = response["request_id"].as_str().unwrap();
    let forged = client
        .call_tool(
            CallToolRequestParams::new("apply_change").with_arguments(
                json!({"request_id":id,"approved":true})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(forged.is_error, Some(true));
    assert_eq!(store.inspect().unwrap()["execution_count"], 0);
    store.approve(id).unwrap();
    let applied = client
        .call_tool(
            CallToolRequestParams::new("apply_change")
                .with_arguments(json!({"request_id":id}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let response: Value =
        serde_json::from_str(&applied.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(response["execution_status"], "executed");
    assert_eq!(
        store.read_file("release.json").unwrap().content,
        "{\"version\":\"1.1.0\"}\n"
    );
    client.cancel().await.unwrap();
}
