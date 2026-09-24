export class BoardView {
  private ctx: CanvasRenderingContext2D;
  constructor(private canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext('2d', { alpha: false });
    if (!ctx) throw new Error('Canvas is unavailable');
    this.ctx = ctx;
  }
  draw(width: number, height: number, tiles: Uint8Array, labels: Uint8Array, state: Uint32Array) {
    const available = this.canvas.parentElement!.clientWidth - 38;
    const tile = Math.max(8, Math.min(48, available / width, 540 / height));
    const ratio = Math.min(devicePixelRatio || 1, 2);
    this.canvas.width = Math.round(width * tile * ratio);
    this.canvas.height = Math.round(height * tile * ratio);
    this.canvas.style.width = `${width * tile}px`;
    this.canvas.style.height = `${height * tile}px`;
    const c = this.ctx;
    c.setTransform(ratio, 0, 0, ratio, 0, 0);
    c.fillStyle = '#101b21'; c.fillRect(0, 0, width * tile, height * tile);
    const xy = (cell: number) => [(cell % width) * tile, Math.floor(cell / width) * tile];
    for (let cell = 0; cell < tiles.length; cell++) {
      const [x, y] = xy(cell);
      c.fillStyle = tiles[cell] === 255 ? '#344c59' : '#1c2e37';
      c.fillRect(x + 1, y + 1, tile - 2, tile - 2);
      if (tiles[cell] !== 0 && tiles[cell] !== 255) {
        c.strokeStyle = '#8dcdaa'; c.lineWidth = Math.max(1, tile / 25);
        c.beginPath(); c.arc(x + tile / 2, y + tile / 2, tile * .27, 0, Math.PI * 2); c.stroke();
        c.fillStyle = '#a7dbbc'; c.textAlign = 'center'; c.textBaseline = 'middle'; c.font = `600 ${tile * .36}px system-ui`;
        c.fillText(tiles[cell] === 88 ? '·' : String.fromCharCode(tiles[cell]).toLowerCase(), x + tile / 2, y + tile / 2);
      }
    }
    for (let i = 0; i < labels.length; i++) {
      const cell = state[i + 4]; const [x, y] = xy(cell);
      c.fillStyle = tiles[cell] === labels[i] ? '#86cfa4' : '#ddb87c';
      c.fillRect(x + tile * .13, y + tile * .13, tile * .74, tile * .74);
      c.strokeStyle = '#15282b'; c.lineWidth = 1; c.strokeRect(x + tile * .21, y + tile * .21, tile * .58, tile * .58);
      c.fillStyle = '#273335'; c.textAlign = 'center'; c.textBaseline = 'middle'; c.font = `750 ${tile * .39}px system-ui`;
      c.fillText(labels[i] === 88 ? '•' : String.fromCharCode(labels[i]), x + tile / 2, y + tile / 2);
    }
    const [x, y] = xy(state[0]);
    c.fillStyle = '#b9daf2'; c.beginPath(); c.arc(x + tile / 2, y + tile / 2, tile * .29, 0, Math.PI * 2); c.fill();
    c.fillStyle = '#274252'; c.beginPath(); c.arc(x + tile * .6, y + tile * .44, tile * .06, 0, Math.PI * 2); c.fill();
    this.canvas.setAttribute('aria-label', `${width} by ${height} puzzle, ${labels.length} boxes. ${state[1]} moves, ${state[2]} pushes.${state[3] ? ' Solved.' : ''} Use arrow keys or WASD.`);
  }
}
