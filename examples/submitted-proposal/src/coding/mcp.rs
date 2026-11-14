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
    active: Arc<Mutex<ConnectionState>>,
    next_correlation: Arc<AtomicU64>,
}
#[derive(Default)]
struct ConnectionState {
    closed: bool,
    controls: BTreeMap<u64, PendingCall>,
}
struct PendingCall {
    control: RunControl,
    is_submission: bool,
}
impl CodingServer {
    pub fn new(store: CodingStore) -> Self {
        Self {
            store,
            active: Arc::default(),
            next_correlation: Arc::default(),
        }
    }
    pub fn shutdown(&self) -> bool {
        let mut state = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let first_close = !state.closed;
        state.closed = true;
        for call in state.controls.values() {
            call.control.cancel();
        }
        first_close
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
                let mut state = self.store.inspect_control(&control)?;
                if name == "run_status" {
                    return Ok(
                        json!({"latest_run":state["runs"].as_array().and_then(|r|r.last()),"inflight":state["inflight"],"subject_digest":state["subject_digest"],"active_submissions":self.active.lock().unwrap_or_else(|e| e.into_inner()).controls.values().filter(|call| call.is_submission).count()}),
                    );
                }
                state.as_object_mut().unwrap().remove("runs");
                Ok(state)
            }
            _ => Err("unknown tool; call tools/list for available operations".into()),
        }
    }
}
struct ActiveCall {
    id: u64,
    server: CodingServer,
    control: RunControl,
}
impl Drop for ActiveCall {
    fn drop(&mut self) {
        // Dropping a handler mustn't detach a live write from cancellation.
        // The blocking worker still drains and stores its actual outcome.
        self.control.cancel();
        self.server
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .controls
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
        let _active = {
            let id = self.next_correlation.fetch_add(1, Ordering::Relaxed);
            let mut state = self.active.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Ok(CallToolResult::error(vec![ContentBlock::text(
                    "Connection closed before admission; no effect dispatched. Inspect saved outcomes with a new connection."
                )]).into());
            }
            state.controls.insert(
                id,
                PendingCall {
                    control: control.clone(),
                    is_submission: request.name == "submit_change",
                },
            );
            ActiveCall {
                id,
                server: self.clone(),
                control: control.clone(),
            }
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
            Err(error) => CallToolResult::error(vec![ContentBlock::text(json!({"error":error,"next_action":"Inspect run_status and actual files before resubmitting. Preserve unresolved sessions; this version can't reconcile or retry an uncertain effect."}).to_string())]),
        }.into())
    }
}
