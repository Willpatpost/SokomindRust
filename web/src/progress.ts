import * as storage from './storage.ts';
import { browserScheduler, type Scheduler } from './scheduler.ts';

interface Context {
  id: string;
  rows: string;
}
interface Score {
  moves: number;
  pushes: number;
}
interface Options {
  /** Replays `route` on `rows` and returns its counts; throws unless it solves the puzzle. */
  verify(rows: string, route: string): Score;
  show(best: storage.Best | null): void;
  status(message: string): void;
  scheduler?: Scheduler;
}
/** How long a delayed session save waits for another move: each move restarts the wait, so a run
 * of moves writes browser storage once. */
const SAVE_DELAY_MS = 350;
/** Keeps the selected puzzle's session and verified best in browser storage. */
export class Progress {
  private context: Context | undefined;
  /** The best last passed to `show`, which is always the current context's. */
  private displayed: storage.Best | null = null;
  private options: Options;
  private clock: Scheduler;
  private saveTimer: number | undefined;
  constructor(options: Options) {
    this.options = options;
    this.clock = options.scheduler ?? browserScheduler;
  }
  /** Switches to puzzle `id` on `rows`: cancels a pending session save and shows the stored best. */
  select(id: string, rows: string) {
    if (this.saveTimer !== undefined) this.clock.clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
    this.context = { id, rows };
    this.display(this.best());
  }
  /** Saves `actions` as the session's moves; `delayed` waits SAVE_DELAY_MS for another move,
   * whose save replaces this one. */
  session(actions: string, delayed = false) {
    if (!this.context) return;
    if (this.saveTimer !== undefined) this.clock.clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
    const context = this.context;
    const save = () => {
      if (this.context !== context) return;
      this.saveTimer = undefined;
      if (!storage.write('session', { ...context, actions }))
        this.options.status('Browser storage is unavailable. This session has not been saved.');
    };
    if (delayed) this.saveTimer = this.clock.timeout(save, SAVE_DELAY_MS);
    else save();
  }
  /** The selected puzzle's stored best, or null unless it replays to its recorded counts. */
  best(): storage.Best | null {
    if (!this.context) return null;
    const best = storage.best(this.context.id, this.context.rows);
    if (!best) return null;
    try {
      const score = this.options.verify(this.context.rows, best.route);
      return score.moves === best.moves && score.pushes === best.pushes ? best : null;
    } catch {
      return null;
    }
  }
  /** Stores a solving route only when it is strictly better (fewer moves, then fewer pushes)
   * than the verified stored best, and shows whichever best remains. */
  keep(fullRoute: string, moves: number, pushes: number) {
    if (!this.context) return;
    // Only a strictly better route replaces the best, so a tie with the best on show
    // (Replay best, or redoing the final move) returns before re-reading and replaying it.
    // A better best another tab saved meanwhile appears on the next select instead.
    if (this.displayed?.moves === moves && this.displayed.pushes === pushes) return;
    const old = this.best();
    let best = old;
    if (!old || moves < old.moves || (moves === old.moves && pushes < old.pushes)) {
      const next = { rows: this.context.rows, route: fullRoute, moves, pushes };
      if (storage.write('best.' + this.context.id, next)) best = next;
      else this.options.status('Could not save the best route in this browser.');
    }
    this.display(best);
  }
  private display(best: storage.Best | null) {
    this.displayed = best;
    this.options.show(best);
  }
}
