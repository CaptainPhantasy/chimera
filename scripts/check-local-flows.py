#!/usr/bin/env python3
"""Check installed CHIMERA controls that need no model, browser or listener.

Every command runs in a disposable project. Records and a report are retained.
These checks do not prove model execution, background execution or delivery.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import shutil
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
    work = Path(tempfile.mkdtemp(prefix='chimera-local-flows-', dir=args.output_dir))
    project, other = work / 'project', work / 'other-project'
    project.mkdir(); other.mkdir()
    config = work / 'settings.json'
    env = {k: v for k, v in os.environ.items()
           if not k.startswith('CHIMERA_') and k not in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY')}
    env['CHIMERA_CONFIG'] = str(config)
    report = {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'work': str(work), 'status': 'running', 'checks': [],
              'limits': ['No model, background service, browser or message delivery was exercised.']}

    def cli(*arguments, cwd=project, success=True):
        result = subprocess.run([str(binary), '--format', 'json', *arguments], cwd=cwd,
                                env=env, capture_output=True, text=True, timeout=15)
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        if (result.returncode == 0) != success:
            raise AssertionError({'command': arguments, 'exit': result.returncode,
                                  'stdout': result.stdout, 'stderr': result.stderr})
        if not rows:
            raise AssertionError('no structured result: ' + repr(arguments))
        if not success and rows[-1].get('success') is not False:
            raise AssertionError('failure was not reported: ' + repr(rows))
        return rows[-1]

    def check(name, passed):
        report['checks'].append({'name': name, 'passed': bool(passed)})
        if not passed:
            raise AssertionError(name)

    try:
        check('initialization creates the selected settings file',
              Path(cli('init')['config']) == config and config.is_file())
        # Represent existing user settings, including their exact whitespace.
        settings = '{\n  "provider": {"model": "chosen-local", "base_url": "http://127.0.0.1:1/v1"}\n}\n'
        config.write_text(settings)
        cli('init')
        check('initialization preserves existing settings byte for byte', config.read_bytes() == settings.encode())
        check('diagnostics do not claim a model connection was tested', cli('doctor')['connection_tested'] is False)
        check('invalid global policy is rejected', 'unknown policy' in cli('--profile', 'unknown', 'status', success=False)['error'])
        check('interactive screen rejects structured-output mode', 'requires --format human' in cli('tui', success=False)['error'])

        first = cli('memory', 'remember', 'fruit', 'apricot café 🦋')['memory']
        check('memory show returns the saved international text after restart', cli('memory', 'show', first)['content'] == 'apricot café 🦋')
        cli('memory', 'remember', 'fruit', 'apricot café 🦋')
        distinct = cli('memory', 'remember', 'fruit', 'pear')['memory']
        cli('memory', 'compact')
        facts = cli('memory', 'list')['memory']
        check('compaction keeps distinct facts and combines exact duplicates',
              sorted(row['content'] for row in facts) == ['apricot café 🦋', 'pear'])
        record_path = project / '.chimera/memory/items.json'
        stored = json.loads(record_path.read_text())['data']
        check('compaction preserves an archived duplicate',
              any(row['item']['content'] == 'apricot café 🦋' for row in stored['archived']))
        cli('memory', 'forget', distinct)
        check('forgotten memory leaves active results', all(row['id'] != distinct for row in cli('memory', 'list')['memory']))
        check('forgotten memory cannot be shown as active', 'not found' in cli('memory', 'show', distinct, success=False)['error'])
        stored = json.loads(record_path.read_text())['data']
        check('forgotten memory remains recoverable in its archive', any(row['item']['id'] == distinct for row in stored['archived']))
        check('another project cannot retrieve this memory', cli('memory', 'list', cwd=other)['memory'] == [])
        shutil.copyfile(record_path, other / '.chimera/memory/items.json')
        check('copied records reject the wrong project identity', 'project identity mismatch' in cli('memory', 'list', cwd=other, success=False)['error'])

        def remember(index):
            return cli('memory', 'remember', 'parallel-' + str(index), str(index))['memory']
        with ThreadPoolExecutor(max_workers=4) as workers:
            added = list(workers.map(remember, range(8)))
        saved_ids = {row['id'] for row in cli('memory', 'list')['memory']}
        check('simultaneous command writers preserve all eight new facts', set(added) <= saved_ids)

        # Corrupt only a disposable copy, keeping the original bytes beside it.
        previous = record_path.read_bytes()
        (work / 'memory-before-corruption.json').write_bytes(previous)
        record_path.write_text('{broken')
        check('damaged memory gives a visible error', 'corrupt project record' in cli('memory', 'list', success=False)['error'])
        check('reading damaged memory does not replace it', record_path.read_text() == '{broken')
        record_path.write_bytes(previous)

        for kind, create in [('scheduled job', ('schedule', 'every:60', 'do harmless work')),
                             ('file watcher', ('watch', 'inspect changed notes', '--pattern', '*.txt'))]:
            job = cli(*create)['job']
            check(kind + ' is saved active', cli('jobs', 'inspect', job)['handle']['status'] == 'active')
            cli('jobs', 'pause', job)
            check(kind + ' stays paused after restart', cli('jobs', 'inspect', job)['handle']['status'] == 'paused')
            cli('jobs', 'resume', job)
            check(kind + ' resumes without claiming work ran', cli('jobs', 'inspect', job)['handle']['run_count'] == 0)
            cli('jobs', 'cancel', job)
            check(kind + ' stays cancelled after restart', cli('jobs', 'inspect', job)['handle']['status'] == 'cancelled')
            check(kind + ' cannot restart after cancellation', 'cannot be resumed' in cli('jobs', 'resume', job, success=False)['error'])
        check('invalid frequency fails before creating a job', 'frequency' in cli('schedule', 'nonsense', 'task', success=False)['error'])
        check('only the two intended jobs exist', len(cli('jobs', 'list')['jobs']) == 2)

        cli('notify', 'destination', 'test-inbox', '--terminal')
        cli('notify', 'disable', 'test-inbox')
        check('disabled destination rejects sending before any approval',
              'disabled' in cli('notify', 'send', 'test-inbox', 'test', 'do not deliver', success=False)['error'])
        check('disabled destination has no delivered inbox item', cli('notify', 'inbox')['inbox'] == [])
        check('missing saved workflow fails visibly', bool(cli('skill', 'inspect', 'missing-workflow', success=False)['error']))
        check('empty workflow list contains no invented entries', cli('skill', 'list')['skills'] == [])
        check('no control-only command created a conversation', cli('session', 'list')['sessions'] == [])
        report['status'] = 'passed'
        return 0
    except Exception as error:
        report['status'] = 'failed'
        report['error'] = str(error)
        return 1
    finally:
        path = work / 'report.json'
        path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
        print(json.dumps({'status': report['status'], 'checks': len(report['checks']), 'report': str(path)}))


if __name__ == '__main__':
    raise SystemExit(main())
