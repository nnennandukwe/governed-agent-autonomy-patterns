use schemars::{Schema, schema_for};
use serde_json::Value;

use super::model::{AgentRunRequest, TerminalRunReceipt};
use super::protected_effect::{ProtectedEffectRequest, ProtectedEffectResult};

const REQUEST_SCHEMA_ID: &str = "https://raw.githubusercontent.com/nnennandukwe/governed-agent-autonomy-patterns/main/schemas/agent-run/v0.1.0/agent-run-request.schema.json";
const RECEIPT_SCHEMA_ID: &str = "https://raw.githubusercontent.com/nnennandukwe/governed-agent-autonomy-patterns/main/schemas/agent-run/v0.1.0/terminal-run-receipt.schema.json";
const EFFECT_REQUEST_SCHEMA_ID: &str = "https://raw.githubusercontent.com/nnennandukwe/governed-agent-autonomy-patterns/main/schemas/protected-effect/v0.1.0/protected-effect-request.schema.json";
const EFFECT_RESULT_SCHEMA_ID: &str = "https://raw.githubusercontent.com/nnennandukwe/governed-agent-autonomy-patterns/main/schemas/protected-effect/v0.1.0/protected-effect-result.schema.json";

/// Generate the Draft 2020-12 Agent Run Request schema from the Rust types.
pub fn agent_run_request_schema() -> Value {
    document(
        schema_for!(AgentRunRequest),
        REQUEST_SCHEMA_ID,
        "GAAP Agent Run Request 0.1.0",
    )
}

/// Generate the Draft 2020-12 Terminal Run Receipt schema from the Rust types.
pub fn terminal_run_receipt_schema() -> Value {
    document(
        schema_for!(TerminalRunReceipt),
        RECEIPT_SCHEMA_ID,
        "GAAP Terminal Run Receipt 0.1.0",
    )
}

/// Generate the Draft 2020-12 Protected Effect Request schema from the Rust type.
pub fn protected_effect_request_schema() -> Value {
    document(
        schema_for!(ProtectedEffectRequest),
        EFFECT_REQUEST_SCHEMA_ID,
        "GAAP Protected Effect Request 0.1.0",
    )
}

/// Generate the Draft 2020-12 Protected Effect Result schema from the Rust type.
pub fn protected_effect_result_schema() -> Value {
    document(
        schema_for!(ProtectedEffectResult),
        EFFECT_RESULT_SCHEMA_ID,
        "GAAP Protected Effect Result 0.1.0",
    )
}

fn document(mut schema: Schema, id: &str, title: &str) -> Value {
    schema.insert("$id".to_owned(), id.into());
    schema.insert("title".to_owned(), title.into());
    serde_json::to_value(schema).expect("schemars schemas must serialize")
}
