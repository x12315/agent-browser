#!/bin/bash
set -euo pipefail
job=$(mktemp -d /tmp/agent-browser-work-mode-cli.XXXXXX)
/usr/bin/caffeinate -d -i -u -t 1200 &
keep_awake_pid=$!
trap 'kill "$keep_awake_pid" 2>/dev/null || true' EXIT
export NODE_OPTIONS=--max-old-space-size=2048
mkdir -p "$job/source"
tar -xzf /tmp/ahsb-push/work-mode-source.tar.gz -C "$job/source"
cd "$job/source"
export AGENT_BROWSER_SKILLS_DIR="$job/source/skill-data"
chmod +x /tmp/ahsb-push/agent-browser-candidate
printf 'STAGE Candidate identity\n'
shasum -a 256 /tmp/ahsb-push/agent-browser-candidate
/tmp/ahsb-push/agent-browser-candidate --help | grep -- --work-mode
printf 'STAGE Candidate actual CLI work-mode regression\n'
python3 /tmp/ahsb-push/cli-smoke.py /tmp/ahsb-push/agent-browser-candidate
printf 'STAGE Matching bundled core discovery\n'
/tmp/ahsb-push/agent-browser-candidate skills get core | grep -- --work-mode
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
printf 'STAGE Docs locked install\n'
pnpm install --frozen-lockfile --ignore-scripts
printf 'STAGE Docs tests\n'
pnpm --filter docs test
printf 'STAGE Docs type-check\n'
pnpm --filter docs type-check
printf 'STAGE Docs production build\n'
pnpm --filter docs build
printf 'STAGE Docs routes\n'
pnpm --filter docs test:routes
printf 'WORK_MODE_CLI_DOCS_SANDBOX_ALL_PASSED\n'
