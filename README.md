# gchatui

A mouse- and keyboard-friendly Google Chat terminal client written in Rust.

> **Status:** Early development. The first release is a read-only client.

## Initial scope

The first usable release will support:

- OAuth authorization using a user-provided Google desktop client configuration
- Browsing spaces, messages, and thread replies
- Pagination and manual refresh
- Configurable keybindings whose active values drive both behavior and UI hints
- First-class mouse and keyboard navigation
- macOS, Linux, and Windows

Sending messages, reactions, search, read-state mutation, local conversation
history, and real-time Workspace Events are outside the first release.

See the [official OAuth rollout](docs/official-oauth-rollout.md) for the planned
maintainer-operated sign-in experience and release gates. Local testing currently
still requires a private desktop OAuth client file; the official client is not yet
bundled or verified. A [privacy-policy draft](docs/privacy-policy-draft.md) is
available for maintainer review before publication.

## Privacy and public development

Repository content uses synthetic examples only. Do not add real account details,
workspace names, conversation content, private identifiers, OAuth credentials,
tokens, screenshots, recordings, or diagnostic captures.

## Development

Install stable Rust, then run:

```sh
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

See [TESTING.md](TESTING.md), [INVARIANTS.md](INVARIANTS.md), and
[AGENTS.md](AGENTS.md) before making changes.

## License

MPL-2.0. See [LICENSE](LICENSE).
