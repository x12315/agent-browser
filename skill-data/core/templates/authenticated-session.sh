#!/bin/bash
# Reuse one task/site/account session; checkpoint only verified authentication.
# Usage: ./authenticated-session.sh <protected-url> <signed-in-text> [--checkpoint]
# Caller must retain AGENT_BROWSER_SESSION and these nonsecret validation inputs
# in the task record across invocations, plus any explicitly used nonsecret
# namespace/engine/profile launch identity. Never copy credential values into
# notes. browser() repeats the same full checks
# on EVERY call, including state save: restore checks are not sticky across CLI
# calls. A native checkpoint with no nonempty app-specific check is an error.
# The owner-private native store is the baseline; encryption is optional.
# For encryption, reuse a durable existing key from trusted local secret storage.
# Never generate or rotate keys/passwords, or use a per-run random key.
# Exit 2 means verification needs attention; the same browser stays open for
# manual login if needed. Other failures exit 1. No credentials are accepted.
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 || ( $# -eq 3 && "$3" != "--checkpoint" ) ]]; then
    printf 'Usage: %s <protected-url> <signed-in-text> [--checkpoint]\n' "$0" >&2
    exit 1
fi
if [[ ! "${AGENT_BROWSER_SESSION:-}" =~ ^[a-zA-Z0-9_-]+$ || "${AGENT_BROWSER_SESSION:-}" == "default" ]]; then
    printf 'Set AGENT_BROWSER_SESSION to the existing task/site/account key, not the shared default.\n' >&2
    exit 1
fi
if [[ -n "${AGENT_BROWSER_ENCRYPTION_KEY:-}" && ! "${AGENT_BROWSER_ENCRYPTION_KEY:-}" =~ ^[a-fA-F0-9]{64}$ ]]; then
    printf 'Invalid encryption key format; expected 64 hexadecimal characters. The supplied value is not shown.\n' >&2
    exit 1
fi
if ! command -v agent-browser >/dev/null 2>&1; then
    printf 'agent-browser is required.\n' >&2
    exit 1
fi

PROTECTED_URL="$1"
SIGNED_IN_TEXT="$2"
if [[ ! "$PROTECTED_URL" =~ ^https?:// || -z "$SIGNED_IN_TEXT" ]]; then
    printf 'Provide a protected HTTP(S) URL and a nonsecret signed-in account signal.\n' >&2
    exit 1
fi

browser() {
    agent-browser --session "$AGENT_BROWSER_SESSION" --restore \
        --restore-save auto --restore-check-url "$PROTECTED_URL" \
        --restore-check-text "$SIGNED_IN_TEXT" "$@" >/dev/null 2>&1
}

if ! browser session info --json; then
    printf 'Session inspection failed; existing state was not deleted.\n' >&2
    exit 1
fi

if [[ "${3:-}" != "--checkpoint" ]]; then
    if ! browser open "$PROTECTED_URL"; then
        printf 'Protected-page navigation failed; inspect this session before retrying. Existing state was not deleted.\n' >&2
        exit 1
    fi
fi

if ! browser wait --load domcontentloaded ||
   ! browser wait --url "$PROTECTED_URL" ||
   ! browser wait --text "$SIGNED_IN_TEXT"; then
    printf 'Signed-in verification did not pass. Inspect the same session for loading errors or the wrong account before requesting login.\n' >&2
    printf 'If login is needed, complete it manually in this same browser (use the dashboard if headless), then rerun with the same session key and --checkpoint.\n' >&2
    exit 2
fi

if ! browser state save; then
    printf 'Checkpoint failed; existing saved state was not deleted. Inspect readiness and configured login checks.\n' >&2
    exit 1
fi
printf 'Verified authentication checkpoint saved in the native private restore store.\n'
