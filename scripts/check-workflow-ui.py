#!/usr/bin/env python3
"""Exercise the real chat controls with isolated files and a local fixture service."""
import argparse, hashlib, http.server, importlib.util, json, os, select, subprocess, sys, tempfile, threading, time
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
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        last = body['messages'][-1]
        prompt = next((m['content'] for m in reversed(body['messages']) if m['role'] == 'user'), '')
        if self.path.endswith('/chat/completions') and last['role'] == 'tool' and 'matches' in str(last['content']):
            reply = {'choices': [{'delta': {'content': 'GLOB RAN AFTER THE OVERLAY APPROVAL'}, 'finish_reason': None}]}
        elif self.path.endswith('/chat/completions') and last['role'] == 'tool' and 'denied' in str(last['content']):
            reply = {'choices': [{'delta': {'content': 'THE OVERLAY DENIED THIS WRITE'}, 'finish_reason': None}]}
        elif self.path.endswith('/chat/completions') and prompt == 'find the project files':
            reply = {'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'glob-1', 'type': 'function', 'function': {'name': 'glob', 'arguments': json.dumps({'pattern': '*', 'path': '.'})}}]}, 'finish_reason': 'tool_calls'}]}
        elif self.path.endswith('/chat/completions') and prompt == 'write the overlay file':
            reply = {'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'write-1', 'type': 'function', 'function': {'name': 'write_file', 'arguments': json.dumps({'path': 'overlay-denied.txt', 'content': 'must stay absent\n'})}}]}, 'finish_reason': 'tool_calls'}]}
        else:
            self.send_response(500); self.end_headers(); self.wfile.write(b'{"error":"fixture only serves the scripted chat completions"}'); return
        self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.end_headers()
        def data(value):
            self.wfile.write(('data: ' + json.dumps(value) + '\n\n').encode()); self.wfile.flush()
        data(reply)
        if reply['choices'][0]['delta'].get('content') is not None:
            data({'choices': [{'delta': {}, 'finish_reason': 'stop'}]})
            data({'choices': [], 'usage': {'prompt_tokens': 9, 'completion_tokens': 4, 'total_tokens': 13}})
        self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush()

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Service)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, daemon=True).start()
config = root/'settings.json'
project = root/'project'; project.mkdir()
row = {}
waiters = []
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
    # /permissions — one place to review and change permissions in chat.
    policy_path = project/'.chimera'/'policy'/'settings.json'
    def saved_policy():
        return json.loads(policy_path.read_text())['data']
    send('/permissions\r', 'Permissions')
    screen.wait_for('Autonomy: medium — current')
    screen.wait_for('bash — enabled')
    send('write_file')
    screen.wait_for('write_file — enabled')
    send('\x7f' * len('write_file'))
    send('autonomy: low\r')
    screen.wait_for('Autonomy: low — current')
    check('autonomy change persists through the real policy store', saved_policy()['autonomy'] == 'low')
    send('git_status\r')
    screen.wait_for('git_status — disabled')
    check('disabling a tool persists', 'git_status' in saved_policy()['disabled_tools'])
    send('git_status\r')
    screen.wait_for('Allow a disabled tool again')
    check('enabling a tool asks before it widens access', saved_policy()['disabled_tools'] == ['git_status'])
    send('\r')
    screen.wait_for('git_status — enabled')
    check('confirmed enable persists', 'git_status' not in saved_policy()['disabled_tools'])
    send('\x1b')
    screen.wait_for('Autonomy saved: low.')
    screen.wait_for('git_status is disabled; the agent cannot call it in this project.')
    screen.wait_for('git_status is enabled; normal action approvals still apply.')
    # A real agent run waits for a decision; the overlay decides it in place.
    run_env = {k: v for k, v in os.environ.items()
               if not k.startswith('CHIMERA_') and k not in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'NO_COLOR')}
    run_env.update(CHIMERA_CONFIG=str(config), TERM='xterm-256color')
    def waiting_run(prompt):
        process = subprocess.Popen([str(binary), '--format', 'json', 'run', prompt], cwd=project,
                                   env={**run_env, 'CHIMERA_WAIT_APPROVAL': '1'},
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        waiters.append(process)
        action, buffered, until = None, '', time.monotonic() + 20
        while time.monotonic() < until and action is None and process.poll() is None:
            ready, _, _ = select.select([process.stdout], [], [], .1)
            if ready:
                buffered += os.read(process.stdout.fileno(), 65536).decode('utf8', 'replace')
                while '\n' in buffered:
                    line, buffered = buffered.split('\n', 1)
                    try:
                        row = json.loads(line)
                    except ValueError:
                        continue
                    if row.get('event') == 'approval_required':
                        action = row['id']
                        break
        check('agent run exposes a pending action for ' + prompt, action is not None)
        return process
    waiter = waiting_run('find the project files')
    send('/permissions\r', 'Autonomy: low — current')
    screen.wait_for('1 action(s) waiting for approval.')
    send('pending approval')
    screen.wait_for('Pending approval: glob')
    send('\r', 'Approve glob once')
    send('\r', 'No actions are waiting for approval.')
    send('\x1b')
    screen.wait_for('Approved glob once.')
    out, err = waiter.communicate(timeout=20)
    (root/'waiter-approve.txt').write_text(out + err)
    check('overlay approval continues the waiting agent run', waiter.returncode == 0)
    check('the approved tool really ran', 'GLOB RAN AFTER THE OVERLAY APPROVAL' in out)
    waiter = waiting_run('write the overlay file')
    send('/permissions\r', 'Autonomy: low — current')
    screen.wait_for('1 action(s) waiting for approval.')
    send('pending approval')
    screen.wait_for('Pending approval: write_file')
    send('\r', 'Deny write_file once')
    send('deny\r', 'No actions are waiting for approval.')
    send('\x1b')
    screen.wait_for('Denied write_file.')
    out, err = waiter.communicate(timeout=20)
    (root/'waiter-deny.txt').write_text(out + err)
    check('overlay denial stops the waiting agent run', waiter.returncode == 1)
    check('denied write never touched the project', not (project/'overlay-denied.txt').exists())
    # The panel refuses to open while this screen has its own request running.
    send('find the project files\r', 'Use /approve or /deny')
    send('/permissions\r', 'before changing permissions')
    check('panel refuses while a request is active', True)
    send('/cancel\r', 'cancelled by user')
    # Esc round trip: open, leave, and find the input focused again.
    send('/permissions\r', 'Autonomy: low — current')
    send('\x1b')
    send('/perm', 'review and change permissions in one place')
    send('\x7f' * len('/perm'))
    screen.finish()
except BaseException as error:
    report['error'] = repr(error)
    raise
finally:
    for item in screens: item.preserve_and_close()
    for process in waiters:
        if process.poll() is None:
            process.kill()
    server.shutdown(); server.server_close()
    (root/'report.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'workspace': str(root), 'checks': len(checks), 'passed': sum(c['pass'] for c in checks), 'error': report.get('error')}), flush=True)
