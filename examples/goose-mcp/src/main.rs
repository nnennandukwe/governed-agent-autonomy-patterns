use gaap_goose_demo::{
    coding::{CodingStore, mcp::CodingServer},
    engine,
    protocol::BoundaryServer,
    store::Store,
};
use rmcp::ServiceExt;
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};

const HELP: &str = "GAAP conference example\n\n  gaap-goose-demo coding-init ROOT\n  gaap-goose-demo coding-serve ROOT\n  gaap-goose-demo coding-inspect ROOT\n  gaap-goose-demo coding-submit ROOT  (JSON on stdin)\n\n  gaap-goose-demo init ROOT\n  gaap-goose-demo serve --state-dir ROOT/operator\n  gaap-goose-demo inspect ROOT/operator\n  gaap-goose-demo approve ROOT/operator REQUEST_ID [--yes]\n  gaap-goose-demo engine [completed|approval-required|stale-verification]\n\nApproval is operator-only. --yes explicitly approves without an interactive prompt.\nUse a fresh ROOT for every rehearsal; existing sessions are never reset.";
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
        eprintln!("gaap-goose-demo: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["--help"] | ["-h"] => {
            println!("{HELP}");
            Ok(())
        }
        ["coding-init", root] => print(CodingStore::initialize(Path::new(root))?.inspect()?),
        ["coding-inspect", root] => print(CodingStore::open(Path::new(root))?.inspect()?),
        ["coding-submit", root] => {
            let input = serde_json::from_reader(io::stdin()).map_err(|e| e.to_string())?;
            print(CodingStore::open(Path::new(root))?.submit(input)?)
        }
        ["coding-serve", root] => {
            let service = CodingServer(CodingStore::open(Path::new(root))?)
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| e.to_string())?;
            service.waiting().await.map_err(|e| e.to_string())?;
            Ok(())
        }
        ["init", root] => {
            let store = Store::initialize(Path::new(root))?;
            print(serde_json::json!({"workspace":store.workspace(),"state_dir":store.state_dir()}))
        }
        ["serve", "--state-dir", directory] => {
            let server = BoundaryServer(Store::open(Path::new(directory))?);
            let service = server
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| e.to_string())?;
            service.waiting().await.map_err(|e| e.to_string())?;
            Ok(())
        }
        ["inspect", directory] => print(Store::open(Path::new(directory))?.inspect()?),
        ["approve", directory, id] | ["approve", directory, id, "--yes"] => {
            let store = Store::open(Path::new(directory))?;
            let state = store.inspect()?;
            let proposal = state["proposals"].get(*id).ok_or("unknown request_id")?;
            println!("Operator approval for {id}\nExact immutable proposal (before / content):");
            let proposal_json =
                serde_json::to_string_pretty(&proposal["proposal"]).map_err(|e| e.to_string())?;
            // JSON escapes C0 controls; render all remaining Unicode as visible code points.
            let safe_view: String = proposal_json
                .chars()
                .map(|c| {
                    if c.is_ascii() {
                        c.to_string()
                    } else {
                        c.escape_unicode().to_string()
                    }
                })
                .collect();
            println!("{safe_view}");
            if args.len() == 3 {
                if !io::stdin().is_terminal() {
                    return Err("interactive approval requires a terminal; operator automation can explicitly use --yes".into());
                }
                eprint!("Approve this exact proposal? Type approve: ");
                io::stderr().flush().map_err(|e| e.to_string())?;
                let mut answer = String::new();
                io::stdin()
                    .read_line(&mut answer)
                    .map_err(|e| e.to_string())?;
                if answer.trim() != "approve" {
                    return Err("approval cancelled; no authority recorded".into());
                }
            }
            print(store.approve(id)?)
        }
        ["engine"] => print(engine::run("completed")?),
        ["engine", scenario] => print(engine::run(scenario)?),
        _ => Err(format!("invalid arguments\n\n{HELP}")),
    }
}
