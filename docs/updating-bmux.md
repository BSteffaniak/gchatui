# Updating bmux

gchatui follows the `master` branch for its bmux Git dependencies. `Cargo.lock`
records the exact commit used by normal builds and CI.

Advance bmux intentionally:

```sh
cargo update -p bmux_tui
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo nextest run --no-fail-fast
cargo machete --with-metadata
./scripts/check-architecture.sh
./scripts/check-public-safety.sh
```

Review the lockfile commit change and transitive dependency changes. Exercise mouse,
keyboard, resize, and terminal-restoration acceptance tests before accepting an
update. Do not replace Git dependencies with sibling paths or copied source.
