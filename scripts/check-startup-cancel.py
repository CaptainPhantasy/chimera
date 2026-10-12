#!/usr/bin/env python3
"""Check immediate startup interrupts without a model or local listener.

The deliberately unavailable provider may fail before cancellation is handled.
Either saved failure or saved cancellation is valid; abrupt signal death,
missing final output and active saved work are not. Stalled-request cancellation
is checked separately by check-user-flows.py with its controlled local server.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--output-dir', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    if args.output_dir:
        args.output_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='chimera-startup-cancel-', dir=args.output_dir))
    config = root / 'settings.json'
    config.write_text(json.dumps({'provider': {'name': 'local', 'model': 'unused',
        'base_url': 'http://127.0.0.1:1/v1', 'request_timeout_secs': 2, 'idle_timeout_secs': 1}}))
    env = {k: v for k, v in os.environ.items()
           if not k.startswith('CHIMERA_') and k not in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY')}
    env['CHIMERA_CONFIG'] = str(config)
    report = {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'status': 'running', 'checks': [], 'limits': ['Provider failure can win the startup race; this does not prove stalled-request cancellation.']}
    try:
        for command in ('run', 'swarm'):
            for index in range(20):
                project = root / (command + '-' + str(index))
                project.mkdir()
                process = subprocess.Popen([str(binary), '--format', 'json', command, 'cancel at startup'],
                    cwd=project, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    # Retain bytes following the identifier without delaying the signal.
                    first = bytearray()
                    while b'\n' not in first:
                        if not select.select([process.stdout], [], [], 10)[0]:
                            raise AssertionError('no initial identifier before startup test deadline')
                        chunk = os.read(process.stdout.fileno(), 65536)
                        if not chunk:
                            raise AssertionError('process ended before publishing its identifier')
                        first.extend(chunk)
                        if len(first) > 100_000:
                            raise AssertionError('initial output exceeded 100000 bytes')
                    line, _, remainder = first.partition(b'\n')
                    initial = json.loads(line)
                    process.send_signal(signal.SIGINT)
                    stdout, stderr = process.communicate(timeout=10)
                    rows = [json.loads(line) for line in (remainder + stdout).splitlines()]
                    row = {'command': command, 'index': index, 'exit_code': process.returncode}
                    report['checks'].append(row)
                    assert process.returncode == 1, row
                    if command == 'run':
                        state = json.loads((project / '.chimera/sessions' / (initial['id'] + '.json')).read_text())['data']
                        outcomes = [e['payload']['outcome'] for e in state['events'] if e.get('key') == 'run_outcome']
                        row.update(active=state['active'], outcome=outcomes[-1] if outcomes else None)
                        assert not state['active'] and row['outcome'] in ('cancelled', 'failed'), row
                        assert any(r.get('success') is False and
                                   (r.get('event') == 'result' or isinstance(r.get('error'), str) and r['error'])
                                   for r in rows), rows
                    else:
                        final = next((r['team'] for r in rows if isinstance(r.get('team'), dict)), None)
                        assert final is not None, rows
                        row.update(outcome=final['status'])
                        assert final['status'] in ('cancelled', 'failed'), row
                        assert all(m['status'] not in ('queued', 'running') for m in final['members']), final
                    row['passed'] = True
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
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
