use crate::{Error, Host, Result};
use std::fs::File;
use std::mem::{size_of, size_of_val};
use std::os::fd::AsRawFd;

fn unavailable() -> Error {
    Error::unavailable("observer_entry_gate_unavailable")
}

fn option<T: Copy + Default>(file: &File, opt: i32) -> Result<T> {
    let mut value = T::default();
    let mut len = size_of::<T>() as libc::socklen_t;
    // SAFETY: getsockopt receives an initialized, aligned buffer with its actual size.
    let rc = unsafe {
        libc::getsockopt(
            file.as_raw_fd(),
            libc::SOL_SOCKET,
            opt,
            (&mut value as *mut T).cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != size_of::<T>() {
        return Err(unavailable());
    }
    Ok(value)
}

fn endpoint(host: &Host) -> Result<Option<File>> {
    let Some(cookie) = host
        .reconciliation
        .as_ref()
        .and_then(|r| r.entry_socket_cookie.as_deref())
    else {
        return Ok(None);
    };
    let socket = crate::handoff::inherited(5)?;
    if option::<i32>(&socket, libc::SO_DOMAIN)? != libc::AF_UNIX
        || option::<i32>(&socket, libc::SO_TYPE)? != libc::SOCK_SEQPACKET
        || option::<u64>(&socket, libc::SO_COOKIE)?.to_string() != cookie
    {
        return Err(unavailable());
    }
    Ok(Some(socket))
}

/// Mark the original gate CLOEXEC before the shipping process starts its read-only witness.
pub fn seal_gate(host: &Host) -> Result<()> {
    drop(endpoint(host)?);
    Ok(())
}

/// Called after durable dispatch and before the engine may enter the single effect.
pub fn enter(host: &Host, attempt: &str, binding_digest: &str) -> Result<()> {
    let Some(socket) = endpoint(host)? else {
        return Ok(());
    };
    let result = exchange(&socket, attempt, binding_digest);
    // SAFETY: retiring this verified socket also retires duplicate handles to the same endpoint.
    if unsafe { libc::shutdown(socket.as_raw_fd(), libc::SHUT_RDWR) } != 0 && result.is_ok() {
        return Err(unavailable());
    }
    result
}

fn exchange(socket: &File, attempt: &str, binding_digest: &str) -> Result<()> {
    if !crate::hex(attempt, 32) || !crate::is_digest(binding_digest) {
        return Err(unavailable());
    }
    let deadline = libc::timeval {
        tv_sec: 12,
        tv_usec: 0,
    };
    for opt in [libc::SO_SNDTIMEO, libc::SO_RCVTIMEO] {
        // SAFETY: the timeout points to a correctly sized, initialized timeval.
        if unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                opt,
                (&deadline as *const libc::timeval).cast(),
                size_of_val(&deadline) as libc::socklen_t,
            )
        } != 0
        {
            return Err(unavailable());
        }
    }
    let request = format!("enter {attempt} {binding_digest}");
    let expected = format!("permit {attempt} {binding_digest}");
    let fd = socket.as_raw_fd();
    // SAFETY: send and recv access only the initialized request and allocated response buffer.
    let sent = unsafe {
        libc::send(
            fd,
            request.as_ptr().cast(),
            request.len(),
            libc::MSG_NOSIGNAL,
        )
    };
    if sent != request.len() as isize {
        return Err(unavailable());
    }
    let mut response = [0u8; 160];
    // SAFETY: MSG_TRUNC reports the whole packet length even if the buffer is shorter.
    let size = unsafe {
        libc::recv(
            fd,
            response.as_mut_ptr().cast(),
            response.len(),
            libc::MSG_TRUNC,
        )
    };
    if size != expected.len() as isize || response[..expected.len()] != *expected.as_bytes() {
        return Err(unavailable());
    }
    Ok(())
}
