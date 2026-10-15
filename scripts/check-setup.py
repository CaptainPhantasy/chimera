#!/usr/bin/env python3
"""Check interactive setup and recovery against the actual CHIMERA executable.

Only a disposable workspace and a controlled loopback model are used. No user
settings or credentials are read. All transcripts and saved sessions are kept.
This checks setup behavior, not a real model service or full beta readiness.
"""
import argparse
import fcntl
import hashlib
import http.server
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import threading
import time

NEEDED = 'Model setup needed.'
RELOADED = 'Model settings reloaded. Send your request when ready.'
RELOAD_FAILED = 'Could not reload model settings. Fix the settings file, then use /reload.'
ANSWER = 'SETUP_RECOVERY_ANSWER'
SENTINEL = 'FAKE_SETUP_SECRET_MUST_NOT_APPEAR'


class ObservationTimeout(Exception):
    pass


class Screen:
    def __init__(self, binary, cwd, config, arguments, check, row, profile='read-only'):
        self.check, self.row = check, row
        self.master, slave = pty.openpty()
        self.original = termios.tcgetattr(self.master)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 120, 0, 0))
        env = {k: v for k, v in os.environ.items()
               if not k.startswith('CHIMERA_') and k not in
               ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'NO_COLOR')}
        env.update(CHIMERA_CONFIG=str(config), TERM='xterm-256color', COLORTERM='truecolor')

        def control():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        self.argv = [str(binary), '--profile', profile, *arguments]
        self.process = subprocess.Popen(self.argv, cwd=cwd, env=env,
                                        stdin=slave, stdout=slave, stderr=slave,
                                        preexec_fn=control)
        os.close(slave)
        self.output, self.events = bytearray(), []
        self.closed = False
        self.started = time.monotonic()
        self.directory = cwd
        row.update(argv=self.argv, working_directory=str(cwd), pid=self.process.pid)

    def drain(self, seconds=.05):
        if self.closed:
            return
        if select.select([self.master], [], [], seconds)[0]:
            try:
                data = os.read(self.master, 65536)
            except OSError:
                data = b''
            if data:
                self.output.extend(data)
                self.events.append({'at_seconds': round(time.monotonic() - self.started, 3),
                                    'output': data.decode('utf8', 'replace')})
            else:
                self.closed = True

    def wait_for(self, marker, offset=0, seconds=10):
        def visible():
            # Cursor moves may split a wrapped word in the byte stream.
            return re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', self.output[offset:]).replace('  ℹ '.encode(), b'')
        deadline = time.monotonic() + seconds
        while marker.encode() not in visible():
            self.drain()
            if self.process.poll() is not None:
                self.drain(0)
                self.check('screen shows ' + marker, marker.encode() in visible())
                return
            if time.monotonic() >= deadline:
                raise ObservationTimeout('Still waiting for screen text: ' + marker)
        self.check('screen shows ' + marker, True)

    def send(self, text):
        offset = len(self.output)
        os.write(self.master, text.encode())
        return offset

    def refresh(self):
        offset = self.send('\x1bOP')
        self.wait_for('CHIMERA controls', offset)
        offset = self.send('\x1b')
        self.wait_for('CHIMERA', offset)
        return offset

    def clear_input(self, text):
        # Home and Delete are real supported keys; no shell editing shortcuts.
        self.send('\x1b[H' + '\x1b[3~' * len(text))

    def finish(self):
        self.send('/quit\r')
        deadline = time.monotonic() + 10
        while self.process.poll() is None:
            self.drain()
            if time.monotonic() >= deadline:
                raise ObservationTimeout('Interactive program has not exited after /quit')
        self.drain(0)
        self.row['exit_code'] = self.process.returncode
        self.check('quit exits successfully', self.process.returncode == 0)
        self.check('screen is restored on exit', b'\x1b[?1049l' in self.output)
        try:
            restored = termios.tcgetattr(self.master) == self.original
            self.row['terminal_settings_restored'] = restored
            self.check('terminal settings restored', restored)
        except termios.error as error:
            self.row['terminal_settings_readback_unavailable'] = str(error)

    def preserve_and_close(self):
        try:
            if self.process.poll() is None:
                self.row['cleanup_needed'] = True
                self.process.terminate()
                deadline = time.monotonic() + 3
                while self.process.poll() is None and time.monotonic() < deadline:
                    self.drain()
                if self.process.poll() is None:
                    self.row['cleanup_killed_owned_process'] = True
                    self.process.kill()
                    self.process.wait(timeout=3)
            self.drain(0)
        finally:
            self.row['final_exit_code'] = self.process.poll()
            (self.directory / 'terminal.ansi').write_bytes(self.output)
            (self.directory / 'terminal-events.json').write_text(
                json.dumps(self.events, ensure_ascii=False) + '\n')
            self.row['transcript'] = str(self.directory / 'terminal.ansi')
            os.close(self.master)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='chimera-setup-', dir=args.output_dir)).resolve()
    report = {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'status': 'running', 'workspace': str(root), 'checks': [], 'cases': [],
              'limits': ['Controlled local model only; no real provider, paid request, normal launcher, or installation tested.',
                         'An observation timeout is inconclusive, not proof the application has failed.']}
    report_path = root / 'report.json'
    requests, lock = [], threading.Lock()
    screens = []
    server = None
    current_case = 'initialization'

    def save():
        report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')

    def check(name, passed):
        report['checks'].append({'case': current_case, 'check': name, 'passed': bool(passed)})
        save()
        if not passed:
            raise AssertionError(current_case + ': ' + name)

    def count():
        with lock:
            return len(requests)

    class Model(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            with lock:
                requests.append(body)
                reply = ANSWER + '_' + str(len(requests))
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for item in [
                {'choices': [{'delta': {'content': reply}, 'finish_reason': None}]},
                {'choices': [{'delta': {}, 'finish_reason': 'stop'}]},
                {'choices': [], 'usage': {'prompt_tokens': 5, 'completion_tokens': 3, 'total_tokens': 8}},
            ]:
                self.wfile.write(('data: ' + json.dumps(item) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()

    def local_settings(path):
        path.write_text(json.dumps({'provider': {'name': 'controlled-setup',
            'base_url': 'http://127.0.0.1:' + str(server.server_port) + '/v1',
            'model': 'controlled-setup-model', 'api_key': SENTINEL,
            'request_timeout_secs': 5, 'idle_timeout_secs': 3}}) + '\n')

    def start(name, arguments, contents='{}'):
        nonlocal current_case
        current_case = name
        directory = root / name
        directory.mkdir()
        config = directory / 'settings.json'
        config.write_text(contents + '\n')
        row = {'name': name}
        report['cases'].append(row)
        screen = Screen(binary, directory, config, arguments, check, row)
        screens.append(screen)
        return screen, config, directory

    def complete_request(screen, directory, prompt, before, offset):
        # A redraw can contain an older "Answer saved." notice. First observe
        # this request at the model, then its unique answer and saved outcome.
        deadline = time.monotonic() + 10
        while count() <= before:
            screen.drain()
            check_running = screen.process.poll() is None
            if not check_running:
                check('program remains open until model receives request', False)
            if time.monotonic() >= deadline:
                raise ObservationTimeout('Model has not received current objective: ' + prompt)
        check('exactly one model request sent', count() == before + 1)
        with lock:
            last = requests[-1]
        user = [m.get('content') for m in last['messages'] if m.get('role') == 'user']
        check('explicit objective reaches model intact', user[-1] == prompt)
        answer = ANSWER + '_' + str(before + 1)
        screen.wait_for(answer, offset)

        def saved_current_result():
            for path in (directory / '.chimera' / 'sessions').glob('*.json'):
                data = json.loads(path.read_text())['data']
                events = data['events']
                starts = [e['payload'] for e in events if e.get('key') == 'run_started']
                results = [e['payload'] for e in events if e.get('key') == 'run_result']
                outcomes = [e['payload'] for e in events if e.get('key') == 'run_outcome']
                if not starts or not results or not outcomes:
                    continue
                result = results[-1]
                messages = [m for m in result.get('messages', [])
                            if m.get('role') in ('user', 'assistant')]
                if (not data['active'] and starts[-1].get('objective') == prompt
                        and outcomes[-1].get('outcome') == 'completed'
                        and result.get('done_reason') == 'final_answer'
                        and result.get('final_answer') == answer
                        and len(messages) >= 2
                        and messages[-2].get('role') == 'user'
                        and messages[-2].get('content') == prompt
                        and messages[-1].get('role') == 'assistant'
                        and messages[-1].get('content') == answer):
                    return True
            return False

        deadline = time.monotonic() + 10
        while not saved_current_result():
            screen.drain()
            if screen.process.poll() is not None:
                check('program remains open until current answer is saved', False)
            if time.monotonic() >= deadline:
                raise ObservationTimeout('Current objective and answer are not yet saved: ' + prompt)
        check('current objective and answer saved with completed outcome', True)
        screen.wait_for('Answer saved.', offset)

    try:
        # The operating system selects a free temporary loopback port.
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
        report['controlled_model_address'] = '127.0.0.1:' + str(server.server_port)
        screen, config, directory = start('empty-settings', ['tui'])
        screen.wait_for(NEEDED)
        check('startup sends no model request', count() == 0)
        offset = screen.send('/setup\r')
        screen.wait_for('Connect a provider', offset)
        screen.send('\x1b')
        until = time.monotonic() + .25
        while time.monotonic() < until: screen.drain(.03)
        offset = screen.send('/help\r')
        screen.wait_for('CHIMERA controls', offset)
        offset = screen.send('\x1b')
        screen.wait_for(NEEDED, offset)
        prompt = 'remember setup recovery'
        offset = screen.send(prompt + '\r')
        screen.wait_for(NEEDED, offset)
        retained = 'editable ' + prompt
        screen.send('\x1b[Heditable ')
        offset = screen.refresh()
        screen.wait_for(retained, offset)
        check('blocked request sends no model request', count() == 0)
        screen.clear_input(retained)
        local_settings(config)
        offset = screen.send('/reload\r')
        screen.wait_for(RELOADED, offset)
        check('reload does not automatically send a request', count() == 0)
        offset = screen.send(prompt + '\r')
        complete_request(screen, directory, prompt, 0, offset)
        # A bad reload must prevent new work even after a previous success.
        config.write_text('{broken settings\n')
        offset = screen.send('/reload\r')
        screen.wait_for(RELOAD_FAILED, offset)
        blocked = 'wait for repaired settings'
        before = count()
        offset = screen.send(blocked + '\r')
        screen.wait_for(NEEDED, offset)
        check('failed reload blocks new model work', count() == before)
        screen.clear_input(blocked)
        local_settings(config)
        offset = screen.send('/reload\r')
        screen.wait_for(RELOADED, offset)
        offset = screen.send(blocked + '\r')
        complete_request(screen, directory, blocked, before, offset)
        screen.finish()

        # Keep a fake secret in the invalid field to catch error-chain leakage.
        malformed = json.dumps({'provider': {'max_tokens': SENTINEL, 'api_key': SENTINEL}})
        screen, config, _ = start('malformed-settings', ['tui'], malformed)
        before = count()
        screen.wait_for(NEEDED)
        offset = screen.send('/setup\r')
        screen.wait_for('Connect a provider', offset)
        screen.send('\x1b')
        until = time.monotonic() + .25
        while time.monotonic() < until: screen.drain(.03)
        screen.finish()
        check('malformed startup sends no model request', count() == before)
        check('malformed settings never reveal secret value', SENTINEL.encode() not in screen.output)

        for name, arguments, prompt in [
            ('tui-objective', ['tui', 'initial screen objective'], 'initial screen objective'),
            ('interactive-run', ['run', '--interactive', 'initial run objective'], 'initial run objective'),
        ]:
            screen, config, directory = start(name, arguments)
            before = count()
            screen.wait_for(NEEDED)
            retained = 'editable ' + prompt
            screen.send('\x1b[Heditable ')
            offset = screen.refresh()
            screen.wait_for(retained, offset)
            check('initial objective does not run without settings', count() == before)
            screen.clear_input(retained)
            local_settings(config)
            offset = screen.send('/reload\r')
            screen.wait_for(RELOADED, offset)
            check('reload leaves initial objective unsent', count() == before)
            # Recall /reload, then the initial objective. Do not retype it.
            offset = screen.send('\x1b[A\x1b[A\r')
            complete_request(screen, directory, prompt, before, offset)
            screen.finish()

        # Resume a real saved conversation produced above, with settings missing.
        current_case = 'resume-without-key'
        directory = root / 'empty-settings'
        session = next((directory / '.chimera' / 'sessions').glob('*.json'))
        config = directory / 'settings.json'
        config.write_text('{}\n')
        row = {'name': current_case}
        report['cases'].append(row)
        before = count()
        screen = Screen(binary, directory, config, ['session', 'resume', session.stem], check, row)
        # Preserve this distinct launch alongside the original transcript.
        screen.directory = root / 'resume-without-key'
        screen.directory.mkdir()
        screens.append(screen)
        screen.wait_for(NEEDED)
        screen.wait_for(ANSWER)
        check('resume displays saved history without calling a model', count() == before)
        screen.finish()
        for screen in screens:
            check('no setup screen reveals the fake secret', SENTINEL.encode() not in screen.output)
        check('binary unchanged during checks', hashlib.sha256(binary.read_bytes()).hexdigest() == report['binary_sha256'])
        report['status'] = 'passed'
    except ObservationTimeout as error:
        report['status'], report['error'] = 'inconclusive', str(error)
    except Exception as error:
        report['status'], report['error'] = 'failed', str(error)
    finally:
        for screen in screens:
            try:
                screen.preserve_and_close()
            except Exception as error:
                report.setdefault('cleanup_errors', []).append(str(error))
                report['status'] = 'failed'
        if server:
            server.shutdown()
            server.server_close()
        report['controlled_model_requests'] = count()
        (root / 'controlled-model-requests.json').write_text(json.dumps(requests, indent=2) + '\n')
        save()
        print(json.dumps({'status': report['status'], 'checks': len(report['checks']),
                          'model_requests': count(), 'report': str(report_path)}), flush=True)
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
