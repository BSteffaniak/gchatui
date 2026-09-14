# Cloudflare website infrastructure

This independent OpenTofu stack follows Bcode's `models.bmux.dev` pattern:
OpenTofu owns the proxied DNS record and Worker route; Wrangler owns the deployed
Worker and static assets. No sibling checkout is required.

The target is `gchatui.bmux.dev`, routed to a Worker named `gchatui-site`.
This stack does not change the apex domain, models site, zone ownership, or any
Google configuration. The documentation-only IP address is intentional: requests
must be handled by the Worker rather than an origin server.

## Prerequisites and custody

- Existing `bmux.dev` Cloudflare zone and authority to modify its DNS/routes.
- A deployed Worker named `gchatui-site` serving the reviewed static homepage and
  `/privacy` policy. This stack does not deploy a site or publish the policy draft.
- A private R2 state bucket, provisioned separately. An existing bucket may be
  reused if its owner approves, but this stack uses the distinct object key
  `cloudflare/gchatui-site.tfstate`. Never initialize against Bcode's state key.
- Cloudflare API token with only the required DNS and Workers Routes edit access
  for the intended zone. Worker deployment needs its own appropriate permissions.
- R2 credentials scoped to the state bucket. Protect and back up state; it contains
  infrastructure identifiers. Do not commit state, plans, credentials, or real IDs.

No Google OAuth client JSON, user token, AWS credential, runtime secret, or model
snapshot bucket is needed for this static site infrastructure.

## Private configuration

Keep the following outside the checkout in a current-user-only directory. Use
mode `0600` for files containing credentials and `0700` for their directory.
Do not pass secret values as command arguments or environment variables.

1. `cloudflare-token`: token contents only.
2. `site.tfvars`: `cloudflare_api_token_file` pointing to that file and
   `cloudflare_zone_id` containing the existing zone ID.
3. `r2-credentials`: AWS shared-credentials format containing R2 access key ID and
   secret access key under `[default]`.
4. `site.tfbackend`: `bucket`, `endpoints = { s3 = "https://ACCOUNT.r2.cloudflarestorage.com" }`,
   and `shared_credentials_files = ["/absolute/private/path/r2-credentials"]`.
   Replace the illustrative account and path privately; never commit this file.

OpenTofu reads the Cloudflare token from its private file. Backend credentials are
read through the shared-credentials file, not embedded in backend arguments.
Treat `.terraform` and generated plan files as private too.

## Validate, plan, and apply

Install OpenTofu 1.8 or newer. From the repository root:

```sh
tofu -chdir=infra/cloudflare fmt -check
tofu -chdir=infra/cloudflare init -backend=false
tofu -chdir=infra/cloudflare validate
```

Commit the generated provider lockfile after reviewing it. Before applying on a
different platform, add its provider checksums using `tofu providers lock`.

For deployment, first deploy the static-assets Worker as `gchatui-site`. Then
initialize this stack with the private configuration paths:

```sh
tofu -chdir=infra/cloudflare init -reconfigure \
  -backend-config=/absolute/private/path/site.tfbackend
tofu -chdir=infra/cloudflare plan \
  -var-file=/absolute/private/path/site.tfvars \
  -out=/absolute/private/path/site.tfplan
```

Review the plan locally. Expect only one DNS record and one Worker route. If
`gchatui.bmux.dev` is already managed elsewhere, stop and resolve ownership or
import the intended resources; do not overwrite another stack's resources.

After explicit approval:

```sh
tofu -chdir=infra/cloudflare apply /absolute/private/path/site.tfplan
```

Check HTTPS, homepage, privacy policy, unknown-path behavior, and absence of
OAuth endpoints. Never direct Google desktop callbacks to this site. An absent
Worker or a Worker that falls through to the placeholder origin will not serve a
working site. Removing the route is not a rollback to a functioning origin.

## Remaining deployment work

The static site must be implemented and reviewed before DNS is applied. Use
Wrangler static assets with no runtime bindings, analytics, or OAuth processing.
Do not publish `docs/privacy-policy-draft.md` as an approved policy. Confirm the
operator/contact, effective date, and Cloudflare website request-data disclosures.

Then verify `bmux.dev` in Search Console and configure Google Auth Platform with
`bmux.dev` as the authorized domain and the homepage/privacy URLs above. DNS
provisioning does not perform domain verification or OAuth verification.

No automatic apply workflow is installed. This intentionally avoids copying
Bcode's secret-in-environment/argument deployment pattern into gchatui and avoids
publishing an unreviewed policy on a push.
