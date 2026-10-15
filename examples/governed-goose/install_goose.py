"""Install the pinned Goose 1.54.0 binary into a new local directory."""
import argparse
import hashlib
import pathlib
import platform
import shutil
import tarfile
import tempfile
import urllib.request

PINS = {
    ("Darwin", "arm64"): ("goose-aarch64-apple-darwin.tar.bz2", "63738f0eed2fe9d8585a4ba988c3ffb23d600db0b43858a6cd868fa6b035a7af", "4b8258c24adad9e07957d1dddfc7d965b97883dce2eff64af7e3d3c005655d81"),
    ("Linux", "x86_64"): ("goose-x86_64-unknown-linux-gnu.tar.bz2", "bf01cb6359f7a2b1acb54ca4827e1d059b18aabb3d2b75202e21066cfd3a3396", "2419b124b0b1eb4e02b19cbc2373a91e8927010c8ed2902680c7a2864a49785f"),
}


def digest(path):
    """Stream a finite downloaded artifact through SHA-256."""
    hashed = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            hashed.update(block)
    return hashed.hexdigest()


def install(destination):
    """Verify archive and binary pins, refusing existing destination content."""
    pin = PINS.get((platform.system(), platform.machine()))
    if pin is None:
        raise ValueError("Supported platforms: macOS arm64 and Linux x86_64")
    filename, archive_hash, binary_hash = pin
    destination = destination.resolve()
    if destination.exists():
        raise FileExistsError("Choose a new install directory; existing files won't be replaced")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="goose-install-", dir=str(destination.parent)) as temporary:
        staging = pathlib.Path(temporary)
        archive = staging / filename
        url = "https://github.com/block/goose/releases/download/v1.54.0/" + filename
        with urllib.request.urlopen(url, timeout=30) as source, archive.open("wb") as output:
            total = 0
            while True:
                block = source.read(1024 * 1024)
                if not block:
                    break
                total += len(block)
                if total > 512 * 1024 * 1024:
                    raise ValueError("Archive exceeds the download limit")
                output.write(block)
        if digest(archive) != archive_hash:
            raise ValueError("Archive checksum mismatch")
        binary = staging / "goose"
        with tarfile.open(archive, "r:bz2") as package:
            members = [member for member in package.getmembers() if member.name in ("goose", "./goose")]
            if len(members) != 1 or not members[0].isfile() or members[0].size > 512 * 1024 * 1024:
                raise ValueError("Unexpected binary archive entry")
            source = package.extractfile(members[0])
            with source, binary.open("xb") as output:
                shutil.copyfileobj(source, output, 1024 * 1024)
        if digest(binary) != binary_hash:
            raise ValueError("Binary checksum mismatch")
        binary.chmod(0o755)
        destination.mkdir()
        binary.rename(destination / "goose")
    return destination / "goose"


def main():
    """Parse one local destination and print the verified executable path."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=pathlib.Path, help="New local tool directory; nothing existing is replaced")
    args = parser.parse_args()
    print(install(args.destination))


if __name__ == "__main__":
    main()
