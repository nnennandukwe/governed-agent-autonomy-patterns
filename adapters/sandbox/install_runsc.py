#!/usr/bin/env python3
"""Download a checksum-pinned official gVisor release into a new host directory."""
import argparse
import hashlib
import json
import pathlib
import platform
import subprocess
import tarfile
import urllib.request

RELEASE = 'release-20261005.0'
PINS = {
    'x86_64': 'bd4eb2ca0ee74f0fbbb5f5273ba011e79c56a29fbb047393b88b981cd3230ede',
    'aarch64': 'd0387aa061248756a201f716db25b02bf9376a278543cb96c1855644ab815a8d',
}
FLAGS = ['--platform=systrap', '--network=none', '--directfs=false']


def main():
    """Verify the pinned archive, install runsc locally, and record its identity."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=pathlib.Path, help='New private directory for runsc and its installation record')
    args = parser.parse_args()
    machine = platform.machine()
    if platform.system() != 'Linux' or machine not in PINS:
        parser.error("Use Linux x86_64 or aarch64; macOS execution isn't gVisor evidence")
    destination = args.destination.resolve()
    destination.mkdir(mode=0o700, parents=True, exist_ok=False)
    archive = destination / 'release.tar.bz2'
    url = f'https://github.com/google/gvisor/releases/download/{RELEASE}/gvisor-{machine}.tar.bz2'
    with urllib.request.urlopen(url, timeout=60) as response, archive.open('xb') as output:
        while chunk := response.read(65536):
            output.write(chunk)
    archive_digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if archive_digest != PINS[machine]:
        raise ValueError('Official release archive checksum mismatch; no runtime was installed')
    with tarfile.open(archive, 'r:bz2') as bundle:
        selected = [member for member in bundle.getmembers() if pathlib.PurePosixPath(member.name).name == 'runsc' and member.isfile()]
        if len(selected) != 1 or selected[0].size > 128 * 1024 * 1024:
            raise ValueError('Expected one bounded runsc binary in the verified archive')
        with bundle.extractfile(selected[0]) as source, (destination / 'runsc').open('xb') as output:
            output.write(source.read())
    executable = destination / 'runsc'
    executable.chmod(0o755)
    version = subprocess.check_output([str(executable), '--version'], text=True, timeout=10).strip()
    if not version.startswith(f'runsc version {RELEASE}'):
        raise ValueError('Installed runsc version differs from the pinned release')
    record = {'release': RELEASE, 'url': url, 'archive_sha256': archive_digest,
              'executable_digest': 'sha256:' + hashlib.sha256(executable.read_bytes()).hexdigest(),
              'runtime_version': version, 'flags': FLAGS}
    (destination / 'installation.json').write_text(json.dumps(record, indent=2) + '\n')
    archive.unlink()
    print(json.dumps(record))


if __name__ == '__main__':
    main()
