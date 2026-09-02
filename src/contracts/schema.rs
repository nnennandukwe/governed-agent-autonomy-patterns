use schemars::{Schema, schema_for};
use serde_json::Value;

use super::model::{AgentRunRequest, TerminalRunReceipt};

const REQUEST_SCHEMA_ID: &str = "https://raw.githubusercontent.com/nnennandukwe/governed-agent-autonomy-patterns/main/schemas/agent-run/v0.1.0/agent-run-request.schema.json";
const RECEIPT_SCHEMA_ID: &str = "https://raw.githubusercontent.com/nnennandukwe/governed-agent-autonomy-patterns/main/schemas/agent-run/v0.1.0/terminal-run-receipt.schema.json";

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

fn document(mut schema: Schema, id: &str, title: &str) -> Value {
    schema.insert("$id".to_owned(), id.into());
    schema.insert("title".to_owned(), title.into());
    serde_json::to_value(schema).expect("schemars schemas must serialize")
}
