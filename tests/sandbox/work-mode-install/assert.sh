#!/bin/bash
# Host assertion is data-only: never extract or execute the native package.
set -euo pipefail
D=${1:?run directory required}
command -v python3 >/dev/null
python3 "${AHSB_INPUT_ROOT:?}/test/sandbox/work-mode/install-export.py" consume "$D"
