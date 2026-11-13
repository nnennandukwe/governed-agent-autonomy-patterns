use gaap_submitted_proposal_reference::coding::{CodingStore, mcp::CodingServer};
use rmcp::ServiceExt;
use std::{io, path::Path};
fn print(value: impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("gaap-submitted-proposal-reference: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["--help"] | ["-h"] => {
            println!(
                "GAAP submitted-proposal reference\n\n  coding-init ROOT\n  coding-inspect ROOT\n  coding-submit ROOT (JSON on stdin)\n  coding-serve ROOT (MCP stdio)\n\nCreate a fresh ROOT; existing sessions stay intact."
            );
            Ok(())
        }
        ["coding-init", root] => print(CodingStore::initialize(Path::new(root))?.inspect()?),
        ["coding-inspect", root] => print(CodingStore::open(Path::new(root))?.inspect()?),
        ["coding-submit", root] => {
            let input = serde_json::from_reader(io::stdin()).map_err(|e| e.to_string())?;
            print(CodingStore::open(Path::new(root))?.submit(input)?)
        }
        ["coding-serve", root] => {
            let server = CodingServer::new(CodingStore::open(Path::new(root))?);
            let service = server
                .clone()
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| e.to_string())?;
            let result = service.waiting().await.map_err(|e| e.to_string());
            server.cancel_all();
            eprintln!(
                "MCP connection closed; draining pending work and retaining actual outcomes."
            );
            result?;
            Ok(())
        }
        _ => Err("invalid arguments; run --help".into()),
    }
}
