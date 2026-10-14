//! Strict stdio framing around the SDK's typed protocol messages.
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use rmcp::{
    RoleClient,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio_util::sync::CancellationToken;

use crate::{MCP_PROTOCOL_VERSION, McpError};

#[derive(Default)]
pub(crate) struct WireState {
    pub failure: Option<McpError>,
    pub pending: BTreeMap<String, String>,
    pub server_requests: BTreeSet<String>,
    pub generation: u64,
    pub call_generation: Option<u64>,
    pub call_started: bool,
}

pub(crate) type SharedState = Arc<Mutex<WireState>>;

pub(crate) fn fail(state: &SharedState, code: &'static str, detail: impl Into<String>) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.failure.is_none() {
        state.failure = Some(McpError::new(code, detail));
    }
}

pub(crate) fn health(state: &SharedState) -> Result<(), McpError> {
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.failure.clone().map_or(Ok(()), Err)
}

pub(crate) struct StdioTransport {
    pub reader: BufReader<ChildStdout>,
    pub writer: Arc<tokio::sync::Mutex<ChildStdin>>,
    pub state: SharedState,
    pub cancellation: CancellationToken,
    pub max_frame: usize,
}

impl Transport<RoleClient> for StdioTransport {
    type Error = io::Error;
    fn send(
        &mut self,
        message: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let writer = self.writer.clone();
        let state = self.state.clone();
        let cancellation = self.cancellation.clone();
        let max_frame = self.max_frame;
        async move {
            health(&state).map_err(io::Error::other)?;
            let json = serde_json::to_value(message).map_err(io::Error::other)?;
            let mut bytes = serde_json::to_vec(&json).map_err(io::Error::other)?;
            if bytes.len() > max_frame {
                fail(
                    &state,
                    "mcp.frame_limit",
                    "outgoing frame exceeds configured bound",
                );
                return Err(io::Error::other("outgoing frame limit"));
            }
            if let (Some(id), Some(method)) =
                (json.get("id"), json.get("method").and_then(Value::as_str))
            {
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if method == "tools/call" && state.call_generation != Some(state.generation) {
                    return Err(io::Error::other(
                        "capability generation changed before dispatch",
                    ));
                }
                if state.pending.len() >= 8
                    || state
                        .pending
                        .insert(id.to_string(), method.into())
                        .is_some()
                {
                    return Err(io::Error::other("duplicate or excessive outgoing requests"));
                }
            }
            if json.get("method").is_none() {
                let id = json
                    .get("id")
                    .ok_or_else(|| io::Error::other("response ID is absent"))?;
                if !state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .server_requests
                    .remove(&id.to_string())
                {
                    return Err(io::Error::other("unrequested client response"));
                }
            }
            bytes.push(b'\n');
            let is_cancellation =
                json.get("method").and_then(Value::as_str) == Some("notifications/cancelled");
            let mut writer = writer.lock().await;
            if is_cancellation {
                writer.write_all(&bytes).await?;
                return writer.flush().await;
            }
            let is_call = json.get("method").and_then(Value::as_str) == Some("tools/call");
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(io::Error::other("session cancelled")),
                result = async {
                    if is_call {
                        let mut written = 0;
                        while written < bytes.len() {
                            let size = std::future::poll_fn(|cx| {
                                // Hold the notification lock only for this nonblocking write poll.
                                // No notification can commit between the generation check and these bytes.
                                let mut state = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                                if let Some(error) = &state.failure { return Poll::Ready(Err(io::Error::other(error.clone()))); }
                                if state.call_generation != Some(state.generation) {
                                    state.failure = Some(McpError::new("mcp.capability_drift", "inventory changed before the call frame was written"));
                                    return Poll::Ready(Err(io::Error::other("capability drift before dispatch")));
                                }
                                match std::pin::Pin::new(&mut *writer).poll_write(cx, &bytes[written..]) {
                                    Poll::Ready(Ok(size)) => { state.call_started |= size > 0; Poll::Ready(Ok(size)) }
                                    other => other,
                                }
                            }).await?;
                            if size == 0 { return Err(io::Error::new(io::ErrorKind::WriteZero, "call frame wasn't written")); }
                            written += size;
                        }
                    } else {
                        writer.write_all(&bytes).await?;
                    }
                    writer.flush().await
                } => {
                    if let Err(error) = &result { fail(&state,"mcp.write_failed",error.to_string()); }
                    result
                }
            }
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        if health(&self.state).is_err() {
            return None;
        }
        let mut bytes = Vec::new();
        loop {
            let read = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return None,
                result = self.reader.fill_buf() => result,
            };
            let chunk = match read {
                Ok(chunk) if !chunk.is_empty() => chunk,
                Ok(_) => {
                    fail(&self.state, "mcp.server_exit", "server closed stdout");
                    return None;
                }
                Err(error) => {
                    fail(&self.state, "mcp.read_failed", error.to_string());
                    return None;
                }
            };
            let newline = chunk.iter().position(|byte| *byte == b'\n');
            let length = newline.unwrap_or(chunk.len());
            if bytes.len().saturating_add(length) > self.max_frame {
                fail(
                    &self.state,
                    "mcp.frame_limit",
                    "incoming frame exceeds configured bound",
                );
                return None;
            }
            bytes.extend_from_slice(&chunk[..length]);
            self.reader.consume(length + usize::from(newline.is_some()));
            if newline.is_some() {
                break;
            }
        }
        let parsed = match strict_json(&bytes).and_then(|json| validate_message(json, &self.state))
        {
            Ok(json) => serde_json::from_value(json)
                .map_err(|e| McpError::new("mcp.invalid_message", e.to_string())),
            Err(error) => Err(error),
        };
        match parsed {
            Ok(message) => Some(message),
            Err(error) => {
                fail(&self.state, error.code, error.detail);
                None
            }
        }
    }

    async fn close(&mut self) -> io::Result<()> {
        self.writer.lock().await.shutdown().await
    }
}

fn validate_message(json: Value, shared: &SharedState) -> Result<Value, McpError> {
    let envelope = json
        .as_object()
        .ok_or_else(|| McpError::new("mcp.invalid_message", "one JSON-RPC object is required"))?;
    if envelope.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(McpError::new(
            "mcp.invalid_message",
            "JSON-RPC 2.0 is required",
        ));
    }
    if let Some(method) = envelope.get("method").and_then(Value::as_str) {
        if method == "ping" {
            let id = envelope
                .get("id")
                .filter(|id| id.is_string() || id.as_i64().is_some())
                .ok_or_else(|| {
                    McpError::new(
                        "mcp.invalid_message",
                        "ping request ID is missing or invalid",
                    )
                })?;
            if envelope
                .keys()
                .any(|key| !["jsonrpc", "id", "method", "params"].contains(&key.as_str()))
                || envelope.get("params").is_some_and(|params| {
                    params.as_object().is_none_or(|params| !params.is_empty())
                })
            {
                return Err(McpError::new(
                    "mcp.invalid_message",
                    "ping doesn't accept operation input",
                ));
            }
            let mut state = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.server_requests.len() >= 8 || !state.server_requests.insert(id.to_string()) {
                return Err(McpError::new(
                    "mcp.request_identity",
                    "duplicate or excessive active server pings",
                ));
            }
            return Ok(json);
        }
        if envelope.contains_key("id") || method != "notifications/tools/list_changed" {
            return Err(McpError::new(
                "mcp.unsupported_server_message",
                "only control-plane pings and declared tool-list notifications are accepted",
            ));
        }
        if envelope
            .keys()
            .any(|key| !["jsonrpc", "method", "params"].contains(&key.as_str()))
        {
            return Err(McpError::new(
                "mcp.invalid_message",
                "unknown notification field",
            ));
        }
        let mut state = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.generation = state.generation.checked_add(1).ok_or_else(|| {
            McpError::new("mcp.capability_drift", "notification generation overflow")
        })?;
        return Ok(json);
    }
    if envelope
        .keys()
        .any(|key| !["jsonrpc", "id", "result", "error"].contains(&key.as_str()))
        || envelope.contains_key("result") == envelope.contains_key("error")
    {
        return Err(McpError::new(
            "mcp.invalid_message",
            "response must contain exactly one result or error",
        ));
    }
    let id = envelope
        .get("id")
        .ok_or_else(|| McpError::new("mcp.invalid_message", "response ID is missing"))?
        .to_string();
    let method = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .pending
        .remove(&id)
        .ok_or_else(|| {
            McpError::new(
                "mcp.response_identity",
                "duplicate, unknown or out-of-order response ID",
            )
        })?;
    if let Some(result) = envelope.get("result") {
        let object = result
            .as_object()
            .ok_or_else(|| McpError::new("mcp.invalid_result", "result must be an object"))?;
        match method.as_str() {
            "initialize" => {
                if object.get("protocolVersion").and_then(Value::as_str)
                    != Some(MCP_PROTOCOL_VERSION)
                {
                    return Err(McpError::new(
                        "mcp.protocol_version",
                        "server selected an unsupported protocol version",
                    ));
                }
            }
            "tools/list" => {
                if object
                    .keys()
                    .any(|key| !["tools", "nextCursor", "_meta"].contains(&key.as_str()))
                {
                    return Err(McpError::new(
                        "mcp.invalid_descriptor",
                        "unknown tool-list field",
                    ));
                }
                let tools = object
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        McpError::new("mcp.invalid_descriptor", "tools array is required")
                    })?;
                for tool in tools {
                    if tool.get("annotations").is_some_and(|annotations| {
                        annotations.as_object().is_none_or(|object| {
                            object.keys().any(|key| {
                                ![
                                    "title",
                                    "readOnlyHint",
                                    "destructiveHint",
                                    "idempotentHint",
                                    "openWorldHint",
                                ]
                                .contains(&key.as_str())
                            })
                        })
                    }) {
                        return Err(McpError::new(
                            "mcp.invalid_descriptor",
                            "unknown tool annotation field",
                        ));
                    }
                    let tool = tool.as_object().ok_or_else(|| {
                        McpError::new(
                            "mcp.invalid_descriptor",
                            "tool descriptor must be an object",
                        )
                    })?;
                    if tool.keys().any(|key| {
                        ![
                            "name",
                            "title",
                            "description",
                            "inputSchema",
                            "outputSchema",
                            "annotations",
                            "icons",
                            "_meta",
                        ]
                        .contains(&key.as_str())
                    }) {
                        return Err(McpError::new(
                            "mcp.invalid_descriptor",
                            "unknown tool descriptor field",
                        ));
                    }
                }
            }
            "tools/call" => {
                if object.keys().any(|key| {
                    !["content", "structuredContent", "isError", "_meta"].contains(&key.as_str())
                }) {
                    return Err(McpError::new(
                        "mcp.invalid_result",
                        "unknown or unsupported result field",
                    ));
                }
                if object
                    .get("structuredContent")
                    .is_some_and(|content| !content.is_object())
                {
                    return Err(McpError::new(
                        "mcp.invalid_result",
                        "this protocol profile requires object structured content",
                    ));
                }
                let content = object
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        McpError::new("mcp.invalid_result", "complete content array is required")
                    })?;
                for block in content {
                    if block.get("type").and_then(Value::as_str) != Some("text") {
                        return Err(McpError::new(
                            "mcp.content_type",
                            "only declared text content is supported by this profile",
                        ));
                    }
                    if block.as_object().is_none_or(|object| {
                        object.keys().any(|key| {
                            !["type", "text", "annotations", "_meta"].contains(&key.as_str())
                        })
                    }) {
                        return Err(McpError::new(
                            "mcp.invalid_result",
                            "unknown text-content field",
                        ));
                    }
                    if block.get("text").and_then(Value::as_str).is_none() {
                        return Err(McpError::new(
                            "mcp.invalid_result",
                            "text content must contain a string",
                        ));
                    }
                }
            }
            _ => {
                return Err(McpError::new(
                    "mcp.invalid_result",
                    "unexpected response method",
                ));
            }
        }
    }
    Ok(json)
}

pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value, McpError> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let parsed = StrictValue::deserialize(&mut decoder)
        .map_err(|e| McpError::new("mcp.invalid_json", e.to_string()))?;
    decoder
        .end()
        .map_err(|e| McpError::new("mcp.invalid_json", e.to_string()))?;
    Ok(parsed.0)
}

struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::from(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::from(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(Value::Number)
                    .map(StrictValue)
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<StrictValue>()? {
                    values.push(value.0);
                }
                Ok(StrictValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut object = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if object.contains_key(&key) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    object.insert(key, map.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(object)))
            }
        }
        decoder.deserialize_any(JsonVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::process::Stdio;
    use tokio::process::Command;

    #[tokio::test]
    async fn descriptor_change_while_waiting_for_writer_blocks_the_first_call_byte() {
        for drift in [false, true] {
            let dir = tempfile::TempDir::new().unwrap();
            let marker = dir.path().join("wire");
            let mut child = Command::new("python3").args(["-c", "import sys; from pathlib import Path; Path(sys.argv[1]).write_bytes(sys.stdin.buffer.readline())"]).arg(&marker)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
            let state = Arc::new(Mutex::new(WireState {
                call_generation: Some(0),
                ..WireState::default()
            }));
            let writer = Arc::new(tokio::sync::Mutex::new(child.stdin.take().unwrap()));
            let held = writer.lock().await;
            let mut transport = StdioTransport {
                reader: BufReader::new(child.stdout.take().unwrap()),
                writer: writer.clone(),
                state: state.clone(),
                cancellation: CancellationToken::new(),
                max_frame: 4096,
            };
            let message: TxJsonRpcMessage<RoleClient> = serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":{"message":"hello"}}})).unwrap();
            let task = tokio::spawn(transport.send(message));
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while state.lock().unwrap().pending.is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if drift {
                validate_message(
                    json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"}),
                    &state,
                )
                .unwrap();
            }
            drop(held);
            let result = task.await.unwrap();
            if drift {
                assert!(result.is_err());
                assert_eq!(health(&state).unwrap_err().code, "mcp.capability_drift");
                assert!(!state.lock().unwrap().call_started);
                child.kill().await.unwrap();
                child.wait().await.unwrap();
                assert!(!marker.exists());
            } else {
                result.unwrap();
                assert!(child.wait().await.unwrap().success());
                assert!(state.lock().unwrap().call_started);
                assert!(marker.exists());
            }
        }
    }
}
