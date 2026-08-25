# Testing

Tests must use synthetic data. Never capture fixtures, snapshots, screenshots, or
recordings from a real Google account.

## Test layers

- **Domain/update tests:** deterministic state transitions, request correlation,
  cancellation, pagination, and recovery.
- **Keybinding tests:** defaults, overrides, unbinding, conflicts, normalized input,
  semantic dispatch, and registry-derived labels.
- **Adapter tests:** local mock HTTP services with hand-authored synthetic payloads.
- **Credential tests:** temporary directories and unmistakably synthetic secrets;
  verify permissions, deletion, redaction, and theft-model behavior.
- **Render tests:** deterministic frames using synthetic spaces and messages.
- **PTY/input tests:** startup, mouse and keyboard input, resize, interruption, and
  terminal restoration.
- **Live acceptance:** ignored and manual. Use an isolated test account and do not
  persist output or captured private data.

## Standard validation

```sh
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo nextest run --no-fail-fast
cargo machete --with-metadata
cargo deny check
./scripts/check-architecture.sh
./scripts/check-public-safety.sh
./scripts/smoke-pty.sh
```
