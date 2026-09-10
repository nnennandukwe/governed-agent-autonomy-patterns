#!/usr/bin/env python3
"""Install and verify the pinned conference goose binary without changing PATH."""
import hashlib
import io
import os
from pathlib import Path
import platform
import stat
import tarfile
import tempfile
import urllib.request

VERSION = "1.50.0"
ARCHIVE = "goose-aarch64-apple-darwin.tar.bz2"
SHA256 = "2ac186f03e547057a7d8aaa28c2ec34bef0c3718ead98bbbda50945273f93147"
# Derived from the executable in the official archive verified by SHA256 above.
BINARY_SHA256 = "a01ab640ab8104d115039054a3ef1332da9040dabe8b9d25d36847f1800fd0f1"
URL = f"https://github.com/aaif-goose/goose/releases/download/v{VERSION}/{ARCHIVE}"


def verify_binary(path):
    """Verify executable bytes before running it or giving it provider credentials."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as handle:
        before = os.fstat(handle.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
            raise SystemExit("Goose must be a regular executable with one filesystem link.")
        checksum = hashlib.sha256()
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            checksum.update(chunk)
        after = os.fstat(handle.fileno())
    current = os.stat(path, follow_symlinks=False)
    if (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_size) != (after.st_dev, after.st_ino, after.st_mtime_ns, after.st_size) or (after.st_dev, after.st_ino) != (current.st_dev, current.st_ino):
        raise SystemExit("Goose changed during checksum verification; refusing to execute it.")
    if checksum.hexdigest() != BINARY_SHA256:
        raise SystemExit("Goose executable checksum mismatch. Nothing executed; inspect or quarantine that file before reinstalling.")


def publish_binary(directory, content):
    """Publish a fully synced executable without overwriting an existing destination."""
    destination = directory / "goose"
    with tempfile.NamedTemporaryFile(prefix=".goose-install-", dir=directory) as temporary:
        os.fchmod(temporary.fileno(), 0o700)
        temporary.write(content)
        temporary.flush()
        os.fsync(temporary.fileno())
        # link is atomic and refuses an existing destination; only complete bytes publish.
        os.link(temporary.name, destination)
    try:
        fd = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    except OSError as error:
        return f"Executable installed, but directory durability is uncertain: {error}"
    return None


def main():
    """Install the verified Apple Silicon release, preserving any existing destination."""
    if (platform.system(), platform.machine()) != ("Darwin", "arm64"):
        raise SystemExit("This conference installer supports Apple Silicon macOS. See README for other hosts.")
    directory = Path.home() / ".local" / "share" / "gaap-demo" / "tools" / f"goose-{VERSION}"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    if directory.is_symlink():
        raise SystemExit("Refusing a symlinked install directory.")
    destination = directory / "goose"
    if destination.exists() or destination.is_symlink():
        verify_binary(destination)
        print(f"Verified goose {VERSION} already installed: {destination}")
        return
    print(f"Downloading official goose v{VERSION}; verifying published archive SHA-256.", flush=True)
    with urllib.request.urlopen(URL, timeout=60) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise SystemExit("Archive checksum mismatch; nothing installed.")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:bz2") as archive:
        members = [member for member in archive.getmembers() if member.name in ("./goose", "goose")]
        if len(members) != 1 or not members[0].isfile():
            raise SystemExit("Archive has no unique regular goose binary; nothing installed.")
        content = archive.extractfile(members[0]).read()
    if hashlib.sha256(content).hexdigest() != BINARY_SHA256:
        raise SystemExit("Extracted executable checksum mismatch; nothing installed.")
    warning = publish_binary(directory, content)
    if warning:
        print(warning)
    verify_binary(destination)
    print(f"Installed and verified goose {VERSION}: {destination}")


if __name__ == "__main__":
    main()
