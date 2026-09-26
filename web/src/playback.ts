import { browserScheduler, type Scheduler } from './solver-client.ts';
export type PlaybackState =
  | { kind: 'idle' }
  | { kind: 'playing'; route: string; index: number; timer: number }
  | { kind: 'paused'; route: string; index: number };
export class Playback {
  state: PlaybackState = { kind: 'idle' };
  private clock: Scheduler;
  private step: (direction: number) => boolean;
  private changed: () => void;
  private ended: (blocked: boolean) => void;
  constructor(step: (direction: number) => boolean, changed: () => void,
    ended: (blocked: boolean) => void, clock: Scheduler = browserScheduler) {
    this.step = step; this.changed = changed; this.ended = ended; this.clock = clock;
  }
  get active() { return this.state.kind !== 'idle'; }
  stop() {
    if (this.state.kind === 'playing') this.clock.clearInterval(this.state.timer);
    this.state = { kind: 'idle' };
  }
  end(blocked = false) { this.stop(); this.ended(blocked); }
  play(route: string, index = 0) {
    this.stop();
    const state = { kind: 'playing' as const, route, index, timer: 0 };
    this.state = state;
    state.timer = this.clock.interval(() => {
      if (this.state !== state) return;
      if (state.index >= state.route.length) { this.end(); return; }
      if (!this.step('UDLR'.indexOf(state.route[state.index++]))) { this.end(true); return; }
      this.changed();
    }, 70);
  }
  toggle(route?: string) {
    const state = this.state;
    if (state.kind === 'playing') {
      this.clock.clearInterval(state.timer);
      this.state = { kind: 'paused', route: state.route, index: state.index };
    } else if (state.kind === 'paused') this.play(state.route, state.index);
    else if (route !== undefined) this.play(route);
  }
}
