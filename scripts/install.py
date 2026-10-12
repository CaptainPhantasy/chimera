#!/usr/bin/env python3
"""Install a verified local package; retain every replaced file in quarantine.

Only bin/chimera and share/chimera/install.json are managed. Project .chimera
folders, model settings, and saved conversations are never opened or changed.
Python 3.9+ and Unix are required. This does not download or publish anything.
"""
import argparse
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import uuid


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()


def safe_directory(path):
    path = Path(os.path.abspath(path))
    for part in [*reversed(path.parents), path]:
        if part.is_symlink():
            raise ValueError('installation directory cannot contain symbolic links: ' + str(part))
        if part.exists() and not part.is_dir():
            raise ValueError('installation directory is not a directory: ' + str(part))
    path.mkdir(parents=True, exist_ok=True)
    return path


def regular(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError('expected a regular file: ' + str(path))


def package(path):
    path = path.resolve(strict=True)
    manifest_path = path / 'release.json'
    sums_path = path / 'SHA256SUMS'
    regular(manifest_path)
    regular(sums_path)
    manifest = json.loads(manifest_path.read_text())
    required = {'bin/chimera', 'LICENSE', 'install.py', 'PACKAGE.md', 'release.json'}
    found = set()
    for line in sums_path.read_text().splitlines():
        expected, name = line.split('  ', 1)
        if name not in required or name in found:
            raise ValueError('unexpected or repeated package member: ' + name)
        source = path / name
        if any(parent.is_symlink() for parent in source.parents if parent != path.parent):
            raise ValueError('package member has a symbolic-link parent: ' + name)
        regular(source)
        if digest(source) != expected:
            raise ValueError('package checksum failed: ' + name)
        found.add(name)
    if found != required:
        raise ValueError('package is missing required checksums')
    if digest(path / 'bin/chimera') != manifest['binary_sha256']:
        raise ValueError('binary does not match release manifest')
    machine = {'arm64': 'aarch64', 'x86_64': 'x86_64'}.get(platform.machine(), platform.machine())
    current = machine + '-apple-darwin' if sys.platform == 'darwin' else machine + '-unknown-linux-gnu'
    if manifest['target'] != current:
        raise ValueError('package target ' + manifest['target'] + ' does not match this host ' + current)
    return path, manifest


def installed(prefix):
    binary = prefix / 'bin/chimera'
    record = prefix / 'share/chimera/install.json'
    if not binary.exists() and not binary.is_symlink() and not record.exists() and not record.is_symlink():
        return None
    regular(binary)
    regular(record)
    value = json.loads(record.read_text())
    if value.get('program') != 'chimera' or digest(binary) != value.get('binary_sha256'):
        raise ValueError('installed program was changed outside this installer; existing files were preserved')
    return value


def write_json(path, value):
    with path.open('x') as stream:
        os.chmod(path, 0o600)
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def apply(args):
    prefix = safe_directory(args.prefix)
    binary_dir = safe_directory(prefix / 'bin')
    state_dir = safe_directory(prefix / 'share/chimera')
    lock_path = state_dir / '.install.lock'
    if lock_path.is_symlink():
        raise ValueError('install lock cannot be a symbolic link')
    with lock_path.open('a') as lock:
        os.chmod(lock_path, 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        previous = installed(prefix)
        if args.action == 'status':
            return {'installed': previous, 'prefix': str(prefix)}
        if args.action == 'install' and previous:
            raise ValueError('CHIMERA is already installed at this prefix; use update')
        if args.action in ('update', 'uninstall') and not previous:
            raise ValueError('no managed CHIMERA installation exists at this prefix')
        transaction = str(uuid.uuid4())
        binary = binary_dir / 'chimera'
        record = state_dir / 'install.json'
        pending_binary = binary_dir / ('.chimera-stage-' + transaction)
        pending_record = state_dir / ('.install-stage-' + transaction + '.json')
        manifest = None
        if args.action != 'uninstall':
            source, manifest = package(args.package)
            with (source / 'bin/chimera').open('rb') as read, pending_binary.open('xb') as write:
                shutil.copyfileobj(read, write)
                write.flush()
                os.fsync(write.fileno())
            os.chmod(pending_binary, 0o755)
            if digest(pending_binary) != manifest['binary_sha256']:
                raise ValueError('packaged program changed while copying; existing installation was preserved')
            version = subprocess.run([str(pending_binary), '--version'], text=True, capture_output=True, timeout=10, check=True).stdout.strip()
            if version != 'chimera ' + manifest['version']:
                raise ValueError('packaged program version differs from manifest: ' + version)
            write_json(pending_record, {'program': 'chimera', 'version': manifest['version'],
                'binary_sha256': manifest['binary_sha256'], 'target': manifest['target'],
                'source_tree_sha256': manifest['source_tree_sha256'], 'transaction': transaction})
        quarantine = safe_directory(args.quarantine_dir) / transaction
        quarantine.mkdir(mode=0o700)
        write_json(quarantine / 'WHY.json', {'action': args.action, 'prefix': str(prefix),
            'at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'reason': 'Recoverable installation transaction. User project files and settings are not managed here.'})
        moved = []
        placed = []
        try:
            if previous:
                for source in (binary, record):
                    destination = quarantine / source.name
                    os.rename(source, destination)
                    moved.append((source, destination))
            if args.action != 'uninstall':
                os.rename(pending_binary, binary)
                placed.append(binary)
                os.rename(pending_record, record)
                placed.append(record)
            directory_fd = os.open(str(binary_dir), os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
        except Exception:
            for destination in reversed(placed):
                os.rename(destination, quarantine / ('failed-new-' + destination.name))
            for source, destination in reversed(moved):
                os.rename(destination, source)
            raise
        return {'action': args.action, 'prefix': str(prefix), 'version': manifest['version'] if manifest else previous['version'],
            'quarantine': str(quarantine), 'user_data': 'preserved', 'binary': str(binary) if manifest else None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['install', 'update', 'uninstall', 'status'])
    parser.add_argument('--package', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--prefix', type=Path, default=Path.home() / '.local')
    parser.add_argument('--quarantine-dir', type=Path, default=Path.home() / '.quarantine/chimera')
    args = parser.parse_args()
    try:
        print(json.dumps(apply(args), sort_keys=True))
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print('Installation failed; existing user data was preserved: ' + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
