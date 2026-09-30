import { test } from 'node:test';
import assert from 'node:assert/strict';
import { STATUSES, type Metrics, type Proof, type SearchStatus } from '../src/protocol.ts';
import { statusText } from '../src/verdict.ts';

const ENDINGS: Record<SearchStatus, string> = {
  running: 'Searching…',
  solved: 'Search complete.',
  exhausted: 'Search ended without finding a route (not a proof — use Optimal to prove unsolvability).',
  state_limit: 'State limit reached.',
  memory_limit: 'Memory limit reached.',
  time_limit: 'Time budget reached.',
  cancelled: 'Stopped.',
};
const UNSOLVABLE_ENDING = 'No solution exists from this position.';
const PROOFS: Record<Proof['kind'], Proof> = {
  optimal: { kind: 'optimal', moves: 24 },
  bounded: { kind: 'bounded', lower: 20, upper: 24 },
  unsolvable: { kind: 'unsolvable' },
  none: { kind: 'none' },
};
// The claims for a 24-move route with a certified lower bound of 20. The transport and
// SolverClient reject an unsolvable proof beside a route, but the text is still pinned.
const CLAIMS: Record<Proof['kind'], string> = {
  optimal: 'proven move-optimal from this position',
  bounded: 'within 4 of optimal',
  unsolvable: 'proven unsolvable',
  none: 'within 4 of optimal',
};
const ROUTE = 'D'.repeat(24);
const metrics = (status: SearchStatus, proof: Proof): Metrics => ({
  expanded: 10,
  generated: 20,
  reservedBytes: 1024,
  best: 24,
  lowerBound: 20,
  proof,
  status,
});

test('every status and proof kind maps to its pinned claim and ending', () => {
  for (const status of STATUSES) {
    for (const kind of Object.keys(PROOFS) as Proof['kind'][]) {
      const ending = status === 'exhausted' && kind === 'unsolvable' ? UNSOLVABLE_ENDING : ENDINGS[status];
      const current = metrics(status, PROOFS[kind]);
      assert.equal(statusText(current, undefined), ending, `${status} / ${kind} without a route`);
      assert.equal(statusText(current, ROUTE), `24 remaining moves · ${CLAIMS[kind]}. ${ending}`, `${status} / ${kind} with a route`);
    }
  }
});
test('a route without a certified bound is unproven, and the gap is measured on the route shown', () => {
  for (const kind of ['bounded', 'none'] as const) {
    const unbounded = { ...metrics('time_limit', PROOFS[kind]), lowerBound: undefined };
    assert.equal(statusText(unbounded, ROUTE), '24 remaining moves · optimality unproven. Time budget reached.');
  }
  // A throttled worker route can trail the best the search has found.
  const trailing = { ...metrics('running', PROOFS.none), best: 22 };
  assert.equal(statusText(trailing, ROUTE), '24 remaining moves · within 4 of optimal. Searching…');
  const optimal = metrics('solved', PROOFS.optimal);
  assert.equal(statusText(optimal, ROUTE), '24 remaining moves · proven move-optimal from this position. Search complete.');
});
