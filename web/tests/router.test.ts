import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatHash, parseHash, type Route } from '../src/router.ts';

test('every route formats to a hash that parses back to it', () => {
  const routes: Route[] = [
    { kind: 'home' },
    { kind: 'puzzles' },
    { kind: 'puzzles-difficulty', difficulty: 'expert', page: 1 },
    { kind: 'puzzles-difficulty', difficulty: 'expert', page: 3 },
    { kind: 'puzzles-collection', difficulty: 'beginner', collection: 'Sokomind Generated', page: 2 },
    { kind: 'editor' },
    { kind: 'stats' },
    { kind: 'solver-lab', puzzleId: '' },
    { kind: 'solver-lab', puzzleId: 'huge' },
    { kind: 'solver-lab', puzzleId: 'huge', actionLog: 'DDL' },
    { kind: 'play', puzzleId: 'ultra-tiny' },
    { kind: 'play', puzzleId: 'huge', actionLog: 'UDLR' },
    { kind: 'play', puzzleId: 'a puzzle id with spaces', actionLog: 'DD' },
  ];
  for (const route of routes) assert.deepEqual(parseHash(formatHash(route)), route, formatHash(route));
});

test('hashes without a known page are the home route', () => {
  for (const hash of ['', '#', '#/', '#/garbage', '#/play', '#/play/', '#/play/a/b', '#/puzzles/a/b/c', '#/solver-lab/a/b', '#/Puzzles']) {
    assert.deepEqual(parseHash(hash), { kind: 'home' }, hash);
  }
});

test('a puzzles drill-down carries its difficulty, collection and page', () => {
  assert.deepEqual(parseHash('#/puzzles/expert'), { kind: 'puzzles-difficulty', difficulty: 'expert', page: 1 });
  assert.deepEqual(parseHash('#/puzzles/expert?page=3'), { kind: 'puzzles-difficulty', difficulty: 'expert', page: 3 });
  assert.deepEqual(parseHash('#/puzzles/beginner/Sokomind%20Generated?page=2'), {
    kind: 'puzzles-collection',
    difficulty: 'beginner',
    collection: 'Sokomind Generated',
    page: 2,
  });
  // The page parameter never reads below one, whatever the URL says.
  assert.deepEqual(parseHash('#/puzzles/expert?page=0'), { kind: 'puzzles-difficulty', difficulty: 'expert', page: 1 });
  assert.deepEqual(parseHash('#/puzzles/expert?page=garbage'), { kind: 'puzzles-difficulty', difficulty: 'expert', page: 1 });
  // The tier's validity is the page's to check, not the router's.
  assert.deepEqual(parseHash('#/puzzles/huge'), { kind: 'puzzles-difficulty', difficulty: 'huge', page: 1 });
});

test('a play hash carries its puzzle id and optional action log', () => {
  assert.deepEqual(parseHash('#/play/huge'), { kind: 'play', puzzleId: 'huge' });
  assert.deepEqual(parseHash('#/play/huge?play=DDL'), { kind: 'play', puzzleId: 'huge', actionLog: 'DDL' });
  assert.deepEqual(parseHash('#/play/ultra-tiny?other=x'), { kind: 'play', puzzleId: 'ultra-tiny' });
  // The log passes through unvalidated; the play page replays it through Rust.
  assert.deepEqual(parseHash('#/play/x?play=whatever!'), { kind: 'play', puzzleId: 'x', actionLog: 'whatever!' });
});

test('a lab hash carries its room and optional action log, bare naming none', () => {
  assert.deepEqual(parseHash('#/solver-lab'), { kind: 'solver-lab', puzzleId: '' });
  assert.deepEqual(parseHash('#/solver-lab/huge'), { kind: 'solver-lab', puzzleId: 'huge' });
  assert.deepEqual(parseHash('#/solver-lab/huge?play=DDL'), { kind: 'solver-lab', puzzleId: 'huge', actionLog: 'DDL' });
  assert.deepEqual(parseHash('#/solver-lab/a%20puzzle'), { kind: 'solver-lab', puzzleId: 'a puzzle' });
  // The log passes through unvalidated; the lab replays it through Rust.
  assert.deepEqual(parseHash('#/solver-lab/x?play=whatever!'), { kind: 'solver-lab', puzzleId: 'x', actionLog: 'whatever!' });
});
