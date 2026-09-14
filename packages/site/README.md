# gchatui website

The `gchatui-site` workspace package renders the homepage and privacy information
using HyperChad containers and its static-route exporter, following Bcode's site
pattern. There are no hand-maintained HTML pages or sibling source dependencies.
The root remains the default package; HyperChad is not a desktop dependency.

```sh
cargo run --locked -p gchatui-site -- gen --output dist
npm --prefix packages/site ci
npm --prefix packages/site test
```

Run these commands from the repository root. Generated assets live in ignored
`dist/`; Wrangler deploys only that directory. Node dependencies are deployment
 tooling, not browser application code. Both dependency lockfiles are tracked.

The privacy page explicitly remains pre-release information. Confirm an operator,
private contact, effective date, and final policy before Google verification.

The production workflow builds and checks the site, deploys the static Worker,
then applies DNS/routes. See [deployment setup](../../infra/cloudflare/README.md).
