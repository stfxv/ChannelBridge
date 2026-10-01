#!/usr/bin/env python3
"""Preserve the legacy installation without changing it or audio services."""
import argparse
import hashlib
import json
import os
import pathlib
import plistlib
import shutil
import stat
import subprocess


def inventory(root):
    result = {}
    for path in sorted(root.rglob('*')):
        info = path.lstat()
        entry = {'mode': stat.S_IMODE(info.st_mode)}
        if path.is_symlink():
            entry.update(type='symlink', target=str(path.readlink()))
        elif path.is_dir():
            entry.update(type='directory')
        elif path.is_file():
            digest = hashlib.sha256()
            with path.open('rb') as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                    digest.update(chunk)
            entry.update(type='file', sha256=digest.hexdigest())
        else:
            raise ValueError('Unsupported backup entry')
        result[str(path.relative_to(root))] = entry
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=pathlib.Path, required=True)
    parser.add_argument('--destination', type=pathlib.Path, required=True)
    args = parser.parse_args()
    app = args.app.resolve(strict=True)
    driver = pathlib.Path('/Library/Audio/Plug-Ins/HAL/ChannelBridge.driver')
    settings = pathlib.Path.home() / 'Library/Application Support/ChannelBridge/settings.json'
    sources = [('app', app, 'local.channelbridge.app', '0.4.0'),
               ('driver', driver, 'local.channelbridge.driver', '0.1.0')]
    metadata = {}
    for label, path, identifier, version in sources:
        with (path / 'Contents/Info.plist').open('rb') as stream:
            info = plistlib.load(stream)
        if info.get('CFBundleIdentifier') != identifier or info.get('CFBundleShortVersionString') != version:
            raise ValueError('Backup requires the planned legacy app 0.4.0 and driver 0.1.0')
        metadata[label] = {'original_path': str(path), 'identifier': identifier, 'version': version}
    # Validate before creating anything; missing settings must not mean a silent empty backup.
    if not settings.is_file() or settings.is_symlink():
        raise ValueError('Legacy settings must exist as a regular file')
    destination = args.destination.resolve()
    for _, path, _, _ in sources:
        if destination == path or path in destination.parents or destination in path.parents:
            raise ValueError('Backup destination must be separate from source bundles')
    destination.mkdir(mode=0o700)  # Existing destinations are never replaced.
    try:
        for label, path, _, _ in sources:
            target = destination / (path.name if label == 'app' else 'ChannelBridge.driver')
            before = inventory(path)
            subprocess.run(['ditto', '--norsrc', '--noextattr', '--noacl', str(path), str(target)], check=True)
            if before != inventory(target) or before != inventory(path):
                raise ValueError('Backup verification failed or source changed during copying')
            metadata[label]['entries'] = before
        before = settings.read_bytes()
        target = destination / 'settings.json'
        with target.open('xb') as stream:
            stream.write(before)
        target.chmod(0o600)
        if target.read_bytes() != before or settings.read_bytes() != before:
            raise ValueError('Settings changed during backup')
        metadata['settings'] = {'original_path': str(settings), 'sha256': hashlib.sha256(before).hexdigest()}
        manifest = destination / 'manifest.json'
        with manifest.open('x') as stream:
            json.dump(metadata, stream, indent=2)
            stream.write('\n')
        manifest.chmod(0o600)
    except BaseException:
        shutil.rmtree(destination)
        raise
    print('Legacy app, driver and settings backup verified; sources unchanged')


if __name__ == '__main__':
    os.umask(0o077)
    main()
