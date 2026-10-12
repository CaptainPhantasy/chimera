#!/usr/bin/env python3
"""Exercise installation, update, session resume and removal in an isolated home.

This proves isolated-home behavior, not a separate macOS user account or a hosted
provider. The model is a controlled loopback fixture; it makes no paid requests.
All files, transcripts and reports are retained in --work-dir.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import pty
import select
import shutil
import subprocess
import sys
import threading
import time
import uuid


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def snapshot(path):
    return {str(file.relative_to(path)): digest(file) for file in path.rglob('*') if file.is_file()}


def verify(package, work):
    work.mkdir(parents=True, exist_ok=False)
    home = work / 'isolated-home'
    project = work / 'project'
    temp = work / 'tmp'
    for path in (home, project, temp): path.mkdir()
    prefix = home / '.local'
    config = home / 'model-settings.json'
    settings = home / 'personal-setting.txt'
    settings.write_text('preserve this user setting\n')
    requests = []

    class Model(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args): pass
        def do_POST(self):
            value = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            requests.append(value)
            old = any(message.get('content') == 'remember install marker' for message in value['messages'][:-1])
            text = 'previous message retained' if old else 'stored install check'
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for response in [
                {'choices': [{'delta': {'content': text}, 'finish_reason': None}]},
                {'choices': [{'delta': {}, 'finish_reason': 'stop'}]},
            ]:
                self.wfile.write(('data: ' + json.dumps(response) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    config.write_text(json.dumps({'provider': {'name': 'controlled-install-check',
        'base_url': 'http://127.0.0.1:' + str(server.server_port) + '/v1',
        'model': 'controlled-model', 'api_key': 'LOCAL_INSTALL_FIXTURE',
        'request_timeout_secs': 10, 'idle_timeout_secs': 5}}))
    env = {key: value for key, value in os.environ.items() if not key.startswith('CHIMERA_') and key not in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'HOME', 'TMPDIR')}
    env.update(HOME=str(home), TMPDIR=str(temp), CHIMERA_CONFIG=str(config), TERM='xterm-256color')
    checks = []

    def check(name, condition):
        checks.append({'check': name, 'pass': bool(condition)})
        (work / 'checks.json').write_text(json.dumps(checks, indent=2) + '\n')
        if not condition: raise AssertionError(name)

    def install(action, source=package, success=True):
        command = [sys.executable, str(package / 'install.py'), action, '--prefix', str(prefix), '--package', str(source)]
        result = subprocess.run(command, env=env, cwd=project, capture_output=True, text=True, timeout=20)
        (work / ('installer-' + action + '-' + str(uuid.uuid4()) + '.txt')).write_text(result.stdout + result.stderr)
        check(action + (' exit' if success else ' rejected damaged package'), result.returncode == 0 if success else result.returncode != 0)
        return json.loads(result.stdout) if success else None

    def cli(*arguments):
        result = subprocess.run([str(prefix / 'bin/chimera'), '--format', 'json', *arguments], env=env, cwd=project, capture_output=True, text=True, timeout=20)
        check('program ' + ' '.join(arguments), result.returncode == 0)
        return [json.loads(line) for line in result.stdout.splitlines() if line.strip()]

    terminal = None
    master = slave = None
    transcript = bytearray()
    try:
        initial = install('install')
        expected_binary = digest(package / 'bin/chimera')
        check('installed exact packaged bytes', digest(prefix / 'bin/chimera') == expected_binary)
        rows = cli('run', 'remember install marker')
        session = next(row['id'] for row in rows if row.get('event') == 'session')
        before_update = snapshot(project)
        config_hash = digest(config)
        update = install('update')
        check('update preserved project bytes', snapshot(project) == before_update)
        check('update preserved old program', digest(Path(update['quarantine']) / 'chimera') == expected_binary)
        # A bad package must fail before replacing the working installation.
        bad = work / 'tampered-package'
        shutil.copytree(package, bad)
        with (bad / 'bin/chimera').open('ab') as stream: stream.write(b'bad checksum')
        install('update', bad, success=False)
        check('failed update retained working program', digest(prefix / 'bin/chimera') == expected_binary)
        # Resume the actual saved conversation through the installed terminal UI.
        import fcntl
        import struct
        import termios
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        terminal = subprocess.Popen([str(prefix / 'bin/chimera'), 'session', 'resume', session], env=env, cwd=project, stdin=slave, stdout=slave, stderr=slave, preexec_fn=controlling_terminal)
        def read_until(text):
            deadline = time.monotonic() + 15
            while text.encode() not in transcript and time.monotonic() < deadline:
                if select.select([master], [], [], 0.1)[0]:
                    try: transcript.extend(os.read(master, 65536))
                    except OSError: break
            check('resumed terminal shows ' + text, text.encode() in transcript)
        read_until('controlled-model')
        os.write(master, b'what did I say?\r')
        read_until('previous message retained')
        os.write(master, b'/quit\r')
        deadline = time.monotonic() + 15
        while terminal.poll() is None and time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                try: transcript.extend(os.read(master, 65536))
                except OSError: break
        check('resumed terminal closed cleanly', terminal.wait(timeout=1) == 0)
        (work / 'resumed-session.ansi').write_bytes(transcript)
        before_uninstall = snapshot(project)
        removal = install('uninstall')
        check('uninstall removed launch path', not (prefix / 'bin/chimera').exists())
        check('uninstall quarantined exact program', digest(Path(removal['quarantine']) / 'chimera') == expected_binary)
        check('uninstall retained saved conversations', snapshot(project) == before_uninstall)
        check('settings retained', digest(config) == config_hash and settings.read_text() == 'preserve this user setting\n')
        # Reinstalling must make the saved session readable without a migration.
        install('install')
        cli('session', 'export', session)
        install('uninstall')
        return {'pass': True, 'checks': checks, 'session': session, 'work_directory': str(work),
            'verification_boundary': 'isolated HOME on this account; controlled local model; not a fresh OS account or hosted model'}
    finally:
        (work / 'resumed-session.ansi').write_bytes(transcript)
        # Closing the pseudo-terminal also releases a child blocked in terminal
        # output. Always drain during normal exit; never wait on a full pipe.
        for fd in (master, slave):
            if fd is not None: os.close(fd)
        if terminal and terminal.poll() is None:
            terminal.terminate()
            try: terminal.wait(timeout=5)
            except subprocess.TimeoutExpired:
                terminal.kill()
                try: terminal.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    checks.append({'check': 'owned terminal cleanup', 'pass': False, 'pid': terminal.pid})
        server.shutdown()
        server.server_close()
        (work / 'checks.json').write_text(json.dumps(checks, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('package', type=Path)
    parser.add_argument('--work-dir', type=Path, required=True)
    args = parser.parse_args()
    try:
        result = verify(args.package.resolve(strict=True), args.work_dir.resolve())
        (args.work_dir / 'report.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result, sort_keys=True))
    except Exception as error:
        print('Install check failed: ' + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
