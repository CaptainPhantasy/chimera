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
    def __init__(self, binary, cwd, config, arguments, check, row, profile='read-only'):
        super().__init__(binary, cwd, config, arguments, check, row, profile=profile)

    def visible(self):
        terminal = pyte.Screen(120, 32)
        pyte.Stream(terminal).feed(bytes(self.output).decode('utf8', 'replace'))
        return '\n'.join(terminal.display)

    def wait_for(self, marker, offset=0, seconds=10):
        deadline = time.monotonic() + seconds
        while True:
            self.drain(.04)
            visible = self.visible()
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
        calls.append({'method': 'POST', 'path': self.path,
                      'authorized': self.headers.get('Authorization') == 'Bearer UI_TEST_KEY'})
        if self.headers.get('Authorization') != 'Bearer UI_TEST_KEY':
            self.send_response(401); self.end_headers(); self.wfile.write(b'{"error":"do not echo service secrets"}'); return
        if not self.path.endswith('/chat/completions'):
            self.send_response(500); self.end_headers(); return
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        last = body['messages'][-1]
        prompt = next((m['content'] for m in reversed(body['messages']) if m['role'] == 'user'), '')
        reply = None
        if last['role'] == 'tool' and 'matches' in str(last['content']):
            reply = {'choices': [{'delta': {'content': 'GLOB RAN AFTER THE OVERLAY APPROVAL'}, 'finish_reason': None}]}
        elif last['role'] == 'tool' and 'denied' in str(last['content']):
            reply = {'choices': [{'delta': {'content': 'THE OVERLAY DENIED THIS WRITE'}, 'finish_reason': None}]}
        elif prompt == 'find the project files':
            reply = {'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'glob-1', 'type': 'function', 'function': {'name': 'glob', 'arguments': json.dumps({'pattern': '*', 'path': '.'})}}]}, 'finish_reason': 'tool_calls'}]}
        elif prompt == 'write the overlay file':
            reply = {'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'write-1', 'type': 'function', 'function': {'name': 'write_file', 'arguments': json.dumps({'path': 'overlay-denied.txt', 'content': 'must stay absent\n'})}}]}, 'finish_reason': 'tool_calls'}]}
        elif prompt == 'use the echo tool':
            with completion_lock:
                echo_completions['count'] += 1
                echo_n = echo_completions['count']
            if echo_n == 1:
                reply = {'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'call-external-1', 'type': 'function', 'function': {'name': 'mcp__reachable-fixture__echo', 'arguments': '{"query":"hello"}'}}]}, 'finish_reason': 'tool_calls'}]}
            else:
                reply = {'choices': [{'delta': {'content': 'external echo answered hello'}, 'finish_reason': None}]}
        if reply is not None:
            self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.end_headers()
            def data(value):
                self.wfile.write(('data: ' + json.dumps(value) + '\n\n').encode()); self.wfile.flush()
            data(reply)
            if reply['choices'][0]['delta'].get('content') is not None:
                data({'choices': [{'delta': {}, 'finish_reason': 'stop'}]})
                data({'choices': [], 'usage': {'prompt_tokens': 9, 'completion_tokens': 4, 'total_tokens': 13}})
            self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush()
            return
        with completion_lock:
            completions.append([m.get('role') for m in body.get('messages', [])])
            index = len(completions) - 1
        events = SCRIPTED_COMPLETIONS[index] if index < len(SCRIPTED_COMPLETIONS) else text_events('Fixture reply for the smoke check.')
        self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.end_headers()
        for item in events:
            self.wfile.write(('data: ' + json.dumps(item) + '\n\n').encode())
        self.wfile.write(b'data: [DONE]\n\n')
        self.wfile.flush()

def tool_call_events(name, arguments):
    call = {'index': 0, 'id': 'call-' + name, 'type': 'function',
            'function': {'name': name, 'arguments': json.dumps(arguments)}}
    return [
        {'choices': [{'delta': {'tool_calls': [call]}, 'finish_reason': None}]},
        {'choices': [{'delta': {}, 'finish_reason': 'tool_calls'}]},
        {'choices': [], 'usage': {'prompt_tokens': 5, 'completion_tokens': 3, 'total_tokens': 8}},
    ]

def text_events(text):
    return [
        {'choices': [{'delta': {'content': text}, 'finish_reason': None}]},
        {'choices': [{'delta': {}, 'finish_reason': 'stop'}]},
        {'choices': [], 'usage': {'prompt_tokens': 5, 'completion_tokens': 3, 'total_tokens': 8}},
    ]

# Two editing turns for the /rewind flow: turn one writes note.txt, turn two
# writes also-edited.txt, so the rewind list holds two ordered snapshots.
SCRIPTED_COMPLETIONS = [
    tool_call_events('write_file', {'path': 'note.txt', 'content': 'agent version one'}),
    text_events('NOTE_FILE_IS_WRITTEN_FOR_REWIND'),
    tool_call_events('write_file', {'path': 'also-edited.txt', 'content': 'second turn file'}),
    text_events('SECOND_FILE_IS_WRITTEN_FOR_REWIND'),
]
completions, completion_lock = [], threading.Lock()
echo_completions = {'count': 0}

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
waiters = []
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

    # /rewind — automatic per-turn snapshots, exact preview, confirmed restore.
    # A supervised screen is required so the agent may really edit files.
    rewind_row = {}
    screen = RenderedScreen(binary, project, config, ['tui'], check, rewind_row, profile='supervised'); screens.append(screen)
    screen.wait_for('model-beta', seconds=20)
    state_dir = project/'.chimera'/'sessions'

    def checkpoints():
        return sorted((state_dir/'checkpoints').glob('*.json'))

    def wait_until(condition, seconds=20):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if condition(): return
            screen.drain(.05)
        raise setup.ObservationTimeout('a rewind precondition was not reached')

    def open_rewind():
        # A just-finished run may still be joining; retry the guarded command.
        for _ in range(10):
            send('/rewind\r')
            try:
                screen.wait_for('Rewind files', seconds=2)
                break
            except setup.ObservationTimeout:
                screen.drain(.2)
        else:
            raise setup.ObservationTimeout('the rewind overlay did not open')
        # The busy panel swallows keys; wait until the searchable list exists.
        screen.wait_for('Type to search', seconds=10)

    check('no snapshots exist before any turn edits a file', not checkpoints())
    send('/rewind\r', 'No snapshots exist yet')
    screen.wait_for('is taken automatically')
    send('\x1b')
    # Turn one: the fixture model writes note.txt; approve it like a real user.
    send('/run write the note file\r')
    screen.wait_for('Approval ', seconds=20)
    send('/approve\r')
    screen.wait_for('NOTE_FILE_IS_WRITTEN_FOR_REWIND', seconds=20)
    wait_until(lambda: len(checkpoints()) == 1)
    check('a turn that edits a file leaves one automatic snapshot', len(checkpoints()) == 1)
    screen.drain(.3)
    # Turn two: a second edited file so the rewind list can be ordered.
    send('/run write a second file\r')
    screen.wait_for('also-edited.txt', seconds=20)
    send('/approve\r')
    screen.wait_for('SECOND_FILE_IS_WRITTEN_FOR_REWIND', seconds=20)
    wait_until(lambda: len(checkpoints()) == 2)
    screen.drain(.3)
    # The default selection previews the newest snapshot first.
    open_rewind()
    screen.wait_for('note.txt')
    send('\r')
    screen.wait_for('Confirm restore')
    newest = screen.visible()
    check('the newest snapshot is selected by default', 'also-edited.txt' in newest)
    send('\x1b')
    open_rewind()
    send('note')
    screen.wait_for('Search: note')
    send('\r')
    screen.wait_for('Confirm restore')
    older = screen.visible()
    check('search selects the older note.txt snapshot', 'note.txt' in older and 'Restoring rewrites 1 file(s)' in older)
    check('the confirm names the archived current versions', 'private archive' in older)
    # An edit after the preview makes exactly this preview stale.
    (project/'note.txt').write_text('edited after the preview')
    send('\r')
    screen.wait_for('file changed since restore preview')
    check('a stale preview refuses to overwrite newer work', (project/'note.txt').read_text() == 'edited after the preview')
    send('\x1b')
    open_rewind()
    send('note')
    screen.wait_for('Search: note')
    send('\r')
    screen.wait_for('Confirm restore')
    send('\r')
    screen.wait_for('Files restored from the snapshot')
    check('restore rewrites the file exactly as snapshotted', (project/'note.txt').read_text() == 'agent version one')
    check('the untouched snapshot file is left alone', (project/'also-edited.txt').read_text() == 'second turn file')
    archives = sorted((state_dir/'restores').glob('*'))
    check('the refused restore archived nothing', len(archives) == 1)
    originals = list(archives[0].glob('*.original'))
    check('displaced contents are preserved in a private archive', len(originals) == 1 and originals[0].read_text() == 'edited after the preview')
    check('the restore journaled and completed', (archives[0]/'journal.json').exists() and (archives[0]/'completed.json').exists())
    restored_view = screen.visible()
    check('the conversation returns to the snapshot point in place', 'SECOND_FILE_IS_WRITTEN_FOR_REWIND' not in restored_view and 'NOTE_FILE_IS_WRITTEN_FOR_REWIND' in restored_view)
    # /memory and /remember — browse, search, edit and archive saved project
    # facts; remembering writes only after the explicit confirmation form.
    send('/memory\r', 'Project memory')
    screen.wait_for('No saved project facts yet.')
    send('\x1b')
    # Esc must close the overlay: the slash command can only reach a focused input.
    send('/memory\r', 'No saved project facts yet.')
    check('cancelling project memory returns to an interactive chat', True)
    send('\x1b')
    send('/remember check the deploy tags\r', 'Remember a fact')
    memory_record = project/'.chimera'/'memory'/'items.json'
    check('remember confirms before anything is written', not memory_record.exists())
    send('deploy-tags\r')
    send('\r', 'Remembered deploy-tags')
    screen.wait_for('Search: ')
    stored = json.loads(memory_record.read_text())['data']
    check('confirmed remember writes a shared project fact',
          len(stored['items']) == 1 and stored['items'][0]['key'] == 'deploy-tags'
          and stored['items'][0]['content'] == 'check the deploy tags'
          and stored['items'][0]['session_id'] is None)
    send('\r', 'Edit text')
    send('\r', 'Edit fact')
    send('\t')
    send(' v2')
    send('\r', 'Saved deploy-tags')
    screen.wait_for('check the deploy tags v2')
    stored = json.loads(memory_record.read_text())['data']
    check('editing persists the new text and keeps the previous version',
          stored['items'][0]['content'] == 'check the deploy tags v2'
          and any(entry['item']['content'] == 'check the deploy tags' for entry in stored['archived']))
    send('\r', 'Edit text')
    send('\x1b[B\r', 'Archive this fact?')
    send('\r', 'Archived.')
    screen.wait_for('No matches. Backspace to change the search.')
    stored = json.loads(memory_record.read_text())['data']
    check('archiving hides the fact but keeps its history',
          len(stored['items']) == 0
          and any(entry['item']['key'] == 'deploy-tags' for entry in stored['archived']))
    send('\x1b')
    send('/memory\r', 'Project memory')
    screen.wait_for('No saved project facts yet.')
    check('reopening project memory shows the archived-out empty list', True)
    send('\x1b')
    # A completed request is the source for capture-from-current-conversation.
    skills_posts_before = sum(
        1 for c in calls
        if c['method'] == 'POST' and c['path'].endswith('/chat/completions'))
    send('summarize the smoke check\r', 'Answer saved.')
    check('the chat request reached the fixture service', any(
        c['method'] == 'POST' and c['path'].endswith('/chat/completions') and c['authorized']
        for c in calls))
    # /skills — an empty project explains itself and offers capture; Esc
    # returns to the untouched chat input.
    send('/skills\r', 'No saved procedures yet')
    send('\x1b')
    screen.wait_for('Describe a task')
    check('closing the procedure list returns to the chat input', True)
    # Capture this conversation under an explicit name and description.
    send('/skills\r', 'Saved procedures')
    screen.wait_for('No saved procedures yet')
    send('\r', 'Name the procedure')
    send('smoke-check\r')
    send('Checks the smoke path end to end\r', "Saved 'smoke-check'")
    screen.wait_for('Checks the smoke path end to end')
    send('\x1b')
    # The saved procedure survives reopening the overlay from disk.
    send('/skills\r', 'Saved procedures')
    screen.wait_for('smoke-check')
    check('the saved procedure is offered after reopening /skills', True)
    # Search, confirm, and run it as an ordinary request in this conversation.
    send('smoke')
    screen.wait_for('Search: smoke')
    send('\r', "Run 'smoke-check'")
    send('\r', "Run the saved workflow 'smoke-check'")
    screen.wait_for('Answer saved.')
    check('running a procedure sends a real request with approvals', sum(
        1 for c in calls
        if c['method'] == 'POST' and c['path'].endswith('/chat/completions')) >= 2)
    # An unknown procedure name is refused by name, with nothing sent.
    send('/skills smoke-check\r', "Run the saved workflow 'smoke-check'")
    screen.wait_for('Answer saved.')
    send('/skills no-such-procedure\r', 'Could not run the saved procedure')
    check('an unknown procedure name is refused without sending', sum(
        1 for c in calls
        if c['method'] == 'POST' and c['path'].endswith('/chat/completions')) - skills_posts_before == 3)
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
          echo_completions['count'] >= 2)
    # /jobs — create, pause and stop background work through the real scheduler.
    send('/jobs\r', 'Background work')
    screen.wait_for('No background jobs exist yet')
    screen.wait_for('Service: not running')
    send('\x1b')
    screen.wait_for('ready to send')
    check('closing the job panel returns to the conversation', True)
    send('/jobs\r', 'Background work')
    send('Describe')
    screen.wait_for('Search: Describe')
    send('\r', 'New background job')
    send('Nightly digest\r')
    send('Summarize the project state\r')
    send('\x15every:3600\r')
    send('\r', 'Saved job')
    schedules = project/'.chimera'/'jobs'/'schedules.json'
    saved_jobs = json.loads(schedules.read_text())['data']['workflows']
    check('created job is saved by the real scheduler', len(saved_jobs) == 1)
    job = next(iter(saved_jobs.values()))
    check('saved job keeps its objective and schedule', job['handle']['spec']['objective'] == 'Summarize the project state' and job['handle']['spec']['frequency'] == {'type': 'interval', 'every_secs': 3600})
    send('\r', 'Pause this job')
    send('\r')
    screen.wait_for('is paused')
    saved_jobs = json.loads(schedules.read_text())['data']['workflows']
    check('pause reaches the real scheduler', next(iter(saved_jobs.values()))['handle']['status'] == 'paused')
    send('\r', 'Start this job again')
    send('stop this')
    screen.wait_for('Search: stop this')
    send('\r', 'Stop this job?')
    check('stopping asks for confirmation first', True)
    send('permanently')
    screen.wait_for('Search: permanently')
    send('\r', 'stopped permanently')
    saved_jobs = json.loads(schedules.read_text())['data']['workflows']
    check('confirmed stop cancels the job in the scheduler', next(iter(saved_jobs.values()))['handle']['status'] == 'cancelled')
    send('\x1b')
    screen.wait_for('ready to send')
    check('leaving the job panel keeps the conversation intact', True)
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
