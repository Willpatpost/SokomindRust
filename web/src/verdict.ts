import type { Metrics, Proof, SearchStatus } from './protocol.ts';

/** The search status line: what is proven about the verified route on show, then
 * how the search ended. These are the page's only proof claims. The transport has
 * already checked the proof against the metrics, and SolverClient against the route. */
export function statusText(metrics: Metrics, route: string | undefined): string {
  const ending = endingText(metrics.status, metrics.proof);
  return route === undefined ? ending : `${route.length} remaining moves · ${routeClaim(metrics, route)}. ${ending}`;
}

/** What is proven about the route on show. Worker routes arrive throttled, so it
 * can trail `metrics.best`, and a gap to optimal is measured on the route itself. */
function routeClaim({ lowerBound, proof }: Metrics, route: string): string {
  switch (proof.kind) {
    case 'optimal':
      return 'proven move-optimal from this position';
    case 'unsolvable':
      return 'proven unsolvable';
    case 'bounded':
    case 'none':
      return lowerBound === undefined ? 'optimality unproven' : `within ${route.length - lowerBound} moves of optimal`;
  }
}

/** How the search ended, or that it is still running. */
function endingText(status: SearchStatus, proof: Proof): string {
  switch (status) {
    case 'running':
      return 'Searching…';
    case 'solved':
      return 'Search complete.';
    case 'exhausted':
      // Only Optimal's exhaustion carries an unsolvable proof.
      return proof.kind === 'unsolvable'
        ? 'No solution exists from this position.'
        : 'Search ended without finding a route (not a proof — use Optimal to prove unsolvability).';
    case 'state_limit':
      return 'State limit reached.';
    case 'memory_limit':
      return 'Memory limit reached.';
    case 'time_limit':
      return 'Time budget reached.';
    case 'cancelled':
      return 'Stopped.';
  }
}
