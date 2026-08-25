# gchatui Architectural Invariants

These are acceptance criteria for every valid change.

## Product and data boundaries

- **Google remains authoritative.** Spaces, messages, and threads come from Google;
  transient UI state and indexes do not become competing sources of truth.
- **The first release is read-only.** Write operations and persisted conversation
  caches require an explicit product and architecture decision.
- **Repository content is public-safe.** Source, history, tests, fixtures, examples,
  logs, diagnostics, screenshots, recordings, and release artifacts contain only
  synthetic data. Real private affiliations, accounts, people, workspaces,
  conversations, credentials, and identifiers are prohibited.

## Architecture

- **Domain semantics are presentation-independent.** Google wire DTOs and bmux UI
  types remain inside their adapters and do not define application models.
- **Rendering performs no I/O.** Rendering does not access the network, filesystem,
  credentials, or blocking services.
- **Asynchronous results are correlated.** Stale or cancelled work cannot overwrite
  newer selections or authentication state.
- **The terminal is restored.** Normal exit, interruption, and handled failure paths
  leave the terminal usable.
- **Crates are domain-owned.** Do not create speculative or vaguely named crates.

## Interaction

- **Mouse and keyboard are first-class.** Primary actions are practical with both
  input methods through bmux interaction routing and component capabilities.
- **Keybindings have one source of truth.** A configurable registry maps normalized
  chords to semantic actions. Feature handlers dispatch actions, not literal keys.
  Help, hints, menus, dialogs, onboarding, and prompts derive displayed shortcuts
  from the active registry. Default chords are declared only in registry defaults.
- **bmux components are preferred.** Use suitable `bmux_tui_components` controls
  rather than duplicating them. Product-specific controls require a demonstrated
  gap; broadly reusable gaps should be evaluated upstream.

## Authentication and secrets

- **OAuth is least-privilege.** Request only required read-only scopes; never use
  administrator or write scopes for the first release.
- **Secrets have explicit custody.** Tokens and private key material never enter
  ordinary config, logs, diagnostics, fixtures, process arguments, environment
  variables, or release artifacts.
- **Persistent credentials are encrypted.** Never silently fall back to plaintext.
  Session-only authentication is the fallback when secure persistence is
  unavailable.
- **The app owns its vault.** gchatui never reads or mutates a global sshenv vault.

## Dependencies

- **External dependencies are reproducible.** Branch-following Git dependencies are
  resolved by the committed `Cargo.lock` and advanced only through reviewed,
  validated updates.
- **No sibling source dependency is required.** A clean checkout builds without
  adjacent bmux or sshenv repositories.

## Invariant evolution

Conflicts must be surfaced rather than silently bypassed. Intentional invariant
changes require explicit approval plus corresponding documentation, tests, and
mechanical guards where practical.
