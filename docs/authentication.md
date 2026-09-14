# Authentication

The desktop application defaults to the approved official Google Desktop OAuth
client. Set `oauth_client_path` only to override it with a private Desktop client
file. Invalid overrides fail rather than falling back to a different client.
All six existing scopes are retained. Google verification and Workspace policy
still determine who can authorize; bundling a client does not establish approval.

## Public application metadata

The official client ID and generated Desktop client value are public native-app
metadata in `packages/gchatui/src/official_oauth.rs`. They are not user credentials
and cannot be treated as confidential in a distributed executable. The same
existing Google exchange/refresh protocol is retained, including the generated
value; client-ID-only support is not assumed. PKCE and state validation remain
required. This exception does not cover Web secrets, tokens, private keys, or the
downloaded JSON. The public-safety guard limits these exact values to their module.

## Local storage

New configurations default to `session_only = true`. No token or vault identity is
persisted in this mode. Existing explicit settings remain honored. To opt into
persistent authorization, set these top-level local config values:

```toml
session_only = false
vault_passphrase = true
```

Persistent refresh tokens are encrypted in an app-specific sshenv vault. Setting
`vault_passphrase = false` explicitly opts into filesystem-permission protection
for the generated identity: copying both identity and vault permits token recovery.
There is no automatic plaintext fallback.

Credentials now live under the platform state directory at
`oauth-clients/<SHA-256-of-client-ID>/auth.vault` and `identity`. Changing clients
selects another namespace. The old unscoped vault is left untouched and never
silently migrated or reused; existing persistent users must sign in once again.
The app never reads or changes a global sshenv vault.

## Consent and remaining onboarding work

Select all requested Google permissions. Explicit partial grants are rejected
before new tokens are used or stored. Restart to authorize again after incomplete
new consent. Session-only mode neither loads nor overwrites stored credentials.
Workspace restrictions can still deny APIs after complete consent.

Interactive storage selection and in-app reauthorization are not yet implemented.
Persistent refresh failures currently return an error; for a fresh authorization
without deleting credentials, temporarily use session-only mode. Do not imply
that this workaround repairs or migrates the stored token.

Do not share callback URLs, authorization codes, token output, or conversations.
Verification demonstrations must use synthetic data. Cross-platform authorization
and the full onboarding experience remain release gates.
