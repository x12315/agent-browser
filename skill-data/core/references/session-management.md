# Session Management

Multiple isolated browser sessions with state persistence and concurrent browsing.

**Related**: [authentication.md](authentication.md) for login patterns, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [Named Sessions](#named-sessions)
- [Browser Work Modes](#browser-work-modes)
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

## Browser Work Modes

Use `--session <name> --work-mode collaboration|background` to protect an owned nondefault session's local Chrome launch policy. Select intent using the [core skill](../SKILL.md#choose-a-browser-work-mode): collaboration is headed, background is headless. MCP tools accept the same policy via the common `workMode` option and nondefault `session`.

```bash
agent-browser --session research --work-mode background open https://example.com
agent-browser --session research session info --json  # Inherits the mode
agent-browser --session research close               # Releases the mode
agent-browser --session research --work-mode collaboration open https://example.com
```

When using authenticated persistence, keep the same restore key and full application/account checks on every call, including these inspection and close commands. Record mode with session identity, but do not confuse it with saved authentication: policy lives in the daemon, survives omission on later calls, and is released by explicit close or daemon exit. Reassert it for a fresh daemon.

Mode overrides `headed` config/environment defaults; explicit opposite `--headed true|false` is an error even when the mode is inherited. While a protected browser is live, mode changes, adoption of an unprotected browser, launch configuration changes requiring relaunch, restore-key changes, and daemon fingerprint/version reconfiguration cannot implicitly close it. Resolve the conflict, then explicitly close only your owned session with any required authorization before reopening. Do not bypass protection with a kill/restart. If manual login requires moving from background to collaboration, make this transition explicit and retain the existing verified checkpoint flow.

Explicit `--work-mode` requires a read-only `session_info` capability handshake before existing-daemon version/configuration restart checks and after a fresh daemon is ready. Native `session_info` advertises `data.workModePolicyVersion: 1`; a missing/wrong marker or failed handshake rejects opt-in without killing, restarting, or mutating the daemon. Matching versions or source alone do not establish capability. Without an explicit mode, the existing path is unchanged. Explicit `close` cleanup remains available.

Work-mode cleanup via `close` still obeys action-policy and confirmation rules. Batch applies local launch configuration per inner command, so an explicit `close` can release the old mode before the next command reconfigures the browser. Use `--bail` to stop if close is denied. `chat` rejects `--work-mode` before making model requests because propagation to browser tool calls is not yet supported; use direct CLI or MCP browser tools for protected sessions.

Only locally launched Chrome is supported. Opt-in rejects external CDP, providers, auto-connect, profiles, other engines, launch-mutator plugins, and browser arguments overriding visibility or attachment. Background rejects extensions. Conflicting config/environment settings must also be removed; omitting the corresponding CLI flag is insufficient. Linux collaboration requires an existing `DISPLAY` and disables automatic private Xvfb. Neither the policy nor display presence verifies physical human visibility.

`session info --json` exposes `workMode` and `effectiveMode`, both at the top level of `data` and in `data.effectiveLaunch`. Browser response `data.lifecycle` and its `effectiveLaunch` carry the same fields. Values are `collaboration`/`background` and `headed`/`headless`, respectively, or `null` without a policy. These describe the effective policy, not proof of a visible window; inspect `browserLaunched` separately.

No new environment variable or config key is provided. Sessions that never opt in keep existing behavior. Use a deliberately built fork binary and matching bundled skills; editing source or installing the upstream CLI does not deploy this feature or migrate live daemons.

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

After a successful confirmed browser close, the shared CLI/MCP send path waits up to 5 seconds for the daemon endpoint to stop accepting connections. This includes close inside batch, close executed by `confirm`, and internal shutdown. After success, reopen immediately without an extra sleep. Endpoint confirmation prevents the old daemon from accepting the next launch and then disconnecting; it is not a fixed-delay workaround.

A timeout returns `success: false`, code `daemon_shutdown_unconfirmed`, and `data.daemonShutdownConfirmed: false` while preserving the original `closed: true` proof, including nested confirmation results. Browser close is confirmed, but daemon shutdown is not. The handshake does not kill, resend, or auto-launch; do not continue a work-mode transition on this failure. Confirmation prompts, denials, errors, and `outcome_unknown` do not trigger the wait. Preserve the no-resend rule for unknown mutations: reconcile their effects before considering another action.

Report the actual work mode, completed work, and any required user handoff concisely. Do not report a completed transition from a confirmation prompt or shutdown timeout, or claim physical visibility from headed policy alone.

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
