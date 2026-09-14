import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';

for (const page of ['index', 'privacy']) {
  test(`${page} exports accessible navigation without OAuth payloads`, () => {
    const html = readFileSync(new URL(`../../dist/${page}.html`, import.meta.url), 'utf8');
    assert.match(html, /gchatui/);
    assert.match(html, /<a\b[^>]*href=/);
    assert.doesNotMatch(html, /client_secret|access_token|refresh_token|127\.0\.0\.1/);
    assert.doesNotMatch(html, /<form\b/);
  });
}
test('home links to privacy and privacy discloses hosting', () => {
  const home = readFileSync(new URL('../../dist/index.html', import.meta.url), 'utf8');
  const privacy = readFileSync(new URL('../../dist/privacy.html', import.meta.url), 'utf8');
  assert.match(home, /href="\/privacy"/);
  assert.match(privacy, /Cloudflare/);
  assert.doesNotMatch(privacy, /Pre-release notice/);
  assert.match(privacy, /href="mailto:[^"]+"/);
  assert.match(privacy, /Limited Use requirements/);
  assert.match(privacy, /Effective September 14, 2026/);
});
