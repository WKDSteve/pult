# pult

A personal virtual audio mixer for Windows in Rust (Linux and macOS are meant to work too, see README). The README and
`docs/` are in German on purpose; code, code comments and commit messages are in English.

## Layout

- `crates/pult-core`: mixing, routing, levels, drift control; no device I/O, fully testable.
- `crates/pult-engine`: devices through `cpal`, profiles, process loopback (`ffi`).
- `crates/pult-hub`: the local control interface on `appkit-hub` (`docs/CONTROL.md`), with the remote control page in
  `crates/pult-hub/ui` (one HTML, one JS, two CSS files, vanilla, no build step; CSP and Trusted Types in `src/pages.rs`).
- `crates/pult-app` (the window), `pult-cli`, `pult-midi`, `pult-plughost` (spike).

## Rules

- Real time: nothing allocates, locks or logs in an audio callback (`assert_no_alloc` aborts a debug build).
- `unsafe` only in the `ffi` module of `pult-engine`, in `pult-app` and in `pult-plughost`.
- Measuring runs of an automatic session use only "Voicemeeter In 4" and "Voicemeeter In 5"; every other device may be
  what someone is listening to (README, "Bauen und prüfen").
- CI runs on the author's self-hosted Windows runner (`.github/workflows/ci.yml`); no `pull_request` trigger on purpose.

## Checks

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
cargo audit --deny warnings   # the ignored advisories are in ci.yml
```

The page: `pnpm --dir tools/page-check install --frozen-lockfile`, `cargo build -p pult-hub --example demo-hub --locked`,
`node tools/page-check.mjs tools/page-check/node_modules target/debug/examples/demo-hub.exe` (Chrome, axe light and
dark). Test sheets: `TESTS/sheets`, started through `.testing.lua` (testing.nvim).

## Web platform
- Browser Support: Baseline Newly available (`.mwgrc`; no `browserslist`, the page has no package.json and no build
  step). The hub listens on `127.0.0.1` only, so the page opens in the local desktop browser (`pult open`), an
  evergreen Chrome, Edge or Firefox; the page check runs in Chrome. Newer features only behind feature detection with
  a fallback; polyfills where browsers still in use need them (rather one too many), run only after the check: dynamic `import()`, or bundled in a `file://` page.
- Before writing or changing HTML, CSS or client-side JS, use the `modern-web-guidance` skill (`search` → `retrieve` →
  verify). Not for Rust, backend, CI or build config.
- For open-ended modernisation: `npx -y modern-web-guidance@latest legacy-scan crates/pult-hub/ui --json`, check each
  finding against `keepWhen`/`dropWhen`. Local and agent sessions only, no CI step.
- Rules: the maintainer's ruleset for the web platform (private, so not named or cited here by id).
- `.claude/settings.json` turns the tool's telemetry off and allows only `npx -y modern-web-guidance@latest *`.
- Git: in a cloud session (`CLAUDE_CODE_REMOTE` is set) never push to `main`, only to your own `claude/…` branch; a local session lands it by rebase and the device key re-signs it (`GIT-09`). "Push to main" (`GIT-01`) applies locally only.
