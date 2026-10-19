import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatHash, parseHash, type Route } from '../src/router.ts';

test('every route formats to a hash that parses back to it', () => {
  const routes: Route[] = [
    { kind: 'home' },
    { kind: 'puzzles' },
    { kind: 'editor' },
    { kind: 'stats' },
    { kind: 'solver-lab' },
    { kind: 'play', puzzleId: 'ultra-tiny' },
    { kind: 'play', puzzleId: 'huge', actionLog: 'UDLR' },
    { kind: 'play', puzzleId: 'a puzzle id with spaces', actionLog: 'DD' },
  ];
  for (const route of routes) assert.deepEqual(parseHash(formatHash(route)), route, formatHash(route));
});

test('hashes without a known page are the home route', () => {
  for (const hash of ['', '#', '#/', '#/garbage', '#/play', '#/play/', '#/play/a/b', '#/puzzles/huge', '#/Puzzles']) {
    assert.deepEqual(parseHash(hash), { kind: 'home' }, hash);
  }
});

test('a play hash carries its puzzle id and optional action log', () => {
  assert.deepEqual(parseHash('#/play/huge'), { kind: 'play', puzzleId: 'huge' });
  assert.deepEqual(parseHash('#/play/huge?play=DDL'), { kind: 'play', puzzleId: 'huge', actionLog: 'DDL' });
  assert.deepEqual(parseHash('#/play/ultra-tiny?other=x'), { kind: 'play', puzzleId: 'ultra-tiny' });
  // The log passes through unvalidated; the play page replays it through Rust.
  assert.deepEqual(parseHash('#/play/x?play=whatever!'), { kind: 'play', puzzleId: 'x', actionLog: 'whatever!' });
});
