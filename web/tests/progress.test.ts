import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Progress } from '../src/progress.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();
test('keep returns early on a tie with the best on show, and a better route still replaces it', () => {
  let verified = 0;
  const shown: unknown[] = [];
  const client = new Progress({
    verify: (_rows, route) => {
      verified++;
      return { moves: route.length, pushes: 1 };
    },
    show: best => shown.push(best),
    status: () => {},
  });
  client.select('p', 'rows');
  client.keep('DDD', 3, 1);
  assert.deepEqual(shown, [null, { rows: 'rows', route: 'DDD', moves: 3, pushes: 1 }]);
  assert.equal(verified, 0);
  client.keep('UUU', 3, 1);
  assert.equal(shown.length, 2);
  assert.equal(verified, 0);
  client.keep('DD', 2, 1);
  assert.equal(verified, 1);
  assert.deepEqual(shown.at(-1), { rows: 'rows', route: 'DD', moves: 2, pushes: 1 });
  assert.equal(client.best()?.route, 'DD');
});
