# Automated Cloudflare website deployment

GitHub Actions owns deployment: pushes to `master` affecting the website, root
Cargo manifests, infrastructure, or deployment workflow build the HyperChad site,
deploy Worker assets, and apply the independent OpenTofu DNS/route stack.
`workflow_dispatch` supports retries without local deployment commands.

## One-time setup

Create a GitHub environment named `production`; restrict deployment to `master`
and configure required reviewers if desired. Add these environment secrets:

- `CLOUDFLARE_API_TOKEN`: account Workers Scripts edit and zone DNS/Workers Routes
  edit permissions, scoped to the intended account and `bmux.dev` zone.
- `CLOUDFLARE_ACCOUNT_ID`
- `CLOUDFLARE_ZONE_ID`
- `R2_STATE_ACCESS_KEY_ID`
- `R2_STATE_SECRET_ACCESS_KEY`

Set environment variable `R2_STATE_BUCKET` to `gchatui-tofu-state` (recommended).
Run **Bootstrap Website State** in GitHub Actions once before deployment. It uses
Cloudflare's API to create a private bucket if missing, without needing an existing
state bucket or persisting a second bootstrap state. Existing buckets are left
unchanged, including their access settings; ensure an existing bucket is private.
The bootstrap API token also needs account R2 Storage edit permission.
After bootstrap, ensure the R2 credentials above are scoped to this bucket.

Alternatively, Bcode state bucket can be reused with its owner's approval and bucket-scoped
R2 credentials. This stack uses the separate key `cloudflare/gchatui-site.tfstate`;
it must never use Bcode's key. The bucket must exist before the first deployment.
No application-data bucket or AWS service access is required.

Infrastructure credentials are scoped to the protected deployment job environment;
never embed them in command arguments, artifacts, source, or frontend assets.
This infrastructure custody is distinct from the desktop app's OAuth/token rules.
No Google credentials belong in these jobs. Keep infrastructure state private.

## Deploy

Push reviewed changes to `master`, or run **Deploy Website** in Actions. There are
no manual Wrangler/OpenTofu commands required. A concurrency group serializes all
production updates without cancelling an in-progress apply. Configure branch
protection so checked changes reach production.

The workflow generates and checks static assets, deploys Worker `gchatui-site`,
then plans and applies one proxied DNS record and one Worker route. DNS points to
an intentional documentation-only placeholder address; the Worker serves all
requests, not an origin. An absent Worker makes the site unavailable. Restore a
previous reviewed commit and rerun the workflow to roll back site code.

If the hostname already exists in another stack, stop and resolve resource
ownership/import before deploying. Do not overwrite resources owned elsewhere.
The workflow does not bootstrap a state bucket, change the apex zone, or mutate
Google Cloud configuration. Plan files stay on the ephemeral runner, not artifacts.

After deployment verify HTTPS and configure Google Branding with authorized domain
`bmux.dev`, homepage `https://gchatui.bmux.dev/`, and privacy URL
`https://gchatui.bmux.dev/privacy`. Search Console verification and final privacy
policy review remain required. The initial page is explicitly pre-release privacy
information, not a claim of Google verification.
