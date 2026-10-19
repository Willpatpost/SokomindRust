import { test } from 'node:test';
import assert from 'node:assert/strict';
import { WALL } from '../src/protocol.ts';
import { walkRoute } from '../src/walk.ts';

/** A board of floor ringed by walls, as WasmGame.tiles returns it. */
function ringed(rows: number): { tiles: Uint8Array; width: number } {
  const width = 5;
  const tiles = new Uint8Array(rows * width);
  for (let cell = 0; cell < tiles.length; cell++) {
    const row = Math.floor(cell / width);
    const column = cell % width;
    if (row === 0 || row === rows - 1 || column === 0 || column === width - 1) tiles[cell] = WALL;
  }
  return { tiles, width };
}

/** Whether following `route` from `player` reaches `target` without entering a blocked cell. */
function follows(route: string, width: number, player: number, target: number, blocked: Set<number>): boolean {
  let cell = player;
  for (const step of route) {
    const delta = step === 'U' ? -width : step === 'D' ? width : step === 'L' ? -1 : 1;
    if (blocked.has(cell + delta)) return false;
    cell += delta;
  }
  return cell === target;
}

test('a walk path is the shortest route around walls and boxes', () => {
  const { tiles, width } = ringed(5);
  // The robot at row 1, column 2; its floor neighbors are one step away.
  assert.equal(walkRoute(width, tiles, [], 7, 6), 'L');
  assert.equal(walkRoute(width, tiles, [], 7, 8), 'R');
  assert.equal(walkRoute(width, tiles, [], 7, 12), 'D');
  // Around a box at row 2, column 2, the target below it takes four steps, whichever way the path goes.
  const detour = walkRoute(width, tiles, [12], 7, 17);
  assert.ok(detour !== undefined && detour.length === 4, `the detour is ${detour}`);
  assert.ok(follows(detour!, width, 7, 17, new Set([12])), detour);
});

test('a walk never crosses a row end, a wall, a box, or an unreachable cell', () => {
  const { tiles, width } = ringed(4);
  assert.equal(walkRoute(width, tiles, [], 7, 7), undefined);
  assert.equal(walkRoute(width, tiles, [], 7, 0), undefined);
  assert.equal(walkRoute(width, tiles, [8], 7, 8), undefined);
  // A robot enclosed by boxes reaches nothing.
  assert.equal(walkRoute(width, tiles, [6, 8, 12], 7, 16), undefined);
  // On an all-floor board, the cell after a row's last column belongs to the
  // next row: the path around to it is five steps, never the one-step wrap.
  const open = new Uint8Array(15);
  const around = walkRoute(5, open, [], 9, 10);
  assert.ok(around !== undefined && around.length === 5, `the around path is ${around}`);
  assert.ok(follows(around!, 5, 9, 10, new Set()), around);
});
