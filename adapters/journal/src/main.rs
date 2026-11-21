use gaap_journal::{Error, Result, token};
#[cfg(target_os = "linux")]
use serde_json::Value;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

const HELP: &str = "Usage: gaap-journal enroll\n       gaap-journal lookup KEY [--proposal-stdin]\n\nThe trusted host supplies configuration on fd 3 and the volume health latch on fd 4.\nLookup reads a complete local-file proposal from stdin only with --proposal-stdin.\n";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Enroll,
    Lookup(String, bool),
}

fn command(args: Vec<OsString>) -> Result<Command> {
    let invalid = || {
        Error::new(
            "invalid_request",
            "use gaap-journal --help for accepted arguments",
        )
    };
    let args = args
        .iter()
        .map(|s| s.to_str().ok_or_else(invalid))
        .collect::<Result<Vec<_>>>()?;
    match args.as_slice() {
        ["--help"] | ["-h"] => Ok(Command::Help),
        ["enroll"] => Ok(Command::Enroll),
        ["lookup", key] if token(key) => Ok(Command::Lookup((*key).to_owned(), false)),
        ["lookup", key, "--proposal-stdin"] if token(key) => {
            Ok(Command::Lookup((*key).to_owned(), true))
        }
        _ => Err(invalid()),
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use gaap_journal::Request;
    use gaap_journal::handoff::{caller_proposal, host_config, inherited};
    pub(super) fn execute(command: Command) -> Result<Value> {
        let host = host_config()?;
        let allowed = match command {
            Command::Enroll => host.can_enroll,
            Command::Lookup(..) => host.can_read,
            Command::Help => unreachable!(),
        };
        if !allowed {
            return Err(Error::new("forbidden", "current host permission denied"));
        }
        let request = match command {
            Command::Enroll => Request::Enroll,
            Command::Lookup(key, with_proposal) => Request::Lookup {
                key,
                proposal: with_proposal.then(caller_proposal).transpose()?,
            },
            Command::Help => unreachable!(),
        };
        let health = inherited(4)?;
        gaap_journal::execute(&host, &health, &request)
    }
}

fn main() -> ExitCode {
    let result: Result<Option<serde_json::Value>> = command(std::env::args_os().skip(1).collect())
        .and_then(|cmd| {
            if cmd == Command::Help {
                return Ok(None);
            }
            #[cfg(target_os = "linux")]
            {
                linux::execute(cmd).map(Some)
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(Error::unsupported(
                    "journal storage requires qualified local Linux ext4",
                ))
            }
        });
    let (reply, exit) = match result {
        Ok(Some(reply)) => (reply, ExitCode::SUCCESS),
        Ok(None) => {
            return if io::stdout().lock().write_all(HELP.as_bytes()).is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        Err(error) => (serde_json::json!(error), ExitCode::FAILURE),
    };
    if writeln!(io::stdout().lock(), "{reply}").is_err() {
        return ExitCode::FAILURE;
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_rights_and_roots_are_never_command_arguments() {
        let parse = |args: &[&str]| command(args.iter().map(OsString::from).collect());
        assert_eq!(
            parse(&["lookup", "my.key", "--proposal-stdin"]).unwrap(),
            Command::Lookup("my.key".into(), true)
        );
        for bad in [
            &["enroll", "/other"][..],
            &["lookup", "a/b"],
            &["lookup", "key", "--principal"],
        ] {
            assert!(parse(bad).is_err());
        }
    }
}
