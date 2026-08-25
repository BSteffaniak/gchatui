# Architecture

`gchatui` begins as one binary crate organized by domain. Crates are added only when
implemented behavior establishes an independent ownership boundary.

## Completion path

```text
executable
→ config and app-state resolution
→ credential vault or session-only auth
→ installed-app OAuth and token refresh
→ typed Google Chat REST adapter
→ domain state and correlated effects
→ bmux update/render loop
→ mouse- and keyboard-friendly conversation browsing
```

## Boundaries

- OAuth owns authorization, refresh, revocation, and reauthentication.
- Credential storage owns refresh-token persistence behind an application interface.
- The Chat adapter owns HTTP and Google wire types.
- Domain state owns spaces, messages, threads, selection, and request lifecycle.
- Effects perform asynchronous I/O and return correlated messages.
- Rendering is pure presentation and uses bmux components whenever suitable.
- The keybinding registry maps normalized chords to semantic actions and supplies
  every shortcut label shown by the UI.

See [INVARIANTS.md](../INVARIANTS.md) for binding requirements.
