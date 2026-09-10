use crate::store::{Store, digest};
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadFile {
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeWrite {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyChange {
    pub request_id: String,
}

fn tool<T: JsonSchema>(name: &'static str, description: &'static str) -> Tool {
    let schema = serde_json::to_value(schema_for!(T)).expect("static schema serializes");
    Tool::new(
        name,
        description,
        Arc::new(schema.as_object().unwrap().clone()),
    )
}

pub fn tools() -> Vec<Tool> {
    vec![
        tool::<ReadFile>(
            "read_file",
            "Read the allowlisted release.json fixture. No other paths are accessible.",
        ),
        tool::<ProposeWrite>(
            "propose_write",
            "Propose exact new release.json contents. Returns a request_id and GAAP decision. This never writes the fixture. Approval is available only to the human operator outside MCP. Stop and report the request_id when approval is required.",
        ),
        tool::<ApplyChange>(
            "apply_change",
            "Apply an existing request_id once, only after exact operator approval. Rechecks the current file and authority. Cannot approve requests. Report the decision and observed effect accurately; an ask or block means no write occurred.",
        ),
    ]
}

/// Authority is pinned to the same descriptors actually advertised over MCP.
pub fn capability_digest() -> String {
    digest(&serde_jcs::to_vec(&tools()).expect("static tool descriptors canonicalize"))
}

#[derive(Clone)]
pub struct BoundaryServer(pub Store);

impl BoundaryServer {
    pub fn dispatch(&self, name: &str, arguments: Value) -> Result<Value, String> {
        match name {
            "read_file" => {
                let p: ReadFile = serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                serde_json::to_value(self.0.read_file(&p.path)?).map_err(|e| e.to_string())
            }
            "propose_write" => {
                let p: ProposeWrite =
                    serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                serde_json::to_value(self.0.propose_write(&p.path, &p.content)?)
                    .map_err(|e| e.to_string())
            }
            "apply_change" => {
                let p: ApplyChange =
                    serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                serde_json::to_value(self.0.apply_change(&p.request_id)?).map_err(|e| e.to_string())
            }
            _ => Err(
                "unknown tool; only read_file, propose_write, apply_change are available".into(),
            ),
        }
    }
}

impl ServerHandler for BoundaryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|tool| tool.name == name)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let server = self.clone();
        // File locks and fsync belong on the blocking pool, not Tokio's protocol task.
        let result = tokio::task::spawn_blocking(move || {
            server.dispatch(
                &request.name,
                Value::Object(request.arguments.unwrap_or_default()),
            )
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let response = match result {
            Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(
                json!({"error":error,"execution_status":"not_dispatched_by_this_call"}).to_string(),
            )]),
        };
        Ok(response.into())
    }
}
