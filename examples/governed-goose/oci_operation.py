"""Fixed reference operations inside the selected OCI profile; arguments are data."""
import json
import pathlib
import subprocess
import sys

arguments = json.loads(sys.argv[1])
operation = arguments['operation']
content = arguments['content']
if set(arguments) != {'operation', 'content'} or not isinstance(content, str) or len(content) > 8192:
    raise ValueError('Invalid bounded operation')
paths = {'file_write': 'shipping.py', 'terminal_echo': 'terminal.txt', 'mcp_echo': 'mcp.txt'}
target = pathlib.Path('/workspace') / paths[operation]
if target.is_symlink() or not target.is_file():
    raise ValueError('Select the predeclared regular output file')
if operation == 'terminal_echo':
    result = subprocess.run([sys.executable, '-I', '-S', '-B', '-c', "import sys;print(sys.argv[1],end='')", content], env={}, capture_output=True, timeout=2, check=True)
    if result.stderr or len(result.stdout) > 8192:
        raise ValueError('Unexpected terminal output')
    target.write_bytes(result.stdout)
else:
    target.write_text(content)
print('Declared OCI effect finished')
