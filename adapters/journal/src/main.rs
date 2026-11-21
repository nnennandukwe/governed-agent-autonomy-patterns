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
    use gaap_journal::{Host, PROPOSAL_LIMIT, Request, canonical, digest, json_document};
    use rustix::fs::{OFlags, fcntl_getfl};
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    // The supervisor assigns these descriptors before exec. F_DUPFD_CLOEXEC fails
    // safely for a missing descriptor and gives this process a separately owned fd.
    fn inherited(index: i32) -> Result<File> {
        // SAFETY: fcntl accepts any integer fd, including an invalid one.
        let fd = unsafe { libc::fcntl(index, libc::F_DUPFD_CLOEXEC, 5) };
        if fd < 0 {
            return Err(Error::unavailable("trusted host descriptor missing"));
        }
        // SAFETY: a successful F_DUPFD_CLOEXEC just created this descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn read_bounded(reader: impl Read, max: usize, oversize: Error) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        reader
            .take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::unavailable("couldn't read bounded input"))?;
        if bytes.len() > max {
            return Err(oversize);
        }
        Ok(bytes)
    }

    fn host_config() -> Result<Host> {
        let file = inherited(3)?;
        let md = file
            .metadata()
            .map_err(|_| Error::unavailable("couldn't inspect trusted configuration"))?;
        let flags = fcntl_getfl(&file)
            .map_err(|_| Error::unavailable("couldn't inspect host descriptor flags"))?;
        if !md.is_file()
            || md.mode() & 0o7777 != 0o600
            || md.nlink() != 1
            || md.uid() != rustix::process::geteuid().as_raw()
            || flags.intersects(OFlags::WRONLY | OFlags::RDWR | OFlags::PATH)
        {
            return Err(Error::unavailable(
                "host configuration needs a private read-only descriptor",
            ));
        }
        const HOST_LIMIT: usize = 32 * 1024 * 1024;
        if md.len() > HOST_LIMIT as u64 {
            return Err(Error::unavailable("host configuration too large"));
        }
        // Read from offset zero even if a supervisor reused the open description.
        let reader = File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .map_err(|_| Error::unavailable("couldn't reopen configuration"))?;
        let raw = read_bounded(
            reader,
            HOST_LIMIT,
            Error::unavailable("host configuration too large"),
        )?;
        let value = json_document(&raw, false)?;
        let original = value
            .get("profile")
            .ok_or_else(|| Error::invalid("missing original profile"))?;
        if Some(digest(&canonical(original)?).as_str())
            != value
                .pointer("/slot/profile_digest")
                .and_then(Value::as_str)
        {
            return Err(Error::invalid(
                "profile bytes disagree with provisioning pin",
            ));
        }
        serde_json::from_value(value)
            .map_err(|_| Error::invalid("invalid host configuration shape"))
    }

    fn caller_proposal() -> Result<Value> {
        let maxed = || Error::new("capacity", "proposal exceeds its enrolled byte limit");
        let raw = read_bounded(io::stdin().lock(), PROPOSAL_LIMIT, maxed())?;
        let proposal = json_document(&raw, false)
            .map_err(|_| Error::new("invalid_request", "invalid proposal JSON"))?;
        if canonical(&proposal)?.len() > PROPOSAL_LIMIT {
            return Err(maxed());
        }
        Ok(proposal)
    }

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
