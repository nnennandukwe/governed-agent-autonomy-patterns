#!/usr/bin/env python3
"""Temporary issue 17 Linux resource diagnostic; retain every Docker observation."""
import json
import os
import pathlib
import subprocess
import tempfile
import uuid


def capture(arguments):
    """Retain the exact command outcome without promoting a failed probe to success."""
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=45)
    return {'command': arguments, 'code': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}


def main():
    """Vary one resource constraint at a time with fixed isolation and mount settings."""
    destination = pathlib.Path('/tmp/gaap-runsc-probes')
    destination.mkdir(exist_ok=False)
    with tempfile.TemporaryDirectory(prefix='gaap-runsc-probe-') as root:
        workspace = pathlib.Path(root) / 'workspace'
        workspace.mkdir()
        os.chmod(root, 0o755)
        (workspace / 'source.txt').write_text('declared input')
        for case, memory, pids, fsize, directory in [
            ('baseline', 64, 32, 65536, '/workspace'),
            ('pids', 64, 256, 65536, '/workspace'),
            ('memory', 256, 32, 65536, '/workspace'),
            ('both', 256, 256, 65536, '/workspace'),
            ('fsize', 64, 32, 16777216, '/workspace'),
            ('directory', 256, 256, 65536, '/'),
        ]:
            name = 'gaap-probe-' + uuid.uuid4().hex
            command = ['docker', 'create', '--name', name, '--runtime', 'runsc', '--network', 'none',
                       '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
                       '--user', f'{os.getuid()}:{os.getgid()}', '--workdir', directory, '--ipc', 'private',
                       '--memory', f'{memory}m', '--memory-swap', f'{memory}m', '--pids-limit', str(pids),
                       '--cpus', '1', '--ulimit', 'core=0:0', '--ulimit', f'fsize={fsize}:{fsize}',
                       '--mount', f'type=bind,src={workspace},dst=/workspace,bind-recursive=disabled,bind-propagation=rprivate',
                       '--entrypoint', '/usr/bin/env', 'python@sha256:2d9aefe2fef018a7eb2c13064c89c71929800fd2e5dccdbf52ea5da5bb8d929a',
                       '-i', '/usr/local/bin/python3', '-I', '-S', '-B', '-c',
                       "import os,pathlib;assert os.getuid()!=0;print(pathlib.Path('/workspace/source.txt').read_text())"]
            evidence = {'case': case, 'create': capture(command)}
            try:
                if evidence['create']['code'] == 0:
                    evidence['start'] = capture(['docker', 'start', '--attach', name])
                    evidence['inspect'] = capture(['docker', 'inspect', name])
            finally:
                evidence['cleanup'] = capture(['docker', 'rm', '--force', '--volumes', name])
                (destination / (case + '.json')).write_text(json.dumps(evidence, indent=2))
                print('[DEBUG-issue17] ' + json.dumps({'case': case, 'start': evidence.get('start')}), flush=True)


if __name__ == '__main__':
    main()
