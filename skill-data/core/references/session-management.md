# Session Management

Multiple isolated browser sessions with state persistence and concurrent browsing.

**Related**: [authentication.md](authentication.md) for login patterns, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [Named Sessions](#named-sessions)
- [Session Isolation Properties](#session-isolation-properties)
- [Tab Pinning in a Shared Browser](#tab-pinning-in-a-shared-browser)
- [Session State Persistence](#session-state-persistence)
- [Common Patterns](#common-patterns)
- [Default Session](#default-session)
- [Session Cleanup](#session-cleanup)
- [Best Practices](#best-practices)

## Named Sessions

Use `--session` to isolate browser contexts. Derive ONE stable task/site/account id once, with a nonsecret prefix distinguishing the intended account, and pass it with `--restore` on every separate shell command:

```bash
SESSION="$(agent-browser session id --scope worktree --prefix task-myapp-work)"
browser() {
    agent-browser --session "$SESSION" --restore --restore-save auto \
        --restore-check-url "https://app.example.com/dashboard" \
        --restore-check-text "Work account dashboard" "$@"
}
browser open https://app.example.com/dashboard
```

`--scope worktree` uses the Git worktree root when available, then the Git root, then the canonical current directory. Record the resulting nonsecret key, application/account-specific validation options, and any explicitly used nonsecret namespace/engine/profile launch identity in task/handoff notes. Retain them across harness and working-directory changes; never copy credential values into notes. Recreate the `browser` helper in each shell with those same options. Repeat the same full validation flags on every `--restore` command, including inspection, waits, checkpointing, and close: checks are not sticky across CLI calls, and a later call omitting them clears launch checks. Recover the same key across separate shells, context resets, harness switches, or working-directory changes; do not rerun derivation after changing directories. Never adopt the shared default or an unrelated account's session. Environment defaults alone are insufficient across shell boundaries.

```bash
# Session 1: Authentication flow
agent-browser --session auth open https://app.example.com/login

# Session 2: Public browsing (separate cookies, storage)
agent-browser --session public open https://example.com

# Commands are isolated by session
agent-browser --session auth fill @e1 "user@example.com"
agent-browser --session public get text body
```

## Session Isolation Properties

Each session has independent:
- Cookies
- LocalStorage / SessionStorage
- IndexedDB
- Cache
- Browsing history
- Open tabs

## Tab Pinning in a Shared Browser

Full isolation applies when each session launches its own browser. When sessions instead share one Chrome over `--cdp <port>`, cookies and storage are shared, and only the tab selection separates the sessions. Add `--pin-tab` so each session sticks to its own tab:

```bash
agent-browser --session agent1 --cdp 9222 --pin-tab open https://site-a.com
agent-browser --session agent2 --cdp 9222 --pin-tab open https://site-b.com
```

Every session remembers which tab it is bound to (by CDP target id, persisted in the session's state directory), so a restarted daemon reattaches to the session's own tab instead of adopting the most recently active one. `--pin-tab` (env `AGENT_BROWSER_PIN_TAB=1`) additionally makes the binding strict:

- Attaching with no binding opens a fresh tab instead of adopting an existing one
- If the bound tab is closed, commands fail with a `tab_gone` error instead of silently acting on another tab. JSON output includes `"code": "tab_gone"`, `data.targetId`, and optional `data.lastUrl`
- Recovery commands still work in that state: run `tab new <url>` to bind a fresh tab, or `tab list` and switch to an existing one
- Tabs opened by other sessions or the user never steal the pinned session's active tab

The flag is sticky per session: pass it once at session creation and later commands and daemon restarts keep the strict semantics. Pass `--no-pin-tab` to explicitly turn the pin off again. Use each tab's `targetId` from `tab list --json` when one session needs to reference another session's tab; target ids stay stable across daemon restarts.

The structured `lastUrl` is limited to sanitized HTTP(S) URLs and `about:blank`. Credentials, query strings, and fragments are removed from HTTP(S) URLs. Opaque URLs such as `data:` are omitted. In batch JSON, the recovery object appears under `result` instead of `data`.

When re-running a shared-tab script such as the repro from #1530, add `--pin-tab` to the first command for every session. Without it, `open` intentionally preserves the legacy behavior and navigates the shared active tab, so the original script still collides. The same rule applies when sessions attach with `--auto-connect` instead of `--cdp`.

## Session State Persistence

### Automatic Restore

```bash
# Bare --restore uses the current --session as the persistence key
# Recover SESSION and recreate the browser helper from task notes.
browser open https://app.example.com/dashboard
```

When `--restore` or another restore key is configured, state is loaded before navigation and saved on close, daemon shutdown, idle timeout, and compatible relaunch. It is also saved periodically while the browser is open (after commands settle, at most once per `AGENT_BROWSER_AUTOSAVE_INTERVAL_MS`, default 30000; set to `0` to save only on close), so a browser window the user closes by hand still leaves a recent save behind. A session ID by itself only isolates the daemon and does not enable persistence; without a restore key, shutdown discards transient browser state and open tabs. Idle sessions with configured persistence keep saving on the same interval, capturing changes the page makes on its own such as token refreshes. The daemon exits after one hour without commands or dashboard input by default; `--idle-timeout <time>` or `AGENT_BROWSER_IDLE_TIMEOUT_MS` tunes this, and `0` disables it. Headed, Safari/iOS WebDriver, and user-attached browsers are exempt from the default timeout; provider-owned cloud browsers are not. The default save policy is `--restore-save auto`, which skips auto-save if restore failed or validation failed; `never` disables periodic autosave too.

```bash
# The helper repeats both application/account-specific checks on every call.
browser open https://app.example.com/dashboard
browser session info --json
# If adding --restore-check-fn, use a nonsecret signed-in app/account signal
# and retain that same expression in the helper and task record too.
```

Use the same helper for diagnostics so inspection does not clear validation:

```bash
browser session info --json
```

### Immediate verified checkpoint

The [authenticated-session.sh](../templates/authenticated-session.sh) template saves refreshed authentication in both modes: normal mode first navigates to the protected target, while `--checkpoint` preserves the current manual-login page. Both verify readiness, URL and account text before calling native pathless `state save` exactly once. Failure does not retry, close the browser, or delete saved state.

Before prompting for login, inspect the current session/restore status and visit the protected page. Reuse valid authentication for the intended account. If the user must log in, leave the SAME browser open; then verify readiness, the protected URL, and an application/account-specific signal and checkpoint immediately:

```bash
# Use the helper above with the SAME validation flags on every invocation.
browser wait --load domcontentloaded
browser wait --url "https://app.example.com/dashboard"
browser wait --text "Work account dashboard"
browser state save
```

Pathless `state save` writes to the native private restore store without requiring an encryption key. On Unix the baseline is plaintext state in owner-private directories (0700) and files (0600). Windows inherits destination ACLs; use trusted private user storage. For optional `AGENT_BROWSER_ENCRYPTION_KEY` encryption, reuse a durable existing 64-hex-character key from trusted local secret storage. Reject malformed supplied keys without printing them. Never generate or rotate keys or passwords, or use a per-run random key. A missing key does not require login, authentication reconstruction, or cookie exports. The native checkpoint requires page readiness and at least one nonempty application-specific `--restore-check-url`, `--restore-check-text`, or `--restore-check-fn`, and verifies every supplied check before saving. Missing checks are an error; a ready generic login page is not enough. Only a successful checkpoint clears a prior restore-failure autosave block; failure preserves the old state and block. Do not wait for periodic autosave after login or switch globally to `--restore-save always`. Automatic saves remain governed by the existing policy, normally `auto`.

### Manual State Files

Use `state save <path>`, `state load <path>`, and `--state <path>` only when an explicit portable file is needed; these operations are unchanged. Store sensitive files outside the repository and never log or commit their contents. Do not construct paths under `~/.agent-browser/sessions/`; prefer native restore and pathless checkpoints for reusable agent sessions.

## Common Patterns

### Authenticated Session Reuse

```bash
#!/bin/bash
# Recover SESSION and recreate the browser helper from task notes.
browser open https://app.example.com/dashboard
```

### Concurrent Scraping

```bash
#!/bin/bash
# Scrape multiple sites concurrently

# Start all sessions
agent-browser --session site1 open https://site1.com &
agent-browser --session site2 open https://site2.com &
agent-browser --session site3 open https://site3.com &
wait

# Extract from each
agent-browser --session site1 get text body > site1.txt
agent-browser --session site2 get text body > site2.txt
agent-browser --session site3 get text body > site3.txt

# Cleanup
agent-browser --session site1 close
agent-browser --session site2 close
agent-browser --session site3 close
```

### A/B Testing Sessions

```bash
# Test different user experiences
agent-browser --session variant-a open "https://app.com?variant=a"
agent-browser --session variant-b open "https://app.com?variant=b"

# Compare
agent-browser --session variant-a screenshot /tmp/variant-a.png
agent-browser --session variant-b screenshot /tmp/variant-b.png
```

## Default Session

When `--session` is omitted, commands use the default session:

```bash
# These use the same default session
agent-browser open https://example.com
agent-browser snapshot -i
agent-browser close  # Closes default session
```

## Session Cleanup

```bash
# Close specific session
agent-browser --session auth close

# List active sessions
agent-browser session list
```

## Best Practices

### 1. Name Sessions Semantically

```bash
# GOOD: Clear purpose
agent-browser --session github-auth open https://github.com
agent-browser --session docs-scrape open https://docs.example.com

# AVOID: Generic names
agent-browser --session s1 open https://github.com
```

### 2. Close Only Your Owned Session

```bash
browser close
```

Routine close, timeout, navigation errors, and restore failures are not reasons to clear cookies or delete authentication state. Preserve the known-good checkpoint. Expiry, revocation, MFA, or an intentional account switch may require login; explain the observed reason rather than promising tokens never expire.

### 3. Keep Authentication Private

Keep raw cookies, tokens, state files, and encryption keys out of Git, logs, and handoff notes. Use the native private restore store rather than repo-local plaintext files, raw token extraction, or broad Chrome-profile copying. Delete saved authentication only when explicitly requested or required by an established retention policy, never as routine error recovery.

### 4. Timeout Long Sessions

```bash
# Set timeout for automated scripts
timeout 60 agent-browser --session long-task get text body
```
