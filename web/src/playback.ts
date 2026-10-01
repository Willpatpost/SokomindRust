import { ACTIONS } from './protocol.ts';
import { browserScheduler, type Scheduler } from './scheduler.ts';
/** The time between two replayed moves. */
const STEP_INTERVAL_MS = 70;
export type PlaybackState =
  | { kind: 'idle' }
  | { kind: 'playing'; route: string; index: number; timer: number }
  | { kind: 'paused'; route: string; index: number };
/** Replays a route through `step`, one move per STEP_INTERVAL_MS, with pause and resume. */
export class Playback {
  state: PlaybackState = { kind: 'idle' };
  private clock: Scheduler;
  private step: (direction: number) => boolean;
  private changed: () => void;
  private ended: (blocked: boolean) => void;
  constructor(
    step: (direction: number) => boolean,
    changed: () => void,
    ended: (blocked: boolean) => void,
    clock: Scheduler = browserScheduler,
  ) {
    this.step = step;
    this.changed = changed;
    this.ended = ended;
    this.clock = clock;
  }
  /** Whether a route is playing or paused. */
  get active() {
    return this.state.kind !== 'idle';
  }
  /** Stops without reporting to `ended`. */
  stop() {
    if (this.state.kind === 'playing') this.clock.clearInterval(this.state.timer);
    this.state = { kind: 'idle' };
  }
  /** Stops and reports the end to `ended`; `blocked` when `step` refused a move. */
  end(blocked = false) {
    this.stop();
    this.ended(blocked);
  }
  /** Plays `route` from `index`, replacing any playback: `changed` follows each move, and `ended`
   * the route's end or a refused move. */
  play(route: string, index = 0) {
    this.stop();
    const state = { kind: 'playing' as const, route, index, timer: 0 };
    this.state = state;
    state.timer = this.clock.interval(() => {
      if (this.state !== state) return;
      if (state.index >= state.route.length) {
        this.end();
        return;
      }
      if (!this.step(ACTIONS.indexOf(state.route[state.index++]))) {
        this.end(true);
        return;
      }
      this.changed();
    }, STEP_INTERVAL_MS);
  }
  /** Pauses, resumes, or else starts `route` when one is given. */
  toggle(route?: string) {
    const state = this.state;
    if (state.kind === 'playing') {
      this.clock.clearInterval(state.timer);
      this.state = { kind: 'paused', route: state.route, index: state.index };
    } else if (state.kind === 'paused') this.play(state.route, state.index);
    else if (route !== undefined) this.play(route);
  }
}
