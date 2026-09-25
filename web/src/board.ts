/** WasmGame.snapshot decoded once: robot and box cells plus the move counters. */
export interface Snapshot { player: number; moves: number; pushes: number; solved: boolean; boxes: Uint32Array }
const WALL = 255; // sokomind_core::WALL
const PLAIN = 88; // 'X': the label of unlettered boxes and their goals
export class BoardView {
  private ctx: CanvasRenderingContext2D;
  /** Live computed style of the board container. */
  private wrap: CSSStyleDeclaration;
  /** The whole board fits at 4 px tiles or larger, so it never scrolls and swipes can move the robot. */
  fits = true;
  constructor(private canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext('2d', { alpha: false });
    if (!ctx) throw new Error('Canvas is unavailable');
    this.ctx = ctx;
    this.wrap = getComputedStyle(canvas.parentElement!);
  }
  draw(
    width: number,
    height: number,
    tiles: Uint8Array,
    labels: Uint8Array,
    state: Snapshot,
    onGoal: boolean[],
  ) {
    // clientWidth includes the padding but not the border, and is rounded to
    // whole pixels: one pixel of slack keeps a fitted board from overflowing.
    const available = this.canvas.parentElement!.clientWidth
      - parseFloat(this.wrap.paddingLeft)
      - parseFloat(this.wrap.paddingRight)
      - 1;
    const fit = Math.min(48, available / width, 540 / height);
    const tile = Math.max(4, fit);
    this.fits = fit >= 4;
    // An oversized board scrolls, so touch pans it instead of swiping.
    this.canvas.classList.toggle('pan', !this.fits);
    // The backing store stays well under the 32,767 px browser limit on both
    // axes: an oversized board renders at a lower pixel ratio, never blank.
    const ratio = Math.min(devicePixelRatio || 1, 2, 14000 / (width * tile), 14000 / (height * tile));
    const pixelWidth = Math.round(width * tile * ratio);
    const pixelHeight = Math.round(height * tile * ratio);
    // Assigning a size reallocates and clears the backing store, even an unchanged one.
    if (this.canvas.width !== pixelWidth) this.canvas.width = pixelWidth;
    if (this.canvas.height !== pixelHeight) this.canvas.height = pixelHeight;
    this.canvas.style.width = `${width * tile}px`;
    this.canvas.style.height = `${height * tile}px`;
    const c = this.ctx;
    c.setTransform(ratio, 0, 0, ratio, 0, 0);
    // Cover the whole backing store: a kept one still holds the last frame.
    c.fillStyle = '#101b21';
    c.fillRect(0, 0, pixelWidth / ratio, pixelHeight / ratio);
    const xy = (cell: number) => [(cell % width) * tile, Math.floor(cell / width) * tile];
    for (let cell = 0; cell < tiles.length; cell++) {
      const [x, y] = xy(cell);
      c.fillStyle = tiles[cell] === WALL ? '#344c59' : '#1c2e37';
      c.fillRect(x + 1, y + 1, tile - 2, tile - 2);
      if (tiles[cell] !== 0 && tiles[cell] !== WALL) {
        c.strokeStyle = '#8dcdaa';
        c.lineWidth = Math.max(1, tile / 25);
        c.beginPath();
        c.arc(x + tile / 2, y + tile / 2, tile * .27, 0, Math.PI * 2);
        c.stroke();
        c.fillStyle = '#a7dbbc';
        c.textAlign = 'center';
        c.textBaseline = 'middle';
        c.font = `600 ${tile * .36}px system-ui`;
        c.fillText(
          tiles[cell] === PLAIN ? '·' : String.fromCharCode(tiles[cell]).toLowerCase(),
          x + tile / 2,
          y + tile / 2,
        );
      }
    }
    for (let i = 0; i < labels.length; i++) {
      const [x, y] = xy(state.boxes[i]);
      c.fillStyle = onGoal[i] ? '#86cfa4' : '#ddb87c';
      c.fillRect(x + tile * .13, y + tile * .13, tile * .74, tile * .74);
      c.strokeStyle = '#15282b';
      c.lineWidth = 1;
      c.strokeRect(x + tile * .21, y + tile * .21, tile * .58, tile * .58);
      c.fillStyle = '#273335';
      c.textAlign = 'center';
      c.textBaseline = 'middle';
      c.font = `750 ${tile * .39}px system-ui`;
      c.fillText(labels[i] === PLAIN ? '•' : String.fromCharCode(labels[i]), x + tile / 2, y + tile / 2);
    }
    const [x, y] = xy(state.player);
    c.fillStyle = '#b9daf2';
    c.beginPath();
    c.arc(x + tile / 2, y + tile / 2, tile * .29, 0, Math.PI * 2);
    c.fill();
    c.fillStyle = '#274252';
    c.beginPath();
    c.arc(x + tile * .6, y + tile * .44, tile * .06, 0, Math.PI * 2);
    c.fill();
    const label =
      `${width} by ${height} puzzle, ${labels.length} boxes. ${state.moves} moves, ${state.pushes} pushes.${state.solved ? ' Solved.' : ''} Use arrow keys or WASD.`;
    if (this.canvas.getAttribute('aria-label') !== label) this.canvas.setAttribute('aria-label', label);
  }
}
