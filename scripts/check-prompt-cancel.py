#!/usr/bin/env python3
"""Exercise real approval prompts without a model or executing a shell action.

Use a disposable project and a controlling terminal. Keep the transcripts,
saved approvals and test report. This does not test a model-led conversation.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import termios
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--output-dir', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    if args.output_dir:
        args.output_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='chimera-prompt-check-', dir=args.output_dir)).resolve()
    env = {k: v for k, v in os.environ.items()
           if not k.startswith('CHIMERA_') and k not in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY')}
    config = root / 'settings.json'
    config.write_text(json.dumps({'provider': {'name': 'local', 'model': 'unused',
        'base_url': 'http://127.0.0.1:1/v1', 'request_timeout_secs': 2, 'idle_timeout_secs': 1}}))
    env['CHIMERA_CONFIG'] = str(config)
    report = {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'status': 'running', 'checks': [], 'limits': ['Direct command and checkpoint prompts only; an unavailable provider creates disposable failed sessions. No successful model conversation, shell action or external delivery.']}
    cases = [('immediate-interrupt-' + str(i), b'\x03', 0, 'cancelled') for i in range(5)]
    cases += [('settled-interrupt', b'\x03', 0.2, 'cancelled'),
              ('explicit-denial', b'no\n', 0, 'denied'), ('end-of-input', b'\x04', 0.2, 'denied'),
              ('rollback-immediate-interrupt', b'\x03', 0, 'unchanged'),
              ('rollback-settled-interrupt', b'\x03', 0.2, 'unchanged'),
              ('rollback-declined', b'no\n', 0, 'unchanged')]
    try:
        for name, reply, delay, expected in cases:
            project = root / name
            project.mkdir()
            (project / 'sample.txt').write_text('sample\n')
            rollback = name.startswith('rollback-')
            if rollback:
                result = subprocess.run([str(binary), '--format', 'json', 'run', 'prepare recovery test'],
                    cwd=project, env=env, capture_output=True, text=True, timeout=10)
                assert result.returncode == 1, result.stdout + result.stderr
                session = next(row['id'] for row in map(json.loads, result.stdout.splitlines()) if row.get('event') == 'session')
                created = subprocess.run([str(binary), '--format', 'json', 'checkpoint', 'create',
                    '--session', session, '--file', 'sample.txt'], cwd=project, env=env,
                    capture_output=True, text=True, timeout=10)
                assert created.returncode == 0, created.stdout + created.stderr
                checkpoint = json.loads(created.stdout)['checkpoint']
                (project / 'sample.txt').write_text('later user edit\n')
                session_path = project / '.chimera/sessions' / (session + '.json')
                saved_before = session_path.read_bytes()
                arguments = ['rollback', checkpoint]
                prompt = b'Type yes to restore, or no to leave them unchanged:'
            else:
                defined = subprocess.run([str(binary), '--format', 'json', 'eval', 'define', 'prompt-check',
                    '--file', 'sample.txt', '--command', 'touch should-not-exist.txt'],
                    cwd=project, env=env, capture_output=True, text=True, timeout=10)
                assert defined.returncode == 0, defined.stdout + defined.stderr
                arguments = ['eval', 'run', 'prompt-check']
                prompt = b'Type yes or no:'
            master, slave = pty.openpty()
            before = termios.tcgetattr(master)

            def controlling_terminal():
                os.setsid()
                fcntl.ioctl(0, termios.TIOCSCTTY, 0)

            process = subprocess.Popen([str(binary), *arguments],
                cwd=project, env=env, stdin=slave, stdout=slave, stderr=slave, preexec_fn=controlling_terminal)
            os.close(slave)
            transcript = bytearray()
            row = {'name': name, 'expected_decision': expected, 'pid': process.pid}
            report['checks'].append(row)
            terminal_open = True

            def drain(wait):
                nonlocal terminal_open
                if terminal_open and select.select([master], [], [], wait)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        chunk = b''
                    if chunk:
                        transcript.extend(chunk)
                    else:
                        terminal_open = False

            def wait_with_output(timeout):
                # A real terminal consumes output while the command exits.
                # Leaving a PTY unread can block the command's final report.
                deadline = time.monotonic() + timeout
                while process.poll() is None:
                    if time.monotonic() >= deadline:
                        raise subprocess.TimeoutExpired(process.args, timeout)
                    drain(0.05)
                    if not terminal_open:
                        time.sleep(0.01)
                return process.returncode
            try:
                deadline = time.monotonic() + 10
                while prompt not in transcript:
                    assert time.monotonic() < deadline, 'approval prompt did not appear'
                    if select.select([master], [], [], 0.1)[0]:
                        chunk = os.read(master, 65536)
                        assert chunk, 'terminal closed before the prompt'
                        transcript.extend(chunk)
                if delay:
                    time.sleep(delay)
                os.write(master, reply)
                # Prompt cancellation/denial must exit without an extra newline.
                wait_with_output(5)
                row['exit_code'] = process.returncode
                assert process.returncode == 1, row
                while terminal_open and select.select([master], [], [], 0.05)[0]:
                    drain(0)
                expected_message = (b'restore cancelled; nothing changed' if rollback else
                    b'action cancelled; nothing executed' if expected == 'cancelled' else
                    b'action denied or expired; nothing executed')
                assert expected_message in transcript, 'expected prompt outcome missing from terminal output'
                if rollback:
                    assert (project / 'sample.txt').read_text() == 'later user edit\n', 'cancelled restore overwrote a user edit'
                    assert session_path.read_bytes() == saved_before, 'cancelled restore changed saved conversation'
                else:
                    record = json.loads((project / '.chimera/approvals/actions.json').read_text())['data']
                    row['decisions'] = [a['decision'] for a in record]
                    assert row['decisions'] == [expected], row
                assert not (project / 'should-not-exist.txt').exists(), 'unapproved command ran'
                assert termios.tcgetattr(master) == before, 'terminal settings changed after exit'
                row['passed'] = True
            finally:
                try:
                    if process.poll() is None:
                        row['cleanup_needed'] = True
                        # Release the old stdin bug before stopping this owned test process.
                        os.write(master, b'\n')
                        try:
                            wait_with_output(2)
                        except subprocess.TimeoutExpired:
                            row['cleanup_terminated'] = True
                            process.terminate()
                            try:
                                wait_with_output(2)
                            except subprocess.TimeoutExpired:
                                row['cleanup_killed'] = True
                                process.kill()
                                wait_with_output(2)
                    while terminal_open and select.select([master], [], [], 0.05)[0]:
                        drain(0)
                except Exception as error:
                    row['cleanup_error'] = str(error)
                finally:
                    row['final_exit_code'] = process.poll()
                    os.close(master)
                    (project / 'terminal.txt').write_bytes(transcript)
        report['status'] = 'passed'
        return 0
    except Exception as error:
        report['status'] = 'failed'
        report['error'] = str(error)
        return 1
    finally:
        path = root / 'report.json'
        path.write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({'status': report['status'], 'checks': len(report['checks']), 'report': str(path)}))


if __name__ == '__main__':
    raise SystemExit(main())
