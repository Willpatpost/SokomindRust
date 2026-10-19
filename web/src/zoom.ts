/** The most a pinch may enlarge the board. */
export const MAX_ZOOM = 3;

/** The next scale from a pinch's distances, held between 1 and MAX_ZOOM. */
export function pinchScale(base: number, baseDistance: number, distance: number): number {
  return Math.min(MAX_ZOOM, Math.max(1, base * (distance / baseDistance)));
}

/** A pan offset clamped so a zoomed board still covers the room it sits in: it
 * cannot move past an edge its scaled size has not yet reached. */
export function clampPan(offset: number, scaled: number, room: number): number {
  const limit = Math.max(0, (scaled - room) / 2);
  // 0 - limit, because negating a zero limit yields negative zero, which a
  // strict compare or a translate string would show.
  return Math.min(limit, Math.max(0 - limit, offset));
}

/** Owns the board's touch zoom: two fingers scale it between 1x and MAX_ZOOM
 * while their midpoint pans it, one finger pans it while zoomed, and reset()
 * restores its place — which the tap dispatcher in main.ts calls on a double
 * tap. The transform is written straight to the board element, so nothing
 * re-renders and the pieces' own animations are untouched. */
export class PinchZoom {
  /** Whether the board is enlarged past 1x: pans move it and swipes stop. */
  zoomed = false;
  private element: HTMLElement;
  private wrap: HTMLElement;
  private scale = 1;
  private x = 0;
  private y = 0;
  private pinch: { distance: number; midX: number; midY: number; scale: number; x: number; y: number } | undefined;
  private pan: { id: number; x: number; y: number } | undefined;

  constructor(element: HTMLElement) {
    this.element = element;
    this.wrap = element.parentElement!;
    element.addEventListener(
      'touchstart',
      event => {
        if (event.touches.length === 2) {
          const [a, b] = [event.touches[0]!, event.touches[1]!];
          this.pinch = {
            distance: Math.hypot(b.clientX - a.clientX, b.clientY - a.clientY),
            midX: (a.clientX + b.clientX) / 2,
            midY: (a.clientY + b.clientY) / 2,
            scale: this.scale,
            x: this.x,
            y: this.y,
          };
          this.pan = undefined;
        } else if (event.touches.length === 1 && this.zoomed) {
          this.pinch = undefined;
          const touch = event.touches[0]!;
          this.pan = { id: touch.identifier, x: touch.clientX, y: touch.clientY };
        }
      },
      { passive: true },
    );
    element.addEventListener(
      'touchmove',
      event => {
        if (this.pinch !== undefined && event.touches.length === 2) {
          event.preventDefault();
          const [a, b] = [event.touches[0]!, event.touches[1]!];
          this.scale = pinchScale(this.pinch.scale, this.pinch.distance, Math.hypot(b.clientX - a.clientX, b.clientY - a.clientY));
          this.x = this.pinch.x + (a.clientX + b.clientX) / 2 - this.pinch.midX;
          this.y = this.pinch.y + (a.clientY + b.clientY) / 2 - this.pinch.midY;
          this.apply();
        } else if (this.pan !== undefined && event.touches.length === 1 && event.touches[0]!.identifier === this.pan.id) {
          event.preventDefault();
          const touch = event.touches[0]!;
          this.x += touch.clientX - this.pan.x;
          this.y += touch.clientY - this.pan.y;
          this.pan = { id: touch.identifier, x: touch.clientX, y: touch.clientY };
          this.apply();
        }
      },
      { passive: false },
    );
    element.addEventListener(
      'touchend',
      () => {
        this.pinch = undefined;
        this.pan = undefined;
        // A pinch that came home snaps clean.
        if (this.scale <= 1) this.reset();
      },
      { passive: true },
    );
  }

  /** Restores the board to its place: no transform, the wrap scrolling again. */
  reset(): void {
    this.scale = 1;
    this.x = 0;
    this.y = 0;
    this.apply();
  }

  private apply(): void {
    this.zoomed = this.scale > 1.001;
    this.wrap.classList.toggle('zoomed', this.zoomed);
    if (!this.zoomed) {
      this.element.style.transform = '';
      return;
    }
    const style = getComputedStyle(this.wrap);
    const roomX = this.wrap.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    const roomY = this.wrap.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
    this.x = clampPan(this.x, this.element.offsetWidth * this.scale, roomX);
    this.y = clampPan(this.y, this.element.offsetHeight * this.scale, roomY);
    this.element.style.transform = `translate(${this.x}px, ${this.y}px) scale(${this.scale})`;
  }
}
