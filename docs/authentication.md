# Authentication

The first release uses Google installed-app OAuth with a user-provided desktop
client configuration and only the read-only scopes required to list spaces and
messages.

- Access tokens remain in protected process memory.
- Persistent mode stores the refresh token in an app-specific encrypted sshenv
  vault. By default, a passphrase protects the generated app identity.
- Setting `vault_passphrase = false` opts into prompt-free persistent startup. The
  vault remains encrypted, but the app identity is protected only by current-user
  filesystem permissions; copying both state files permits token recovery.
- Session-only mode writes no token or generated identity to disk.
- Plaintext persistence is never an automatic fallback.
- Logout attempts remote revocation and removes local authorization according to
  explicit user intent.

OAuth client files, authorization codes, tokens, private keys, and callback payloads
must never enter source control, logs, fixtures, diagnostics, or release archives.
Implementation details will be added after the credential and OAuth spikes are
validated.
