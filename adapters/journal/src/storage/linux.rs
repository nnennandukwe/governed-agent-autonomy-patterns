//! Linux descriptor-relative checks for one host-provisioned ext4 slot.
use crate::{Error, Host, Identity, Result};
use rustix::fs::{self, AtFlags, FlockOperation, Mode, OFlags};
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const READ: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK);
const DIRECTORY: OFlags = READ.union(OFlags::DIRECTORY);

pub(super) struct Volume<'a> {
    pub health: &'a File,
    pub slot: &'a crate::Slot,
}

impl Volume<'_> {
    /// Translate errors at the syscall that observed them, before releasing the slot lock.
    pub fn io<T, E: Into<io::Error>>(
        &self,
        operation: std::result::Result<T, E>,
        writing: bool,
        reason: &'static str,
    ) -> Result<T> {
        operation.map_err(|error| {
            let error = error.into();
            if error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) {
                return self.quarantine();
            }
            if writing && capacity_error(&error) {
                return Error::new("capacity", reason);
            }
            Error::unavailable(reason)
        })
    }

    fn quarantine(&self) -> Error {
        match self.write_health(b'1') {
            Ok(()) => Error::unavailable("volume_quarantined"),
            Err(failure) => failure,
        }
    }

    fn read_health(&self) -> Result<u8> {
        let mut state = [0; 2];
        match self.health.read_at(&mut state, 0) {
            Ok(1) if matches!(state[0], b'0' | b'P' | b'1') => Ok(state[0]),
            _ => Err(Error::unavailable(
                "health_authority_failed_withdraw_volume",
            )),
        }
    }

    pub fn healthy(&self) -> Result<()> {
        match self.read_health()? {
            b'0' => Ok(()),
            b'P' => Err(Error::unavailable("volume_operation_unconfirmed")),
            _ => Err(Error::unavailable("volume_quarantined")),
        }
    }

    fn write_health(&self, value: u8) -> Result<()> {
        self.check_health_binding()?;
        let failed = || Error::unavailable("health_authority_failed_withdraw_volume");
        if self.health.write_at(&[value], 0).map_err(|_| failed())? != 1 {
            return Err(failed());
        }
        self.health.sync_all().map_err(|_| failed())?;
        self.check_health_binding()?;
        if self.read_health()? != value {
            return Err(failed());
        }
        Ok(())
    }

    /// Use an independent open description. Inherited descriptors can share a supervisor's flock.
    pub fn lock_health(&self, writer: bool) -> Result<File> {
        self.check_health_binding()?;
        let descriptor = File::open(format!("/proc/self/fd/{}", self.health.as_raw_fd()))
            .map_err(|_| Error::unavailable("health_authority_failed_withdraw_volume"))?;
        let stat = descriptor
            .metadata()
            .map_err(|_| Error::unavailable("health_authority_failed_withdraw_volume"))?;
        if !identity_matches(&stat, &self.slot.health) {
            return Err(Error::unavailable(
                "health_authority_failed_withdraw_volume",
            ));
        }
        match bounded_flock(&descriptor, writer) {
            Ok(()) => (),
            Err(rustix::io::Errno::AGAIN) => return Err(Error::new("busy", "health_lock_busy")),
            Err(_) => {
                return Err(Error::unavailable(
                    "health_authority_failed_withdraw_volume",
                ));
            }
        }
        self.health_handoff()?;
        Ok(descriptor)
    }

    pub fn mutation<T>(
        &self,
        reason: &'static str,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> Result<T> {
        self.guarded_mutation(reason, false, operation)
    }

    /// A sync or checked close can report delayed loss of a previous write.
    pub fn writeback<T>(
        &self,
        reason: &'static str,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> Result<T> {
        self.guarded_mutation(reason, true, operation)
    }

    /// The exclusive health lock serializes the pending marker with both readers and writers.
    /// A new process can't infer what happened between the syscall and confirmation.
    fn guarded_mutation<T>(
        &self,
        reason: &'static str,
        writeback: bool,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> Result<T> {
        self.healthy()?;
        self.write_health(b'P')?;
        let result = operation();
        if let Err(error) = &result {
            let io_failure = error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error());
            if io_failure || (writeback && capacity_error(error)) {
                #[cfg(feature = "qualification")]
                if io_failure {
                    super::publication::stop_after_eio()?;
                }
                return Err(self.quarantine());
            }
        }
        // A different health state is never cleared. All cooperating operators must take the same lock.
        if self.read_health()? != b'P' {
            return Err(Error::unavailable(
                "health_authority_failed_withdraw_volume",
            ));
        }
        self.write_health(b'0')?;
        self.io(result, true, reason)
    }

    /// Walk every component from the actual root descriptor. No symlink is followed.
    pub fn directory(&self, name: &Path) -> Result<File> {
        match open_directory(name) {
            Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP) => {
                Err(Error::invalid("unsafe_directory_path"))
            }
            result => self.io(result, false, "open_directory"),
        }
    }

    pub fn check_pins(&self, file: &File, pin: &Identity, dir: bool) -> Result<()> {
        let stat = self.io(file.metadata(), false, "identity_metadata")?;
        let expected_mode = if dir { 0o700 } else { 0o600 };
        if !identity_matches(&stat, pin)
            || pin.mode != expected_mode
            || (dir && !stat.is_dir())
            || (!dir && (!stat.is_file() || stat.nlink() != 1))
        {
            return Err(Error::invalid("provisioned_identity_mismatch"));
        }
        Ok(())
    }

    pub fn existing(&self, dir: &File, name: &str, uid: u32) -> Result<Option<File>> {
        let fd = match fs::openat2(
            dir,
            name,
            READ,
            Mode::empty(),
            fs::ResolveFlags::NO_XDEV | fs::ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(rustix::io::Errno::LOOP) => return Err(Error::invalid("symlink_entry")),
            Err(rustix::io::Errno::XDEV | rustix::io::Errno::NOSYS) => {
                return Err(Error::unsupported("unqualified_entry_mount"));
            }
            error => self.io(error, false, "open_existing_entry")?,
        };
        let file = File::from(fd);
        let stat = self.io(file.metadata(), false, "entry_metadata")?;
        if !stat.is_file()
            || stat.nlink() != 1
            || stat.mode() & 0o7777 != 0o600
            || stat.uid() != uid
        {
            return Err(Error::invalid("unsafe_state_file"));
        }
        self.same_entry(dir, name, &stat)?;
        Ok(Some(file))
    }

    pub fn same_entry(&self, dir: &File, name: &str, held: &Metadata) -> Result<()> {
        let entry = self.io(
            fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW),
            false,
            "recheck_entry",
        )?;
        if entry.st_dev != held.dev()
            || entry.st_ino != held.ino()
            || entry.st_uid != held.uid()
            || entry.st_mode != held.mode()
            || entry.st_nlink != 1
        {
            return Err(Error::invalid("entry_replaced"));
        }
        Ok(())
    }

    pub fn check_health_binding(&self) -> Result<()> {
        let pin = &self.slot.health;
        let failed = || Error::unavailable("health_authority_failed_withdraw_volume");
        let stat = self.health.metadata().map_err(|_| failed())?;
        let flags = fs::fcntl_getfl(self.health).map_err(|_| failed())?;
        if !identity_matches(&stat, pin)
            || !stat.is_file()
            || stat.nlink() != 1
            || stat.len() != 1
            || pin.mode != 0o600
            || pin.dev == self.slot.control.dev
            || pin.uid != self.slot.control.uid
            || flags.intersects(OFlags::APPEND)
            || flags & OFlags::RWMODE != OFlags::RDWR
        {
            return Err(failed());
        }
        let path = Path::new(&pin.path);
        if !path.is_absolute()
            || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
        {
            return Err(failed());
        }
        let parent = open_directory(path.parent().ok_or_else(failed)?).map_err(|_| failed())?;
        let name = path.file_name().ok_or_else(failed)?;
        let entry = fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| failed())?;
        if entry.st_dev != stat.dev()
            || entry.st_ino != stat.ino()
            || entry.st_nlink != 1
            || entry.st_mode != stat.mode()
            || entry.st_uid != stat.uid()
        {
            return Err(failed());
        }
        Ok(())
    }

    pub fn health_handoff(&self) -> Result<()> {
        self.check_health_binding()?;
        self.healthy()
    }
}

fn capacity_error(error: &io::Error) -> bool {
    use rustix::io::Errno;
    error.raw_os_error().is_some_and(|code| {
        code == Errno::NOSPC.raw_os_error() || code == Errno::DQUOT.raw_os_error()
    })
}

fn open_directory(name: &Path) -> rustix::io::Result<File> {
    if !name.is_absolute() || name.components().collect::<PathBuf>().as_os_str() != name.as_os_str()
    {
        return Err(rustix::io::Errno::INVAL);
    }
    let mut fd = fs::open("/", DIRECTORY, Mode::empty())?;
    for part in name.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(child) => fd = fs::openat(&fd, child, DIRECTORY, Mode::empty())?,
            _ => return Err(rustix::io::Errno::INVAL),
        }
    }
    Ok(fd.into())
}

fn identity_matches(stat: &Metadata, pin: &Identity) -> bool {
    stat.dev().to_string() == pin.dev
        && stat.ino().to_string() == pin.ino
        && stat.uid() == pin.uid
        && stat.mode() & 0o7777 == pin.mode
}

pub(super) struct Handles {
    pub workspace: File,
    pub control: File,
    pub store: File,
    pub lock: File,
}

impl Handles {
    pub fn open(host: &Host, volume: &Volume<'_>) -> Result<Self> {
        let slot = &host.slot;
        let pins = [&slot.workspace, &slot.control, &slot.store];
        for (index, pin) in pins.iter().enumerate() {
            if pin.dev != slot.control.dev
                || pin.uid != slot.control.uid
                || rustix::process::geteuid().as_raw() != pin.uid
            {
                return Err(Error::invalid("slot_device_or_owner"));
            }
            for other in &pins[index + 1..] {
                if pin.ino == other.ino
                    || Path::new(&pin.path).starts_with(&other.path)
                    || Path::new(&other.path).starts_with(&pin.path)
                {
                    return Err(Error::invalid("slot_directories_overlap"));
                }
            }
        }
        if Path::new(&slot.control.path).join("lock") != Path::new(&slot.lock.path)
            || slot.lock.uid != slot.control.uid
            || slot.lock.dev != slot.control.dev
        {
            return Err(Error::invalid("lock_pin_outside_control"));
        }
        let workspace = volume.directory(Path::new(&slot.workspace.path))?;
        let control = volume.directory(Path::new(&slot.control.path))?;
        let store = volume.directory(Path::new(&slot.store.path))?;
        let lock = volume
            .existing(&control, "lock", slot.lock.uid)?
            .ok_or_else(|| Error::unavailable("missing_lock"))?;
        let handles = Self {
            workspace,
            control,
            store,
            lock,
        };
        handles.recheck(host, volume)?;
        Ok(handles)
    }

    pub fn recheck(&self, host: &Host, volume: &Volume<'_>) -> Result<()> {
        volume.health_handoff()?;
        let slot = &host.slot;
        for (file, pin) in [
            (&self.workspace, &slot.workspace),
            (&self.control, &slot.control),
            (&self.store, &slot.store),
        ] {
            volume.check_pins(file, pin, true)?;
            let reopened = volume.directory(Path::new(&pin.path))?;
            volume.check_pins(&reopened, pin, true)?;
            for fd in [file, &reopened] {
                if mount_id(fd)? != slot.mount_id {
                    return Err(Error::unsupported("mount_alias_or_replacement"));
                }
            }
        }
        qualify_ext4(
            slot.mount_id,
            volume
                .io(self.control.metadata(), false, "control_metadata")?
                .dev(),
        )?;
        volume.check_pins(&self.lock, &slot.lock, false)?;
        let stat = volume.io(self.lock.metadata(), false, "lock_metadata")?;
        volume.same_entry(&self.control, "lock", &stat)?;
        if mount_id(&self.lock)? != slot.mount_id {
            return Err(Error::unsupported("lock_mount_alias"));
        }
        Ok(())
    }

    pub fn acquire(&self, writer: bool, volume: &Volume<'_>) -> Result<()> {
        match bounded_flock(&self.lock, writer) {
            Err(rustix::io::Errno::AGAIN) => Err(Error::new("busy", "workspace_lock_busy")),
            result => volume.io(result, false, "workspace_lock"),
        }
    }
}

fn bounded_flock(file: &File, writer: bool) -> rustix::io::Result<()> {
    let operation = if writer {
        FlockOperation::NonBlockingLockExclusive
    } else {
        FlockOperation::NonBlockingLockShared
    };
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match fs::flock(file, operation) {
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR)
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
                return Err(rustix::io::Errno::AGAIN);
            }
            result => return result,
        }
    }
}

pub fn mount_id(fd: &File) -> Result<u64> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd()))
        .map_err(|_| Error::unsupported("mount_id_unavailable"))?;
    info.lines()
        .find_map(|line| line.strip_prefix("mnt_id:")?.trim().parse().ok())
        .ok_or_else(|| Error::unsupported("mount_id_unavailable"))
}

fn qualify_ext4(id: u64, dev: u64) -> Result<()> {
    let mounts = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|_| Error::unsupported("mountinfo_unavailable"))?;
    let requested = id.to_string();
    let line = mounts
        .lines()
        .find(|line| line.split_ascii_whitespace().next() == Some(&requested))
        .ok_or_else(|| Error::unsupported("mount_missing"))?;
    let (left, right) = line
        .split_once(" - ")
        .ok_or_else(|| Error::unsupported("mountinfo_invalid"))?;
    let fields: Vec<_> = left.split_ascii_whitespace().collect();
    let source: Vec<_> = right.split_ascii_whitespace().collect();
    if fields.len() < 6
        || source.len() != 3
        || fields[3] != "/"
        || source[0] != "ext4"
        || !fields[5].split(',').any(|v| v == "rw")
    {
        return Err(Error::unsupported("local_writable_ext4_required"));
    }
    if fields[2] != format!("{}:{}", fs::major(dev), fs::minor(dev)) {
        return Err(Error::unsupported("volume_identity_mismatch"));
    }
    // The filesystem driver exposes its effective defaults here even when mountinfo omits them.
    let kernel_device = std::fs::read_link(format!("/sys/dev/block/{}", fields[2]))
        .map_err(|_| Error::unsupported("kernel_block_device_unavailable"))?;
    let name = kernel_device
        .file_name()
        .ok_or_else(|| Error::unsupported("kernel_block_device_unavailable"))?;
    let options = std::fs::read_to_string(Path::new("/proc/fs/ext4").join(name).join("options"))
        .map_err(|_| Error::unsupported("ext4_configuration_unavailable"))?;
    if !options.lines().any(|v| v == "data=ordered")
        || !options.lines().any(|v| v == "barrier" || v == "barrier=1")
        || options
            .lines()
            .any(|v| matches!(v, "nobarrier" | "barrier=0" | "noload"))
    {
        return Err(Error::unsupported(
            "ordered_journaling_and_barriers_required",
        ));
    }
    Ok(())
}

pub fn checked_close(fd: OwnedFd) -> rustix::io::Result<()> {
    use std::os::fd::IntoRawFd;
    // into_raw_fd transfers sole ownership. Linux consumes it even when close reports an error.
    unsafe { rustix::io::try_close(fd.into_raw_fd()) }
}
