#!/usr/bin/env python3
"""Create a local, portable CHIMERA archive without downloading or publishing.

The archive contains only the program, license, installer and release metadata.
It contains no project data, credentials, conversations or build caches.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import struct
import sys
import tarfile
import uuid


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def source_files(root):
    files = [root / 'Cargo.toml', root / 'Cargo.lock', root / 'LICENSE']
    for directory in ('crates', 'scripts'):
        for path in sorted((root / directory).rglob('*')):
            relative = path.relative_to(root)
            if any(part.startswith('.') or part in ('target', '__pycache__', 'node_modules') for part in relative.parts):
                continue
            if path.is_symlink():
                raise ValueError('source snapshot cannot include symbolic links: ' + str(path))
            if path.is_file() and path.suffix not in ('.pyc', '.png', '.mp4', '.log'):
                files.append(path)
    return sorted(set(files))


def source_hash(root):
    value = hashlib.sha256()
    for path in source_files(root):
        value.update(path.relative_to(root).as_posix().encode() + b'\0' + digest(path).encode() + b'\n')
    return value.hexdigest()


def unpack_archive(archive, destination):
    """Extract only bounded regular files/directories into a new test folder."""
    destination.mkdir(parents=True, exist_ok=False)
    roots = set()
    seen = set()
    with tarfile.open(archive, 'r:gz') as tar:
        members = tar.getmembers()
        if len(members) > 32 or sum(member.size for member in members) > 512 * 1024 * 1024:
            raise ValueError('package archive exceeds size limits')
        for member in members:
            name = Path(member.name)
            if name.is_absolute() or '..' in name.parts or not name.parts or member.name in seen:
                raise ValueError('unsafe or repeated archive member: ' + member.name)
            if not (member.isdir() or member.isfile()):
                raise ValueError('package archive cannot contain links or special files')
            roots.add(name.parts[0])
            seen.add(member.name)
            target = destination / name
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as source, target.open('xb') as output:
                    shutil.copyfileobj(source, output)
                os.chmod(target, 0o755 if name.name == 'chimera' else 0o644)
    if len(roots) != 1:
        raise ValueError('package archive must have exactly one root folder')
    return destination / next(iter(roots))


def build_package(binary, root, output, build_info=None):
    if binary.is_symlink():
        raise ValueError('binary must not be a symbolic link')
    binary = binary.resolve(strict=True)
    root = root.resolve(strict=True)
    if binary.is_symlink() or not binary.is_file():
        raise ValueError('binary must be a regular file')
    version_output = subprocess.run([str(binary), '--version'], check=True, capture_output=True, text=True, timeout=10).stdout.strip()
    match = re.fullmatch(r'chimera ([0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.\-]+)?)', version_output)
    if not match:
        raise ValueError('unexpected program version: ' + version_output)
    version = match.group(1)
    manifest_text = (root / 'Cargo.toml').read_text()
    configured_version = re.search(r'(?m)^version\s*=\s*"([^"]+)"', manifest_text)
    if not configured_version or configured_version.group(1) != version:
        raise ValueError('binary and workspace version differ')
    rust = subprocess.run(['rustc', '--version', '--verbose'], check=True, capture_output=True, text=True).stdout
    with binary.open('rb') as stream:
        header = stream.read(8)
    if len(header) != 8 or header[:4] != b'\xcf\xfa\xed\xfe':
        raise ValueError('this release supports macOS only; other OS builds require their own functional validation')
    architecture = {0x0100000c: 'aarch64', 0x01000007: 'x86_64'}.get(struct.unpack('<I', header[4:8])[0])
    if architecture is None:
        raise ValueError('unsupported macOS executable architecture')
    binary_hash = digest(binary)
    tree_hash = source_hash(root)
    lock_hash = digest(root / 'Cargo.lock')
    if build_info:
        verified = json.loads(build_info.read_text())
        if any(verified.get(key) != expected for key, expected in (
                ('binary_sha256', binary_hash), ('source_tree_sha256', tree_hash),
                ('cargo_lock_sha256', lock_hash))):
            raise ValueError('build report does not match the binary, source copy and lockfile')
        if not any(check.get('name') == 'release-build' and check.get('passed') is True
                and check.get('exit_code') == 0 and '--release' in check.get('command', [])
                for check in verified.get('checks', [])):
            raise ValueError('build report has no successful optimized build')
    target = architecture + '-apple-darwin'
    identity = 'chimera-' + version + '-' + target
    output.mkdir(parents=True, exist_ok=True)
    destination = output / (identity + '-' + str(uuid.uuid4()))
    destination.mkdir(mode=0o700)
    package = destination / identity
    (package / 'bin').mkdir(parents=True)
    shutil.copyfile(binary, package / 'bin/chimera')
    os.chmod(package / 'bin/chimera', 0o755)
    if digest(package / 'bin/chimera') != binary_hash:
        raise ValueError('binary changed while making the package')
    shutil.copyfile(root / 'LICENSE', package / 'LICENSE')
    shutil.copyfile(root / 'scripts/install.py', package / 'install.py')
    metadata = {'program': 'chimera', 'version': version, 'target': target,
        'binary_sha256': binary_hash, 'source_tree_sha256': tree_hash,
        'cargo_lock_sha256': lock_hash, 'packaging_rustc': rust.strip(),
        'build_profile': 'release' if build_info else 'operator-supplied', 'signed_or_notarized': False,
        'source_binding': 'build-report-matches-source-hash' if build_info else 'unverified-current-source',
        'verification_report': str(build_info.resolve()) if build_info else None}
    (package / 'release.json').write_text(json.dumps(metadata, indent=2, sort_keys=True) + '\n')
    (package / 'PACKAGE.md').write_text('''# CHIMERA local package

Requires macOS on the architecture named in release.json and Python 3.9 or newer.
This local package is not signed or notarized for public distribution.
The installer checks every packaged file and checks the program version.

From the extracted package:

    python3 install.py install
    ~/.local/bin/chimera --version
    ~/.local/bin/chimera --help

To replace an existing managed installation:

    python3 install.py update

To remove the executable while preserving conversations and configuration:

    python3 install.py uninstall

Use --prefix /absolute/path for another install location. Replaced files move to
~/.quarantine/chimera. If that path is on a different volume from the prefix,
choose --quarantine-dir on the same volume. No saved project data is deleted.
The installer never changes your shell profile. Put <prefix>/bin on PATH yourself.

## Set up a model

Run CHIMERA from the project folder you want to work on. The commands below use
the default install location; substitute your own prefix if you changed it.

    cd /absolute/path/to/your/project
    ~/.local/bin/chimera init

Edit the settings file printed by init (normally ~/.chimera/config.json).
Init preserves existing settings. For an already running local model server,
the file can contain:

```json
{
  "provider": {
    "name": "local",
    "base_url": "http://127.0.0.1:11434/v1",
    "model": "YOUR_INSTALLED_MODEL",
    "request_timeout_secs": 120,
    "idle_timeout_secs": 30
  },
  "agent": { "max_iterations": 40 }
}
```

Replace the address and YOUR_INSTALLED_MODEL with your server's actual values.
The server must support OpenAI-compatible chat completions. CHIMERA does not
install or start that server. For a hosted service, use its exact address and
model, and supply its key through CHIMERA_API_KEY. Keep keys out of reports.
CHIMERA_CONFIG selects another settings file; CHIMERA_* environment values take
precedence over file settings.

    ~/.local/bin/chimera doctor
    ~/.local/bin/chimera run "Reply with a short greeting. Do not use tools."

Doctor checks settings and saved records, not a live model connection. The run
command checks that connection. For a connection error, confirm the server is
running and check its address, model name and credentials before retrying.

## Work, approve and stop

Try the first edit in a disposable project copy:

    ~/.local/bin/chimera plan "Read this project and describe its files."
    ~/.local/bin/chimera run "Create chimera-check.txt containing setup checked, then read it back."
    ~/.local/bin/chimera run --interactive "Read this project and suggest a small change."

Plan mode rejects file changes and shell commands. A normal run asks before
each change or command: review the displayed action and type yes or no.
Permission applies to that exact action once and expires after five minutes.
A denied action is not performed. Use an interactive terminal to review it.
Separate run commands start separate conversations; use --interactive for
follow-up requests, or resume a saved session below.

In the full-screen conversation, use /approve or /deny for a pending action,
/cancel to stop work, and /quit or /exit to leave. F1 shows the other controls.
Control+C cancels active work, including an approval or restore prompt. Inspect
saved progress and any changed files before retrying interrupted work. If you
started background jobs with chimera up, stop that project's service with:

    ~/.local/bin/chimera down

## Continue and recover

Use the identifiers printed by these commands in place of SESSION_ID and
CHECKPOINT_ID. Resume opens the interactive screen with the saved conversation.

    ~/.local/bin/chimera session list
    ~/.local/bin/chimera session resume SESSION_ID
    ~/.local/bin/chimera replay SESSION_ID
    ~/.local/bin/chimera session export SESSION_ID

Replay displays saved messages without running their tools. Export produces
diagnostic JSON without conversation bodies, tool arguments or file paths.
Before further edits, capture the conversation and selected project files:

    ~/.local/bin/chimera checkpoint create --session SESSION_ID --file chimera-check.txt
    ~/.local/bin/chimera checkpoint list
    ~/.local/bin/chimera rollback CHECKPOINT_ID

Stop any active run before restoring. Rollback asks before replacing captured
files, rejects files changed during review, and keeps replaced copies and
restoration history. Files not named with --file are left alone. If a saved
session record is damaged, restore its previous saved copy with:

    ~/.local/bin/chimera session recover SESSION_ID

Recovery preserves the damaged record. If no readable previous copy exists,
retain the error and your saved files rather than replacing them with empty data.
Conversations and recovery copies live in the project's .chimera/ folder; model
settings normally live in ~/.chimera/config.json. Keep both private and backed
up. Add .chimera/ to your project's .gitignore when using Git.

## Help and limits

Use ~/.local/bin/chimera --help or append --help to a command for its arguments.
For a problem report, retain the version, command, error and a reviewed session
export. Do not share keys or raw conversations. A private security-reporting
contact is not configured yet; keep sensitive reports private until one is supplied.

Browser use additionally needs Node.js, Puppeteer and Chrome. Android use needs
the Android SDK and an installed virtual device. Those are not bundled here.
Network-disabled Android and non-macOS shell isolation are not supported.
''')
    names = ['bin/chimera', 'LICENSE', 'install.py', 'PACKAGE.md', 'release.json']
    (package / 'SHA256SUMS').write_text(''.join(digest(package / name) + '  ' + name + '\n' for name in names))
    archive = destination / (identity + '.tar.gz')
    with archive.open('xb') as raw:
        with gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.PAX_FORMAT) as tar:
                for path in sorted(package.rglob('*')):
                    info = tar.gettarinfo(str(path), identity + '/' + path.relative_to(package).as_posix())
                    info.uid = info.gid = 0
                    info.uname = info.gname = ''
                    info.mtime = 0
                    info.mode = 0o755 if path.is_dir() or path.name == 'chimera' else 0o644
                    if path.is_file():
                        with path.open('rb') as stream:
                            tar.addfile(info, stream)
                    else:
                        tar.addfile(info)
    result = {'package': str(package), 'archive': str(archive), 'archive_sha256': digest(archive), 'binary_sha256': metadata['binary_sha256'], 'version': version, 'target': target}
    (destination / 'package-result.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--source-root', type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument('--output-dir', type=Path)
    parser.add_argument('--build-info', type=Path)
    args = parser.parse_args()
    try:
        result = build_package(args.binary, args.source_root, args.output_dir or args.source_root / '.floyd/releases', args.build_info)
        print(json.dumps(result, sort_keys=True))
    except (OSError, ValueError, StopIteration, subprocess.SubprocessError) as error:
        print('Packaging failed: ' + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
