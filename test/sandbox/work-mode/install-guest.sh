#!/bin/bash
# Build and exercise a private relocatable package only in the disposable guest.
set -euo pipefail
for tool in sysctl python3 tar codesign caffeinate osascript agent-browser; do
    command -v "$tool" >/dev/null || { printf 'NOT_READY: missing %s\n' "$tool" >&2; exit 78; }
done
[ "$(sysctl -n kern.hv_vmm_present 2>/dev/null)" = 1 ] || { echo 'NOT_READY: sandbox VM required' >&2; exit 78; }
legacy_cli=$(command -v agent-browser)
umask 077
job=$(mktemp -d /tmp/agent-browser-work-mode-install.XXXXXX)
printf 'GUEST_INSTALL_JOB=%s\n' "$job"
/usr/bin/caffeinate -d -i -u -t 2400 &
keep_awake_pid=$!
trap 'kill "$keep_awake_pid" 2>/dev/null || true' EXIT
anchor="$job/work"
cache_prefix=$(python3 - <<'PY'
import json,re
from pathlib import Path
value=json.loads(Path('/tmp/ahsb-push/public-build-cache-manifest.json').read_text()).get('guestBuildPrefix','')
if re.fullmatch(r'/tmp/agent-browser-work-mode\.[A-Za-z0-9]+/work',value):print(value)
PY
)
if [ -n "$cache_prefix" ] && [ ! -e "$cache_prefix" ]; then
    anchor="$cache_prefix"
fi
source_root="$anchor/Projects/agent-browser"
toolchain="$anchor/.rustup/toolchains/stable-aarch64-apple-darwin"
export CARGO_HOME="$anchor/.cargo" CARGO_NET_OFFLINE=true
printf 'STAGE verify-install-inputs\n'
python3 - <<'PY'
from pathlib import Path
import hashlib,json
root=Path('/tmp/ahsb-push')
expected='04168d1afd9cff1590d9d59689e754f610941248e96089cae6d25048b63bbacb'
def digest(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda:f.read(1024*1024),b''):h.update(chunk)
    return h.hexdigest()
assert digest(root/'work-mode-source.tar.gz')==expected,'frozen source hash mismatch'
manifest=json.loads((root/'public-build-cache-manifest.json').read_text())
for name in ('rust-toolchain-cache.tar.gz','cargo-registry-cache.tar.gz','rust-dependency-target-cache.tar.gz'):
    item=manifest['archives'][name]
    assert (root/name).stat().st_size==item['bytes'],name
    assert digest(root/name)==item['sha256'],name
print('INSTALL_INPUTS_PASS',flush=True)
PY
mkdir -p "$source_root" "$toolchain" "$CARGO_HOME" "$source_root/cli/target" "$job/outside-source"
tar -xzf /tmp/ahsb-push/rust-toolchain-cache.tar.gz -C "$toolchain"
tar -xzf /tmp/ahsb-push/cargo-registry-cache.tar.gz -C "$CARGO_HOME"
tar -xzf /tmp/ahsb-push/work-mode-source.tar.gz -C "$source_root"
tar -xzf /tmp/ahsb-push/rust-dependency-target-cache.tar.gz -C "$source_root/cli/target"
export PATH="$toolchain/bin:$PATH"
[ "$(command -v agent-browser)" = "$legacy_cli" ] || { echo 'legacy CLI PATH identity changed' >&2; exit 1; }
command -v cargo >/dev/null
cd "$source_root"
printf 'STAGE build-install-ci\n'
python3 - <<'PY'
import subprocess
subprocess.run(['cargo','build','--locked','--offline','--manifest-path','cli/Cargo.toml','--profile','ci','-j2'],check=True,timeout=1800)
PY
printf 'STAGE prepare-private-package\n'
python3 /tmp/ahsb-push/install-verify.py prepare "$source_root" "$job/package"
codesign --force --sign - "$job/package/bin/agent-browser"
codesign --verify --strict "$job/package/bin/agent-browser"
# Leave PATH's legacy command untouched; only the absolute package path is new.
unset AGENT_BROWSER_SKILLS_DIR AGENT_BROWSER_CDP AGENT_BROWSER_AUTO_CONNECT AGENT_BROWSER_PROVIDER
cd "$job/outside-source"
printf 'STAGE relocated-package-checks\n'
python3 /tmp/ahsb-push/install-verify.py verify "$job/package" "$job/outside-source"
printf 'STAGE packaged-actual-cli-six-checks\n'
python3 /tmp/ahsb-push/cli-smoke.py "$job/package/bin/agent-browser"
printf 'STAGE seal-native-bundle\n'
python3 /tmp/ahsb-push/install-verify.py seal "$job/package" "$job/native-bundle.tar.gz"
printf 'STAGE export-native-bundle\n'
python3 /tmp/ahsb-push/install-export.py export "$job/native-bundle.tar.gz"
printf 'STAGE final-installed-collaboration-window\n'
python3 /tmp/ahsb-push/install-verify.py final-gui "$job/package" "$job/outside-source"
printf 'WORK_MODE_INSTALL_SANDBOX_ALL_PASSED\n'
