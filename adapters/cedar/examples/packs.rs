//! Author and inspect local examples; activation stays with the embedding host.
use gaap::contracts::*;
use gaap_cedar::{ApprovedAction, PreparedPack, content_digest, exact_permit};
use serde_json::{Value, json};
use std::path::PathBuf;

fn fixture_digest(hex_digit: char) -> String {
    format!("sha256:{}", hex_digit.to_string().repeat(64))
}
fn examples() -> Vec<(&'static str, ApprovedAction, RequestedScope, Value)> {
    let cap = CapabilityIdentity {
        name: "local.example".into(),
        version: "1".into(),
        digest: fixture_digest('a'),
    };
    let mcp=ApprovedAction::mcp(fixture_digest('1'),"2025-11-25".into(),json!({"name":"lookup","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}})).unwrap();
    vec![
        (
            "filesystem",
            ApprovedAction::native(
                OperationFamily::Filesystem,
                "filesystem.write".into(),
                cap.clone(),
            ),
            RequestedScope::Filesystem {
                root: "/workspace/report.txt".into(),
                access: vec![FilesystemAccess::Modify],
                recursive: false,
            },
            json!({"path":"/workspace/report.txt","content":"approved"}),
        ),
        (
            "process",
            ApprovedAction::native(OperationFamily::Process, "process.exec".into(), cap.clone()),
            RequestedScope::Process {
                executable: "/usr/bin/git".into(),
                working_directory: "/workspace".into(),
            },
            json!({"argv":["status","--short"]}),
        ),
        (
            "network",
            ApprovedAction::native(OperationFamily::Network, "network.connect".into(), cap),
            RequestedScope::Network {
                protocol: NetworkProtocol::Https,
                host: "example.invalid".into(),
                port: 443,
            },
            json!({"method":"GET","path":"/health"}),
        ),
        (
            "mcp",
            mcp,
            RequestedScope::ExternalService {
                service: fixture_digest('1'),
                operation: "lookup".into(),
                resource: "/workspace".into(),
            },
            json!({"server":fixture_digest('1'),"tool":"lookup","arguments":{"query":"approved"}}),
        ),
    ]
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        println!(
            "Usage: packs [--write|--check]\nDefault: inspect validated local examples as JSON.\n--write: explicitly regenerate local policy files and pins.\n--check: reject local source or identity drift.\nPolicies are never downloaded or activated."
        );
        return Ok(());
    }
    if args.len() > 1
        || args
            .first()
            .is_some_and(|arg| !["--write", "--check"].contains(&arg.as_str()))
    {
        return Err("use --help for authoring and inspection commands".into());
    }
    let write = args.first().is_some_and(|arg| arg == "--write");
    let check = args.first().is_some_and(|arg| arg == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packs");
    let mut pins = serde_json::Map::new();
    let mut inspected = serde_json::Map::new();
    for (name, action, scope, input) in examples() {
        let input_digest = content_digest(&input)?;
        let source = format!(
            "// Exact {name} example. Review the scope and ordered input before pinning.\n{}\nforbid(principal, action, resource) when {{ context.subject_locator == \"/workspace/blocked\" }};\n",
            exact_permit("example-run", &action, &scope, &input_digest)?
        );
        let file = root.join(format!("{name}.cedar"));
        if write {
            std::fs::write(&file, &source)?;
        }
        let saved = std::fs::read_to_string(&file)?;
        if saved != source {
            return Err(format!(
                "{} differs from its example; inspect saved edits before choosing --write",
                file.display()
            )
            .into());
        }
        let pack = PreparedPack::prepare(
            &format!("example-{name}"),
            "1",
            "example-run",
            &saved,
            vec![action.clone()],
        )?;
        pins.insert(name.into(), json!(pack.identities()));
        inspected.insert(name.into(),json!({"policies":pack.identities(),"schema":pack.schema_document(),"action":action,"scope":scope,"input":input,"input_digest":input_digest,"source":saved}));
    }
    let pin_path = root.join("pins.json");
    if write {
        std::fs::write(&pin_path, serde_json::to_string_pretty(&pins)? + "\n")?;
    }
    if check {
        let saved: Value = serde_json::from_slice(&std::fs::read(&pin_path)?)?;
        if saved != Value::Object(pins) {
            return Err("local policy/schema pins drifted; review before choosing --write".into());
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&Value::Object(inspected))?
    );
    Ok(())
}
