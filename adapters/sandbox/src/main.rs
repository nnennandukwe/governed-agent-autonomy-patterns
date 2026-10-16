//! One bounded JSON request over stdin; host configuration is an explicit file.
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use gaap::sandbox::{SandboxAdapter, SandboxRequest};
use gaap_sandbox::{LocalOciAdapter, OciConfig};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 {
        return Err("Usage: gaap-sandbox <host-config.json>; send one request on stdin".into());
    }
    let cancellation = Arc::new(AtomicBool::new(false));
    let signal = cancellation.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Release))?;
    if !std::fs::metadata(&arguments[0])?.is_file() {
        return Err("Select a regular host configuration file".into());
    }
    let mut config_bytes = Vec::new();
    std::fs::File::open(&arguments[0])?
        .take(65537)
        .read_to_end(&mut config_bytes)?;
    if config_bytes.len() > 65536 {
        return Err("Host configuration exceeds 64 KiB".into());
    }
    let config: OciConfig = serde_json::from_slice(&config_bytes)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::io::stdin()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let bytes = loop {
        if cancellation.load(Ordering::Acquire) {
            return Err("Sandbox input cancelled before dispatch".into());
        }
        if Instant::now() >= deadline {
            return Err("Send one bounded request and close stdin within five seconds".into());
        }
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(result) => break result?,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(error) => return Err(error.into()),
        }
    };
    if bytes.len() > 1024 * 1024 {
        return Err("Sandbox request exceeds 1 MiB".into());
    }
    let request: SandboxRequest = serde_json::from_slice(&bytes)?;
    let observation =
        LocalOciAdapter::new(config).execute(&request, &|| cancellation.load(Ordering::Acquire));
    let encoded = serde_jcs::to_vec(&observation)?;
    let mut output = std::io::stdout().lock();
    output.write_all(&encoded)?;
    output.write_all(b"\n")?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Sandbox request rejected: {error}");
        std::process::exit(1);
    }
}
