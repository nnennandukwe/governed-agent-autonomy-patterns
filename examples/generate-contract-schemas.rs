use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gaap::contracts::{agent_run_request_schema, terminal_run_receipt_schema};

const REQUEST_PATH: &str = "schemas/agent-run/v0.1.0/agent-run-request.schema.json";
const RECEIPT_PATH: &str = "schemas/agent-run/v0.1.0/terminal-run-receipt.schema.json";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("schema generation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let check = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--check" => true,
        _ => {
            return Err(
                "usage: cargo run --locked --example generate-contract-schemas -- [--check]"
                    .to_owned(),
            );
        }
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let schemas = [
        (REQUEST_PATH, agent_run_request_schema()),
        (RECEIPT_PATH, terminal_run_receipt_schema()),
    ];
    for (relative_path, schema) in schemas {
        let path = root.join(relative_path);
        let contents = format!(
            "{}\n",
            serde_json::to_string_pretty(&schema)
                .map_err(|error| format!("could not serialize {relative_path}: {error}"))?
        );
        if check {
            check_schema(&path, relative_path, &contents)?;
        } else {
            write_schema(&path, relative_path, &contents)?;
        }
    }
    Ok(())
}

fn check_schema(path: &Path, relative_path: &str, expected: &str) -> Result<(), String> {
    let actual = fs::read_to_string(path)
        .map_err(|error| format!("could not read {relative_path}: {error}"))?;
    if actual != expected {
        return Err(format!(
            "{relative_path} is stale; run `cargo run --locked --example generate-contract-schemas`"
        ));
    }
    println!("schema is current: {relative_path}");
    Ok(())
}

fn write_schema(path: &Path, relative_path: &str, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("schema path has no parent: {relative_path}"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    fs::write(path, contents)
        .map_err(|error| format!("could not write {relative_path}: {error}"))?;
    println!("wrote schema: {relative_path}");
    Ok(())
}
