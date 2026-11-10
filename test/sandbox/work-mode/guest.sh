#!/bin/bash
set -euo pipefail
[ "$(sysctl -n kern.hv_vmm_present 2>/dev/null)" = 1 ] || { echo 'NOT_READY: this build must run in the sandbox VM' >&2; exit 78; }
job=$(mktemp -d /tmp/agent-browser-work-mode.XXXXXX)
/usr/bin/caffeinate -d -i -u -t 1800 &
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
export CARGO_HOME="$anchor/.cargo" CARGO_NET_RETRY=1 CARGO_HTTP_TIMEOUT=45
export NODE_OPTIONS=--max-old-space-size=2048
printf 'STAGE verify public build inputs\n'
python3 - <<'PY'
from pathlib import Path
import hashlib,json
root=Path('/tmp/ahsb-push')
manifest=json.loads((root/'public-build-cache-manifest.json').read_text())
for name,item in manifest['archives'].items():
    digest=hashlib.sha256()
    with (root/name).open('rb') as stream:
        for chunk in iter(lambda:stream.read(1024*1024),b''):digest.update(chunk)
    assert digest.hexdigest()==item['sha256'],name
print('public toolchain/registry/dependency cache hashes verified',flush=True)
PY
mkdir -p "$source_root" "$toolchain" "$CARGO_HOME" "$source_root/cli/target"
tar -xzf /tmp/ahsb-push/rust-toolchain-cache.tar.gz -C "$toolchain"
tar -xzf /tmp/ahsb-push/cargo-registry-cache.tar.gz -C "$CARGO_HOME"
tar -xzf /tmp/ahsb-push/work-mode-source.tar.gz -C "$source_root"
tar -xzf /tmp/ahsb-push/rust-dependency-target-cache.tar.gz -C "$source_root/cli/target"
export PATH="$toolchain/bin:$PATH"
cd "$source_root"
printf 'GUEST_SOURCE_ROOT=%s\n' "$source_root"
rustc --version
if ! cargo fetch --locked --manifest-path cli/Cargo.toml --target aarch64-apple-darwin --offline; then
    printf 'STAGE fill missing locked guest-only dependencies\n'
    cargo fetch --locked --manifest-path cli/Cargo.toml --target aarch64-apple-darwin
fi
failed=0
LAST_RC=0
run_stage() {
    local label=$1
    shift
    printf 'STAGE %s\n' "$label"
    if "$@"; then LAST_RC=0; else LAST_RC=$?; failed=1; fi
    printf 'STAGE_RESULT name=%s rc=%s\n' "$label" "$LAST_RC"
}
run_stage rust-unit cargo test --locked --offline --manifest-path cli/Cargo.toml -j 1 -- --test-threads=1
run_stage native-work-mode-e2e cargo test --locked --offline --manifest-path cli/Cargo.toml -j 1 e2e_work_mode -- --ignored --test-threads=1 --nocapture
run_stage build-cli cargo build --locked --offline --manifest-path cli/Cargo.toml -j 1
if [ "$LAST_RC" = 0 ]; then
    run_stage actual-cli python3 /tmp/ahsb-push/cli-smoke.py "$source_root/cli/target/debug/agent-browser"
else
    printf 'SKIP actual-cli: source build did not succeed\n'
fi
printf 'STAGE Prepare pnpm 11.1.3 inside guest\n'
curl --fail --silent --show-error --location --max-time 40 https://registry.npmjs.org/pnpm/11.1.3 -o "$job/pnpm-meta.json"
curl --fail --silent --show-error --location --max-time 120 https://registry.npmjs.org/pnpm/-/pnpm-11.1.3.tgz -o "$job/pnpm.tgz"
python3 - "$job" <<'PY'
from pathlib import Path
import json,hashlib,base64,sys
p=Path(sys.argv[1]);alg,digest=json.loads((p/'pnpm-meta.json').read_text())['dist']['integrity'].split('-',1)
assert base64.b64encode(hashlib.new(alg,(p/'pnpm.tgz').read_bytes()).digest()).decode()==digest
PY
mkdir "$job/pnpm" "$job/bin"
tar -xzf "$job/pnpm.tgz" -C "$job/pnpm"
printf '#!/bin/bash\nexec node "%s/pnpm/package/bin/pnpm.cjs" "$@"\n' "$job" > "$job/bin/pnpm"
chmod +x "$job/bin/pnpm"
export PATH="$job/bin:$PATH"
pnpm --version
run_stage docs-install pnpm install --frozen-lockfile --ignore-scripts
if [ "$LAST_RC" = 0 ]; then
    run_stage docs-test pnpm --filter docs test
    run_stage docs-types pnpm --filter docs type-check
    run_stage docs-build pnpm --filter docs build
    run_stage docs-routes pnpm --filter docs test:routes
else
    printf 'SKIP docs checks: locked dependency installation did not succeed\n'
fi
if [ "$failed" = 0 ]; then printf 'WORK_MODE_SANDBOX_ALL_PASSED\n'; else exit 1; fi
