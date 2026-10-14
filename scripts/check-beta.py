#!/usr/bin/env python3
"""Build and test a frozen CHIMERA source copy with the committed dependency lock.

Uses the installed Rust 1.98.0 toolchain and cached dependencies; no downloads.
Every build, temporary project, log and package stays under --output-dir.
The complete source copy and logs are retained when a check fails.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import sys
import time
import uuid


def load_packager(root):
    spec = importlib.util.spec_from_file_location('chimera_packager', root / 'scripts/package-release.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def compare_builds(first, second):
    """Compare all bytes; separately identify Mach-O UUID/signature metadata.

    A metadata-only result still requires codesign verification of both files.
    Code, embedded strings, load paths and every other byte must match exactly.
    """
    left, right = first.read_bytes(), second.read_bytes()
    result = {'byte_equal': left == right, 'first_size': len(left), 'second_size': len(right)}
    if left == right:
        return {**result, 'content_equivalent': True, 'difference': 'none'}
    if len(left) != len(right):
        return {**result, 'content_equivalent': False, 'difference': 'file sizes differ'}

    def metadata_ranges(value):
        if len(value) < 32 or value[:4] != b'\xcf\xfa\xed\xfe':
            raise ValueError('not a 64-bit little-endian Mach-O file')
        count, command_size = struct.unpack_from('<II', value, 16)
        if count > 128 or 32 + command_size > len(value):
            raise ValueError('invalid Mach-O load command table')
        offset, ranges = 32, []
        for _ in range(count):
            command, size = struct.unpack_from('<II', value, offset)
            if size < 8 or offset + size > 32 + command_size:
                raise ValueError('invalid Mach-O load command')
            if command == 0x1b and size == 24:  # LC_UUID
                ranges.append((offset + 8, offset + 24, 'LC_UUID'))
            if command == 0x1d and size == 16:  # LC_CODE_SIGNATURE
                start, length = struct.unpack_from('<II', value, offset + 8)
                if start < 32 + command_size or start + length != len(value):
                    raise ValueError('invalid Mach-O signature range')
                ranges.append((start, start + length, 'LC_CODE_SIGNATURE'))
            offset += size
        return ranges

    differences = [index for index, pair in enumerate(zip(left, right)) if pair[0] != pair[1]]
    result['different_byte_count'] = len(differences)
    result['different_offsets'] = differences if len(differences) <= 128 else differences[:128]
    try:
        ranges = metadata_ranges(left)
        equivalent = ranges == metadata_ranges(right) and all(
            any(start <= index < end for start, end, _ in ranges) for index in differences)
    except (ValueError, struct.error):
        ranges, equivalent = [], False
    return {**result, 'content_equivalent': equivalent, 'metadata_ranges': ranges,
        'difference': 'Mach-O UUID/signature metadata only' if equivalent else 'non-metadata bytes differ'}


def verify(args):
    root = Path(__file__).resolve().parent.parent
    packager = load_packager(root)
    output = (args.output_dir or root / '.floyd/releases').resolve()
    output.mkdir(parents=True, exist_ok=True)
    work = output / ('verify-' + str(uuid.uuid4()))
    work.mkdir(mode=0o700)
    source = work / 'source'
    source.mkdir()
    temp = work / 'tmp'
    temp.mkdir()
    report = {'status': 'in_progress', 'work_directory': str(work), 'checks': [],
        'limits': ['Fresh source copy and isolated HOME on the current account, not a separate OS account.',
                   'Controlled local model checks do not prove a paid or hosted provider.',
                   'No real Android device was exercised by this script.',
                   'Only the current macOS architecture is built; package is unsigned.']}
    report_path = work / 'verification.json'

    def save():
        report_path.write_text(json.dumps(report, indent=2) + '\n')

    env = dict(os.environ)
    for key in list(env):
        if key.startswith(('CHIMERA_', 'CARGO_PROFILE_')) or key in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET'):
            env.pop(key, None)
    env.update(TMPDIR=str(temp), PYTHONPYCACHEPREFIX=str(work / 'python-cache'), CARGO_NET_OFFLINE='true', CARGO_INCREMENTAL='0', SOURCE_DATE_EPOCH='0')

    def run(name, command, command_env=None):
        log = work / (str(len(report['checks']) + 1).zfill(2) + '-' + name + '.log')
        started = time.monotonic()
        print('CHECK ' + name + ' -> ' + str(log), flush=True)
        with log.open('w') as stream:
            process = subprocess.Popen(command, cwd=source, env=command_env or env, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, text=True, errors='replace', start_new_session=True)
            try:
                for line in process.stdout:
                    stream.write(line)
                    stream.flush()
                    print(line, end='', flush=True)
                code = process.wait()
            except BaseException:
                os.killpg(process.pid, signal.SIGTERM)
                try: process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                raise
        report['checks'].append({'name': name, 'command': command, 'exit_code': code,
            'passed': code == 0, 'log': str(log), 'duration_seconds': round(time.monotonic() - started, 2)})
        save()
        if code:
            raise RuntimeError(name + ' failed with exit ' + str(code) + '; see ' + str(log))
        return log.read_text()

    try:
        if shutil.disk_usage(work).free < 8 * 1024 ** 3:
            raise RuntimeError('fresh verification needs at least 8 GiB free at ' + str(work))
        original_hash = packager.source_hash(root)
        for path in packager.source_files(root):
            destination = source / path.relative_to(root)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)
        if packager.source_hash(root) != original_hash or packager.source_hash(source) != original_hash:
            raise RuntimeError('source changed while making the copy; retry after the current edit finishes')
        report['source_tree_sha256'] = original_hash
        report['cargo_lock_sha256'] = packager.digest(source / 'Cargo.lock')
        rust = run('toolchain', ['rustc', '--version', '--verbose'])
        if 'release: ' + args.rust_version + '\n' not in rust:
            raise RuntimeError('Rust ' + args.rust_version + ' is required; the installed toolchain differs')
        if 'apple-darwin' not in rust:
            raise RuntimeError('the supported release check currently targets macOS only')
        target = work / 'target'
        env['CARGO_TARGET_DIR'] = str(target)
        env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(['--remap-path-prefix=' + str(source) + '=/chimera', '--remap-path-prefix=' + str(target) + '=/chimera-target'])
        run('workspace-tests', ['cargo', 'test', '--workspace', '--locked', '--offline'])
        run('release-build', ['cargo', 'build', '-p', 'chimera-cli', '--release', '--locked', '--offline'])
        binary = target / 'release/chimera'
        report['binary_sha256'] = packager.digest(binary)
        if args.repeat_build:
            repeat_env = dict(env)
            repeat_target = work / 'target-repeat'
            repeat_env['CARGO_TARGET_DIR'] = str(repeat_target)
            repeat_env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(['--remap-path-prefix=' + str(source) + '=/chimera', '--remap-path-prefix=' + str(repeat_target) + '=/chimera-target'])
            run('independent-release-build', ['cargo', 'build', '-p', 'chimera-cli', '--release', '--locked', '--offline'], repeat_env)
            repeat_binary = repeat_target / 'release/chimera'
            comparison = compare_builds(binary, repeat_binary)
            comparison['first_sha256'] = report['binary_sha256']
            comparison['second_sha256'] = packager.digest(repeat_binary)
            report['repeat_build_comparison'] = comparison
            if comparison['content_equivalent'] and not comparison['byte_equal']:
                run('first-build-signature', ['/usr/bin/codesign', '--verify', '--strict', str(binary)])
                run('second-build-signature', ['/usr/bin/codesign', '--verify', '--strict', str(repeat_binary)])
                report['limits'].append('Independent builds differ only in verified Mach-O UUID/signature metadata; exact byte reproducibility is not claimed.')
            report['checks'].append({'name': 'repeat-build-content-equivalence', 'passed': comparison['content_equivalent']})
            save()
            if not comparison['content_equivalent']:
                raise RuntimeError('independent optimized builds differ outside UUID/signature metadata; both binaries were preserved for inspection')
        run('startup-cancellation', [sys.executable, str(source / 'scripts/check-startup-cancel.py'), str(binary), '--output-dir', str(work)])
        run('approval-prompt-cancellation', [sys.executable, str(source / 'scripts/check-prompt-cancel.py'), str(binary), '--output-dir', str(work)])
        run('local-user-flows', [sys.executable, str(source / 'scripts/check-local-flows.py'), str(binary), '--output-dir', str(work)])
        run('public-user-flows', [sys.executable, str(source / 'scripts/check-user-flows.py'), str(binary)])
        if args.repeat_build:
            run('repeat-build-public-user-flows', [sys.executable, str(source / 'scripts/check-user-flows.py'), str(repeat_binary)])
        chrome = Path('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
        puppeteer = Path('/opt/homebrew/lib/node_modules/puppeteer/lib/cjs/puppeteer/puppeteer.js')
        if chrome.is_file() and puppeteer.is_file() and shutil.which('node'):
            run('real-browser', ['cargo', 'test', '-p', 'chimera-browser', '--locked', '--offline', '--', '--ignored', '--nocapture'])
        else:
            report['limits'].append('Real browser checks skipped: this host lacks the configured Chrome/Puppeteer/Node dependencies.')
        save()
        package = packager.build_package(binary, source, work / 'packages', report_path)
        report['package'] = package
        extracted = packager.unpack_archive(Path(package['archive']), work / 'extracted-package')
        run('install-update-remove', [sys.executable, str(source / 'scripts/check-install.py'), str(extracted), '--work-dir', str(work / 'install-check')])
        # Detect accidental changes to the frozen source or its lockfile.
        if packager.source_hash(source) != original_hash:
            raise RuntimeError('a verification step changed the frozen source')
        report['status'] = 'passed'
        save()
        print(json.dumps({'passed': True, 'report': str(report_path), **package}, sort_keys=True))
        return 0
    except Exception as error:
        report['status'] = 'failed'
        report['error'] = str(error)
        save()
        print('Verification failed: ' + str(error) + '\nEvidence: ' + str(report_path), file=sys.stderr)
        return 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path)
    parser.add_argument('--repeat-build', action='store_true', help='build twice in empty target directories, record byte differences, require matching code and run user checks on both')
    parser.add_argument('--rust-version', default='1.98.0', help='exact required installed Rust version; default 1.98.0')
    return verify(parser.parse_args())


if __name__ == '__main__':
    sys.exit(main())
