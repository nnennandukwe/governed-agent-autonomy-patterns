//! Inbound MCP transport; the outbound MCP client remains a separate adapter.
use super::*;
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

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
            "Read the shipping task, files and exact subject digest. Use the digest as base_digest for a proposal.",
        ),
        tool::<Change>(
            "submit_change",
            "Submit a plan and complete contents for one file. The host owns policy, identity, execution and verification. Check both effect and completion status: a failed verifier can leave an executed write.",
        ),
        tool::<Empty>(
            "run_status",
            "Read the latest saved outcome, in-flight marker and number of active submissions. Duplicate delivery returns a historical receipt without replay.",
        ),
    ]
}
pub fn capability_digest() -> String {
    digest(&serde_jcs::to_vec(&tools()).expect("static schema"))
}
#[derive(Clone)]
pub struct CodingServer {
    store: CodingStore,
    active: Arc<Mutex<BTreeMap<u64, RunControl>>>,
    next_correlation: Arc<AtomicU64>,
}
impl CodingServer {
    pub fn new(store: CodingStore) -> Self {
        Self {
            store,
            active: Arc::default(),
            next_correlation: Arc::default(),
        }
    }
    pub fn cancel_all(&self) {
        for control in self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            control.cancel();
        }
    }
    pub fn dispatch(&self, name: &str, input: Value) -> Result<Value> {
        self.dispatch_control(
            name,
            input,
            RunControl::new(std::time::Duration::from_secs(10)),
        )
    }
    fn dispatch_control(&self, name: &str, input: Value, control: RunControl) -> Result<Value> {
        match name {
            "submit_change" => self.store.submit_control(input, control),
            "read_project" | "run_status" => {
                let _: Empty = serde_json::from_value(input).map_err(|e| e.to_string())?;
                let mut state = self.store.inspect()?;
                if name == "run_status" {
                    return Ok(
                        json!({"latest_run":state["runs"].as_array().and_then(|r|r.last()),"inflight":state["inflight"],"subject_digest":state["subject_digest"],"active_submissions":self.active.lock().unwrap_or_else(|e| e.into_inner()).len()}),
                    );
                }
                state.as_object_mut().unwrap().remove("runs");
                Ok(state)
            }
            _ => Err("unknown tool; call tools/list for available operations".into()),
        }
    }
}
struct ActiveSubmission {
    id: u64,
    server: CodingServer,
    control: RunControl,
}
impl Drop for ActiveSubmission {
    fn drop(&mut self) {
        // Dropping a handler mustn't detach a live write from cancellation.
        // The blocking worker still drains and stores its actual outcome.
        self.control.cancel();
        self.server
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
impl ServerHandler for CodingServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
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
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let control = RunControl::new(std::time::Duration::from_secs(10));
        let _active = if request.name == "submit_change" {
            let id = self.next_correlation.fetch_add(1, Ordering::Relaxed);
            self.active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, control.clone());
            Some(ActiveSubmission {
                id,
                server: self.clone(),
                control: control.clone(),
            })
        } else {
            None
        };
        let server = self.clone();
        let worker_control = control.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            server.dispatch_control(
                &request.name,
                Value::Object(request.arguments.unwrap_or_default()),
                worker_control,
            )
        });
        let result = tokio::select! {
            result = &mut worker => result,
            _ = context.ct.cancelled() => {
                control.cancel();
                worker.await
            }
        }
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(match result {
            Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(json!({"error":error,"next_action":"Inspect run_status and actual files before resubmitting. An unresolved outcome requires a fresh session after inspection."}).to_string())]),
        }.into())
    }
}
