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
        self.send_response(500); self.end_headers()

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Service)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, daemon=True).start()
config = root/'settings.json'
project = root/'project'; project.mkdir()
row = {}
# Fixture editor for /editor: prepends a marker to whatever draft file it is
# handed. The chat screen passes the file path as the final argument.
editor_script = root/'fixture-editor.sh'
editor_script.write_text('#!/bin/sh\nprintf \'EDITOR-WAS-HERE \' | cat - "$1" > "$1.new" && mv "$1.new" "$1"\n')
editor_script.chmod(0o755)
os.environ['VISUAL'] = str(editor_script)
screen = RenderedScreen(binary, project, config, ['tui'], check, row); screens.append(screen)
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
    # /editor — the unsent draft round-trips through the external editor.
    before = len(calls)
    send('chat draft ')
    send('\x05', 'EDITOR-WAS-HERE chat draft')  # Ctrl+E composes in the editor
    check('editor round trip keeps the edited draft in the input', True)
    check('editing the draft sends no model request', len(calls) == before)
    # A configured editor that cannot start must preserve the draft exactly.
    screen.clear_input('EDITOR-WAS-HERE chat draft ')
    saved = json.loads(config.read_text())
    saved['editor'] = '/chimera-no-such-editor-xyz'
    config.write_text(json.dumps(saved))
    send('/reload\r', 'Model settings reloaded')
    before = len(calls)
    send('chat draft ')
    send('\x05', 'Could not start the editor')
    screen.wait_for('chat draft')
    check('a missing editor preserves the draft', len(calls) == before)
    screen.clear_input('chat draft ')
    screen.finish()
except BaseException as error:
    report['error'] = repr(error)
    raise
finally:
    for item in screens: item.preserve_and_close()
    server.shutdown(); server.server_close()
    (root/'report.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'workspace': str(root), 'checks': len(checks), 'passed': sum(c['pass'] for c in checks), 'error': report.get('error')}), flush=True)
