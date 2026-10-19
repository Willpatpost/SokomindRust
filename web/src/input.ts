/** What player input drives. Directions index ACTIONS in protocol.ts ('UDLR'): 0 up, 1 down, 2 left, 3 right. */
export interface InputHandlers {
  move(direction: number): void;
  /** The Z key. */
  undo(): void;
  /** Whether the whole board fits unscrolled and unzoomed, so a swipe on it may move the robot. */
  swipeable(): boolean;
  /** A still touch or click on the board, at client coordinates and whether it
   * came from touch: the robot walks toward it. */
  tap(x: number, y: number, touch: boolean): void;
}
/** How far a swipe must travel along its main axis, in CSS pixels. */
const SWIPE_MIN_PX = 24;
/** The direction of a touch that moved `dx`, `dy` from where it started (screen y grows down),
 * or undefined unless it is a swipe: SWIPE_MIN_PX along one axis and at least twice as far as
 * along the other, so the axes never tie. */
export function swipeDirection(dx: number, dy: number): number | undefined {
  const ax = Math.abs(dx),
    ay = Math.abs(dy);
  if (Math.max(ax, ay) < SWIPE_MIN_PX || Math.max(ax, ay) < 2 * Math.min(ax, ay)) return undefined;
  return ax > ay ? (dx > 0 ? 3 : 2) : dy > 0 ? 1 : 0;
}
/** Keyboard keys by their lowercased `key`: arrows and WASD move, Z undoes. */
const KEYS = new Map<string, number | 'undo'>([
  ['arrowup', 0],
  ['w', 0],
  ['arrowdown', 1],
  ['s', 1],
  ['arrowleft', 2],
  ['a', 2],
  ['arrowright', 3],
  ['d', 3],
  ['z', 'undo'],
]);
/** What a keydown's `key` does, in either case: a direction, 'undo', or undefined for any other key. */
export function keyAction(key: string): number | 'undo' | undefined {
  return KEYS.get(key.toLowerCase());
}
/** Whether a keydown stays with the browser: one chorded with Ctrl, Meta or Alt (a shortcut such as
 * Ctrl+R or Cmd+W), or one typed into a form field, which `editable` reports. A Shift chord still
 * plays, since keyAction reads either case. */
export function ignoresKey({ ctrlKey, metaKey, altKey }: { ctrlKey: boolean; metaKey: boolean; altKey: boolean }, editable: boolean) {
  return ctrlKey || metaKey || altKey || editable;
}
/** Binds the on-screen direction buttons, swipes on the board and the keyboard
 * (arrow keys, WASD, Z) to the handlers. */
export function bindInput(target: HTMLElement, { move, undo, swipeable, tap }: InputHandlers) {
  for (const control of document.querySelectorAll<HTMLButtonElement>('[data-direction]'))
    control.onclick = () => move(Number(control.dataset.direction));
  // Swipe on the board moves the robot; taps walk it. Only a clean one-finger
  // gesture counts: a second finger (pinch), a cancelled touch, or any scroll
  // during the gesture discards it, and a swipe needs a board that fits and is
  // not zoomed — while either holds, a drag pans or scrolls instead.
  const scrollOffsets = () => `${scrollX},${scrollY},${target.parentElement!.scrollLeft},${target.parentElement!.scrollTop}`;
  let swipe: { id: number; x: number; y: number; scroll: string } | undefined;
  target.addEventListener(
    'touchstart',
    event => {
      const touch = event.changedTouches[0];
      swipe =
        event.touches.length === 1 ? { id: touch.identifier, x: touch.clientX, y: touch.clientY, scroll: scrollOffsets() } : undefined;
    },
    { passive: true },
  );
  // A second finger that lands off the board still makes this a pinch.
  document.addEventListener(
    'touchstart',
    event => {
      if (event.touches.length > 1) swipe = undefined;
    },
    { passive: true },
  );
  target.addEventListener(
    'touchcancel',
    () => {
      swipe = undefined;
    },
    { passive: true },
  );
  // A touch that ends acts; the click the browser synthesizes after it must not
  // act twice. The last touch starts unset, not at zero: performance.now() also
  // starts near zero, and an early page's first click would otherwise be taken
  // for a synthesized one.
  let lastTouch: number | undefined;
  target.addEventListener(
    'touchend',
    event => {
      const gesture = swipe;
      swipe = undefined;
      const touch = event.changedTouches[0];
      if (!gesture || event.touches.length !== 0 || touch.identifier !== gesture.id || scrollOffsets() !== gesture.scroll) return;
      lastTouch = performance.now();
      const dx = touch.clientX - gesture.x;
      const dy = touch.clientY - gesture.y;
      const direction = swipeDirection(dx, dy);
      if (direction !== undefined && swipeable()) move(direction);
      // A still touch is a tap; an ambiguous diagonal is neither a swipe nor one.
      else if (Math.max(Math.abs(dx), Math.abs(dy)) < SWIPE_MIN_PX) tap(touch.clientX, touch.clientY, true);
    },
    { passive: true },
  );
  target.addEventListener('click', event => {
    if (lastTouch !== undefined && performance.now() - lastTouch < 500) return;
    tap(event.clientX, event.clientY, false);
  });
  document.addEventListener('keydown', event => {
    if (ignoresKey(event, event.target instanceof Element && event.target.matches('input,textarea,select'))) return;
    const action = keyAction(event.key);
    if (action === undefined) return;
    event.preventDefault();
    if (action === 'undo') undo();
    else move(action);
  });
}
