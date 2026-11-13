"""Exercise real pinned Goose with a local deterministic provider and MCP host."""
import argparse
import http.server
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shlex
import signal
import subprocess
import tempfile
import threading

HERE = Path(__file__).resolve().parent
BINARY = HERE / 'target/debug/gaap-submitted-proposal-reference'
GOOD = """def shipping_quote(quantity, unit_price):
    if quantity <= 0:
        raise ValueError('quantity must be positive')
    subtotal = quantity * unit_price
    if quantity >= 5:
        subtotal = subtotal * 0.9
    if subtotal < 100:
        subtotal = subtotal + 7
    return round(subtotal, 2)
"""

def command(*args):
    """Run one isolated reference CLI command and decode its JSON result."""
    result = subprocess.run([str(BINARY), *map(str, args)], check=True,
        capture_output=True, text=True, timeout=20, env={})
    return json.loads(result.stdout)

def provider(transcript):
    """Serve deterministic tool-call responses without external inference."""
    class Provider(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            """Keep HTTP access logs out of the acceptance evidence."""
            pass
        def do_GET(self):
            """Advertise the single local fixture model."""
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'object': 'list', 'data': [{'id': 'gaap-fixture', 'object': 'model', 'owned_by': 'fixture'}]}).encode())
        def do_POST(self):
            """Emit the next bounded proposal using the prior MCP observation."""
            length = int(self.headers['Content-Length'])
            if length > 1024 * 1024:
                self.send_error(413)
                return
            body = json.loads(self.rfile.read(length))
            transcript.append(body)
            names = [tool['function']['name'] for tool in body.get('tools', [])]
            expected = ['gaap__read_project', 'gaap__submit_change', 'gaap__run_status']
            if sorted(names) != sorted(expected) or not body.get('stream'):
                self.send_error(422, 'Unexpected tool inventory or nonstreaming request')
                return
            messages = [message for message in body['messages'] if message.get('role') == 'tool']
            turn = len(messages)
            if turn in (0, 2):
                name, arguments = 'gaap__read_project', {}
            elif turn in (1, 3):
                # Copy only the digest returned over MCP, as an external caller would.
                text = json.dumps(messages[-1])
                matches = re.findall(r'subject_digest[^a-z0-9]+(sha256:[a-f0-9]{64})', text)
                if not matches:
                    self.send_error(422, 'No subject digest in the read result')
                    return
                name = 'gaap__submit_change'
                arguments = {'path': 'shipping.py' if turn == 1 else 'deployment.json',
                    'content': GOOD if turn == 1 else '{"shipping_enabled":true}',
                    'plan': 'Implement and verify shipping.' if turn == 1 else 'Request production enablement.',
                    'base_digest': matches[-1]}
            else:
                name = None
            if name:
                call = {'index': 0, 'id': 'proposal-' + str(turn), 'type': 'function',
                    'function': {'name': name, 'arguments': json.dumps(arguments)}}
                chunks = [({'role': 'assistant', 'tool_calls': [call]}, None), ({}, 'tool_calls')]
            else:
                chunks = [({'role': 'assistant', 'content': 'The host returned the code and deployment outcomes.'}, None), ({}, 'stop')]
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for delta, finish in chunks:
                packet = {'id': 'fixture', 'object': 'chat.completion.chunk', 'created': 1,
                    'model': 'gaap-fixture', 'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]}
                self.wfile.write(('data: ' + json.dumps(packet) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
    return http.server.ThreadingHTTPServer(('127.0.0.1', 0), Provider)

def validate_outcomes(final, initial):
    """Refuse invalid evidence even when Python optimization is enabled."""
    runs = final['runs']
    checks = [
        (len(runs) == 2, 'Expected exactly two admitted proposals'),
    ]
    for passed, message in checks:
        if not passed:
            raise ValueError(message)
    checks = [
        (runs[0]['receipt']['body']['terminal_status'] == 'completed', 'Code run failed completion'),
        (runs[0]['verification']['passed'] and len(runs[0]['verification']['cases']) == 8, 'Shipping verification failed'),
        (runs[1]['receipt']['body']['terminal_status'] == 'blocked', 'Deployment run failed refusal'),
        (runs[1]['protected_effect_results'][0]['body']['execution_status'] == 'denied', 'Deployment effect failed denial'),
        (final['files']['shipping.py'] == GOOD, 'Shipping file differs from the proposal'),
        (final['files']['deployment.json'] == initial['files']['deployment.json'], 'Deployment file changed'),
        (all(run['model_usage'] is None for run in runs), 'External model usage must stay unavailable'),
        (all(run['terminal_receipt_verified'] for run in runs), 'Host receipt validation failed'),
    ]
    for passed, message in checks:
        if not passed:
            raise ValueError(message)
    return runs

def run(goose, output):
    # Reuse the repository's current pin, independently of the caller's PATH.
    """Verify pinned Goose and retain the observed MCP acceptance evidence."""
    installer_path = HERE.parent / 'governed-goose/install_goose.py'
    spec = importlib.util.spec_from_file_location('goose_pins', installer_path)
    installer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer)
    pin = installer.PINS[(platform.system(), platform.machine())][2]
    if installer.digest(goose) != pin:
        raise ValueError('Goose executable differs from the pinned 1.54.0 artifact')
    output.mkdir(parents=True, exist_ok=False)
    transcript = []
    with tempfile.TemporaryDirectory(prefix='gaap-submission-') as temporary:
        parent = Path(temporary).resolve()
        root = parent / 'session'
        initial = command('coding-init', root)
        state = parent / 'goose'
        (state / 'config').mkdir(parents=True)
        (state / 'config/config.yaml').write_text('extensions:\n  developer:\n    enabled: false\n    type: builtin\n    name: developer\n')
        server = provider(transcript)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        environment = {'HOME': str(state), 'PATH': os.defpath, 'GOOSE_PATH_ROOT': str(state),
            'GOOSE_PROVIDER': 'openai', 'GOOSE_MODEL': 'gaap-fixture', 'GOOSE_MODE': 'auto',
            'GOOSE_DISABLE_KEYRING': '1', 'OPENAI_API_KEY': 'local-fixture-only',
            'OPENAI_BASE_URL': f'http://127.0.0.1:{server.server_port}/v1'}
        invocation = [str(goose), 'run', '--no-profile', '--no-session', '--provider', 'openai',
            '--model', 'gaap-fixture', '--with-extension', 'gaap:' + shlex.join([str(BINARY), 'coding-serve', str(root)]),
            '--max-turns', '8', '--output-format', 'text', '--text', 'Complete the shipping task using the supplied tools.']
        try:
            with subprocess.Popen(invocation, cwd=root / 'workspace', env=environment,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True) as process:
                try:
                    stdout, stderr = process.communicate(timeout=60)
                finally:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait(timeout=3)
            (output / 'goose-stdout.txt').write_bytes(stdout)
            (output / 'goose-stderr.txt').write_bytes(stderr)
            (output / 'provider-transcript.json').write_text(json.dumps(transcript, indent=2) + '\n')
            if process.returncode:
                raise AssertionError(f'Goose failed with {process.returncode}; inspect {output}')
            final = command('coding-inspect', root)
            (output / 'outcomes.json').write_text(json.dumps(final, indent=2) + '\n')
            runs = validate_outcomes(final, initial)
            (output / 'verification.json').write_text(json.dumps({'client': 'Goose 1.54.0',
                'goose_sha256': pin, 'provider': 'deterministic local protocol fixture',
                'live_model': False, 'external_model_usage': None, 'runs': 2,
                'terminal_statuses': [run['receipt']['body']['terminal_status'] for run in runs]}, indent=2) + '\n')
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
    print(f'Deterministic Goose acceptance passed; evidence: {output}')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--goose', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='New evidence directory')
    args = parser.parse_args()
    run(args.goose.resolve(), args.output.resolve())
