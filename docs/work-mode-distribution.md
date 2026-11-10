# GitHub work-mode distribution

This personal fork distributes the native work-mode CLI and matching core Skills through a fixed GitHub Release. It is not a Pi Extension, and the upstream npm/Homebrew package does not contain this fork's work-mode policy.

## Fixed source and artifact

- Repository: https://github.com/x12315/agent-browser
- Source tag: https://github.com/x12315/agent-browser/tree/x12315-work-mode-0.39.1-r1
- Release: https://github.com/x12315/agent-browser/releases/tag/x12315-work-mode-0.39.1-r1
- macOS ARM64 bundle: https://github.com/x12315/agent-browser/releases/download/x12315-work-mode-0.39.1-r1/agent-browser-work-mode-darwin-arm64-ci.tar.gz
- Bundle SHA256: `e8d179f0f0ccc3bffa2d0b40b55845b84b8fcf486c84877479689123e1f7674e`
- Native executable SHA256: `ba5988cbacc029e593ccee9e79f5bbef51e387f374109ed8778e96348e0c660a`

The release provenance records the full source commit, frozen build-input hash and file-content mapping. The bundle retains the base version `0.39.1`; version strings alone do not identify this fork. It uses the existing optimized Cargo `ci` profile and ad-hoc signing, not Apple Developer ID signing or notarization. Only macOS ARM64 is distributed here. The full dashboard was not built or qualified.

Do not install the Git repository through npm and assume that it builds this Rust feature: the existing upstream postinstall fallback downloads upstream binaries. Do not use automatic `upgrade` for this personal bundle. Update through an explicitly selected GitHub release and verified digest instead.

## Fresh parallel installation

Keep an existing `agent-browser` executable and all live sessions untouched. The package layout is required for adjacent `skills/` and `skill-data/` discovery; do not copy just the binary. The following example deliberately fails if the destination or new entry already exists, rather than overwriting an existing installation.

```bash
test "$(uname -s)" = Darwin && test "$(uname -m)" = arm64 || exit 1
sha=e8d179f0f0ccc3bffa2d0b40b55845b84b8fcf486c84877479689123e1f7674e
tmp=$(mktemp -d)
curl --fail --location 'https://github.com/x12315/agent-browser/releases/download/x12315-work-mode-0.39.1-r1/agent-browser-work-mode-darwin-arm64-ci.tar.gz' -o "$tmp/bundle.tar.gz"
printf '%s  %s\n' "$sha" "$tmp/bundle.tar.gz" | shasum -a 256 -c - || exit 1
base="$HOME/.local/share/agent-browser/work-mode"
mkdir -p "$base" "$HOME/.local/bin"
mkdir "$base/$sha" || exit 1
tar -xzf "$tmp/bundle.tar.gz" -C "$base/$sha" || exit 1
ln -s "$base/$sha/package/bin/agent-browser" "$HOME/.local/bin/agent-browser-work-mode"
```

Ensure `~/.local/bin` is on PATH. Inspect the manifest and release provenance before execution. If macOS requires operator approval, follow its normal approval process; do not disable platform protections. For an existing installation, compare the installed manifest/file hashes and preserve the current entry until an explicit update is authorized.

Use the same selected executable for core discovery and all commands in a new task:

```bash
agent-browser-work-mode skills get core
agent-browser-work-mode --session bg-example --work-mode background open https://example.com
```

Existing tasks continue using their original executable and session. Do not migrate an active unprotected daemon or remove `--work-mode` to evade a refusal. Explicit mode requires `session_info` capability `workModePolicyVersion: 1` and a nondefault named session.

## Evidence and limits

The frozen runtime logic passed 1514 nonignored unit/integration tests, three new native Chrome E2E tests, real CLI checks and documentation checks. The exact optimized installation bundle separately passed relocated matching-core discovery, background snapshot/click/PNG, all six policy/legacy-daemon checks, signature verification and a real visible collaboration window captured by the sandbox producer. The release reuses those exact bundle bytes; publication is not a new build or a claim of reproducible builds/GitHub attestation.

Full Harness doctor, LLM intent-selection evaluation, real human handover, other-platform browser checks and the remaining ignored tests were not completed. A visible test window is not a guarantee of physical visibility or operator access in every environment. The mode policy is not an OS security boundary.
