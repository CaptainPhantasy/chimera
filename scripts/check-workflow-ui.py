#!/usr/bin/env python3
"""Exercise the real chat controls with isolated files and a local fixture service."""
import argparse, hashlib, http.server, importlib.util, json, os, sys, tempfile, threading, time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('binary', type=Path)
parser.add_argument('--through', type=int, default=1)
args = parser.parse_args()
binary = args.binary.resolve(strict=True)
root = Path(tempfile.mkdtemp(prefix='chimera-workflow-ui-'))
checks, calls, screens = [], [], []
spec = importlib.util.spec_from_file_location('setup_checks', Path(__file__).with_name('check-setup.py'))
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)
# Optional test-only dependency: python3 -m pip install --target .floyd/testdeps pyte
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'.floyd/testdeps'))
import pyte

class RenderedScreen(setup.Screen):
    def wait_for(self, marker, offset=0, seconds=10):
        deadline = time.monotonic() + seconds
        while True:
            self.drain(.04)
            terminal = pyte.Screen(120, 32)
            pyte.Stream(terminal).feed(bytes(self.output).decode('utf8', 'replace'))
            visible = '\n'.join(terminal.display)
            if marker in visible:
                self.check('screen shows ' + marker, True)
                return
            if self.process.poll() is not None or time.monotonic() >= deadline:
                (self.directory/'last-screen.txt').write_text(visible)
                raise setup.ObservationTimeout('Still waiting for rendered screen text: '+marker)


def check(name, ok):
    checks.append({'check': name, 'pass': bool(ok)})
    if not ok: raise AssertionError(name)

class Service(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        calls.append({'method': 'GET', 'path': self.path, 'authorized': self.headers.get('Authorization') == 'Bearer UI_TEST_KEY'})
        if self.headers.get('Authorization') != 'Bearer UI_TEST_KEY':
            self.send_response(401); self.end_headers(); self.wfile.write(b'{"error":"do not echo service secrets"}'); return
        self.send_response(200); self.send_header('Content-Type', 'application/json'); self.end_headers()
        self.wfile.write(b'{"data":[{"id":"model-beta"},{"id":"model-alpha"}]}')
    def do_POST(self):
        calls.append({'method': 'POST', 'path': self.path})
        if self.headers.get('Authorization') != 'Bearer UI_TEST_KEY':
            self.send_response(401); self.end_headers(); return
        completions['count'] += 1
        if completions['count'] == 1:
            tool_call = {'index': 0, 'id': 'call-external-1', 'type': 'function',
                         'function': {'name': 'mcp__reachable-fixture__echo', 'arguments': '{"query":"hello"}'}}
            chunks = [{'choices': [{'delta': {'tool_calls': [tool_call]}, 'finish_reason': None}]},
                      {'choices': [{'delta': {}, 'finish_reason': 'tool_calls'}]}]
        else:
            chunks = [{'choices': [{'delta': {'content': 'external echo answered hello'}, 'finish_reason': None}]},
                      {'choices': [{'delta': {}, 'finish_reason': 'stop'}]}]
        chunks.append({'choices': [], 'usage': {'prompt_tokens': 5, 'completion_tokens': 3, 'total_tokens': 8}})
        self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.end_headers()
        for item in chunks:
            self.wfile.write(('data: ' + json.dumps(item) + '\n\n').encode())
        self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush()

completions = {'count': 0}

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Service)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, daemon=True).start()
config = root/'settings.json'
project = root/'project'; project.mkdir()
# /mcp — external tool connections: one real stdio fixture server and one
# broken command, both isolated inside the temporary workspace.
mcp_fixture = root/'mcp-fixture.py'
mcp_fixture.write_text(
    "import json, sys\n"
    "for line in sys.stdin:\n"
    "    request = json.loads(line)\n"
    "    if 'id' not in request: continue\n"
    "    method = request['method']\n"
    "    if method == 'initialize':\n"
    "        result = {'protocolVersion': '2025-11-25', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '1'}}\n"
    "    elif method == 'tools/list':\n"
    "        result = {'tools': [{'name': 'echo', 'description': 'Echo the query.', 'inputSchema': {'type': 'object', 'properties': {'query': {'type': 'string'}}, 'required': ['query'], 'additionalProperties': False}}]}\n"
    "    elif method == 'ping':\n"
    "        result = {}\n"
    "    elif method == 'tools/call':\n"
    "        result = {'content': [{'type': 'text', 'text': 'echo from fixture: ' + request['params']['arguments']['query']}]}\n"
    "    else:\n"
    "        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': 'not found'}}), flush=True); continue\n"
    "    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)\n")
mcp_config = root/'mcp.json'
mcp_config.write_text(json.dumps([
    {'name': 'broken', 'transport': {'type': 'stdio'}, 'command': '/nonexistent/chimera-missing-command', 'args': [], 'env': {}},
    {'name': 'reachable-fixture', 'transport': {'type': 'stdio'}, 'command': sys.executable, 'args': ['-u', str(mcp_fixture)], 'env': {'FIXTURE_TOKEN': 'fixture-secret-value'}},
]) + '\n')
row = {}
screen = RenderedScreen(binary, project, config, ['tui'], check, row,
                        extra_env={'CHIMERA_MCP_CONFIG': str(mcp_config)}); screens.append(screen)
report = {'binary': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'workspace': str(root), 'checks': checks, 'screen': row, 'calls': calls}
def send(text, marker=None):
    offset = screen.send(text)
    if marker: screen.wait_for(marker, offset)
    until = time.monotonic() + .18
    while time.monotonic() < until: screen.drain(.03)
    if marker: (root/('screen-'+str(len(checks))+'.ansi')).write_bytes(screen.output)
    return offset
try:
    screen.wait_for('Model setup needed.')
    send('draft to preserve')
    send('\x1bOQ', 'Connect a provider')
    send('\x1b')
    screen.wait_for('draft to preserve')
    check('opening and cancelling provider setup preserves draft', True)
    send('\x7f' * len('draft to preserve'))
    send('/connect\r', 'Connect a provider')
    send('Custom\r', 'Connection name')
    send('\x15Fixture\r')
    send(f'http://127.0.0.1:{server.server_port}/v1\r')
    send('bad-key\r', 'rejected this key')
    check('rejected credentials do not create settings', not config.exists())
    # Retry retains the entered address and name; navigate to the masked key.
    send('\t\t\x15UI_TEST_KEY\r', 'Choose a model')
    check('connection test makes only read-only model requests', all(c['method'] == 'GET' for c in calls))
    send('beta\r', 'Connected to Fixture.')
    saved = json.loads(config.read_text())
    check('wizard saves selected model and named connection', saved['provider']['model'] == 'model-beta' and saved['providers']['Fixture']['model'] == 'model-beta')
    check('saved key file is owner-only', config.stat().st_mode & 0o777 == 0o600)
    check('keys never appear in terminal output', b'UI_TEST_KEY' not in screen.output and b'bad-key' not in screen.output)
    send('/connect\r', 'Fixture')
    send('\x1b')
    # /sessions — browse, search and resume a conversation without leaving chat.
    seed_row = {}
    seed = setup.Screen(binary, project, config, ['tui'], check, seed_row); screens.append(seed)
    seed.wait_for('model-beta', seconds=20)
    seed.finish()
    states = {path.stem: json.loads(path.read_text())['data']
              for path in (project/'.chimera'/'sessions').glob('*.json')}
    check('two saved conversations exist for the chooser', len(states) == 2)
    newest = max(states, key=lambda key: states[key]['meta']['created_at'])
    send('/sessions\r', 'Switch conversation')
    screen.wait_for('Interactive conversation')
    send('interactive')
    screen.wait_for('Search: interactive')
    send('\r')
    screen.wait_for('session ' + newest[:7])
    screen.wait_for('ready to send')
    check('resume opens the newest conversation in place', True)
    send('/sessions\r', 'Switch conversation')
    send('\x1b')
    # /mcp — external tool connections in read-only mode: real status rows, an
    # honest refusal to start programs, and guided setup that appends one
    # entry without touching the saved ones.
    send('/mcp\r', 'External tool connections')
    screen.wait_for('broken')
    screen.wait_for('reachable-fixture')
    send('\r', 'Read-only plan mode cannot start external programs')
    check('read-only refusal keeps the connection list open', True)
    send('\x1b[B\x1b[B')
    send('\r', 'Connection name')
    send('\r\r\r\r', 'Enter a connection name')
    check('empty setup input is refused and changes nothing', True)
    send('\x1b[Z\x1b[Z\x1b[Z')
    send('saved-via-ui\t')
    send('python3\t')
    send(f'-u {mcp_fixture}\t')
    send('UI_KEY=ui-secret-value\r', 'Save this connection?')
    check('guided setup never displays the credential value', b'ui-secret-value' not in screen.output)
    send('\r', 'Connection saved-via-ui saved')
    entries = {server['name']: server for server in json.loads(mcp_config.read_text())}
    check('guided setup appends exactly one new entry',
          list(entries) == ['broken', 'reachable-fixture', 'saved-via-ui'])
    check('setup preserves the saved broken entry',
          entries['broken']['command'] == '/nonexistent/chimera-missing-command')
    check('setup preserves the fixture entry credentials',
          entries['reachable-fixture']['env'] == {'FIXTURE_TOKEN': 'fixture-secret-value'})
    check('saved setup entry keeps its credential for the server process',
          entries['saved-via-ui']['env'] == {'UI_KEY': 'ui-secret-value'})
    send('\x1b')
    send('/mcp\r', 'External tool connections')
    send('\x1b')
    screen.finish()
    # A supervised screen attaches the checked fixture server and uses its
    # echo tool inside a real request through the ordinary chat approval.
    project_supervised = root/'project-supervised'; project_supervised.mkdir()
    config_supervised = root/'settings-supervised.json'
    config_supervised.write_text(json.dumps({'provider': {
        'name': 'Fixture', 'base_url': f'http://127.0.0.1:{server.server_port}/v1',
        'model': 'model-beta', 'api_key': 'UI_TEST_KEY',
        'request_timeout_secs': 5, 'idle_timeout_secs': 3}}) + '\n')
    screen = RenderedScreen(binary, project_supervised, config_supervised, ['tui'], check, row,
                            profile='supervised', extra_env={'CHIMERA_MCP_CONFIG': str(mcp_config)})
    screens.append(screen)
    screen.wait_for('model-beta', seconds=20)
    send('/mcp\r', 'External tool connections')
    send('\x1b[B', 'reachable-fixture')
    send('\r', 'reachable-fixture is reachable')
    send('\r', 'Attach reachable-fixture to this chat?')
    screen.wait_for('mcp_server_start')
    check('attach confirmation masks the credential value', b'fixture-secret-value' not in screen.output)
    check('attach confirmation names the exact startup program',
          ('-u ' + str(mcp_fixture)) in bytes(screen.output).decode('utf8', 'replace'))
    send('\r', 'reachable-fixture is attached with')
    send('\x1b')
    send('use the echo tool\r', 'Use /approve or /deny')
    send('/approve\r', 'approved once')
    send('\t', 'echo from fixture: hello')
    screen.wait_for('external echo answered hello')
    screen.wait_for('Answer saved.')
    check('attached external tool ran through the ordinary approval and answered in chat',
          completions['count'] >= 2)
    screen.finish()
except BaseException as error:
    report['error'] = repr(error)
    raise
finally:
    for item in screens: item.preserve_and_close()
    server.shutdown(); server.server_close()
    (root/'report.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'workspace': str(root), 'checks': len(checks), 'passed': sum(c['pass'] for c in checks), 'error': report.get('error')}), flush=True)
