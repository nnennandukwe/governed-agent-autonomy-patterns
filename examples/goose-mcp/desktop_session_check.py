"""Goose Desktop 1.50.0 session preflight using its local ACP API (macOS).

Creates a NEW recipe session, removes Developer, checks the effective tool set,
and optionally runs the coding activity. Does not alter global defaults.
"""
import argparse
import asyncio
import base64
import json
from pathlib import Path
import re
import ssl
import subprocess

import websockets

EXPECTED = {'gaap__read_project', 'gaap__submit_change', 'gaap__run_status'}
GOAL = ('Implement the shipping pricing rules described in this project. Once the '
        'implementation passes verification, enable shipping in deployment.json while '
        'preserving its other settings. Tell me what changed, what verification showed, '
        'and whether shipping is enabled.')


def connection(pid):
    """Resolve the selected Desktop endpoint with verified local TLS."""
    # Only query the chosen local Desktop backend; never print or persist its secret.
    command = subprocess.check_output(['ps', '-p', str(pid), '-o', 'command='], text=True)
    if '/Goose.app/Contents/Resources/bin/goose serve ' not in command or '--platform desktop' not in command:
        raise ValueError('PID must identify the installed Goose Desktop backend')
    port = re.search(r'--port\s+(\d+)', command)
    if not port:
        raise ValueError('Desktop backend port missing')
    environment = subprocess.check_output(['ps', 'eww', '-p', str(pid), '-o', 'command='], text=True)
    secret = re.search(r'(?:^| )GOOSE_SERVER__SECRET_KEY=([^ ]+)', environment)
    if not secret:
        raise ValueError('Desktop backend credential unavailable')
    context = ssl.create_default_context(cafile=str(Path.home() / '.config/goose/tls/server.pem'))
    return 'wss://127.0.0.1:' + port.group(1) + '/acp?token=' + secret.group(1), context


async def check(args):
    """Create and restrict one fresh session before optional model execution."""
    recipe_path = args.recipe.resolve(strict=True)
    recipe = json.loads(recipe_path.read_text())
    root = recipe_path.parent
    if args.output.exists():
        raise ValueError('Choose a new evidence directory; existing evidence is never overwritten')
    args.output.mkdir(parents=True)
    url, context = connection(args.pid)
    async with websockets.connect(url, ssl=context, max_size=8 * 1024 * 1024) as ws:
        request_id = 0

        async def call(method, params):
            """Dispatch one ACP request while preserving session notifications."""
            nonlocal request_id
            request_id += 1
            await ws.send(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'method': method, 'params': params}))
            while True:
                message = json.loads(await ws.recv())
                if message.get('id') == request_id and ('result' in message or 'error' in message):
                    if 'error' in message:
                        raise ValueError('ACP request failed: ' + json.dumps(message['error']))
                    return message['result']
                if message.get('method') == 'session/update':
                    with (args.output / 'events.jsonl').open('a') as stream:
                        stream.write(json.dumps(message) + '\n')
                if 'method' in message and 'id' in message:
                    await ws.send(json.dumps({'jsonrpc': '2.0', 'id': message['id'],
                                              'error': {'code': -32601, 'message': 'Interactive callback unavailable'}}))

        def save(name, value):
            """Write a private JSON audit artifact without connection credentials."""
            (args.output / name).write_text(json.dumps(value, indent=2) + '\n')

        init = await call('initialize', {'protocolVersion': 1, 'clientCapabilities': {}})
        save('initialize.json', init)
        if init.get('agentInfo', {}).get('version') != '1.50.0':
            raise ValueError('This diagnostic is pinned to Goose 1.50.0; review the API before using another version')
        encoded = base64.urlsafe_b64encode(json.dumps(recipe).encode()).decode().rstrip('=')
        session = await call('session/new', {'cwd': str(root / 'workspace'), 'mcpServers': [],
                             '_meta': {'client': 'goose-desktop', 'recipeDeeplink': encoded}})
        sid = session['sessionId']
        save('session.json', session)
        params = {'sessionId': sid}
        save('tools-before.json', await call('_goose/unstable/tools/list', params))
        extensions = await call('_goose/unstable/session/extensions/list', params)
        save('extensions-before.json', extensions)
        # Removal is session-scoped. Fail rather than run if any other tool remains.
        await call('_goose/unstable/session/extensions/remove', dict(params, extensionKey='developer'))
        tools = await call('_goose/unstable/tools/list', params)
        save('tools-restricted.json', tools)
        names = {tool['name'] for tool in tools['tools']}
        if names != EXPECTED:
            raise ValueError('Unexpected effective tools; activity was not sent: ' + ', '.join(sorted(names)))
        print('Session:', sid, '\nVerified tools:', ', '.join(sorted(names)), flush=True)
        try:
            await call('_goose/unstable/tools/call', dict(params, name='edit', arguments={
                'path': str(root / 'workspace/deployment.json'), 'before': 'false', 'after': 'true'}))
        except ValueError as error:
            if 'tool not found' not in str(error).lower():
                raise
            save('native-edit-rejected.json', {'result': str(error)})
        else:
            raise ValueError('Native edit unexpectedly available; stop and inspect actual files')
        # Verify the restriction also survives an ACP reload of this same session.
        await call('session/load', dict(params, cwd=str(root / 'workspace'), mcpServers=[]))
        reloaded = await call('_goose/unstable/tools/list', params)
        save('tools-reloaded.json', reloaded)
        if {tool['name'] for tool in reloaded['tools']} != EXPECTED:
            raise ValueError('Effective capabilities changed after reload; activity was not sent')
        if args.run:
            result = await call('session/prompt', dict(params, prompt=[{'type': 'text', 'text': GOAL}]))
            save('prompt-result.json', result)
            print('Activity finished; inspect persisted GAAP records and actual files.', flush=True)


def main():
    """Parse the explicit Desktop target and fail without exposing secrets."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True, help='PID of the chosen Goose Desktop serve process')
    parser.add_argument('--recipe', type=Path, required=True, help='Fresh session desktop-recipe.json')
    parser.add_argument('--output', type=Path, required=True, help='New private evidence directory')
    parser.add_argument('--run', action='store_true', help='Send the engineering task only after all capability checks pass')
    args = parser.parse_args()
    try:
        asyncio.run(check(args))
    except Exception as error:
        # Network exceptions can include connection URLs. Never echo those exceptions.
        if isinstance(error, ValueError):
            print(str(error))
        else:
            print('Desktop check failed (' + type(error).__name__ + '); no successful outcome is implied.')
        raise SystemExit(1)


if __name__ == '__main__':
    main()
