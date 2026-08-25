# gchatui Agent Guidelines

Read and preserve [INVARIANTS.md](INVARIANTS.md) before changing covered code.
Invariants are acceptance criteria, not suggestions. Surface conflicts instead of
adding bypasses or temporary architecture.

## Required development behavior

- Keep changes focused on the requested product outcome.
- Add crates only for implemented domain ownership boundaries.
- Prefer suitable `bmux_tui_components`; do not duplicate controls locally.
- Route keyboard input through semantic actions and the configurable registry.
  Never hardcode key chords in feature handlers or presentation surfaces.
- Preserve first-class mouse behavior through bmux hit maps and interaction routing.
- Use synthetic data exclusively in tracked files and generated test artifacts.
- Never add OAuth files, tokens, keys, real account details, conversation captures,
  private identifiers, screenshots, recordings, or sensitive local denylists.
- Add focused tests with behavioral changes.
- Fix clippy root causes rather than adding broad suppressions.
- Use `cargo fmt` during implementation; leave Rust files formatted.

## Validation

For every non-documentation change, run:

```sh
cargo fmt
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo nextest run --no-fail-fast
cargo machete --with-metadata
cargo deny check
./scripts/check-architecture.sh
./scripts/check-public-safety.sh
```

Run relevant PTY/input tests for TUI, input, lifecycle, or terminal changes:

```sh
./scripts/smoke-pty.sh
```

Run cross-platform CI for dependency, credential, release, or platform-sensitive work.
Documentation-only changes require Markdown and public-safety checks.

If a required command cannot run, explain why. Completion reports must list each
required command as PASS, FAIL, or SKIPPED with a reason.
