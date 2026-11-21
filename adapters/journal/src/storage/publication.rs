//! Same-directory stage, checked close, ownership recheck, rename and directory sync.
use super::linux::{Volume, checked_close};
use super::{ANCHOR, JOURNAL, SlotOperation, Snapshot};
use crate::{Error, Result};
use rustix::fs::{self, Mode, OFlags, RenameFlags};
use std::fs::File;
use std::io::Write;

#[derive(Clone, Copy)]
pub(super) enum Stage {
    Reserve,
    Journal,
    Activate,
}

impl Stage {
    fn names(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Reserve => ("reserve", ".gaap-stage-reserve", ANCHOR),
            Self::Journal => ("journal", ".gaap-stage-journal", JOURNAL),
            Self::Activate => ("activate", ".gaap-stage-activate", ANCHOR),
        }
    }
}

#[cfg(feature = "qualification")]
fn selected(stage: &str, operation: &str) -> Option<String> {
    (std::env::var("GAAP_JOURNAL_POINT").ok()?.as_str() == format!("{stage}.{operation}"))
        .then(|| std::env::var("GAAP_JOURNAL_ACTION").unwrap_or_default())
}

// Qualification can stop at the actual syscall error return, before recording quarantine.
#[cfg(feature = "qualification")]
pub(super) fn stop_after_eio() -> Result<()> {
    if selected("volume", "after_eio").as_deref() == Some("stop") {
        eprintln!("journal qualification stop volume.after_eio");
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::STOP)
            .map_err(|_| Error::unavailable("qualification_stop"))?;
    }
    Ok(())
}

pub(super) fn point(_volume: &Volume<'_>, _stage: &str, _operation: &str) -> Result<()> {
    #[cfg(feature = "qualification")]
    if let Some(action) = selected(_stage, _operation) {
        let failure = match action.as_str() {
            "stop" => {
                eprintln!("journal qualification stop {_stage}.{_operation}");
                return _volume.io(
                    rustix::process::kill_process(
                        rustix::process::getpid(),
                        rustix::process::Signal::STOP,
                    ),
                    false,
                    "qualification_stop",
                );
            }
            "eio" => rustix::io::Errno::IO,
            "enospc" => rustix::io::Errno::NOSPC,
            "short" => return Ok(()),
            _ => rustix::io::Errno::INVAL,
        };
        return _volume.io(Err(failure), true, "qualification_fault");
    }
    Ok(())
}

impl SlotOperation<'_> {
    pub(super) fn publish(
        &self,
        stage: Stage,
        bytes: &[u8],
        anchor: &Snapshot,
        journal: Option<&Snapshot>,
    ) -> Result<()> {
        let v = &self.volume;
        let (step, staging, target) = stage.names();
        let dir = if matches!(stage, Stage::Journal) {
            &self.handles.store
        } else {
            &self.handles.control
        };
        self.recheck(anchor, journal)?;
        point(v, step, "create")?;
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = v
            .mutation("create_stage", || {
                match fs::openat(dir, staging, flags, Mode::RUSR | Mode::WUSR) {
                    Ok(fd) => Ok(Some(fd)),
                    Err(rustix::io::Errno::EXIST) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })?
            .ok_or_else(|| Error::invalid("stage_collision"))?;
        let mut file = File::from(fd);
        let owned = v.io(file.metadata(), false, "stage_metadata")?;
        // Pin the staging inode through checked close, rename and conditional cleanup.
        let staged = v
            .existing(dir, staging, self.host.slot.control.uid)?
            .ok_or_else(|| Error::invalid("stage_disappeared"))?;
        v.same_entry(dir, staging, &owned)?;
        let prepared = (|| {
            use std::os::unix::fs::MetadataExt;
            if owned.uid() != self.host.slot.control.uid
                || owned.mode() & 0o7777 != 0o600
                || owned.nlink() != 1
            {
                return Err(Error::invalid("stage_mode_or_owner"));
            }
            point(v, step, "write")?;
            #[cfg(feature = "qualification")]
            if selected(step, "write").as_deref() == Some("short") {
                v.mutation("short_write", || file.write_all(&bytes[..bytes.len() / 2]))?;
                return Err(Error::unavailable("incomplete_stage_write"));
            }
            v.mutation("write_stage", || file.write_all(bytes))?;
            point(v, step, "flush")?;
            v.mutation("flush_stage", || file.flush())?;
            point(v, step, "file_sync")?;
            v.mutation("sync_stage", || file.sync_all())
        })();
        // The host must withdraw after a health authority failure. Dropping the descriptor
        // here releases it without attempting a fresh durable maintenance operation.
        if prepared.as_ref().is_err_and(health_hold) {
            return prepared;
        }
        // Linux consumes the descriptor on close, including when an earlier write failed.
        let closed = v.mutation("close_stage", || {
            checked_close(file.into()).map_err(Into::into)
        });
        let mut renamed = false;
        let publication = (|| {
            closed?;
            prepared?;
            point(v, step, "close")?;
            point(v, step, "before_rename")?;
            self.recheck(anchor, journal)?;
            match stage {
                Stage::Reserve => self.entries(&[])?,
                Stage::Journal => self.entries(&[staging])?,
                Stage::Activate => self.entries(&[JOURNAL])?,
            }
            {
                use std::io::Read;
                let mut persisted = Vec::with_capacity(bytes.len());
                v.io(
                    (&staged)
                        .take(bytes.len() as u64 + 1)
                        .read_to_end(&mut persisted),
                    false,
                    "reread_stage",
                )?;
                if persisted != bytes {
                    return Err(Error::invalid("staged_bytes_changed"));
                }
            }
            v.same_entry(dir, staging, &owned)?;
            // No other entry can be adopted on the initial publication, even after an unexpected creation.
            let flags = if matches!(stage, Stage::Journal) {
                RenameFlags::NOREPLACE
            } else {
                RenameFlags::empty()
            };
            let published = v.mutation("publish_stage", || {
                match fs::renameat_with(dir, staging, dir, target, flags) {
                    Ok(()) => Ok(true),
                    Err(rustix::io::Errno::EXIST) => Ok(false),
                    Err(error) => Err(error.into()),
                }
            })?;
            if !published {
                return Err(Error::invalid("journal_target_exists"));
            }
            renamed = true;
            point(v, step, "after_rename")?;
            point(v, step, "before_dir_sync")?;
            v.mutation("sync_publication_directory", || dir.sync_all())?;
            point(v, step, "after_dir_sync")
        })();
        match publication {
            Err(primary) if !renamed && !health_hold(&primary) => {
                let cleanup = (|| {
                    self.handles.recheck(self.host, v)?;
                    v.same_entry(dir, staging, &owned)?;
                    #[cfg(feature = "qualification")]
                    if std::env::var_os("GAAP_JOURNAL_CLEANUP_FAIL").is_some() {
                        return Err(Error::unavailable("remove_owned_stage"));
                    }
                    v.mutation("remove_owned_stage", || {
                        fs::unlinkat(dir, staging, fs::AtFlags::empty()).map_err(Into::into)
                    })
                })();
                match cleanup {
                    Ok(()) => Err(primary),
                    Err(secondary) if health_hold(&secondary) => {
                        Err(secondary.with_warning(primary.reason))
                    }
                    Err(secondary) if secondary.reason == "remove_owned_stage" => {
                        Err(primary.with_warning("owned_stage_cleanup_failed"))
                    }
                    Err(_) => Err(primary.with_warning("stage_ownership_unconfirmed")),
                }
            }
            other => other,
        }
    }
}

fn health_hold(error: &Error) -> bool {
    matches!(
        error.reason,
        "volume_quarantined"
            | "volume_operation_unconfirmed"
            | "health_authority_failed_withdraw_volume"
    )
}
