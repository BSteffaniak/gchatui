# Authentication

The first release uses Google installed-app OAuth with a user-provided desktop
client configuration and only the read-only scopes required to list spaces,
messages, and resolve sender names through the Workspace directory.

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

## Partial consent

Select all requested permissions on Google's consent screen. gchatui checks scopes
reported in the callback and token responses, and rejects explicit partial grants
before using or persisting the new tokens. If Google omits scope information, OAuth
semantics retain the requested/original grant; this is not independent inspection
of an older stored token's permissions. Workspace restrictions can still cause API
access failures even after complete consent.

For an incomplete new grant, restart gchatui and authorize all permissions. With
`session_only = true`, no prior persisted refresh token is loaded or overwritten.
Do not switch desktop clients with persistent mode enabled until client-bound
credential storage is implemented; the current vault is not client-specific.

The [official-client rollout](official-oauth-rollout.md) records the distribution
and verification gates. The private client-file configuration remains required
until those gates are complete.
