# Authentication Patterns

Login once within a task, verify the intended account, and reuse the same session. Persistence does not prevent server-side expiry, revocation, or MFA.

**Related**: [session-management.md](session-management.md), [trust-boundaries.md](trust-boundaries.md), [authenticated-session.sh](../templates/authenticated-session.sh).

## Check before asking for login

Derive one stable task/site/account key once, using a nonsecret prefix. Record that nonsecret key, the application/account-specific validation options, and any explicitly used nonsecret namespace/engine/profile launch identity in task notes and recover them across shell, context, harness, and working-directory changes. Never copy credential values into notes. A worktree-scoped derivation can change after changing directories, so do not rerun it during the task. Never hijack the shared default or reuse another account's session.

```bash
# Once at task start, not on every invocation
SESSION="$(agent-browser session id --scope worktree --prefix task-myapp-work)"
export AGENT_BROWSER_SESSION="$SESSION"
export AGENT_BROWSER_RESTORE="$SESSION"
# Recreate this helper in each shell using the options retained in task notes.
browser() {
    agent-browser --session "$SESSION" --restore --restore-save auto \
        --restore-check-url "https://app.example.com/dashboard" \
        --restore-check-text "Work account dashboard" "$@"
}
browser session info --json
browser open https://app.example.com/dashboard
browser wait --load domcontentloaded
browser wait --url "https://app.example.com/dashboard"
browser wait --text "Work account dashboard"
```

Use a nonsecret application/account-specific signal, not just the absence of a login URL. If the current or restored session is authenticated as the intended account, continue the task without another login. A timeout or navigation failure alone is not proof of expiry: inspect session/restore status and the page before asking the user to authenticate.

Pass the explicit session, restore, and same full validation flags on every command, including `state save` and close, as the helper does. Validation flags are not sticky across CLI calls: omitting them on a later `--restore` call clears the checks supplied at launch. Environment variables and shell helpers may not survive shell or harness changes; recreate the helper from the nonsecret task record.

## Manual login and immediate checkpoint

If login is genuinely needed, leave the SAME browser open and let the user complete login, SSO, or MFA there. Use its dashboard if the session is headless. Do not accept passwords or tokens as command-line arguments, print them, or extract them with page JavaScript. After the user finishes, preserve the current page and verify the app and intended account:

```bash
# Use the browser helper above with the SAME checks on every invocation.
browser wait --load domcontentloaded
browser wait --url "https://app.example.com/dashboard"
browser wait --text "Work account dashboard"
browser state save
```

Pathless `state save` writes an explicit checkpoint to the native private restore store. No encryption key is required: on Unix, owner-private directories (0700) and files (0600) protect plaintext state by default. Windows inherits the destination's ACLs; use trusted private user storage. Optional `AGENT_BROWSER_ENCRYPTION_KEY` encryption should reuse a durable existing 64-hex-character key from trusted local secret storage. Reject a malformed supplied key without printing it; never generate or rotate keys or passwords, or use a per-run random key. A missing key is not a reason to request login, reconstruct authentication, or export cookies. The native command requires a ready page and at least one nonempty application-specific `--restore-check-url`, `--restore-check-text`, or `--restore-check-fn`, and verifies every supplied check before saving. Missing checks are an error; readiness alone must not save a generic login page. A failed check/save preserves the previous checkpoint and does not clear a prior restore-failure autosave block. Only a successful checkpoint clears that block. Do not wait 30 seconds for autosave after login or change `--restore-save auto` globally to `always`.

Use [authenticated-session.sh](../templates/authenticated-session.sh) for a small executable workflow:

```bash
# AGENT_BROWSER_SESSION must be set; an existing encryption key is optional
./authenticated-session.sh https://app.example.com/dashboard "Work account dashboard"
# If verification needs attention, it exits 2 and leaves the same browser open.
# After manual login, checkpoint without navigating away:
./authenticated-session.sh https://app.example.com/dashboard "Work account dashboard" --checkpoint
```

The template does not log credentials, export plaintext state to the repository, clear cookies, delete old state, or close any session. Normal mode navigates to the protected target first; checkpoint mode preserves the current page after manual login. Both modes verify readiness, URL and account text, then call native pathless `state save` exactly once to save refreshed authentication. A failed check or save exits with failure without retrying, closing the browser, or deleting saved state.

## Auth vault and credential providers

For an authorized automated login, use an existing auth vault entry or configured credential-provider plugin rather than secret arguments. Check current/restored authentication first. Have the user provision credentials using the secure vault input workflow outside agent logs.

```bash
browser auth login my-app
```

`auth login` filters matching controls by layout size, computed visibility and opacity, and disabled/readonly state, including custom CSS selectors. If a selected credential field is replaced or redirects focus before entry, it fails without submitting. After submission, verify the account and checkpoint as above; form submission alone does not prove successful authentication.

For a form reached through an in-page click, challenge clearance, or consent dismissal, preserve the prepared document:

```bash
browser open https://app.example.com/
browser click "a[href='/login']"
browser auth login my-app --no-navigate
```

`--no-navigate` suppresses only the initial navigation. It requires an active top-level HTTP(S) page and compares the page origin with the effective credential URL using scheme, host, and effective port before filling credentials. Paths, queries, and fragments may differ. Selector waits, filling, submission, and submit-triggered navigation remain unchanged. A command-level `--url` overrides stored or provider URL metadata and becomes the expected-origin constraint:

```bash
browser auth login work --no-navigate --url https://identity.example.com/login
```

Credential-provider plugins are external executables configured in `agent-browser.json` using the `agent-browser.plugin.v1` stdio JSON protocol. Inspect the configured plugin before granting secret access:

```bash
browser plugin list
browser plugin show vault
browser auth login my-app --credential-provider vault --item "My App"
browser auth login my-app --credential-provider vault --item "My App" --no-navigate --url https://identity.example.com/login
browser --confirm-actions plugin:vault:credential.read auth login my-app --credential-provider vault --item "My App"
```

Use `--url`, `--username-selector`, `--password-selector`, and `--submit-selector` to override provider metadata for that login. Provider secrets remain in memory rather than generated process arguments or normal output. Never put vault tokens or passwords in plugin arguments or repository config. Use the vault vendor's local login/session mechanism.

## Restarts, expiry, and account changes

```bash
# Routine close preserves configured persistence; never clear auth as cleanup.
browser close
# Later, recover the SAME key and checks; recreate the helper before reuse.
browser open https://app.example.com/dashboard
```

Allow the application to refresh its own tokens. Do not read or print raw cookies/localStorage tokens to decide whether authentication works. Test the protected application and intended account instead. Server expiry, revocation, MFA policy, or an intentional account switch can genuinely require login; explain the observed reason. Use a distinct task/site/account key for another account rather than silently replacing an unrelated session.

Routine close, idle timeout, navigation failure, or a failed restore is not authorization to clear cookies, delete saved authentication, or launch repeated login attempts. Preserve the previous checkpoint and investigate the failure.

## Explicit imports and portable files

Prefer native session restore over importing cookies or copying a broad Chrome profile. Attach to an existing browser or import a narrowly scoped file only when explicitly needed and authorized for the task; attached tabs can share cookies with the user's browser. Remote debugging grants full browser control to local clients.

Explicit `state save <path>`, `state load <path>`, and `--state <path>` remain portable-file operations. Unlike pathless checkpointing, they require managing a sensitive file. Keep it outside the repository, protect access, and never print or commit its contents. Do not ask the user to paste DevTools cURL requests, bearer tokens, or cookie dumps into chat or logs. A native restore checkpoint avoids these exports for normal session reuse.

## Security best practices

- Keep raw cookies, tokens, credentials, portable state files, and encryption keys out of Git, logs, transcripts, screenshots, and task notes. Record only the nonsecret session key, application/account-specific validation options, and any explicitly used nonsecret namespace/engine/profile launch identity.
- Use the native private store as the baseline. For optional encryption, reuse a durable trusted existing key; never generate or rotate keys or passwords, or use a per-run random key. Reject malformed supplied keys without printing them.
- Preserve authentication on routine failures and normal close. Delete or revoke it only for an explicit cleanup/logout request or a known retention policy, not as a retry strategy.
- Verify application and account identity after login and restore. Persistence is not a promise that tokens never expire.
