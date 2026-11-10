"""Frame guest bundle bytes; on host validate data only, without extraction/execution."""
import base64
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile

BEGIN = '==AH_NATIVE_BUNDLE_BEGIN=='
END = '==AH_NATIVE_BUNDLE_END=='
SOURCE_SHA256 = '04168d1afd9cff1590d9d59689e754f610941248e96089cae6d25048b63bbacb'
MAX_BUNDLE_BYTES = 256 * 1024 * 1024
SMOKE_CHECKS = {
    'native_cli_mode_overrides_default_and_inherits',
    'mode_headed_daemon_config_and_external_attach_refused_without_page_loss',
    'batch_close_releases_previous_mode',
    'real_guest_collaboration_window_visible_background_not_visible',
    'explicit_close_allows_new_mode',
    'unsupported_legacy_daemon_is_not_adopted_restarted_or_mutated',
}


def digest_file(path):
    """Hash a file without buffering the binary artifact."""
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def export_bundle(path):
    """Write a single JSON header and 76-column base64 frame to guest stdout."""
    size = path.stat().st_size
    assert 0 < size <= MAX_BUNDLE_BYTES
    print(BEGIN)
    print(json.dumps({'sha256': digest_file(path), 'bytes': size}, sort_keys=True))
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(57), b''):
            print(base64.b64encode(chunk).decode('ascii'))
    print(END, flush=True)


def validate_archive(path):
    """Inspect tar member data and manifest hashes, never extract or run anything."""
    with tarfile.open(path, 'r:gz') as tar:
        members = tar.getmembers()
        names = [member.name for member in members]
        assert len(names) == len(set(names)), 'duplicate archive entries'
        total = 0
        files = {}
        for member in members:
            parts = PurePosixPath(member.name).parts
            assert parts and parts[0] == 'package' and '..' not in parts, member.name
            assert not member.name.startswith('/') and '\\' not in member.name, member.name
            assert member.isdir() or member.isfile(), 'links/special files forbidden'
            assert member.mode in (0o644, 0o755), (member.name, member.mode)
            assert member.size >= 0
            total += member.size
            assert total <= 512 * 1024 * 1024, 'oversized expanded package'
            if member.isfile():
                files['/'.join(parts[1:])] = member
        assert 'manifest.json' in files and files['manifest.json'].size < 1024 * 1024
        with tar.extractfile(files.pop('manifest.json')) as stream:
            manifest = json.load(stream)
        assert manifest['schemaVersion'] == 1
        assert manifest['sourceArchiveSha256'] == SOURCE_SHA256
        assert manifest['buildProfile'] == 'ci'
        assert manifest['generatedCapabilities'] == {'workModePolicyVersion': 1}
        assert isinstance(manifest['version'], str) and manifest['version']
        assert set(files) == set(manifest['files']), 'manifest file set mismatch'
        required = {'bin/agent-browser', 'LICENSE', 'licenses/LICENSE-axe-core.txt',
                    'licenses/LICENSE-axe-core-THIRD-PARTY.txt',
                    'skills/agent-browser/SKILL.md', 'skill-data/core/SKILL.md'}
        assert required <= set(files)
        assert manifest['files']['bin/agent-browser']['mode'] == '0755'
        for name, member in files.items():
            item = manifest['files'][name]
            assert item['bytes'] == member.size, name
            assert item['mode'] == format(member.mode, '04o'), name
            digest = hashlib.sha256()
            with tar.extractfile(member) as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                    digest.update(chunk)
            assert digest.hexdigest() == item['sha256'], name
    return manifest


def consume(run_dir):
    """Require guest success and producer PNG, then decode/hash-check into RUN_DIR."""
    assert (run_dir / 'guest/tmp/ah.rc').read_text().strip() == '0', 'guest failed'
    assert not (run_dir / 'timeout.txt').exists(), 'producer timeout'
    with (run_dir / 'gui.png').open('rb') as image:
        assert image.read(8) == b'\x89PNG\r\n\x1a\n', 'missing/invalid producer GUI PNG'
    output = run_dir / 'native-bundle.tar.gz'
    partial = run_dir / 'native-bundle.tar.gz.partial'
    assert not output.exists() and not partial.exists(), 'do not overwrite existing artifacts'
    state = 'logs'
    header = None
    digest = hashlib.sha256()
    size = 0
    frame_count = 0
    markers = set()
    smoke = []
    gui = []
    with (run_dir / 'guest/tmp/ah.out').open() as log, partial.open('xb') as artifact:
        for raw in log:
            assert len(raw) <= 1024 * 1024, 'oversized stdout line'
            line = raw.rstrip('\r\n')
            if line == BEGIN:
                assert state == 'logs' and frame_count == 0, 'duplicate/nested frame'
                state = 'header'
                frame_count += 1
            elif state == 'header':
                header = json.loads(line)
                assert set(header) == {'sha256', 'bytes'}
                assert re.fullmatch('[a-f0-9]{64}', header['sha256'])
                assert type(header['bytes']) is int and 0 < header['bytes'] <= MAX_BUNDLE_BYTES
                state = 'base64'
            elif state == 'base64':
                if line == END:
                    assert size == header['bytes'] and digest.hexdigest() == header['sha256']
                    state = 'done'
                    continue
                assert 0 < len(line) <= 76 and len(line) % 4 == 0, 'invalid base64 wrap'
                chunk = base64.b64decode(line, validate=True)
                size += len(chunk)
                assert size <= header['bytes'], 'bundle exceeds declared size'
                digest.update(chunk)
                artifact.write(chunk)
            else:
                assert line != END, 'unmatched bundle end'
                markers.add(line)
                if line.startswith('{'):
                    try:
                        item = json.loads(line)
                    except ValueError:
                        continue
                    if item.get('fork_cli_work_mode_smoke') == 'PASS':
                        smoke.append(item)
                    if item.get('installedCollaborationWindow') == 'PASS':
                        gui.append(item)
    assert state == 'done' and frame_count == 1, 'missing/incomplete bundle frame'
    assert {'WORK_MODE_INSTALL_SANDBOX_ALL_PASSED', 'MATCHING_PACKAGED_CORE_PASS',
            'PACKAGE_CHECK_PASS', 'NATIVE_BUNDLE_SEALED_PASS'} <= markers
    assert len(smoke) == 1 and len(smoke[0]['checks']) == 6
    assert set(smoke[0]['checks']) == SMOKE_CHECKS, 'six-check CLI smoke incomplete'
    assert len(gui) == 1 and gui[0]['title'] == 'Installed Work Mode Verification'
    assert gui[0]['backgroundTitleAbsent'] is True
    manifest = validate_archive(partial)
    partial.rename(output)
    (run_dir / 'native-bundle-manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    print(f'Native package, relocated core, six CLI checks and producer GUI PASS: {output}')


if __name__ == '__main__':
    command, path = sys.argv[1:]
    {'export': export_bundle, 'consume': consume}[command](Path(path))
