// The app's offline shell: a revision-keyed cache, populated at install from
// the asset manifest the build writes beside this worker and verified by
// digest, so a corrupt installation never replaces the last known-good shell.
// Navigations stay network-first — an online 404 must remain a 404 — with the
// cached shell as the offline fallback; assets are cache-first, filled at
// runtime only from the manifest's own lists, so the cache never grows past
// what the build named.
const CACHE_REVISION = '__SOKOMIND_BUILD_REVISION__';
const SCOPE_URL = new URL(self.registration.scope);
const CACHE_NAME = `sokomind-shell-${CACHE_REVISION}`;
const APP_SHELL_URL = new URL('./', SCOPE_URL).href;
const ASSET_MANIFEST_URL = new URL('./asset-manifest.json', SCOPE_URL).href;
// The exact application shell, as scripts/asset-manifest.mjs writes it
// (checked by scripts/mirrors.test.mjs).
const REQUIRED_SHELL_PATHS = ['./', './favicon.svg', './icon-192.png', './icon-512.png', './manifest.webmanifest'];

function scopedUrl(path) {
  const url = new URL(path, SCOPE_URL);
  if (url.origin !== SCOPE_URL.origin || !url.pathname.startsWith(SCOPE_URL.pathname)) {
    throw new Error(`Asset manifest entry is outside the app scope: ${path}`);
  }
  return url.href;
}

function parseAssetManifest(value) {
  if (
    !value
    || typeof value !== 'object'
    || value.version !== 2
    || value.revision !== CACHE_REVISION
    || !Array.isArray(value.shell)
    || !Array.isArray(value.precache)
    || !Array.isArray(value.runtime)
    || value.shell.some(
      entry =>
        !entry
        || typeof entry !== 'object'
        || typeof entry.path !== 'string'
        || typeof entry.sha256 !== 'string'
        || !/^[a-f0-9]{64}$/u.test(entry.sha256),
    )
    || [...value.precache, ...value.runtime].some(entry => typeof entry !== 'string')
  ) {
    throw new Error('Asset manifest must match this worker revision and contain valid shell, precache, and runtime records.');
  }
  const shell = value.shell.map(entry => ({ url: scopedUrl(entry.path), sha256: entry.sha256 }));
  const requiredShellUrls = REQUIRED_SHELL_PATHS.map(scopedUrl);
  if (shell.length !== requiredShellUrls.length || requiredShellUrls.some(url => !shell.some(entry => entry.url === url))) {
    throw new Error('Asset manifest does not contain the exact application shell.');
  }
  const precache = value.precache.map(scopedUrl);
  const runtime = value.runtime.map(scopedUrl);
  const all = [...shell.map(entry => entry.url), ...precache, ...runtime];
  if (new Set(all).size !== all.length) {
    throw new Error('Asset manifest paths must be unique.');
  }
  return { shell, precache, runtime };
}

async function sha256(response) {
  const digest = await crypto.subtle.digest('SHA-256', await response.arrayBuffer());
  return [...new Uint8Array(digest)].map(value => value.toString(16).padStart(2, '0')).join('');
}

async function populateCurrentCache() {
  const manifestResponse = await fetch(ASSET_MANIFEST_URL, { cache: 'no-store' });
  if (!manifestResponse.ok) {
    throw new Error(`Asset manifest request failed: ${manifestResponse.status}`);
  }
  const assetUrls = parseAssetManifest(await manifestResponse.clone().json());
  const cache = await caches.open(CACHE_NAME);
  try {
    await cache.put(ASSET_MANIFEST_URL, manifestResponse);
    const shellDigests = new Map(assetUrls.shell.map(entry => [entry.url, entry.sha256]));
    const installUrls = [...assetUrls.shell.map(entry => entry.url), ...assetUrls.precache];
    await Promise.all(
      installUrls.map(async url => {
        // The shell's mutable names bypass the HTTP cache, so a new revision
        // cannot combine stale HTML with the new build's hashed assets.
        const response = await fetch(new Request(url, { cache: 'reload' }));
        if (!response.ok) {
          throw new Error(`Install resource request failed: ${response.status}`);
        }
        const expectedDigest = shellDigests.get(url);
        if (expectedDigest && (await sha256(response.clone())) !== expectedDigest) {
          throw new Error(`Application shell digest mismatch: ${url}`);
        }
        await cache.put(url, response);
      }),
    );
  } catch (error) {
    await caches.delete(CACHE_NAME);
    throw error;
  }
}

async function expectedCacheUrls(cache) {
  const manifestResponse = await cache.match(ASSET_MANIFEST_URL);
  if (!manifestResponse) {
    throw new Error('The cached asset manifest is missing.');
  }
  const assetUrls = parseAssetManifest(await manifestResponse.json());
  return new Set([ASSET_MANIFEST_URL, ...assetUrls.shell.map(entry => entry.url), ...assetUrls.precache, ...assetUrls.runtime]);
}

async function activateCurrentCache() {
  // Validate and prune the staged generation before removing the last
  // known-good cache: a corrupt installation must not discard the active
  // offline shell.
  const cache = await caches.open(CACHE_NAME);
  const expectedUrls = await expectedCacheUrls(cache);
  const requests = await cache.keys();
  await Promise.all(requests.filter(request => !expectedUrls.has(request.url)).map(request => cache.delete(request)));
  const cacheNames = await caches.keys();
  await Promise.all(
    cacheNames
      .filter(cacheName => cacheName.startsWith('sokomind-shell-') && cacheName !== CACHE_NAME)
      .map(cacheName => caches.delete(cacheName)),
  );
  await self.clients.claim();
}

async function respondToNavigation(request) {
  try {
    return await fetch(request);
  } catch {
    const cache = await caches.open(CACHE_NAME);
    return (await cache.match(APP_SHELL_URL)) ?? Response.error();
  }
}

async function respondToAsset(request) {
  const cache = await caches.open(CACHE_NAME);
  const cached = await cache.match(request);
  if (cached) return cached;
  const response = await fetch(request);
  if (!response.ok || response.type !== 'basic') return response;
  const expectedUrls = await expectedCacheUrls(cache);
  if (expectedUrls.has(request.url)) {
    // The response promise stays pending until this write completes, keeping
    // the fetch event alive instead of launching an untracked cache mutation.
    await cache.put(request, response.clone());
  }
  return response;
}

self.addEventListener('install', event => {
  event.waitUntil(populateCurrentCache());
});

self.addEventListener('activate', event => {
  event.waitUntil(activateCurrentCache());
});

self.addEventListener('message', event => {
  if (event.data?.type === 'SKIP_WAITING') {
    event.waitUntil(self.skipWaiting());
  }
});

self.addEventListener('fetch', event => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== SCOPE_URL.origin || !url.pathname.startsWith(SCOPE_URL.pathname)) return;
  event.respondWith(request.mode === 'navigate' ? respondToNavigation(request) : respondToAsset(request));
});
