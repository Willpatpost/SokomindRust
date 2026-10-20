import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { generateAssetManifest, SHELL_PATHS } from './asset-manifest.mjs';

test('the asset manifest digests the shell, lists the assets, and stamps the worker', async () => {
  const dist = await mkdtemp(path.join(tmpdir(), 'sokomind-dist-'));
  try {
    await mkdir(path.join(dist, 'assets'), { recursive: true });
    await writeFile(path.join(dist, 'index.html'), '<html>shell</html>');
    await writeFile(path.join(dist, 'favicon.svg'), '<svg />');
    await writeFile(path.join(dist, 'icon-192.png'), 'png 192');
    await writeFile(path.join(dist, 'icon-512.png'), 'png 512');
    await writeFile(path.join(dist, 'manifest.webmanifest'), '{}');
    await writeFile(path.join(dist, 'sw.js'), "const CACHE_REVISION = '__SOKOMIND_BUILD_REVISION__';");
    await writeFile(path.join(dist, 'assets', 'index-abc123.js'), 'javascript');
    await writeFile(path.join(dist, 'assets', 'sokomind_bg-xyz.wasm'), 'wasm');
    const manifest = await generateAssetManifest(dist);
    assert.equal(manifest.version, 2);
    assert.deepEqual(
      manifest.shell.map(entry => entry.path),
      SHELL_PATHS,
    );
    // Each shell digest is its file's sha256.
    for (const entry of manifest.shell) {
      const file = entry.path === './' ? 'index.html' : entry.path.slice(2);
      const digest = createHash('sha256')
        .update(await readFile(path.join(dist, file)))
        .digest('hex');
      assert.equal(entry.sha256, digest);
    }
    assert.deepEqual(manifest.precache, ['./assets/index-abc123.js', './assets/sokomind_bg-xyz.wasm']);
    assert.deepEqual(manifest.runtime, []);
    assert.match(manifest.revision, /^[a-f0-9]{16}$/);
    // The written manifest is what was returned, and the worker carries the revision.
    const written = JSON.parse(await readFile(path.join(dist, 'asset-manifest.json'), 'utf8'));
    assert.deepEqual(written, manifest);
    const worker = await readFile(path.join(dist, 'sw.js'), 'utf8');
    assert.ok(worker.includes(`'${manifest.revision}'`));
    assert.ok(!worker.includes('__SOKOMIND_BUILD_REVISION__'));
    // A changed shell re-stamps to a different revision, so caches never mix builds.
    await writeFile(path.join(dist, 'index.html'), '<html>changed</html>');
    const rebuilt = await generateAssetManifest(dist);
    assert.notEqual(rebuilt.revision, manifest.revision);
    assert.equal(JSON.parse(await readFile(path.join(dist, 'asset-manifest.json'), 'utf8')).revision, rebuilt.revision);
  } finally {
    await rm(dist, { recursive: true, force: true });
  }
});
