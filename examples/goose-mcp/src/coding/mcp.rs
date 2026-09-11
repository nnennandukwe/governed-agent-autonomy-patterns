use super::*;
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use std::sync::Arc;
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}
fn tool<T: schemars::JsonSchema>(name: &'static str, description: &'static str) -> Tool {
    let schema = serde_json::to_value(schemars::schema_for!(T)).expect("static schema");
    Tool::new(
        name,
        description,
        Arc::new(schema.as_object().unwrap().clone()),
    )
}
pub fn tools() -> Vec<Tool> {
    vec![
        tool::<Empty>(
            "read_project",
            "Read the shipping coding task, source, deployment settings, and current subject digest. Use that exact digest as base_digest when submitting a change.",
        ),
        tool::<Change>(
            "submit_change",
            "Submit a concrete plan and complete proposed file contents to GAAP's full AgentRunEngine. GAAP evaluates authority and capability before writing, measures execution, independently verifies shipping behavior, and returns its terminal receipt. A blocked result must be reported accurately. Check effect status: verification can block completion after a file change executed. Never claim production activation when authority was denied.",
        ),
        tool::<Empty>(
            "run_status",
            "Read the most recent bounded run's real outcome, verification, and execution evidence.",
        ),
    ]
}
pub fn capability_digest() -> String {
    digest(&serde_jcs::to_vec(&tools()).expect("static schema"))
}
#[derive(Clone)]
pub struct CodingServer(pub CodingStore);
impl CodingServer {
    pub fn dispatch(&self, name: &str, input: Value) -> Result<Value> {
        match name {
            "submit_change" => self.0.submit(input),
            "read_project" | "run_status" => {
                let _: Empty = serde_json::from_value(input).map_err(|e| e.to_string())?;
                let mut state = self.0.inspect()?;
                if name == "run_status" {
                    return Ok(
                        json!({"latest_run":state["runs"].as_array().and_then(|r|r.last()),"inflight":state["inflight"],"subject_digest":state["subject_digest"]}),
                    );
                }
                state.as_object_mut().unwrap().remove("runs");
                Ok(state)
            }
            _ => Err("unknown coding tool".into()),
        }
    }
}
impl ServerHandler for CodingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|t| t.name == name)
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let server = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            server.dispatch(
                &request.name,
                Value::Object(request.arguments.unwrap_or_default()),
            )
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(match result {Ok(value)=>CallToolResult::success(vec![ContentBlock::text(value.to_string())]),Err(error)=>CallToolResult::error(vec![ContentBlock::text(json!({"error":error,"next_action":"Inspect run_status and actual files; no success or safe retry is implied."}).to_string())])}.into())
    }
}
