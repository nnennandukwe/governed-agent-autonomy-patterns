use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{Arc, Mutex};

use futures::{Sink, Stream};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use crate::{EvidenceHandle, Limits, TraceEntry, digest};

#[derive(Default, PartialEq, Eq)]
enum Phase {
    #[default]
    New,
    Initialized,
    Ready,
    Prompt,
    Terminal,
}

#[derive(Default)]
struct ProtocolState {
    phase: Phase,
    session: Option<String>,
    outgoing: BTreeMap<String, String>,
    incoming: BTreeSet<String>,
    answered: BTreeSet<String>,
    tools: BTreeMap<String, String>,
    messages: usize,
    total_bytes: usize,
    output_seen: bool,
    goose_inventory: bool,
    inventory_seen: bool,
}

type State = Arc<Mutex<ProtocolState>>;

pub(crate) fn transport(
    stdin: ChildStdin,
    stdout: ChildStdout,
    limits: Limits,
    evidence: EvidenceHandle,
    goose_inventory: bool,
) -> (
    impl Sink<String, Error = io::Error> + Send,
    impl Stream<Item = io::Result<String>> + Send,
) {
    let state = Arc::new(Mutex::new(ProtocolState {
        goose_inventory,
        ..ProtocolState::default()
    }));
    let out_state = state.clone();
    let out_evidence = evidence.clone();
    let outgoing = futures::sink::unfold(stdin, move |mut stdin, line: String| {
        let state = out_state.clone();
        let evidence = out_evidence.clone();
        async move {
            validate(&line, false, &state, limits, &evidence)?;
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
            Ok(stdin)
        }
    });
    let incoming = futures::stream::unfold(
        (BufReader::new(stdout), false),
        move |(mut reader, done)| {
            let state = state.clone();
            let evidence = evidence.clone();
            async move {
                if done {
                    return None;
                }
                let result = read_frame(&mut reader, limits.max_frame_bytes)
                    .await
                    .and_then(|line| {
                        validate(&line, true, &state, limits, &evidence)?;
                        Ok(line)
                    });
                let result = result.map_err(|problem| {
                    let protocol = state.lock().expect("wire lock");
                    if problem.to_string() == "acp.agent_exit"
                        && protocol.phase == Phase::Prompt
                        && protocol.output_seen
                    {
                        fail("acp.partial_output")
                    } else {
                        problem
                    }
                });
                if let Err(ref problem) = result {
                    let code = problem.to_string();
                    evidence.record_failure(if code.starts_with("acp.") {
                        code
                    } else {
                        "acp.read_failed".into()
                    });
                }
                let done = result.is_err();
                Some((result, (reader, done)))
            }
        },
    );
    (outgoing, incoming)
}

async fn read_frame(reader: &mut BufReader<ChildStdout>, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Err(fail(if bytes.is_empty() {
                "acp.agent_exit"
            } else {
                "acp.partial_frame"
            }));
        }
        let take = chunk
            .iter()
            .position(|b| *b == b'\n')
            .map_or(chunk.len(), |n| n + 1);
        if bytes.len().saturating_add(take) > limit {
            return Err(fail("acp.frame_limit"));
        }
        let complete = chunk[take - 1] == b'\n';
        bytes.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        if complete {
            return String::from_utf8(bytes).map_err(|_| fail("acp.invalid_utf8"));
        }
    }
}

fn fail(code: &'static str) -> io::Error {
    io::Error::other(code)
}

fn validate(
    line: &str,
    incoming: bool,
    state: &State,
    limits: Limits,
    evidence: &EvidenceHandle,
) -> io::Result<()> {
    let result = validate_message(line, incoming, state, limits, evidence);
    if let Err(ref e) = result {
        let code = e.to_string();
        evidence.record_failure(code);
    }
    result
}

fn validate_message(
    line: &str,
    incoming: bool,
    state: &State,
    limits: Limits,
    evidence: &EvidenceHandle,
) -> io::Result<()> {
    // Incoming lines retain their delimiter; outgoing lines gain one at write.
    let wire_bytes = line.len().saturating_add(usize::from(!incoming));
    if wire_bytes > limits.max_frame_bytes {
        return Err(fail("acp.frame_limit"));
    }
    let mut decoder = serde_json::Deserializer::from_str(line);
    let value = StrictValue::deserialize(&mut decoder)
        .map_err(|_| fail("acp.invalid_json"))?
        .0;
    decoder.end().map_err(|_| fail("acp.invalid_json"))?;
    validate_schema(&value, incoming)?;
    let object = value
        .as_object()
        .ok_or_else(|| fail("acp.invalid_envelope"))?;
    if value["jsonrpc"] != "2.0"
        || object.keys().any(|k| {
            !matches!(
                k.as_str(),
                "jsonrpc" | "id" | "method" | "params" | "result" | "error"
            )
        })
    {
        return Err(fail("acp.invalid_envelope"));
    }
    let mut state = state.lock().expect("wire lock");
    state.messages += 1;
    state.total_bytes = state.total_bytes.saturating_add(wire_bytes);
    if state.total_bytes > limits.max_total_bytes {
        return Err(fail("acp.total_bytes_limit"));
    }
    if state.messages > limits.max_messages {
        return Err(fail("acp.message_limit"));
    }
    let id = object
        .get("id")
        .map(|id| {
            if !(id
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 256)
                || id.as_i64().is_some())
            {
                return Err(fail("acp.invalid_id"));
            }
            Ok(id.to_string())
        })
        .transpose()?;
    let method = if let Some(method) = object.get("method") {
        if object.contains_key("result") || object.contains_key("error") {
            return Err(fail("acp.invalid_envelope"));
        }
        let method = method.as_str().ok_or_else(|| fail("acp.invalid_method"))?;
        if !value["params"].is_object() {
            return Err(fail("acp.invalid_params"));
        }
        if incoming {
            let passive = state.goose_inventory
                && method == "session/update"
                && matches!(
                    value["params"]["update"]["sessionUpdate"].as_str(),
                    Some(
                        "session_info_update"
                            | "usage_update"
                            | "available_commands_update"
                            | "config_option_update"
                            | "current_mode_update"
                    )
                );
            if (state.phase != Phase::Prompt && !(passive && state.phase == Phase::Ready))
                || value["params"]["sessionId"].as_str() != state.session.as_deref()
            {
                return Err(fail("acp.session_order"));
            }
            if let Some(id) = id {
                if !state.incoming.is_empty() || !state.answered.insert(id.clone()) {
                    return Err(fail("acp.duplicate_request"));
                }
                if !matches!(
                    method,
                    "fs/read_text_file" | "fs/write_text_file" | "session/request_permission"
                ) {
                    return Err(fail("acp.unsupported_effect"));
                }
                if method.starts_with("fs/") {
                    if value["params"]["path"].as_str().is_none() {
                        return Err(fail("acp.incomplete_input"));
                    }
                    if method == "fs/write_text_file"
                        && value["params"]["content"].as_str().is_none()
                    {
                        return Err(fail("acp.incomplete_input"));
                    }
                    for key in ["line", "limit"] {
                        if let Some(v) = value["params"].get(key)
                            && !v.is_null()
                            && v.as_u64().is_none_or(|n| n == 0 || n > u32::MAX.into())
                        {
                            return Err(fail("acp.invalid_file_range"));
                        }
                    }
                }
                state.incoming.insert(id);
            } else {
                if method != "session/update" {
                    return Err(fail("acp.unsupported_notification"));
                }
                if !passive {
                    update(&value["params"]["update"], &mut state)?;
                }
            }
        } else if let Some(id) = id {
            let expected = match method {
                "initialize" => Phase::New,
                "session/new" => Phase::Initialized,
                "session/prompt" => Phase::Ready,
                crate::goose::INVENTORY_METHOD
                    if state.goose_inventory && !state.inventory_seen =>
                {
                    Phase::Ready
                }
                _ => return Err(fail("acp.unsupported_request")),
            };
            if state.phase != expected || !state.outgoing.is_empty() {
                return Err(fail("acp.request_order"));
            }
            if method == crate::goose::INVENTORY_METHOD {
                if value["params"]["sessionId"].as_str() != state.session.as_deref() {
                    return Err(fail("acp.session_order"));
                }
                state.inventory_seen = true;
            }
            if method == "session/prompt" && state.goose_inventory && !state.inventory_seen {
                return Err(fail("acp.inventory_required"));
            }
            if method == "session/prompt" {
                state.phase = Phase::Prompt;
            }
            state.outgoing.insert(id, method.into());
        } else if method != "session/cancel" {
            return Err(fail("acp.unsupported_notification"));
        }
        method.to_owned()
    } else {
        let id = id.ok_or_else(|| fail("acp.missing_id"))?;
        if object.contains_key("result") == object.contains_key("error") {
            return Err(fail("acp.invalid_response"));
        }
        if incoming {
            let method = state
                .outgoing
                .remove(&id)
                .ok_or_else(|| fail("acp.unexpected_response"))?;
            if object.contains_key("error") {
                return Err(fail(if value["error"]["code"].as_i64() == Some(-32000) {
                    "acp.authentication_required"
                } else {
                    "acp.agent_error"
                }));
            }
            validate_response(&method, &value["result"])?;
            match method.as_str() {
                "initialize" => {
                    if value["result"]["protocolVersion"] != 1 {
                        return Err(fail("acp.version_mismatch"));
                    }
                    if !value["result"]["agentCapabilities"].is_object()
                        || !value["result"]["agentInfo"].is_object()
                    {
                        return Err(fail("acp.missing_identity"));
                    }
                    evidence.0.lock().expect("evidence lock").negotiation_digest =
                        Some(digest(&value["result"]).map_err(|_| fail("acp.invalid_identity"))?);
                    state.phase = Phase::Initialized;
                }
                "session/new" => {
                    let session = value["result"]["sessionId"]
                        .as_str()
                        .filter(|v| !v.is_empty() && v.len() <= 256)
                        .ok_or_else(|| fail("acp.invalid_session"))?;
                    state.session = Some(session.into());
                    state.phase = Phase::Ready;
                    evidence.0.lock().expect("evidence lock").session_digest =
                        Some(digest(&value["result"]).map_err(|_| fail("acp.invalid_session"))?);
                }
                crate::goose::INVENTORY_METHOD => (),
                "session/prompt" => {
                    if !state.incoming.is_empty()
                        || state
                            .tools
                            .values()
                            .any(|s| !matches!(s.as_str(), "completed" | "failed"))
                    {
                        return Err(fail("acp.incomplete_operations"));
                    }
                    if value["result"]["stopReason"].as_str().is_none() {
                        return Err(fail("acp.missing_stop_reason"));
                    }
                    state.phase = Phase::Terminal;
                }
                _ => return Err(fail("acp.unexpected_response")),
            }
            method
        } else {
            if !state.incoming.remove(&id) {
                return Err(fail("acp.unexpected_response"));
            }
            "host_effect_response".into()
        }
    };
    evidence
        .0
        .lock()
        .expect("evidence lock")
        .trace
        .push(TraceEntry {
            direction: if incoming {
                "agent_to_host"
            } else {
                "host_to_agent"
            }
            .into(),
            method,
            frame_digest: digest(&line).map_err(|_| fail("acp.invalid_json"))?,
        });
    Ok(())
}

fn update(update: &Value, state: &mut ProtocolState) -> io::Result<()> {
    let kind = update["sessionUpdate"]
        .as_str()
        .ok_or_else(|| fail("acp.invalid_update"))?;
    match kind {
        "agent_message_chunk" | "agent_thought_chunk" => {
            state.output_seen = true;
            if update["content"]["type"] != "text" || !update["content"]["text"].is_string() {
                return Err(fail("acp.invalid_content"));
            }
        }
        "tool_call" => {
            let id = update["toolCallId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 256)
                .ok_or_else(|| fail("acp.invalid_tool_id"))?;
            let status = update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            if !matches!(status, "pending" | "in_progress")
                || state.tools.insert(id.into(), status.into()).is_some()
            {
                return Err(fail("acp.tool_order"));
            }
        }
        "tool_call_update" => {
            let id = update["toolCallId"]
                .as_str()
                .ok_or_else(|| fail("acp.invalid_tool_id"))?;
            let prior = state
                .tools
                .get_mut(id)
                .ok_or_else(|| fail("acp.tool_order"))?;
            if matches!(prior.as_str(), "completed" | "failed") {
                return Err(fail("acp.tool_order"));
            }
            if let Some(status) = update.get("status") {
                let status = status
                    .as_str()
                    .ok_or_else(|| fail("acp.invalid_tool_status"))?;
                if !matches!(status, "pending" | "in_progress" | "completed" | "failed")
                    || (prior == "in_progress" && status == "pending")
                {
                    return Err(fail("acp.tool_order"));
                }
                *prior = status.into();
            }
        }
        "plan" | "usage_update" => (),
        _ => return Err(fail("acp.unsupported_update")),
    }
    Ok(())
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
                if v.unsigned_abs() > 9_007_199_254_740_991 {
                    return Err(E::custom(
                        "integer exceeds the exact canonical profile range",
                    ));
                }
                Ok(StrictValue(Value::from(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                if v > 9_007_199_254_740_991 {
                    return Err(E::custom(
                        "integer exceeds the exact canonical profile range",
                    ));
                }
                Ok(StrictValue(Value::from(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                if !v.is_finite() || v.abs() > 9_007_199_254_740_991_u64 as f64 {
                    return Err(E::custom("number exceeds the canonical profile range"));
                }
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

fn schema<T: schemars::JsonSchema>() -> jsonschema::Validator {
    let schema = serde_json::to_value(schemars::schema_for!(T)).expect("SDK schema serializes");
    jsonschema::validator_for(&schema).expect("SDK schema compiles")
}
fn validate_schema(value: &Value, incoming: bool) -> io::Result<()> {
    use agent_client_protocol::schema::v1::*;
    use std::sync::OnceLock;
    static READ: OnceLock<jsonschema::Validator> = OnceLock::new();
    static WRITE: OnceLock<jsonschema::Validator> = OnceLock::new();
    static PERMISSION: OnceLock<jsonschema::Validator> = OnceLock::new();
    static UPDATE: OnceLock<jsonschema::Validator> = OnceLock::new();
    if !incoming {
        return Ok(());
    }
    let validator = match value.get("method").and_then(Value::as_str) {
        Some("fs/read_text_file") => Some(READ.get_or_init(schema::<ReadTextFileRequest>)),
        Some("fs/write_text_file") => Some(WRITE.get_or_init(schema::<WriteTextFileRequest>)),
        Some("session/request_permission") => {
            Some(PERMISSION.get_or_init(schema::<RequestPermissionRequest>))
        }
        Some("session/update") => Some(UPDATE.get_or_init(schema::<SessionNotification>)),
        _ => None,
    };
    if validator.is_some_and(|v| !v.is_valid(&value["params"])) {
        return Err(fail("acp.malformed_params"));
    }
    Ok(())
}
fn validate_response(method: &str, result: &Value) -> io::Result<()> {
    use agent_client_protocol::schema::v1::*;
    use std::sync::OnceLock;
    static INITIALIZE: OnceLock<jsonschema::Validator> = OnceLock::new();
    static SESSION: OnceLock<jsonschema::Validator> = OnceLock::new();
    static PROMPT: OnceLock<jsonschema::Validator> = OnceLock::new();
    if method == crate::goose::INVENTORY_METHOD {
        return if result["extensions"].is_array() {
            Ok(())
        } else {
            Err(fail("acp.malformed_inventory"))
        };
    }
    let validator = match method {
        "initialize" => INITIALIZE.get_or_init(schema::<InitializeResponse>),
        "session/new" => SESSION.get_or_init(schema::<NewSessionResponse>),
        "session/prompt" => PROMPT.get_or_init(schema::<PromptResponse>),
        _ => return Err(fail("acp.unexpected_response")),
    };
    if !validator.is_valid(result) {
        return Err(fail("acp.malformed_result"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_caps_include_the_transmitted_newline() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;
        for total in [false, true] {
            for extra_byte in [false, true] {
                let mut limits = Limits::default();
                if total {
                    limits.max_total_bytes = line.len() + usize::from(extra_byte);
                } else {
                    limits.max_frame_bytes = line.len() + usize::from(extra_byte);
                }
                let state = State::default();
                let evidence =
                    EvidenceHandle(Arc::new(Mutex::new(crate::SessionEvidence::default())));
                let result = validate(line, false, &state, limits, &evidence);
                if extra_byte {
                    result.unwrap();
                    assert_eq!(state.lock().unwrap().total_bytes, line.len() + 1);
                } else {
                    assert_eq!(
                        result.unwrap_err().to_string(),
                        if total {
                            "acp.total_bytes_limit"
                        } else {
                            "acp.frame_limit"
                        }
                    );
                    assert!(evidence.snapshot().trace.is_empty());
                }
            }
        }
    }
}
