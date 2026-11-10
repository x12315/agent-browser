# Work-mode sandbox validation

The consumer case lives at `tests/sandbox/work-mode/`; its guest scripts live here. Run through the existing agent-harness-sandbox project interface. Do not run the browser, Cargo tests or documentation server on the work host.

Prepare `dist/sandbox-inputs/work-mode-source.tar.gz` from the reviewed tracked source files, preserving repository-relative paths and excluding Git metadata, build output, installed dependencies and authentication. The source snapshot must include the dirty Rust/docs edits being tested. Record its SHA256 and source identity in `dist/sandbox-inputs/manifest.json`; this generated directory is ignored by Git. The case separately injects `guest.sh` and `cli-smoke.py`.

Load the administrator-maintained private macOS-ready profile in your shell, then use the producer entry:

```bash
TART_CPUS=2 TART_MEMORY_MB=4096 CASE_TIMEOUT=2400 \
  /path/to/agent-harness-sandbox/bin/test.sh --project /path/to/agent-browser work-mode
```

The runner uses a disposable macOS GUI clone without a host viewer, clipboard, USB or shared directories. Tools and dependencies are prepared only in that clone: official Rust1.98.1, registry-integrity-verified pnpm11.1.3 and locked project dependencies. The original stopped base and the installed host CLI are not modified.

Stages: full nonignored Rust unit suite; the three `e2e_work_mode` Chrome tests; an actual built-CLI mode/conflict/batch-close smoke; documentation unit, type, production-build and route checks. Stage markers and errors stream into the run directory's `guest/tmp/ah.out` and `ah.err`. Overall PASS requires every stage and the final success marker. Do not report compilation or a prior installed binary's smoke as feature verification.

No model API calls are made by this case. It tests tool enforcement, not whether an LLM infers the correct human intent. It does not install the resulting fork on the host or migrate a live daemon.

## Relocatable native installation bundle

The separate `tests/sandbox/work-mode-install/` consumer builds the frozen formatted runtime source archive `dist/sandbox-inputs/work-mode-install-source.tar.gz` (SHA256 `04168d1afd9cff1590d9d59689e754f610941248e96089cae6d25048b63bbacb`). It reuses the declared public toolchain, registry and dependency-cache inputs with the same validated private guest build prefix. After reviewing the consumer and loading the existing private macOS-ready profile, run:

```bash
TART_CPUS=2 TART_MEMORY_MB=4096 CASE_TIMEOUT=2400 \
  /path/to/agent-harness-sandbox/bin/test.sh --project /path/to/agent-browser work-mode-install
```

Guest stages verify input hashes; build offline with `cargo build --locked --offline --manifest-path cli/Cargo.toml --profile ci -j2` (the upstream optimized, stripped CI profile, not a release); copy the binary, `skills/`, `skill-data/` and licenses into a separate private `package/`; ad-hoc sign and verify; check relocated core discovery with the skills override unset and working directory outside source; run an owned background snapshot/click/screenshot fixture and the existing six-check CLI regression against the package binary while preserving the old CLI on PATH; seal a manifest with file hashes/modes, source hash, CI profile and observed capability version 1; export the tarball; leave an owned teal collaboration page titled `Installed Work Mode Verification` open for producer capture after a bounded actual on-screen window check. Only VM cleanup closes that successful final session. No external website, host viewer, root/TCC grant, global CLI replacement or host package execution is used. Temporary guest caffeinate expires after 2400 seconds and its own process is killed on script exit.

The tar contains a single `package/` root with `bin/agent-browser`, content directories, `LICENSE`, `licenses/` and `manifest.json`. The manifest maps every other regular file to SHA256, byte count and octal mode; it contains no source-directory link. The signed executable is hashed after signing. The bundle is emitted on stdout between `==AH_NATIVE_BUNDLE_BEGIN==` and `==AH_NATIVE_BUNDLE_END==`, with a JSON `{sha256,bytes}` header and base64 wrapped at 76 columns. The host assertion only decodes and inspects data: it requires guest rc 0, all six named smoke checks, matching packaged core and package PASS markers, final window proof and a producer `gui.png` with a valid PNG signature, then validates archive membership and every manifest hash/mode without extracting or executing the package. Successful output is `RUN_DIR/native-bundle.tar.gz` plus `native-bundle-manifest.json`. Failures retain producer logs and any partial export; there is no retry or unknown-outcome replay. This case does not repeat the full Rust/docs suites or install on the host; passing is prerequisite evidence for a separately reviewed installation.
