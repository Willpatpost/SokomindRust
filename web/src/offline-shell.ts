/** The offline shell: the service worker the build stamps with its revision,
 * and the banner when a new build waits behind the active one. */
import { $, button } from './dom.ts';

/** Shows the update banner for a worker waiting behind the active one,
 * dismissing itself after half a minute. */
function showUpdateBanner(worker: ServiceWorker) {
  const banner = $('update-banner');
  banner.hidden = false;
  const dismiss = () => {
    banner.hidden = true;
    clearTimeout(timer);
  };
  const timer = setTimeout(dismiss, 30_000);
  button('update-dismiss').onclick = dismiss;
  button('update-reload').onclick = () => {
    // A worker that already took over ignores SKIP_WAITING; load it directly.
    if (worker.state === 'activating' || worker.state === 'activated') location.reload();
    else worker.postMessage({ type: 'SKIP_WAITING' });
  };
}

/** Registers the service worker in production builds. The first install also
 * passes through waiting before it activates on its own; only a worker
 * waiting behind an active one is an update, and a controller change after
 * one reloads the page onto the new shell. */
function registerServiceWorker() {
  if (!('serviceWorker' in navigator) || !import.meta.env.PROD) return;
  // The page may already have loaded — an async start can outrun its own load
  // event — so a listener alone would wait forever.
  const whenLoaded = (run: () => void) => {
    if (document.readyState === 'complete') run();
    else addEventListener('load', run, { once: true });
  };
  whenLoaded(() => {
    const workerUrl = new URL('sw.js', document.baseURI);
    const scope = new URL('./', document.baseURI).pathname;
    navigator.serviceWorker
      .register(workerUrl, { scope })
      .then(registration => {
        if (registration.waiting && registration.active) showUpdateBanner(registration.waiting);
        registration.addEventListener('updatefound', () => {
          const worker = registration.installing;
          if (worker === null) return;
          worker.addEventListener('statechange', () => {
            if (worker.state === 'installed' && registration.waiting && registration.active) showUpdateBanner(registration.waiting);
          });
        });
      })
      .catch(() => {});
    let hadController = navigator.serviceWorker.controller !== null;
    let refreshing = false;
    navigator.serviceWorker.addEventListener('controllerchange', () => {
      if (!hadController) {
        hadController = true;
        return;
      }
      if (refreshing) return;
      refreshing = true;
      location.reload();
    });
  });
}

export { registerServiceWorker };
