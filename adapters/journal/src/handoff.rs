//! Bootstrap from supervisor-owned descriptors before any child is launched.
use crate::{Error, Host, PROPOSAL_LIMIT, Result, canonical, digest, json_document};
use rustix::fs::{OFlags, fcntl_getfl};
use serde_json::Value;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;

/// Duplicate a supervisor descriptor while sealing the original against later exec.
pub fn inherited(index: i32) -> Result<File> {
    // The original bootstrap fd survives duplication. Seal it as well so a verifier
    // started later can't inherit host configuration, the health latch or a lock.
    // SAFETY: F_SETFD rejects an invalid index and sets only descriptor flags.
    if unsafe { libc::fcntl(index, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(Error::unavailable("trusted host descriptor missing"));
    }
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

/// Decode the authenticated configuration and verify the retained profile digest.
pub fn host_config() -> Result<Host> {
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
    serde_json::from_value(value).map_err(|_| Error::invalid("invalid host configuration shape"))
}

/// Read and canonicalize one bounded proposal from the caller.
pub fn caller_proposal() -> Result<Value> {
    let maxed = || Error::new("capacity", "proposal exceeds its enrolled byte limit");
    let raw = read_bounded(io::stdin().lock(), PROPOSAL_LIMIT, maxed())?;
    let proposal = json_document(&raw, false)
        .map_err(|_| Error::new("invalid_request", "invalid proposal JSON"))?;
    if canonical(&proposal)?.len() > PROPOSAL_LIMIT {
        return Err(maxed());
    }
    Ok(proposal)
}
