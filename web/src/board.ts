import './styles/board.css';
import { WALL, type Snapshot } from './protocol.ts';

// Tile encoding, as WasmGame.tiles returns it: one byte per cell in row-major
// order, 0 for floor, WALL for a wall or row padding, and otherwise the ASCII
// code of a goal's label (PLAIN for an S goal). Boxes and the robot come from
// the snapshot instead. WALL lives in protocol.ts, the contracts module.
const PLAIN = 88; // 'X': the label of unlettered boxes and their goals
/** The largest tile, in CSS pixels: a small board is drawn at this size. */
const MAX_TILE_PX = 44;
/** The most height a board may take, in CSS pixels: a taller one gets smaller tiles, down to MIN_TILE_PX. */
const MAX_BOARD_HEIGHT_PX = 540;
/** The smallest tile, in CSS pixels. A board that would need smaller tiles to fit keeps this size
 * and scrolls instead. */
const MIN_TILE_PX = 4;
/** How many keeper positions the walking trail keeps behind the robot. */
const TRAIL_LENGTH = 6;

/** The hue a label paints its box and goal: unlettered pieces share the design's
 * neutral crate hue, and every typed label gets its own deterministic hue, so a
 * box and its matching goal always paint the same color. */
function hue(label: number): number {
  return label === PLAIN ? 32 : (label * 47) % 360;
}

/** The CSS grid area of a cell, as 'row / column', both 1-based. */
function area(cell: number, width: number): string {
  return `${Math.floor(cell / width) + 1} / ${(cell % width) + 1}`;
}

/** One overlaying grid layer, positioned by board.css. */
function layer(name: string): HTMLElement {
  const element = document.createElement('div');
  element.className = `layer ${name}`;
  return element;
}

/** Re-applies a piece's animation class, so a repeat event replays it. */
function restart(piece: HTMLElement, name: string) {
  piece.classList.remove(name);
  // Read a layout property so the browser sees the classless state first.
  void piece.offsetWidth;
  piece.classList.add(name);
  setTimeout(() => piece.classList.remove(name), 260);
}

/** Draws the board as DOM pieces over a fixed cell grid, in the ported design's
 * move language: pieces slide between cells, a pushed crate squashes, a blocked
 * robot shakes against its direction, the keeper leaves a fading trail, and a box
 * landing on its goal ripples. The cell grid and the pieces are rebuilt only when
 * a new puzzle's tiles arrive; every draw after that moves pieces, so undo and
 * replay animate too. */
export class BoardView {
  /** The board element, inside its scrolling wrap. */
  private element: HTMLElement;
  private wrap: HTMLElement;
  /** Live computed style of the board's container. */
  private wrapStyle: CSSStyleDeclaration;
  /** The puzzle's grid shape, kept for cellAt's point mapping. */
  private columns = 0;
  private rows = 0;
  /** The whole board fits at MIN_TILE_PX tiles or larger, so it never scrolls and swipes can move the robot. */
  fits = true;
  /** The puzzle the elements were built for, identified by its tiles and labels,
   * which a session holds by reference. */
  private tiles: Uint8Array | undefined;
  private labels: Uint8Array | undefined;
  /** The grid layers, in paint order: cells, trail, feedback, pieces. */
  private cells: HTMLElement;
  private trail: HTMLElement;
  private feedback: HTMLElement;
  private pieces: HTMLElement;
  /** The moving pieces: one crate per label, in label order, and the robot. */
  private crates: HTMLElement[] = [];
  private robot: HTMLElement;
  /** The last snapshot drawn, whose positions a change animates from. */
  private previous: Snapshot | undefined;
  private homes: boolean[] = [];
  /** The keeper positions behind the robot, oldest first. */
  private path: number[] = [];

  constructor(element: HTMLElement) {
    this.element = element;
    this.wrap = element.parentElement!;
    this.wrapStyle = getComputedStyle(this.wrap);
    this.cells = document.createElement('div');
    this.cells.className = 'layer cells';
    this.trail = layer('trail');
    this.feedback = layer('feedback');
    this.pieces = layer('pieces');
    this.robot = document.createElement('div');
    this.robot.className = 'robot';
    element.append(this.cells, this.trail, this.feedback, this.pieces);
  }

  /** Paints the board for a snapshot of it: the puzzle's fixed grid once, then
   * the pieces, each animated from the last snapshot's position when it moved. */
  draw(width: number, height: number, tiles: Uint8Array, labels: Uint8Array, state: Snapshot, onGoal: boolean[]) {
    if (tiles !== this.tiles || labels !== this.labels) this.rebuild(width, tiles, labels);
    this.size(width, height);
    const previous = this.previous;
    const pushed = previous !== undefined && state.pushes > previous.pushes;
    for (let i = 0; i < this.crates.length; i++) {
      const crate = this.crates[i];
      const moved = previous !== undefined && previous.boxes[i] !== state.boxes[i];
      this.place(crate, state.boxes[i], width, moved ? previous!.boxes[i] : undefined);
      crate.toggleAttribute('data-home', onGoal[i]);
      if (pushed && moved) restart(crate, 'pushed');
      if (onGoal[i] && !this.homes[i]) this.ripple(state.boxes[i], width);
    }
    if (previous !== undefined && previous.player !== state.player) {
      if (this.path[this.path.length - 1] !== previous.player) this.path.push(previous.player);
      while (this.path.length > TRAIL_LENGTH) this.path.shift();
      this.trail.replaceChildren(
        ...this.path.map(cell => {
          const dot = document.createElement('div');
          dot.className = 'trail-dot';
          dot.style.gridArea = area(cell, width);
          return dot;
        }),
      );
    }
    const walked = previous !== undefined && previous.player !== state.player;
    this.place(this.robot, state.player, width, walked ? previous!.player : undefined);
    this.element.toggleAttribute('data-solved', state.solved);
    const label =
      `${width} by ${height} puzzle, ${labels.length} boxes. `
      + `${state.moves} moves, ${state.pushes} pushes.${state.solved ? ' Solved.' : ''} Use arrow keys or WASD to move, Z to undo.`;
    if (this.element.getAttribute('aria-label') !== label) this.element.setAttribute('aria-label', label);
    this.previous = state;
    this.homes = onGoal;
  }

  /** Shakes the robot against the direction it could not move. */
  blocked(direction: number) {
    restart(this.robot, ['blocked-up', 'blocked-down', 'blocked-left', 'blocked-right'][direction]!);
  }

  /** The cell a client point lands on, correct under the zoom transform: the
   * browser maps the element's rect through it. */
  cellAt(x: number, y: number): number | undefined {
    const rect = this.element.getBoundingClientRect();
    const column = Math.floor(((x - rect.left) / rect.width) * this.columns);
    const row = Math.floor(((y - rect.top) / rect.height) * this.rows);
    if (column < 0 || row < 0 || column >= this.columns || row >= this.rows) return undefined;
    return row * this.columns + column;
  }

  /** Seats a piece at `cell`, sliding it from `from` when the two differ: the
   * piece starts offset by the cells it traveled and transitions home, which also
   * animates undo and replay, the travels those retrace. */
  private place(piece: HTMLElement, cell: number, width: number, from: number | undefined) {
    piece.style.gridArea = area(cell, width);
    if (from === undefined || from === cell) return;
    const columns = (from % width) - (cell % width);
    const rows = Math.floor(from / width) - Math.floor(cell / width);
    piece.style.transform = `translate(${columns * 100}%, ${rows * 100}%)`;
    // Read a layout property so the browser applies the offset first.
    void piece.offsetWidth;
    piece.classList.add('settling');
    piece.style.transform = '';
    setTimeout(() => piece.classList.remove('settling'), 240);
  }

  /** Rings a box's goal when the box lands on it. */
  private ripple(cell: number, width: number) {
    const ripple = document.createElement('div');
    ripple.className = 'ripple';
    ripple.style.gridArea = area(cell, width);
    this.feedback.append(ripple);
    setTimeout(() => ripple.remove(), 700);
  }

  /** Builds the fixed cell grid and fresh pieces for a new puzzle. */
  private rebuild(width: number, tiles: Uint8Array, labels: Uint8Array) {
    this.tiles = tiles;
    this.labels = labels;
    this.previous = undefined;
    this.homes = [];
    this.path = [];
    this.trail.replaceChildren();
    this.feedback.replaceChildren();
    this.cells.replaceChildren();
    for (let cell = 0; cell < tiles.length; cell++) {
      const element = document.createElement('div');
      element.className = 'cell';
      element.style.gridArea = area(cell, width);
      const tile = tiles[cell];
      if (tile === WALL) element.classList.add('wall');
      else {
        element.classList.add('floor');
        if (tile !== 0) {
          const goal = document.createElement('div');
          goal.className = 'goal';
          goal.style.setProperty('--piece-hue', String(hue(tile)));
          const mark = document.createElement('span');
          mark.textContent = tile === PLAIN ? '·' : String.fromCharCode(tile).toLowerCase();
          goal.append(mark);
          element.append(goal);
        }
      }
      this.cells.append(element);
    }
    this.crates = [];
    this.pieces.replaceChildren();
    for (const label of labels) {
      const crate = document.createElement('div');
      crate.className = 'crate';
      crate.style.setProperty('--piece-hue', String(hue(label)));
      const mark = document.createElement('span');
      mark.textContent = label === PLAIN ? '•' : String.fromCharCode(label);
      crate.append(mark);
      this.crates.push(crate);
      this.pieces.append(crate);
    }
    this.pieces.append(this.robot);
  }

  /** Sizes the board's tiles to their container: at most MAX_TILE_PX, fitting
   * the wrap's width and MAX_BOARD_HEIGHT_PX, and never below MIN_TILE_PX — an
   * oversized board keeps that size and scrolls, and touch then pans the wrap
   * instead of swiping the robot. */
  private size(width: number, height: number) {
    const available = this.wrap.clientWidth - parseFloat(this.wrapStyle.paddingLeft) - parseFloat(this.wrapStyle.paddingRight) - 1;
    const fit = Math.min(MAX_TILE_PX, available / width, MAX_BOARD_HEIGHT_PX / height);
    const tile = Math.max(MIN_TILE_PX, fit);
    this.fits = fit >= MIN_TILE_PX;
    this.wrap.classList.toggle('pan', !this.fits);
    this.columns = width;
    this.rows = height;
    const style = this.element.style;
    style.setProperty('--columns', String(width));
    style.setProperty('--rows', String(height));
    style.setProperty('--tile', `${tile}px`);
  }
}
