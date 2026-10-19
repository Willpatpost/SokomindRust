import { ACTIONS, WALL } from './protocol.ts';

/** A walk path from one cell to another as U/D/L/R letters: the shortest route
 * the robot can follow without pushing anything, through floor cells only —
 * walls and the boxes' current cells block it. Returns undefined when the
 * target is the robot's own cell, or is a wall, a box, or unreachable.
 *
 * The path plans over the state Rust reports (tiles, box cells, robot cell);
 * each step still replays through WasmGame, which stays the source of truth:
 * a stale path simply gets blocked. */
export function walkRoute(width: number, tiles: Uint8Array, boxes: Iterable<number>, player: number, target: number): string | undefined {
  if (player === target) return undefined;
  if (tiles[target] === WALL) return undefined;
  const blocked = new Set<number>(boxes);
  if (blocked.has(target)) return undefined;
  const cameFrom = new Map<number, number>();
  cameFrom.set(player, player);
  const queue = [player];
  for (let head = 0; head < queue.length; head++) {
    const cell = queue[head]!;
    // The neighbors in ACTIONS's order: up, down, left, right.
    for (let step = 0; step < 4; step++) {
      const next = cell + (step === 0 ? -width : step === 1 ? width : step === 2 ? -1 : 1);
      // Vertical steps stay in bounds; a horizontal step must not wrap across
      // a row's end, which a shared row check catches.
      if (next < 0 || next >= tiles.length) continue;
      if (step >= 2 && Math.floor(next / width) !== Math.floor(cell / width)) continue;
      if (cameFrom.has(next) || tiles[next] === WALL || blocked.has(next)) continue;
      cameFrom.set(next, cell);
      if (next === target) return retrace(cameFrom, width, player, target);
      queue.push(next);
    }
  }
  return undefined;
}

/** The letters from player to target, walked backward through the map. */
function retrace(cameFrom: Map<number, number>, width: number, player: number, target: number): string {
  let route = '';
  for (let cell = target; cell !== player; cell = cameFrom.get(cell)!) {
    route = ACTIONS[direction(cameFrom.get(cell)!, cell, width)]! + route;
  }
  return route;
}

/** The ACTIONS index of the step between two adjacent cells. */
function direction(from: number, to: number, width: number): number {
  const delta = to - from;
  return delta === -width ? 0 : delta === width ? 1 : delta === -1 ? 2 : 3;
}
