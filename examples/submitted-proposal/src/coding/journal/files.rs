//! File operations relative to the enrolled directory held by the journal transaction.
use super::*;
use rustix::fs::{self as unix, AtFlags, Mode, OFlags, ResolveFlags};
use std::fs::Metadata;
use std::io::{self, Read};
use std::os::fd::IntoRawFd;
use std::os::unix::fs::MetadataExt;

fn open(dir: &File, name: &str, flags: OFlags) -> io::Result<File> {
    Ok(unix::openat2(
        dir,
        name,
        flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        if flags.contains(OFlags::CREATE) {
            Mode::RUSR | Mode::WUSR
        } else {
            Mode::empty()
        },
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?
    .into())
}

fn check_entry(tx: &Submission<'_>, name: &str, pin: &File) -> JResult<Metadata> {
    let dir = tx.workspace_dir();
    let md = tx.target_io("target_metadata", || pin.metadata())?;
    let entry = tx.target_io("target_entry", || {
        Ok(unix::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?)
    })?;
    if !md.is_file()
        || md.nlink() != 1
        || md.uid() != tx.host().slot.workspace.uid
        || md.dev().to_string() != tx.host().slot.workspace.dev
        || md.mode() & 0o7000 != 0
        || entry.st_dev != md.dev()
        || entry.st_ino != md.ino()
        || entry.st_mode != md.mode()
    {
        return Err(Error::invalid("target_entry_identity"));
    }
    Ok(md)
}

fn read_file(tx: &Submission<'_>, name: &str) -> JResult<SnapshotFile> {
    let file = tx.target_io("open_target", || {
        open(tx.workspace_dir(), name, OFlags::RDONLY)
    })?;
    let md = check_entry(tx, name, &file)?;
    let mut content = String::new();
    tx.target_io("read_target", || {
        (&file).take(32769).read_to_string(&mut content)
    })?;
    if content.len() > 32768 {
        return Err(Error::invalid("target_size"));
    }
    check_entry(tx, name, &file)?;
    Ok(SnapshotFile {
        content,
        identity: Identity {
            device: md.dev(),
            inode: md.ino(),
            mode: md.mode() & 0o777,
        },
    })
}

pub(super) fn snapshot(tx: &Submission<'_>) -> JResult<Snapshot> {
    ["TASK.md", "shipping.py", "deployment.json"]
        .into_iter()
        .map(|name| Ok((name.to_owned(), read_file(tx, name)?)))
        .collect()
}

pub(super) struct Stage {
    name: String,
    pin: File,
}

fn checked_close(tx: &Submission<'_>, file: File) -> JResult<()> {
    tx.target_writeback("target_stage_close", || {
        // SAFETY: into_raw_fd transfers the sole ownership of this handle to try_close.
        unsafe { rustix::io::try_close(file.into_raw_fd()).map_err(Into::into) }
    })
}

impl Stage {
    pub(super) fn prepare(
        tx: &Submission<'_>,
        attempt: &str,
        content: &str,
        witness: &mut Witness,
    ) -> JResult<Self> {
        let name = format!(".gaap-stage-target-{attempt}");
        let writer = tx.target_mutation("target_create", || {
            open(
                tx.workspace_dir(),
                &name,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL,
            )
        })?;
        let mut stage = Self { name, pin: writer };
        let mut writable = true;
        let prepared = (|| {
            check_entry(tx, &stage.name, &stage.pin)?;
            tx.target_mutation("target_mode", || {
                stage.pin.set_permissions(fs::Permissions::from_mode(0o600))
            })?;
            tx.target_mutation("target_write", || {
                (&stage.pin).write_all(content.as_bytes())
            })?;
            tx.target_mutation("target_flush", || (&stage.pin).flush())?;
            tx.target_writeback("target_stage_sync", || stage.pin.sync_all())?;
            let reader = tx.target_io("open_synced_stage", || {
                open(tx.workspace_dir(), &stage.name, OFlags::RDONLY)
            })?;
            let original = check_entry(tx, &stage.name, &stage.pin)?;
            let actual = check_entry(tx, &stage.name, &reader)?;
            if original.dev() != actual.dev() || original.ino() != actual.ino() {
                return Err(Error::invalid("target_stage_changed"));
            }
            let writer = std::mem::replace(&mut stage.pin, reader);
            writable = false;
            checked_close(tx, writer)?;
            witness.bind_stage(tx, &stage.name)
        })();
        if let Err(primary) = prepared {
            if primary.health_hold() {
                return Err(primary);
            }
            let failure = match stage.remove(tx) {
                Ok(()) => primary,
                Err(error) if error.health_hold() => error.with_warning(primary.reason),
                Err(_) => primary.with_warning("owned_stage_cleanup_failed"),
            };
            if writable && !failure.health_hold() {
                checked_close(tx, stage.pin).map_err(|error| error.with_warning(failure.reason))?;
            }
            return Err(failure);
        }
        Ok(stage)
    }

    pub(super) fn remove(&self, tx: &Submission<'_>) -> JResult<()> {
        check_entry(tx, &self.name, &self.pin)?;
        tx.target_mutation("target_stage_cleanup", || {
            Ok(unix::unlinkat(
                tx.workspace_dir(),
                &*self.name,
                AtFlags::empty(),
            )?)
        })?;
        tx.target_writeback("target_cleanup_dir_sync", || tx.workspace_dir().sync_all())
    }

    pub(super) fn recheck(&self, tx: &Submission<'_>, expected: &str) -> JResult<()> {
        let md = check_entry(tx, &self.name, &self.pin)?;
        let current = read_file(tx, &self.name)?;
        if md.mode() & 0o7777 != 0o600
            || current.identity.inode != md.ino()
            || current.content != expected
        {
            return Err(Error::invalid("target_stage_changed"));
        }
        Ok(())
    }

    pub(super) fn rename(&self, dir: &File) -> io::Result<()> {
        Ok(unix::renameat(dir, &*self.name, dir, "shipping.py")?)
    }
}
