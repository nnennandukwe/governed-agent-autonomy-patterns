mod agent_run;
mod catalog;
mod protected_effect;
mod reconcile;

use std::path::Path;
use std::process::ExitCode;

use catalog::{CONTRACTS, render_catalog};
use reconcile::{ArtifactError, Mode, Report, reconcile};

const USAGE: &str = "usage: cargo run --locked --example generate-contract-artifacts -- [--check]";

fn main() -> ExitCode {
    let mode = match parse_mode(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::FAILURE;
        }
    };
    match run(Path::new(env!("CARGO_MANIFEST_DIR")), mode) {
        Ok(report) => {
            print_report(&report);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("contract artifact operation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_mode(arguments: impl IntoIterator<Item = String>) -> Result<Mode, String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => Ok(Mode::Generate),
        [argument] if argument == "--check" => Ok(Mode::Check),
        _ => Err(USAGE.to_owned()),
    }
}

fn run(repository_root: &Path, mode: Mode) -> Result<Report, ArtifactError> {
    let artifacts = render_catalog(CONTRACTS)?;
    reconcile(repository_root, &artifacts, mode)
}

fn print_report(report: &Report) {
    for path in &report.written {
        println!("wrote artifact: {path}");
    }
    for path in &report.pruned {
        println!("pruned unregistered artifact: {path}");
    }
    for path in &report.current {
        println!("artifact is current: {path}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_only_generate_and_check_forms() {
        assert_eq!(parse_mode([]).unwrap(), Mode::Generate);
        assert_eq!(parse_mode(["--check".to_owned()]).unwrap(), Mode::Check);
        assert_eq!(parse_mode(["--help".to_owned()]).unwrap_err(), USAGE);
        assert_eq!(
            parse_mode(["--check".to_owned(), "extra".to_owned()]).unwrap_err(),
            USAGE
        );
    }
}
