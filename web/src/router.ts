/** The app's pages, as hash URLs: `#/`, `#/puzzles` with `/:difficulty` and
 * `/:difficulty/:collection` drill-downs whose lists carry `?page=N`,
 * `#/play/:id[?play=UDLR]`, `#/editor`, `#/stats` and `#/solver-lab` with an
 * optional `/:puzzleId[?play=UDLR]` — a bare lab route names no room, and the
 * lab renders its default. The editor is a placeholder until its port phase
 * lands; the rest carry behavior. */
export type Route =
  | { kind: 'home' }
  | { kind: 'puzzles' }
  | { kind: 'puzzles-difficulty'; difficulty: string; page: number }
  | { kind: 'puzzles-collection'; difficulty: string; collection: string; page: number }
  | { kind: 'play'; puzzleId: string; actionLog?: string }
  | { kind: 'editor' }
  | { kind: 'stats' }
  | { kind: 'solver-lab'; puzzleId: string; actionLog?: string };

/** The `?play=` parameter's value, or undefined without one. Only U/D/L/R logs
 * can replay, but the route passes whatever was shared: the receiving page
 * replays it through the Rust game, which rejects a bad log. */
function logParameter(query: string): string | undefined {
  const value = new URLSearchParams(query).get('play');
  return value === null ? undefined : value;
}

/** The `?page=` parameter's value, 1 by default and never below it. */
function pageParameter(query: string): number {
  const value = Number(new URLSearchParams(query).get('page'));
  return Number.isSafeInteger(value) && value >= 1 ? value : 1;
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
    if (page === 'solver-lab') return { kind: 'solver-lab', puzzleId: '' };
  }
  if (page === 'puzzles' && (segments.length === 2 || segments.length === 3)) {
    const difficulty = decodeURIComponent(segments[1]!);
    if (segments.length === 2) return { kind: 'puzzles-difficulty', difficulty, page: pageParameter(query) };
    return {
      kind: 'puzzles-collection',
      difficulty,
      collection: decodeURIComponent(segments[2]!),
      page: pageParameter(query),
    };
  }
  if (page === 'play' && segments.length === 2 && segments[1] !== '') {
    const puzzleId = decodeURIComponent(segments[1]);
    const actionLog = logParameter(query);
    return actionLog === undefined ? { kind: 'play', puzzleId } : { kind: 'play', puzzleId, actionLog };
  }
  if (page === 'solver-lab' && segments.length === 2 && segments[1] !== '') {
    const puzzleId = decodeURIComponent(segments[1]);
    const actionLog = logParameter(query);
    return actionLog === undefined ? { kind: 'solver-lab', puzzleId } : { kind: 'solver-lab', puzzleId, actionLog };
  }
  return { kind: 'home' };
}

/** The hash URL of a route; parsing it back yields the same route. The page
 * parameter rides the URL from page two on. */
function pageQuery(page: number): string {
  return page === 1 ? '' : `?page=${page}`;
}
export function formatHash(next: Route): string {
  switch (next.kind) {
    case 'home':
      return '#/';
    case 'puzzles':
      return '#/puzzles';
    case 'puzzles-difficulty':
      return `#/puzzles/${encodeURIComponent(next.difficulty)}${pageQuery(next.page)}`;
    case 'puzzles-collection':
      return `#/puzzles/${encodeURIComponent(next.difficulty)}/${encodeURIComponent(next.collection)}${pageQuery(next.page)}`;
    case 'editor':
      return '#/editor';
    case 'stats':
      return '#/stats';
    case 'solver-lab':
      return `#/solver-lab${next.puzzleId === '' ? '' : `/${encodeURIComponent(next.puzzleId)}`}${next.actionLog === undefined ? '' : `?play=${next.actionLog}`}`;
    case 'play':
      return `#/play/${encodeURIComponent(next.puzzleId)}${next.actionLog === undefined ? '' : `?play=${next.actionLog}`}`;
  }
}

/** Whether two routes name the same page with the same parameters. */
function sameRoute(a: Route, b: Route): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'play' && b.kind === 'play') return a.puzzleId === b.puzzleId && a.actionLog === b.actionLog;
  if (a.kind === 'solver-lab' && b.kind === 'solver-lab') return a.puzzleId === b.puzzleId && a.actionLog === b.actionLog;
  if (a.kind === 'puzzles-difficulty' && b.kind === 'puzzles-difficulty') return a.difficulty === b.difficulty && a.page === b.page;
  if (a.kind === 'puzzles-collection' && b.kind === 'puzzles-collection')
    return a.difficulty === b.difficulty && a.collection === b.collection && a.page === b.page;
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

/** Navigates to `next` by writing the hash; the hashchange reports it. Setting
 * the hash a route already shows changes nothing, so re-navigating is safe. */
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
