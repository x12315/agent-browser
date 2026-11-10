#!/bin/bash
set -euo pipefail
D=${1:?run directory required}
test "$(< "$D/guest/tmp/ah.rc")" = 0
grep -q 'WORK_MODE_CLI_DOCS_SANDBOX_ALL_PASSED' "$D/guest/tmp/ah.out"
grep -q '"fork_cli_work_mode_smoke": "PASS"' "$D/guest/tmp/ah.out"
test -s "$D/project-source.sha256"
printf 'Exact guest-built candidate CLI, native mode isolation and docs PASS\n'
