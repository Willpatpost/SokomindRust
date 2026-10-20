// Writes the dist's asset manifest and stamps the service worker with the
// build's revision: the app shell's files with their sha256 digests, the
// hashed assets as the precache list, and the revision both the worker and
// the manifest carry, so a new build never serves the old shell. Run by
// build:web after vite writes web/dist.
import { createHash } from 'node:crypto';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/** The exact application shell, as web/public/sw.js requires it (checked by
 * scripts/mirrors.test.mjs). */
export const SHELL_PATHS = ['./', './favicon.svg', './icon-192.png', './icon-512.png', './manifest.webmanifest'];

/** The placeholder the worker ships with, stamped with the build's revision. */
export const REVISION_PLACEHOLDER = '__SOKOMIND_BUILD_REVISION__';

/**
 * The dist file a manifest path names; the shell root is the document.
 * @param {string} distDir
 * @param {string} entry
 * @returns {string}
 */
const distFile = (distDir, entry) => path.join(distDir, entry === './' ? 'index.html' : entry.slice(2));

/**
 * @param {Buffer} bytes
 * @returns {string}
 */
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

/**
 * The manifest written beside the shell: its version, the revision both the
 * worker and the manifest carry, the shell's files with their digests, and
 * the hashed assets to precache.
 * @typedef {object} AssetManifest
 * @property {number} version
 * @property {string} revision
 * @property {Array<{path: string, sha256: string}>} shell
 * @property {string[]} precache
 * @property {string[]} runtime
 */

/**
 * Builds the manifest for `distDir`, writes it beside the shell, stamps the
 * worker, and returns what it wrote. The revision names this build's shell
 * and assets together, so the worker's cache never mixes generations.
 * @param {string} distDir
 * @returns {Promise<AssetManifest>}
 */
export async function generateAssetManifest(distDir) {
  const shell = [];
  for (const entry of SHELL_PATHS) {
    shell.push({ path: entry, sha256: sha256(await readFile(distFile(distDir, entry))) });
  }
  const precache = (await readdir(path.join(distDir, 'assets'))).map(name => `./assets/${name}`).sort();
  const revision = sha256(Buffer.from(JSON.stringify({ shell, precache }))).slice(0, 16);
  const manifest = { version: 2, revision, shell, precache, runtime: [] };
  await writeFile(path.join(distDir, 'asset-manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  const workerPath = path.join(distDir, 'sw.js');
  const worker = await readFile(workerPath, 'utf8');
  // The fresh build ships the placeholder; a re-stamp replaces the revision
  // it already carries.
  if (!worker.includes(REVISION_PLACEHOLDER) && !/CACHE_REVISION = '[a-f0-9]{16}'/.test(worker)) {
    throw new Error('The built service worker has no revision placeholder.');
  }
  const stamped = worker.replaceAll(REVISION_PLACEHOLDER, revision).replace(/(CACHE_REVISION = ')[a-f0-9]{16}(';)/, `$1${revision}$2`);
  await writeFile(workerPath, stamped);
  return manifest;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await generateAssetManifest(path.resolve(fileURLToPath(new URL('../web/dist', import.meta.url))));
}
