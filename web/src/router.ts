/** The app's pages, as hash URLs: `#/`, `#/puzzles`, `#/play/:puzzleId[?play=UDLR]`,
 * `#/editor`, `#/stats` and `#/solver-lab`. The puzzle browser, the editor, the
 * statistics page and the lab are placeholders until their port phases land; the
 * home and play routes carry behavior today. */
export type Route =
  | { kind: 'home' }
  | { kind: 'puzzles' }
  | { kind: 'play'; puzzleId: string; actionLog?: string }
  | { kind: 'editor' }
  | { kind: 'stats' }
  | { kind: 'solver-lab' };

/** The `?play=` parameter's value, or undefined without one. Only U/D/L/R logs
 * can replay, but the route passes whatever was shared: the play page replays it
 * through the Rust game, which rejects a bad log. */
function logParameter(query: string): string | undefined {
  const value = new URLSearchParams(query).get('play');
  return value === null ? undefined : value;
}

/** The route a location hash names; anything malformed is the home route, which
 * startRouter then writes back so the URL bar never shows a route the app lacks. */
export function parseHash(hash: string): Route {
  const [path, query = ''] = hash.replace(/^#/, '').split('?');
  const segments = path.split('/').filter(item => item !== '');
  if (segments.length === 0) return { kind: 'home' };
  const page = segments[0];
  if (segments.length === 1) {
    if (page === 'puzzles') return { kind: 'puzzles' };
    if (page === 'editor') return { kind: 'editor' };
    if (page === 'stats') return { kind: 'stats' };
    if (page === 'solver-lab') return { kind: 'solver-lab' };
  }
  if (page === 'play' && segments.length === 2 && segments[1] !== '') {
    const puzzleId = decodeURIComponent(segments[1]);
    const actionLog = logParameter(query);
    return actionLog === undefined ? { kind: 'play', puzzleId } : { kind: 'play', puzzleId, actionLog };
  }
  return { kind: 'home' };
}

/** The hash URL of a route; parsing it back yields the same route. */
export function formatHash(next: Route): string {
  switch (next.kind) {
    case 'home':
      return '#/';
    case 'puzzles':
      return '#/puzzles';
    case 'editor':
      return '#/editor';
    case 'stats':
      return '#/stats';
    case 'solver-lab':
      return '#/solver-lab';
    case 'play':
      return `#/play/${encodeURIComponent(next.puzzleId)}${next.actionLog === undefined ? '' : `?play=${next.actionLog}`}`;
  }
}

/** Whether two routes name the same page with the same parameters. */
function sameRoute(a: Route, b: Route): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'play' && b.kind === 'play') return a.puzzleId === b.puzzleId && a.actionLog === b.actionLog;
  return true;
}

/** Reports route changes. */
type Listener = (route: Route) => void;
const listeners = new Set<Listener>();
let current: Route = { kind: 'home' };

/** The route the hash currently names. */
export function route(): Route {
  return current;
}

/** Subscribes to route changes; returns its unsubscribe. */
export function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Navigates to `next` by writing the hash; the hashchange reports it. Setting the
 * hash a route already shows changes nothing, so re-navigating is safe. */
export function navigate(next: Route): void {
  const hash = formatHash(next);
  if (location.hash !== hash) location.hash = hash;
}

/** Follows the location hash, reporting each route change to the subscribers and
 * rewriting a malformed hash to the home route it resolves to. Call once, after
 * the shell that shows routes has subscribed. */
export function startRouter(): void {
  let reported = false;
  const report = () => {
    const next = parseHash(location.hash);
    // The first report always notifies: the store starts as home, so a page that
    // opens exactly there would otherwise never hear its route.
    if (!reported || !sameRoute(next, current)) {
      reported = true;
      current = next;
      for (const listener of listeners) listener(next);
    }
    if (location.hash !== formatHash(current)) history.replaceState(null, '', formatHash(current));
  };
  addEventListener('hashchange', report);
  report();
}
