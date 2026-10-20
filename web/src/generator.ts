/** The puzzle generator: grows a room to order. A seeded cellular-automata
 * floor becomes a solved template, reverse pulls scramble it into a puzzle,
 * and the loop keeps whatever the engine itself solves within a tier of the
 * target. The pulls are only a gardener — no claim about a room stands until
 * the injected solve returns a route the engine verified, and the typed pairs
 * ride the traced route, so the engine certifies every room that loads.
 * Pure data and functions — main.ts injects the solver and the tracer. */
import { DIFFICULTIES } from './catalog.ts';
import { TYPED_LABELS } from './editor.ts';

/** A cell's place on the grid. */
export interface Position {
  row: number;
  column: number;
}

/** The puzzle the caller asks for. */
export interface GeneratorParams {
  width: number;
  height: number;
  boxCount: number;
  targetDifficulty: string;
  useLabels: boolean;
  maxAttempts: number;
}

export const DEFAULT_PARAMS: GeneratorParams = {
  width: 7,
  height: 7,
  boxCount: 2,
  targetDifficulty: 'beginner',
  useLabels: false,
  maxAttempts: 50,
};

/** Each tier names the room it grows: its canvas and its boxes. */
export const PRESETS: Record<string, { width: number; height: number; boxCount: number }> = {
  tutorial: { width: 5, height: 5, boxCount: 1 },
  beginner: { width: 6, height: 6, boxCount: 2 },
  intermediate: { width: 8, height: 8, boxCount: 3 },
  advanced: { width: 9, height: 9, boxCount: 5 },
  expert: { width: 11, height: 11, boxCount: 6 },
  master: { width: 12, height: 12, boxCount: 8 },
};

/** How far each tier scrambles its rooms: the reverse pulls it aims for. */
export const PULL_COUNTS: Record<string, { min: number; max: number }> = {
  tutorial: { min: 3, max: 8 },
  beginner: { min: 8, max: 20 },
  intermediate: { min: 20, max: 50 },
  advanced: { min: 40, max: 100 },
  expert: { min: 80, max: 200 },
  master: { min: 150, max: 350 },
};

/** A solved board before any scramble: walls, floor, the goals and the robot. */
export interface Template {
  width: number;
  height: number;
  grid: string[][];
  goalPositions: Position[];
  robotPosition: Position;
}

/** A scrambled board: the boxes pulled off their goals, and the robot. */
export interface Scrambled {
  template: Template;
  boxPositions: Position[];
  robotPosition: Position;
  reversePulls: number;
}

/** One search the engine verified: its route and what the route costs. */
export interface Solution {
  moves: number;
  pushes: number;
  route: string;
}

/** What the page injects: the engine's solve, and the tracer that walks a
 * route to pair each box with the goal it lands on. */
export interface Deps {
  solve(rows: string[]): Promise<Solution | null>;
  trace(rows: string[], route: string): ReadonlyArray<{ box: Position; goal: Position }> | null;
}

export interface Progress {
  attempt: number;
  maxAttempts: number;
  phase: 'generating' | 'solving' | 'classifying';
  message: string;
}

export type ProgressCallback = (progress: Progress) => void;

export type GenerationResult =
  | { status: 'success'; rows: string[]; difficulty: string; boxes: number; attempts: number; moves: number; pushes: number }
  | { status: 'failed'; attempts: number; lastReason: string };

/** Where a tier sits, as the acceptance window counts distance. */
function tierIndex(difficulty: string): number {
  return DIFFICULTIES.indexOf(difficulty);
}

/** Mulberry32: a small seeded PRNG, so a seed grows the same room twice. */
export function createRng(seed: number): () => number {
  return function () {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Fisher-Yates, in place, under the caller's rng. */
export function shuffle<T>(values: T[], rng: () => number): void {
  for (let i = values.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [values[i], values[j]] = [values[j], values[i]];
  }
}

const WALL = 'O';
const FLOOR = ' ';

const key = (position: Position) => `${position.row},${position.column}`;
const inBounds = (grid: readonly (readonly string[])[], row: number, column: number) =>
  row >= 0 && row < grid.length && column >= 0 && column < grid[0]!.length;
const isFloor = (grid: readonly (readonly string[])[], row: number, column: number) => grid[row]![column] !== WALL;

/** The floor cells reachable from `start`, walls aside. */
export function floodFill(grid: readonly (readonly string[])[], start: Position): Set<string> {
  const visited = new Set<string>();
  if (!inBounds(grid, start.row, start.column) || grid[start.row]![start.column] === WALL) return visited;
  visited.add(key(start));
  const queue: Position[] = [start];
  for (let head = 0; head < queue.length; head++) {
    const current = queue[head]!;
    for (const [dr, dc] of [
      [-1, 0],
      [1, 0],
      [0, -1],
      [0, 1],
    ] as const) {
      const row = current.row + dr;
      const column = current.column + dc;
      const cell = `${row},${column}`;
      if (inBounds(grid, row, column) && !visited.has(cell) && grid[row]![column] !== WALL) {
        visited.add(cell);
        queue.push({ row, column });
      }
    }
  }
  return visited;
}

/** Grows a floor by cellular automata: random noise smoothed into rooms, the
 * largest connected keep, everything else walled. Throws when no attempt
 * reaches `minFloorCells` — the caller counts that as a failed attempt. */
export function generateFloorLayout(width: number, height: number, minFloorCells: number, rng: () => number): string[][] {
  for (let attempt = 0; attempt < 20; attempt++) {
    const grid: string[][] = [];
    for (let row = 0; row < height; row++) grid.push(new Array<string>(width).fill(WALL));
    // The border stays wall; the interior seeds floor a little over half.
    for (let row = 1; row < height - 1; row++)
      for (let column = 1; column < width - 1; column++) grid[row]![column] = rng() < 0.58 ? FLOOR : WALL;
    // Three smoothing passes over the 8-connected neighborhood: lonely floor
    // dies, well-attended wall opens.
    for (let pass = 0; pass < 3; pass++) {
      const snapshot = grid.map(row => [...row]);
      for (let row = 1; row < height - 1; row++)
        for (let column = 1; column < width - 1; column++) {
          let floorNeighbors = 0;
          for (let dr = -1; dr <= 1; dr++)
            for (let dc = -1; dc <= 1; dc++) {
              if (dr === 0 && dc === 0) continue;
              if (snapshot[row + dr]![column + dc] !== WALL) floorNeighbors++;
            }
          if (snapshot[row]![column] !== WALL && floorNeighbors < 3) grid[row]![column] = WALL;
          else if (snapshot[row]![column] === WALL && floorNeighbors > 4) grid[row]![column] = FLOOR;
        }
    }
    // Keep only the largest connected floor; the rest becomes wall.
    const visited = new Set<string>();
    let largest = new Set<string>();
    for (let row = 0; row < height; row++)
      for (let column = 0; column < width; column++) {
        const cell = `${row},${column}`;
        if (grid[row]![column] === WALL || visited.has(cell)) continue;
        const component = floodFill(grid, { row, column });
        for (const member of component) visited.add(member);
        if (component.size > largest.size) largest = component;
      }
    for (let row = 0; row < height; row++)
      for (let column = 0; column < width; column++)
        if (grid[row]![column] !== WALL && !largest.has(`${row},${column}`)) grid[row]![column] = WALL;
    if (largest.size >= minFloorCells) return grid;
  }
  throw new Error(`No floor layout of ${minFloorCells} cells grew in 20 attempts`);
}

/** Places the goals and the robot: away from dead corners and one-way
 * alcoves, and where the robot can reach every goal. */
export function placeGoalsAndRobot(
  grid: readonly (readonly string[])[],
  boxCount: number,
  rng: () => number,
): { goalPositions: Position[]; robotPosition: Position } {
  const height = grid.length;
  const width = grid[0]!.length;
  const floor: Position[] = [];
  for (let row = 0; row < height; row++)
    for (let column = 0; column < width; column++) if (grid[row]![column] === FLOOR) floor.push({ row, column });
  // A dead corner cannot leave in either axis; an alcove has one way out.
  const open = floor.filter(position => {
    const up = position.row > 0 ? grid[position.row - 1]![position.column] : WALL;
    const down = position.row < height - 1 ? grid[position.row + 1]![position.column] : WALL;
    const left = position.column > 0 ? grid[position.row]![position.column - 1] : WALL;
    const right = position.column < width - 1 ? grid[position.row]![position.column + 1] : WALL;
    const neighbors = (up !== WALL ? 1 : 0) + (down !== WALL ? 1 : 0) + (left !== WALL ? 1 : 0) + (right !== WALL ? 1 : 0);
    return !(up === WALL && down === WALL && left === WALL && right === WALL) && neighbors > 1;
  });
  const pool = [...(open.length >= boxCount + 1 ? open : floor)];
  shuffle(pool, rng);
  const goalPositions = pool.slice(0, boxCount);
  const remaining = pool.slice(boxCount);
  let robotPosition: Position | undefined;
  for (const candidate of remaining) {
    const reachable = floodFill(grid, candidate);
    if (goalPositions.every(goal => reachable.has(key(goal)))) {
      robotPosition = candidate;
      break;
    }
  }
  return { goalPositions, robotPosition: robotPosition ?? remaining[0] ?? floor[0]! };
}

/** The solved template a scramble starts from. */
export function generateBoardTemplate(width: number, height: number, boxCount: number, rng: () => number): Template {
  const grid = generateFloorLayout(width, height, boxCount * 3 + 5, rng);
  const { goalPositions, robotPosition } = placeGoalsAndRobot(grid, boxCount, rng);
  return { width, height, grid, goalPositions, robotPosition };
}

/** Whether the robot can walk from where it stands to `target`, the boxes
 * blocking as walls do. */
function canRobotReach(grid: readonly (readonly string[])[], robot: Position, target: Position, boxes: readonly Position[]): boolean {
  if (robot.row === target.row && robot.column === target.column) return true;
  const blocked = new Set(boxes.map(key));
  const visited = new Set<string>([key(robot)]);
  const queue: Position[] = [robot];
  for (let head = 0; head < queue.length; head++) {
    const current = queue[head]!;
    for (const [dr, dc] of [
      [-1, 0],
      [1, 0],
      [0, -1],
      [0, 1],
    ] as const) {
      const row = current.row + dr;
      const column = current.column + dc;
      const cell = `${row},${column}`;
      if (visited.has(cell) || !inBounds(grid, row, column) || !isFloor(grid, row, column) || blocked.has(cell)) continue;
      if (row === target.row && column === target.column) return true;
      visited.add(cell);
      queue.push({ row, column });
    }
  }
  return false;
}

/** Every pull the scramble could take next: for each box and direction, the
 * robot must reach the cell it pulls from, and both that cell and the one it
 * retreats to must be floor and clear. */
function enumerateReversePulls(grid: readonly (readonly string[])[], boxes: readonly Position[], robot: Position) {
  const pulls: Array<{ boxIndex: number; box: Position; robot: Position }> = [];
  const boxSet = new Set(boxes.map(key));
  for (let boxIndex = 0; boxIndex < boxes.length; boxIndex++) {
    const box = boxes[boxIndex]!;
    for (const [dr, dc] of [
      [-1, 0],
      [1, 0],
      [0, -1],
      [0, 1],
    ] as const) {
      const pullRow = box.row + dr;
      const pullColumn = box.column + dc;
      const retreatRow = pullRow + dr;
      const retreatColumn = pullColumn + dc;
      if (!inBounds(grid, pullRow, pullColumn) || !isFloor(grid, pullRow, pullColumn) || boxSet.has(`${pullRow},${pullColumn}`)) continue;
      if (
        !inBounds(grid, retreatRow, retreatColumn)
        || !isFloor(grid, retreatRow, retreatColumn)
        || boxSet.has(`${retreatRow},${retreatColumn}`)
      )
        continue;
      if (!canRobotReach(grid, robot, { row: pullRow, column: pullColumn }, boxes)) continue;
      pulls.push({ boxIndex, box: { row: pullRow, column: pullColumn }, robot: { row: retreatRow, column: retreatColumn } });
    }
  }
  return pulls;
}

/** Pulls boxes off their goals, playing the puzzle backwards from solved. The
 * scramble stops at `pullCount` pulls, or when it goes stale; what it leaves
 * is solvable by construction — the loop still makes the engine prove it. */
export function scrambleByReversePull(template: Template, pullCount: number, rng: () => number): Scrambled {
  const boxPositions = template.goalPositions.map(goal => ({ ...goal }));
  let robotPosition = { ...template.robotPosition };
  let reversePulls = 0;
  let stale = 0;
  for (let iteration = 0; iteration < pullCount * 3 && reversePulls < pullCount && stale <= pullCount * 2; iteration++) {
    const pulls = enumerateReversePulls(template.grid, boxPositions, robotPosition);
    if (pulls.length === 0) {
      stale++;
      continue;
    }
    const pull = pulls[Math.floor(rng() * pulls.length)]!;
    boxPositions[pull.boxIndex] = pull.box;
    robotPosition = pull.robot;
    reversePulls++;
    stale = 0;
  }
  return { template, boxPositions, robotPosition, reversePulls };
}

/** The puzzle rows a scramble names: goals, boxes and the robot written into
 * the template's grid, the robot relocated off the goals it would sit on. */
export function buildPuzzleFromScramble(scrambled: Scrambled): { rows: string[]; boxes: number } {
  const { template, boxPositions } = scrambled;
  let robot = scrambled.robotPosition;
  const grid = template.grid.map(row => [...row]);
  const goals = new Set(template.goalPositions.map(key));
  const boxes = new Set(boxPositions.map(key));
  if (goals.has(key(robot))) {
    // The robot walks to the nearest cell that is neither goal nor box.
    const queue: Position[] = [robot];
    const visited = new Set<string>([key(robot)]);
    for (let head = 0; head < queue.length && goals.has(key(robot)); head++) {
      const current = queue[head]!;
      for (const [dr, dc] of [
        [-1, 0],
        [1, 0],
        [0, -1],
        [0, 1],
      ] as const) {
        const row = current.row + dr;
        const column = current.column + dc;
        const cell = `${row},${column}`;
        if (visited.has(cell) || !inBounds(grid, row, column) || grid[row]![column] === WALL) continue;
        visited.add(cell);
        const candidate = { row, column };
        if (!goals.has(cell) && !boxes.has(cell)) {
          robot = candidate;
          break;
        }
        queue.push(candidate);
      }
    }
  }
  for (const goal of template.goalPositions) grid[goal.row]![goal.column] = 'S';
  for (const box of boxPositions) grid[box.row]![box.column] = 'X';
  grid[robot.row]![robot.column] = 'R';
  return { rows: grid.map(row => row.join('')), boxes: boxPositions.length };
}

/** The tier a solved room reads as, by the route the engine found: the first
 * tier whose ceilings the route stays under, master when none do. */
const THRESHOLDS: Record<string, { moves: number; pushes: number; boxes: number }> = {
  tutorial: { moves: 10, pushes: 5, boxes: 2 },
  beginner: { moves: 25, pushes: 15, boxes: 3 },
  intermediate: { moves: 80, pushes: 40, boxes: 5 },
  advanced: { moves: 200, pushes: 80, boxes: 7 },
  expert: { moves: 500, pushes: 200, boxes: 10 },
};

export function classify(moves: number, pushes: number, boxCount: number): string {
  for (const difficulty of DIFFICULTIES) {
    const ceiling = THRESHOLDS[difficulty];
    if (ceiling !== undefined && moves <= ceiling.moves && pushes <= ceiling.pushes && boxCount <= ceiling.boxes) return difficulty;
  }
  return 'master';
}

/** Writes typed pairs over a generic room: each traced box takes a label, its
 * goal the label's lowercase mate, and the rest stay X and S. The pairing came
 * from the engine's own route, so the route still solves the typed room. */
export function assignLabels(
  rows: readonly string[],
  pairing: ReadonlyArray<{ box: Position; goal: Position }>,
  rng: () => number,
): string[] {
  if (pairing.length < 2 || pairing.length > TYPED_LABELS.length) return [...rows];
  const labels = [...TYPED_LABELS.slice(0, pairing.length)];
  shuffle(labels, rng);
  const boxes = new Map(pairing.map((pair, index) => [key(pair.box), labels[index]!]));
  const goals = new Map(pairing.map((pair, index) => [key(pair.goal), labels[index]!.toLowerCase()]));
  return rows.map((row, rowIndex) =>
    [...row]
      .map((symbol, column) => {
        if (symbol !== 'X' && symbol !== 'S') return symbol;
        const cell = `${rowIndex},${column}`;
        return symbol === 'X' ? (boxes.get(cell) ?? symbol) : (goals.get(cell) ?? symbol);
      })
      .join(''),
  );
}

/** Grows rooms until one the engine solves lands within a tier of the target.
 * The best near miss is kept, so a failed run still offers its closest room. */
export async function generate(
  params: GeneratorParams,
  deps: Deps,
  onProgress?: ProgressCallback,
  signal?: { aborted: boolean },
  seed: number = (Date.now() ^ (Math.random() * 0xffffffff)) >>> 0,
): Promise<GenerationResult> {
  const rng = createRng(seed);
  const pulls = PULL_COUNTS[params.targetDifficulty] ?? PULL_COUNTS.beginner!;
  let best: { rows: string[]; boxes: number; moves: number; pushes: number; route: string; difficulty: string } | null = null;
  let lastReason = 'No attempts completed';
  const withinWindow = (difficulty: string) => Math.abs(tierIndex(difficulty) - tierIndex(params.targetDifficulty)) <= 1;
  const closer = (a: string, b: string) =>
    Math.abs(tierIndex(params.targetDifficulty) - tierIndex(a)) < Math.abs(tierIndex(params.targetDifficulty) - tierIndex(b));

  const label = (rows: string[], route: string): string[] => {
    if (!params.useLabels) return rows;
    const pairing = deps.trace(rows, route);
    return pairing === null ? rows : assignLabels(rows, pairing, rng);
  };

  for (let attempt = 1; attempt <= params.maxAttempts; attempt++) {
    if (signal?.aborted) return { status: 'failed', attempts: attempt - 1, lastReason: 'Cancelled' };
    onProgress?.({ attempt, maxAttempts: params.maxAttempts, phase: 'generating', message: `Growing board (attempt ${attempt})...` });
    // Yield so the progress paints between attempts.
    await new Promise(resolve => setTimeout(resolve, 0));

    let template: Template;
    try {
      template = generateBoardTemplate(params.width, params.height, params.boxCount, rng);
    } catch {
      lastReason = 'No floor layout grew';
      continue;
    }
    const pullCount = pulls.min + Math.floor(rng() * (pulls.max - pulls.min + 1));
    const scrambled = scrambleByReversePull(template, pullCount, rng);
    if (scrambled.reversePulls < Math.max(1, Math.floor(pulls.min / 2))) {
      lastReason = 'Too few reverse pulls succeeded';
      continue;
    }
    const goals = new Set(scrambled.template.goalPositions.map(key));
    if (scrambled.boxPositions.some(box => goals.has(key(box)))) {
      lastReason = 'Some boxes remain on their goals';
      continue;
    }
    const built = buildPuzzleFromScramble(scrambled);

    onProgress?.({ attempt, maxAttempts: params.maxAttempts, phase: 'solving', message: `Solving room (attempt ${attempt})...` });
    const solution = await deps.solve(built.rows);
    if (solution === null) {
      lastReason = 'The engine could not solve the room';
      continue;
    }
    const difficulty = classify(solution.moves, solution.pushes, built.boxes);
    onProgress?.({
      attempt,
      maxAttempts: params.maxAttempts,
      phase: 'classifying',
      message: `Read as ${difficulty} (${solution.moves} moves, ${solution.pushes} pushes)`,
    });
    const acceptable = withinWindow(difficulty);
    if (best === null || (acceptable && best.difficulty !== params.targetDifficulty) || (acceptable && closer(difficulty, best.difficulty)))
      best = { rows: built.rows, boxes: built.boxes, moves: solution.moves, pushes: solution.pushes, route: solution.route, difficulty };
    if (!acceptable) {
      lastReason = `Read as ${difficulty}, wanted ${params.targetDifficulty}`;
      continue;
    }
    return {
      status: 'success',
      rows: label(built.rows, solution.route),
      difficulty,
      boxes: built.boxes,
      attempts: attempt,
      moves: solution.moves,
      pushes: solution.pushes,
    };
  }

  if (best !== null)
    return {
      status: 'success',
      rows: label(best.rows, best.route),
      difficulty: best.difficulty,
      boxes: best.boxes,
      attempts: params.maxAttempts,
      moves: best.moves,
      pushes: best.pushes,
    };
  return { status: 'failed', attempts: params.maxAttempts, lastReason };
}
