#!/usr/bin/env python3
"""Launch Goose plus the local inspector, with visible, persistent demo sessions."""
import argparse
from contextlib import contextmanager
from datetime import datetime
import fcntl
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import stat
import subprocess
import sys
import threading
import time
from urllib.parse import urlsplit
import webbrowser
import demo as legacy

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
ASSETS = HERE / 'inspector'
SERVICE = 'gaap-coding-inspector-v1'
GOAL = ('Implement the shipping pricing rules described in this project. '
        'Once the implementation passes verification, enable shipping in deployment.json while preserving its other settings. '
        'Tell me what changed, what verification showed, and whether shipping is enabled.')


def build():
    """Build the locked Rust example in this normal checkout."""
    subprocess.run(['cargo', 'build', '--locked', '--manifest-path', str(HERE / 'Cargo.toml')], check=True)


def fresh_session():
    """Create a new visible coding session without resetting previous runs."""
    parent = session_parent()
    root = parent / ('coding-' + datetime.now().strftime('%Y%m%d-%H%M%S') + '-' + secrets.token_hex(2))
    legacy.binary('coding-init', root, capture=True)
    return root


def inspect(root):
    """Read and integrity-check a coding session through the Rust consumer."""
    return legacy.binary('coding-inspect', root, capture=True)


def session_parent():
    """Use one real directory for launcher-owned selection and startup locking."""
    parent = REPO.resolve() / 'demo-runs'
    parent.mkdir(mode=0o700, exist_ok=True)
    if not stat.S_ISDIR(parent.lstat().st_mode):
        raise OSError('demo-runs must be a real directory, not a link')
    return parent


@contextmanager
def startup_lock():
    """Serialize session selection, identity probing, binding, and publication."""
    parent = session_parent()
    directory = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    descriptor = None
    try:
        # Separate open from exclusive creation: concurrent first creation with
        # O_CREAT | O_NOFOLLOW can itself return ENOENT on macOS.
        flags = os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK
        for _ in range(4):
            try:
                descriptor = os.open('.start.lock', flags, dir_fd=directory)
                break
            except FileNotFoundError:
                try:
                    descriptor = os.open('.start.lock', flags | os.O_CREAT | os.O_EXCL,
                                         0o600, dir_fd=directory)
                    break
                except FileExistsError:
                    continue
        if descriptor is None:
            raise OSError('Startup lock changed repeatedly; retry after checking demo-runs')
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise OSError('Startup lock must be an unlinked regular file')
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        current = os.stat('.start.lock', dir_fd=directory, follow_symlinks=False)
        if (current.st_dev, current.st_ino, current.st_nlink) != (info.st_dev, info.st_ino, 1):
            raise OSError('Startup lock changed while waiting; retry after checking demo-runs')
        yield parent
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(directory)


def read_metadata(root, name):
    """Read bounded JSON through descriptors, rejecting links and nonfiles."""
    if name not in {'current.json', 'inspector.json', 'rehearsal.json'}:
        raise ValueError('Unknown launcher metadata file')
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        try:
            descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        except FileNotFoundError:
            return None
        try:
            info = os.fstat(descriptor)
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                raise OSError(f'{name} must be an unlinked regular file')
            data = os.read(descriptor, 16385)
        finally:
            os.close(descriptor)
        if len(data) > 16384:
            raise ValueError(f'{name} is too large')
        value = json.loads(data)
        if not isinstance(value, dict):
            raise ValueError(f'{name} must contain a JSON object')
        return value
    finally:
        os.close(directory)


def selected_root(parent, value):
    """Resolve only a single real child of demo-runs, never an arbitrary path."""
    if (not isinstance(value, str) or value in {'', '.', '..'}
            or Path(value).name != value or '/' in value or '\\' in value):
        raise ValueError('current.json must name one immediate child directory of demo-runs')
    root = parent / value
    if not stat.S_ISDIR(root.lstat().st_mode):
        raise OSError('Session root must be a real directory, not a link')
    return root


def adoption_root(parent, root):
    """Validate an explicit root without resolving away a linked final entry."""
    candidate = Path(os.path.abspath(root))
    if candidate.parent.resolve() != parent:
        raise ValueError('--root must be an immediate child directory of this checkout\'s demo-runs')
    return selected_root(parent, candidate.name)


def write_metadata(root, name, value):
    """Replace local metadata without following destination links or aliases."""
    if name not in {'current.json', 'inspector.json', 'rehearsal.json'}:
        raise ValueError('Unknown session metadata file')
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    temporary = '.' + name + '-' + secrets.token_hex(12)

    def validate_destination():
        """Reject linked and non-regular existing metadata entries."""
        try:
            info = os.stat(name, dir_fd=directory, follow_symlinks=False)
        except FileNotFoundError:
            return
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise OSError(f'{name} must be an unlinked regular file')

    try:
        validate_destination()
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o600, dir_fd=directory)
        with os.fdopen(descriptor, 'w') as handle:
            json.dump(value, handle, indent=2)
            handle.flush()
            os.fsync(handle.fileno())
        validate_destination()
        # Rename replaces the entry itself, never a late symlink's target.
        os.replace(temporary, name, src_dir_fd=directory, dst_dir_fd=directory)
        try:
            os.fsync(directory)
        except OSError as error:
            print(f'Warning: {root / name} was published, but directory sync failed: {error}. '
                  'The visible selection is retained; power-loss durability is uncertain.', file=sys.stderr)
    finally:
        try:
            os.unlink(temporary, dir_fd=directory)
        except FileNotFoundError:
            pass
        finally:
            os.close(directory)


def recipe(root):
    """Write a Goose Desktop recipe exposing only the three coding tools."""
    value = {
        'version': '1.0.0', 'title': 'Goose + GAAP: code, verify, govern',
        'description': 'Implement and verify a shipping-price function, then enable shipping in the project configuration.',
        'instructions': 'Complete the requested engineering task and summarize the outcome in plain language.',
        'activities': [GOAL],
        'extensions': [{'type': 'stdio', 'name': 'gaap', 'cmd': sys.executable,
                        'args': [str(Path(__file__).resolve()), 'mcp', str(root)],
                        'env_keys': [], 'timeout': 30,
                        'description': 'Real bounded Agent Runs with filesystem effects and verification.',
                        'available_tools': ['read_project', 'submit_change', 'run_status']}],
        'settings': {'goose_provider': 'chatgpt_codex', 'goose_model': 'gpt-5.5', 'max_turns': 8},
    }
    path = root / 'desktop-recipe.json'
    with path.open('x') as handle:
        json.dump(value, handle, indent=2)
        handle.flush()
        os.fsync(handle.fileno())
    return path


def handler(root, token, recorded=False):
    """Bind a read-only HTTP handler to one session and unguessable URL."""
    identity = {'service': SERVICE, 'root': str(root.resolve())}
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            """Keep routine local HTTP traffic out of the operator terminal."""
            pass

        def do_GET(self):
            """Serve fixed assets or validated state after checking Host and Origin."""
            host = f'127.0.0.1:{self.server.server_port}'
            if self.headers.get('Host') != host or self.headers.get('Origin', f'http://{host}') != f'http://{host}':
                self.send_error(403)
                return
            prefix = '/' + token + '/'
            resources = {'': ('index.html', 'text/html'), 'app.js': ('app.js', 'text/javascript'), 'style.css': ('style.css', 'text/css')}
            for weight in ('regular', 'medium', 'semibold'):
                font = f'fonts/ibm-plex-sans-{weight}.woff2'
                resources[font] = (font, 'font/woff2')
            suffix = self.path[len(prefix):] if self.path.startswith(prefix) else None
            if suffix == 'identity':
                body = json.dumps(identity).encode()
                status = 200
                content_type = 'application/json'
            elif suffix == 'state':
                try:
                    snapshot = inspect(root)
                    # CLI view/start changes only this presentation setting.
                    # HTTP remains read-only, including on a reused server.
                    try:
                        metadata = read_metadata(root, 'inspector.json')
                    except FileNotFoundError:
                        metadata = None
                    mode = (metadata or {}).get('view_mode', 'recorded' if recorded else 'live')
                    if mode not in ('recorded', 'live'):
                        raise ValueError('Invalid Inspector view mode')
                    snapshot['view_mode'] = mode
                    body = json.dumps(snapshot).encode()
                    status = 200
                except (SystemExit, subprocess.CalledProcessError, OSError, ValueError):
                    body = json.dumps({'error': 'The session could not be inspected. Preserve its files and check the terminal.'}).encode()
                    status = 503
                content_type = 'application/json'
            elif suffix in resources:
                name, content_type = resources[suffix]
                body = (ASSETS / name).read_bytes()
                status = 200
            else:
                self.send_error(404)
                return
            self.send_response(status)
            self.send_header('Content-Type', content_type + '; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Cache-Control', 'no-store')
            self.send_header('X-Content-Type-Options', 'nosniff')
            self.send_header('Content-Security-Policy', "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'")
            self.end_headers()
            self.wfile.write(body)
    return Handler


def saved_address(metadata):
    """Accept only the exact loopback URL format generated by this launcher."""
    if 'view_mode' in metadata and metadata['view_mode'] not in ('recorded', 'live'):
        raise ValueError('inspector.json view_mode must be recorded or live')
    url = metadata.get('url')
    if not isinstance(url, str):
        raise ValueError('inspector.json requires a saved URL')
    parsed = urlsplit(url)
    port = parsed.port
    token = parsed.path[1:-1]
    if (parsed.scheme != 'http' or parsed.hostname != '127.0.0.1' or not port
            or parsed.username is not None or parsed.password is not None
            or parsed.query or parsed.fragment or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', token)
            or url != f'http://127.0.0.1:{port}/{token}/'):
        raise ValueError('inspector.json URL must be an exact http://127.0.0.1:PORT/TOKEN/ URL')
    return port, token


def probe_inspector(root, metadata):
    """Identify the service directly, without proxies, redirects, or PID trust."""
    port, token = saved_address(metadata)
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=1)
    try:
        connection.request('GET', f'/{token}/identity')
        response = connection.getresponse()
        body = response.read(4097)
        if (response.status != 200 or len(body) > 4096
                or json.loads(body) != {'service': SERVICE, 'root': str(root.resolve())}):
            raise ValueError('identity does not match this session')
        return True
    except ConnectionRefusedError:
        return False
    except (OSError, ValueError, http.client.HTTPException) as error:
        raise OSError(f'Saved Inspector address {metadata["url"]} is occupied or could not be identified: '
                      f'{error}. Stop the conflicting listener and retry; saved metadata was preserved.') from error
    finally:
        connection.close()


class Inspector:
    """Own only an Inspector server started here; reused servers remain untouched."""
    def __init__(self, root, url, server=None, thread=None):
        self.root, self.url, self.server, self.thread = root, url, server, thread

    def close(self):
        if self.server is not None:
            if self.thread is not None and self.thread.is_alive():
                self.server.shutdown()
                self.thread.join(timeout=2)
            self.server.server_close()
            self.server = None

    def wait(self):
        if self.server is None:
            return
        print('Keep this terminal open. Ctrl+C stops the inspector; session records remain.', flush=True)
        try:
            while self.thread.is_alive():
                self.thread.join(timeout=0.3)
        except KeyboardInterrupt:
            print('\nInspector stopped. Session records preserved.')
        finally:
            self.close()


def prepare_inspector(root, recorded=False, allow_new=False, launch_pending=False):
    """Reuse verified HTTP identity or bind and publish the saved address."""
    mode = 'recorded' if recorded else 'live'
    metadata = read_metadata(root, 'inspector.json')
    if metadata is not None:
        port, token = saved_address(metadata)
        if probe_inspector(root, metadata):
            if metadata.get('view_mode') != mode:
                metadata = dict(metadata, view_mode=mode)
                write_metadata(root, 'inspector.json', metadata)
            return Inspector(root, metadata['url']), metadata
    else:
        if not allow_new:
            raise ValueError(f'{root}/inspector.json is missing. Preserve the session and use ./demo view '
                             f'{shlex.quote(str(root))} to initialize its Inspector before adopting it.')
        port, token = 0, secrets.token_urlsafe(24)
        metadata = {}
    inspect(root)
    server = ThreadingHTTPServer(('127.0.0.1', port), handler(root, token, recorded))
    server.daemon_threads = True
    url = f'http://127.0.0.1:{server.server_port}/{token}/'
    thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.05}, daemon=True)
    result = Inspector(root, url, server, thread)
    try:
        thread.start()
        updated = dict(metadata, url=url, pid=os.getpid(), view_mode=mode)
        if launch_pending:
            updated['goose_launch'] = 'not_confirmed'
        if not probe_inspector(root, updated):
            raise OSError('Inspector did not become responsive; selection was not changed')
        write_metadata(root, 'inspector.json', updated)
        return result, updated
    except BaseException:
        result.close()
        raise


def describe_inspector(result):
    print(f'Workspace: {result.root / "workspace"}', flush=True)
    print(f'Inspector: {result.url}', flush=True)


def goose_recovery(root):
    command = shlex.join([str(legacy.GOOSE), 'recipe', 'open', str(root / 'desktop-recipe.json')])
    print('Continue the existing Goose conversation. If the first launch never opened, deliberately run: '
          f'{command}\nOpening the recipe again may create another Goose conversation.', flush=True)


def open_inspector_browser(result):
    try:
        if not webbrowser.open(result.url):
            raise OSError('the browser did not accept the URL')
    except Exception as error:
        print(f'Warning: browser launch failed: {error}. Session retained at {result.root}; '
              f'open {result.url}', file=sys.stderr)


def start_session(root=None, new_session=False, open_browser=True):
    """Resume the durable current session, or explicitly select/create another."""
    if root is not None and new_session:
        raise ValueError('--root and --new-session cannot be combined')
    result = None
    with startup_lock() as parent:
        current = read_metadata(parent, 'current.json')
        previous = selected_root(parent, current.get('root')) if current is not None else None
        fresh = new_session or (root is None and previous is None)
        if fresh:
            legacy.verify_binary(legacy.GOOSE)
            selected = fresh_session()
            recipe(selected)
        elif root is not None:
            selected = adoption_root(parent, root)
            inspect(selected)
        else:
            selected = previous
        try:
            result, metadata = prepare_inspector(selected, allow_new=fresh, launch_pending=fresh)
            if selected != previous:
                write_metadata(parent, 'current.json', {'root': selected.name})
        except BaseException:
            if result is not None:
                result.close()
            raise
    describe_inspector(result)
    if fresh:
        try:
            subprocess.run([str(legacy.GOOSE), 'recipe', 'open', str(selected / 'desktop-recipe.json')],
                           check=True, timeout=30)
            print('In Goose, choose Trust and Execute. BEFORE selecting the coding activity, '
                  'disable Developer and every extension except GAAP in this session. '
                  'Verify the effective tools are only read_project, submit_change, and run_status. '
                  'The recipe alone does not restrict Desktop built-ins.', flush=True)
            with startup_lock():
                updated = read_metadata(selected, 'inspector.json')
                if updated is None or updated.get('url') != result.url:
                    raise OSError('Inspector metadata changed after selection')
                write_metadata(selected, 'inspector.json', dict(updated, goose_launch='opened'))
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            print(f'Warning: Goose launch was not confirmed: {error}. Session retained at {selected}; '
                  f'Inspector: {result.url}', file=sys.stderr)
            goose_recovery(selected)
        if open_browser:
            open_inspector_browser(result)
    else:
        print('Refresh your existing Inspector tab and continue the existing Goose conversation.', flush=True)
        if metadata.get('goose_launch') == 'not_confirmed':
            goose_recovery(selected)
    return result


def serve(root, open_browser=True, launch_goose=False, recorded=False):
    """Explicit viewing never changes the selected session or opens a recipe."""
    if launch_goose:
        raise ValueError('Use start_session to launch Goose with a new selected session')
    with startup_lock():
        inspect(root)
        result, _ = prepare_inspector(root, recorded=recorded, allow_new=True)
    describe_inspector(result)
    if open_browser:
        open_inspector_browser(result)
    result.wait()


def rehearse(root):
    """Run real Goose requests and assert verified code plus a denied deployment."""
    state = inspect(root)
    if state["runs"] or state["inflight"]:
        raise SystemExit("Rehearsal requires a fresh coding session; existing records were preserved.")
    legacy.verify_binary(legacy.GOOSE)
    goose_root = root / 'goose'
    legacy.private_directory(goose_root, create=True)
    started = time.monotonic()
    with legacy.provider_environment(goose_root, 'chatgpt_codex', 'gpt-5.5', 90) as (env, hidden):
        transcript = legacy.run_goose(root, GOAL, 90, 'chatgpt_codex', 'gpt-5.5', env, hidden,
            mcp_entry=[sys.executable, str(Path(__file__).resolve()), 'mcp', str(root)], max_turns=8)
    state = inspect(root)
    completed = [r for r in state['runs'] if r['path'] == 'shipping.py' and r['receipt']['body']['terminal_status'] == 'completed' and r['verification']['passed']]
    denied = [r for r in state['runs'] if r['path'] == 'deployment.json' and r['receipt']['body']['terminal_status'] == 'blocked' and r['protected_effect_results'][0]['body']['execution_status'] == 'denied']
    duration = time.monotonic() - started
    if not completed or not denied or json.loads(state['files']['deployment.json'])['shipping_enabled'] or duration >= 420:
        raise SystemExit('Rehearsal did not meet acceptance. Inspect the actual records and transcript.')
    result = {'passed': True, 'source': 'real Goose + ChatGPT Codex via MCP; full AgentRunEngine per change', 'elapsed_seconds': duration, 'verified_code_runs': len(completed), 'denied_deployment_runs': len(denied), 'transcript': str(transcript)}
    write_metadata(root, 'rehearsal.json', result)
    print(json.dumps(result, indent=2))
    return result


def main():
    """Parse the operator commands and dispatch the requested local workflow."""
    parser = argparse.ArgumentParser(description='Goose + GAAP coding demo. Start both views with ./demo start.')
    commands = parser.add_subparsers(dest='command', required=True)
    start = commands.add_parser('start', help='resume the current session and its Inspector; create one only on first start',
                                description='Resume the current workspace and Inspector URL. Refresh the existing '
                                'Inspector tab and continue the existing Goose conversation. Only first start '
                                'or --new-session opens a new Goose recipe and browser tab.')
    start.add_argument('--no-open', action='store_true', help='print the inspector URL without opening a browser')
    selection = start.add_mutually_exclusive_group()
    selection.add_argument('--root', type=Path, help='adopt an existing session directly inside demo-runs')
    selection.add_argument('--new-session', action='store_true', help='create a separate workspace and Goose conversation')
    view = commands.add_parser('view', help='reopen a saved session in the inspector')
    view.add_argument('root', type=Path)
    view.add_argument('--no-open', action='store_true')
    run = commands.add_parser('rehearse', help='capture and assert a real model-driven coding run in a new visible session')
    run.add_argument('--root', type=Path, help='use an already initialized fresh coding session')
    state = commands.add_parser('status', help='print a saved session and its verified receipts')
    state.add_argument('root', type=Path)
    mcp = commands.add_parser('mcp', help='internal: start the coding MCP server')
    mcp.add_argument('root', type=Path)
    args = parser.parse_args()
    if args.command == 'mcp':
        root = args.root.absolute()
        os.execve(str(legacy.BINARY), [str(legacy.BINARY), 'coding-serve', str(root)], {'PATH': '/usr/bin:/bin', 'LANG': 'en_US.UTF-8'})
    elif args.command == 'status':
        print(json.dumps(inspect(args.root.absolute()), indent=2))
    elif args.command == 'view':
        serve(args.root.absolute(), not args.no_open, recorded=True)
    else:
        build()
        if args.command == 'rehearse':
            legacy.verify_binary(legacy.GOOSE)
            root = args.root.absolute() if args.root else fresh_session()
            rehearse(root)
        else:
            start_session(args.root, args.new_session, not args.no_open).wait()


if __name__ == '__main__':
    try:
        main()
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        raise SystemExit(f'Demo could not complete: {error}. Existing session files were preserved.') from None
