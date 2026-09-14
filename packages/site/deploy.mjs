import { spawnSync } from 'node:child_process';

if (process.env.GITHUB_ACTIONS !== 'true') {
  throw new Error('Website deployment is GitHub Actions-only. Run Deploy Website in GitHub.');
}
const result = spawnSync('wrangler', ['deploy'], { stdio: 'inherit' });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
