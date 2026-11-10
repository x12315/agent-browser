"""Guest-only package preparation, bounded CLI fixtures, sealing and final GUI proof."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import time
from urllib.parse import quote

SOURCE_SHA256 = '04168d1afd9cff1590d9d59689e754f610941248e96089cae6d25048b63bbacb'
BACKGROUND_TITLE = 'Installed Background Fixture'
FINAL_TITLE = 'Installed Work Mode Verification'
PNG_SIGNATURE = b'\x89PNG\r\n\x1a\n'


def file_map(root):
    """Return hashes and permission bits for regular package files; reject links."""
    result = {}
    for path in sorted(root.rglob('*')):
        assert not path.is_symlink(), path
        if path.is_dir():
            continue
        assert stat.S_ISREG(path.stat().st_mode), path
        if path.name == 'manifest.json' and path.parent == root:
            continue
        result[path.relative_to(root).as_posix()] = {
            'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'mode': format(stat.S_IMODE(path.stat().st_mode), '04o'),
            'bytes': path.stat().st_size,
        }
    return result


def prepare(source, package):
    """Copy the native binary, discovery content and licenses without source links."""
    assert not package.exists(), package
    package.mkdir(mode=0o755)
    (package / 'bin').mkdir()
    shutil.copy2(source / 'cli/target/ci/agent-browser', package / 'bin/agent-browser')
    for name in ('skills', 'skill-data'):
        assert not (source / name).is_symlink()
        assert not any(p.is_symlink() for p in (source / name).rglob('*'))
        shutil.copytree(source / name, package / name)
    shutil.copy2(source / 'LICENSE', package / 'LICENSE')
    licenses = source / 'cli/src/native/a11y'
    (package / 'licenses').mkdir()
    for name in ('LICENSE-axe-core.txt', 'LICENSE-axe-core-THIRD-PARTY.txt'):
        shutil.copy2(licenses / name, package / 'licenses' / name)
    for path in [package, *package.rglob('*')]:
        path.chmod(0o755 if path.is_dir() or path.stat().st_mode & 0o111 else 0o644)
    assert file_map(package)['bin/agent-browser']['mode'] == '0755'


class PackageCLI:
    """Use one owned session, explicit empty config and bounded JSON commands."""

    def __init__(self, package, outside, suffix):
        self.binary = str(package / 'bin/agent-browser')
        self.outside = outside
        self.session = f'installed-work-mode-{os.getpid()}-{suffix}'
        self.config = outside / f'{self.session}.json'
        self.config.write_text('{}')

    def run(self, *args):
        process = subprocess.run(
            [self.binary, '--config', str(self.config), '--session', self.session,
             '--json', *args], cwd=self.outside, text=True, capture_output=True, timeout=40)
        print(json.dumps({'packageCommand': args, 'rc': process.returncode,
                          'stdout': process.stdout, 'stderr': process.stderr}), flush=True)
        assert process.returncode == 0, (args, process.stderr, process.stdout)
        response = json.loads(process.stdout)
        assert response['success'] is True, response
        return response['data']


def verify(package, outside):
    """Prove relative core discovery, capability and real background browser actions."""
    assert 'AGENT_BROWSER_SKILLS_DIR' not in os.environ
    assert not outside.is_relative_to(package)
    binary = str(package / 'bin/agent-browser')
    before = file_map(package)
    help_text = subprocess.check_output([binary, '--help'], cwd=outside, text=True, timeout=20)
    assert '--work-mode' in help_text
    core = json.loads(subprocess.check_output(
        [binary, '--json', 'skills', 'get', 'core'], cwd=outside, text=True, timeout=20))
    assert core['success'] is True and len(core['data']) == 1, core
    assert core['data'][0]['name'] == 'core'
    assert core['data'][0]['content'] == (package / 'skill-data/core/SKILL.md').read_text()
    assert '--work-mode' in core['data'][0]['content']
    print('MATCHING_PACKAGED_CORE_PASS', flush=True)
    cli = PackageCLI(package, outside, 'background')
    html = ('<!doctype html><title>' + BACKGROUND_TITLE + '</title>'
            '<h1>Relocated Package Fixture</h1>'
            '<button id="verify" onclick="this.textContent=\'Package Click Verified\'">Verify package click</button>')
    try:
        cli.run('--work-mode', 'background', 'open', 'data:text/html,' + quote(html))
        runtime = cli.run('session', 'info')['runtime']
        assert runtime['workModePolicyVersion'] == 1, runtime
        assert runtime['workMode'] == 'background' and runtime['effectiveMode'] == 'headless', runtime
        snapshot = cli.run('snapshot', '-i')
        assert 'Verify package click' in json.dumps(snapshot), snapshot
        cli.run('click', '#verify')
        assert cli.run('eval', 'document.querySelector("#verify").textContent')['result'] == 'Package Click Verified'
        screenshot = outside / 'package-background.png'
        cli.run('screenshot', str(screenshot))
        assert screenshot.read_bytes().startswith(PNG_SIGNATURE)
        print(json.dumps({'packageBrowserFixture': 'PASS',
                          'screenshotSha256': hashlib.sha256(screenshot.read_bytes()).hexdigest()}), flush=True)
    finally:
        cli.run('close')
    assert file_map(package) == before, 'package changed during CLI checks'
    proof = {'version': subprocess.check_output([binary, '--version'], text=True, timeout=20).strip(),
             'generatedCapabilities': {'workModePolicyVersion': runtime['workModePolicyVersion']},
             'files': before}
    (outside / 'package-check.json').write_text(json.dumps(proof))
    print('PACKAGE_CHECK_PASS', flush=True)


def seal(package, archive):
    """Seal the already signed, verified bytes and create a package-rooted tarball."""
    proof = json.loads((package.parent / 'outside-source/package-check.json').read_text())
    assert file_map(package) == proof['files'], 'package changed since verification'
    subprocess.run(['codesign', '--verify', '--strict', str(package / 'bin/agent-browser')],
                   check=True, timeout=20)
    manifest = {'schemaVersion': 1, 'sourceArchiveSha256': SOURCE_SHA256,
                'buildProfile': 'ci', **proof}
    (package / 'manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    (package / 'manifest.json').chmod(0o644)
    with tarfile.open(archive, 'w:gz', format=tarfile.USTAR_FORMAT) as tar:
        tar.add(package, arcname='package', recursive=True)
    print('NATIVE_BUNDLE_SEALED_PASS', flush=True)


def final_gui(package, outside):
    """Leave only the owned final collaboration fixture open for producer capture."""
    manifest = json.loads((package / 'manifest.json').read_text())
    assert file_map(package) == manifest['files']
    cli = PackageCLI(package, outside, 'final-gui')
    html = ('<!doctype html><title>' + FINAL_TITLE + '</title>'
            '<style>html{background:#075f57;color:white;font:28px system-ui}body{padding:64px}'
            'h1{font-size:56px}p{color:#caffdf}</style>'
            '<h1>Installed Work Mode Verification</h1>'
            '<p>Relocated native package: collaboration window</p>'
            '<p>Sandbox-only fixture. No private information.</p>')
    visible = False
    try:
        cli.run('--work-mode', 'collaboration', 'open', 'data:text/html,' + quote(html))
        runtime = cli.run('session', 'info')['runtime']
        assert runtime['workModePolicyVersion'] == 1
        assert runtime['workMode'] == 'collaboration' and runtime['effectiveMode'] == 'headed'
        code = ("ObjC.import('CoreGraphics'); const ws=ObjC.deepUnwrap(ObjC.castRefToObject("
                "$.CGWindowListCopyWindowInfo(1,0))); JSON.stringify(ws.filter(w=>"
                "w.kCGWindowLayer===0 && w.kCGWindowAlpha>0).map(w=>w.kCGWindowName));")
        deadline = time.monotonic() + 12
        while True:
            windows = json.loads(subprocess.check_output(
                ['/usr/bin/osascript', '-l', 'JavaScript', '-e', code], text=True, timeout=5))
            if FINAL_TITLE in windows or time.monotonic() >= deadline:
                break
            time.sleep(.2)
        assert FINAL_TITLE in windows, windows
        assert BACKGROUND_TITLE not in windows and 'Protected Background' not in windows, windows
        assert file_map(package) == manifest['files'], 'package changed after sealing'
        visible = True
        print(json.dumps({'installedCollaborationWindow': 'PASS', 'title': FINAL_TITLE,
                          'backgroundTitleAbsent': True, 'session': cli.session}), flush=True)
    finally:
        if not visible:
            cli.run('close')


if __name__ == '__main__':
    command, first, second = sys.argv[1:]
    {'prepare': prepare, 'verify': verify, 'seal': seal, 'final-gui': final_gui}[command](
        Path(first).resolve(), Path(second).resolve())
