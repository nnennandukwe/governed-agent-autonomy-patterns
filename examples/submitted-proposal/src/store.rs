use rustix::fs::{Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, String>;
pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
}
pub(crate) fn identity(path: &Path, directory: bool) -> Result<Identity> {
    let m = fs::symlink_metadata(path).map_err(err)?;
    if m.file_type().is_symlink()
        || if directory {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1
        }
    {
        return Err("symlinks, hardlinks and unexpected file types are not allowed".into());
    }
    Ok(Identity {
        device: m.dev(),
        inode: m.ino(),
        mode: m.mode() & 0o777,
    })
}
pub(crate) fn directory(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        identity(&current, true)?;
    }
    Ok(())
}
pub(crate) fn trusted_parent(path: &Path) -> Result<()> {
    directory(path)?;
    let mut current = PathBuf::new();
    let owner = rustix::process::geteuid().as_raw();
    for part in path.components() {
        current.push(part);
        let metadata = fs::symlink_metadata(&current).map_err(err)?;
        if (metadata.uid() != owner && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err("session parents must be owned by you or root, and shared writable parents must be sticky; use a private directory or /tmp".into());
        }
    }
    Ok(())
}
pub(crate) fn open_regular(path: &Path, write: bool) -> Result<File> {
    identity(path, false)?;
    let flags = if write { OFlags::RDWR } else { OFlags::RDONLY };
    let fd = rustix::fs::open(
        path,
        flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(err)?;
    let file = File::from(fd);
    let m = file.metadata().map_err(err)?;
    if !m.is_file()
        || m.nlink() != 1
        || identity(path, false)?
            != (Identity {
                device: m.dev(),
                inode: m.ino(),
                mode: m.mode() & 0o777,
            })
    {
        return Err("file identity changed while opening".into());
    }
    Ok(file)
}
pub(crate) fn read(path: &Path, limit: usize) -> Result<(String, Identity)> {
    let f = open_regular(path, false)?;
    let m = f.metadata().map_err(err)?;
    let mut content = String::new();
    f.take((limit + 1) as u64)
        .read_to_string(&mut content)
        .map_err(err)?;
    if content.len() > limit {
        return Err("file exceeds size limit".into());
    }
    Ok((
        content,
        Identity {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode() & 0o777,
        },
    ))
}
