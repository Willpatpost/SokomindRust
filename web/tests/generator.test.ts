import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DIFFICULTIES } from '../src/catalog.ts';
import { isTypedBox, isTypedGoal } from '../src/editor.ts';
import * as generator from '../src/generator.ts';

test('the seeded rng grows the same sequence twice', () => {
  const first = generator.createRng(42);
  const second = generator.createRng(42);
  for (let draw = 0; draw < 10; draw++) assert.equal(first(), second());
  const value = first();
  assert.ok(value >= 0 && value < 1);
});

test('a floor layout keeps its border walled and its floor connected', () => {
  const grid = generator.generateFloorLayout(9, 9, 10, generator.createRng(7));
  assert.equal(grid.length, 9);
  for (const row of grid) assert.equal(row.length, 9);
  for (let column = 0; column < 9; column++) {
    assert.equal(grid[0]![column], 'O');
    assert.equal(grid[8]![column], 'O');
    assert.equal(grid[column]![0], 'O');
    assert.equal(grid[column]![8], 'O');
  }
  // The floor is one component: every floor cell is reachable from the first.
  const floor: generator.Position[] = [];
  grid.forEach((row, row_index) =>
    [...row].forEach((symbol, column) => {
      if (symbol !== 'O') floor.push({ row: row_index, column });
    }),
  );
  assert.ok(floor.length >= 10);
  const reachable = generator.floodFill(grid, floor[0]!);
  for (const cell of floor) assert.ok(reachable.has(`${cell.row},${cell.column}`));
});

test('a template places distinct goals and a robot that can reach them', () => {
  const template = generator.generateBoardTemplate(7, 7, 2, generator.createRng(11));
  assert.equal(template.goalPositions.length, 2);
  const keys = new Set(template.goalPositions.map(goal => `${goal.row},${goal.column}`));
  assert.equal(keys.size, 2);
  assert.ok(template.grid[template.robotPosition.row]![template.robotPosition.column] !== 'O');
  const reachable = generator.floodFill(template.grid, template.robotPosition);
  for (const goal of template.goalPositions) assert.ok(reachable.has(`${goal.row},${goal.column}`));
});

test('the scramble pulls boxes along the floor, bounded by its budget', () => {
  const rng = generator.createRng(13);
  const template = generator.generateBoardTemplate(7, 7, 2, rng);
  const scrambled = generator.scrambleByReversePull(template, 10, rng);
  assert.ok(scrambled.reversePulls <= 10);
  assert.equal(scrambled.boxPositions.length, template.goalPositions.length);
  for (const box of scrambled.boxPositions) assert.ok(template.grid[box.row]![box.column] !== 'O');
});

test('a scramble becomes puzzle rows with goals, boxes and one robot', () => {
  const rng = generator.createRng(17);
  const template = generator.generateBoardTemplate(7, 7, 2, rng);
  const scrambled = generator.scrambleByReversePull(template, 10, rng);
  const built = generator.buildPuzzleFromScramble(scrambled);
  assert.equal(built.rows.length, template.height);
  for (const row of built.rows) assert.equal(row.length, template.width);
  const flat = built.rows.join('');
  assert.equal(flat.split('R').length - 1, 1);
  assert.equal(flat.split('X').length - 1, 2);
  assert.equal(flat.split('S').length - 1, 2);
});

test('the tier a route reads as', () => {
  assert.equal(generator.classify(10, 5, 2), 'tutorial');
  assert.equal(generator.classify(11, 5, 2), 'beginner');
  assert.equal(generator.classify(25, 15, 3), 'beginner');
  assert.equal(generator.classify(26, 15, 3), 'intermediate');
  assert.equal(generator.classify(80, 40, 5), 'intermediate');
  assert.equal(generator.classify(81, 40, 5), 'advanced');
  assert.equal(generator.classify(200, 80, 7), 'advanced');
  assert.equal(generator.classify(201, 80, 7), 'expert');
  assert.equal(generator.classify(500, 200, 10), 'expert');
  assert.equal(generator.classify(501, 200, 10), 'master');
  // Boxes alone can lift the tier past the route's size.
  assert.equal(generator.classify(5, 2, 4), 'intermediate');
  assert.equal(generator.classify(5, 2, 11), 'master');
});

test('typed pairs ride the traced pairing, even between identical rows', () => {
  const rows = ['OOOOO', 'O R O', 'O X O', 'O X O', 'O S O', 'O S O', 'OOOOO'];
  const pairing = [
    { box: { row: 3, column: 2 }, goal: { row: 5, column: 2 } },
    { box: { row: 2, column: 2 }, goal: { row: 4, column: 2 } },
  ];
  const typed = generator.assignLabels(rows, pairing, generator.createRng(3));
  // Rows 2 and 3 are identical: each traced box takes its own label on its own row.
  assert.ok(isTypedBox(typed[3]![2]!));
  assert.ok(isTypedBox(typed[2]![2]!));
  assert.ok(typed[3]![2] !== typed[2]![2]);
  // Each goal takes its box's lowercase mate.
  assert.ok(isTypedGoal(typed[5]![2]!));
  assert.equal(typed[5]![2], typed[3]![2]!.toLowerCase());
  assert.equal(typed[4]![2], typed[2]![2]!.toLowerCase());
  // The robot and the walls never take a label.
  assert.equal(typed[1]![2], 'R');
  assert.equal(typed[0], 'OOOOO');
});

/** A fake engine: one answer for every room, and the rooms it saw. */
function fakeEngine(answer: generator.Solution | null) {
  const seen: string[][] = [];
  const solve = (rows: string[]) => {
    seen.push(rows);
    return Promise.resolve(answer);
  };
  return { solve, seen };
}

/** Pairs the X cells with the S cells in order, as the real tracer would. */
function pairByOrder(rows: string[]) {
  const boxes: generator.Position[] = [];
  const goals: generator.Position[] = [];
  rows.forEach((row, row_index) =>
    [...row].forEach((symbol, column) => {
      if (symbol === 'X') boxes.push({ row: row_index, column });
      if (symbol === 'S') goals.push({ row: row_index, column });
    }),
  );
  return boxes.map((box, index) => ({ box, goal: goals[index]! }));
}

const GROW: generator.GeneratorParams = {
  width: 7,
  height: 7,
  boxCount: 2,
  targetDifficulty: 'beginner',
  useLabels: false,
  maxAttempts: 60,
};

test('the loop keeps what the engine solves within a tier of the target', async () => {
  const engine = fakeEngine({ moves: 20, pushes: 10, route: 'route' });
  const result = await generator.generate(GROW, { solve: engine.solve, trace: () => null }, undefined, undefined, 20261020);
  if (result.status !== 'success') assert.fail('a solved beginner room was grown');
  assert.equal(result.difficulty, 'beginner');
  assert.ok(result.attempts >= 1);
  assert.equal(result.moves, 20);
  assert.equal(result.pushes, 10);
  // What loads is a room: one robot, as many boxes as goals.
  const flat = result.rows.join('');
  assert.equal(flat.split('R').length - 1, 1);
  assert.equal(flat.split('X').length - 1, 2);
  assert.equal(flat.split('S').length - 1, 2);
  assert.ok(engine.seen.length >= 1);
});

test('a room one tier away still loads, read as its own tier', async () => {
  const engine = fakeEngine({ moves: 60, pushes: 30, route: 'route' });
  const result = await generator.generate(GROW, { solve: engine.solve, trace: () => null }, undefined, undefined, 20261021);
  if (result.status !== 'success') assert.fail('a solved intermediate room was grown');
  assert.equal(result.difficulty, 'intermediate');
});

test('a room no tier window holds is offered as the best near miss', async () => {
  const engine = fakeEngine({ moves: 600, pushes: 300, route: 'route' });
  const result = await generator.generate(GROW, { solve: engine.solve, trace: () => null }, undefined, undefined, 20261022);
  if (result.status !== 'success') assert.fail('the near miss was kept');
  assert.equal(result.difficulty, 'master');
  assert.equal(result.attempts, GROW.maxAttempts);
});

test('a room the engine cannot solve never loads', async () => {
  const engine = fakeEngine(null);
  const result = await generator.generate(GROW, { solve: engine.solve, trace: () => null }, undefined, undefined, 20261023);
  assert.equal(result.status, 'failed');
  if (result.status === 'failed') {
    assert.equal(result.attempts, GROW.maxAttempts);
    assert.equal(result.lastReason, 'The engine could not solve the room');
  }
});

test('a cancelled run stops before its first attempt', async () => {
  const engine = fakeEngine({ moves: 20, pushes: 10, route: 'route' });
  const result = await generator.generate(GROW, { solve: engine.solve, trace: () => null }, undefined, { aborted: true }, 20261024);
  assert.equal(result.status, 'failed');
  if (result.status === 'failed') {
    assert.equal(result.attempts, 0);
    assert.equal(result.lastReason, 'Cancelled');
  }
  assert.equal(engine.seen.length, 0);
});

test('labeled rooms wear typed pairs traced off the route', async () => {
  const engine = fakeEngine({ moves: 20, pushes: 10, route: 'route' });
  const result = await generator.generate(
    { ...GROW, useLabels: true },
    { solve: engine.solve, trace: rows => pairByOrder(rows) },
    undefined,
    undefined,
    20261025,
  );
  if (result.status !== 'success') assert.fail('a labeled room was grown');
  const flat = result.rows.join('');
  assert.ok([...flat].some(isTypedBox));
  assert.ok([...flat].some(isTypedGoal));
  // Every box and goal took a label: none stayed generic.
  assert.ok(!flat.includes('X'));
  assert.ok(!flat.includes('S'));
});

test('a tracer that cannot pair leaves the room generic', async () => {
  const engine = fakeEngine({ moves: 20, pushes: 10, route: 'route' });
  const result = await generator.generate(
    { ...GROW, useLabels: true },
    { solve: engine.solve, trace: () => null },
    undefined,
    undefined,
    20261026,
  );
  if (result.status !== 'success') assert.fail('a room was grown');
  const flat = result.rows.join('');
  assert.ok(![...flat].some(isTypedBox));
  assert.equal(flat.split('X').length - 1, 2);
});

test('every tier names the room it grows and how far it scrambles', () => {
  for (const difficulty of DIFFICULTIES) {
    const preset = generator.PRESETS[difficulty];
    const pulls = generator.PULL_COUNTS[difficulty];
    assert.ok(preset !== undefined, difficulty);
    assert.ok(pulls !== undefined, difficulty);
    assert.ok(preset.boxCount >= 1);
    assert.ok(preset.width >= 5 && preset.height >= 5);
    assert.ok(pulls.min >= 1 && pulls.min <= pulls.max);
  }
});
