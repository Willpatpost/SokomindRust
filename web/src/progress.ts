import * as storage from './storage.ts';
import { decodeHealth, decodeProgress, decodeSaveReply, errorText, unboundFetch } from './transport.ts';
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
  verify(rows: string, route: string): Score;
  show(best: storage.Best | null): void;
  status(message: string): void;
  /** Called with `persistence` whenever /api/health answers. */
  connected?(persistence: boolean): void;
  fetch?: typeof fetch;
  scheduler?: Scheduler;
  profile?: string | null;
}
/** The id of a puzzle loaded from pasted rows. The server stores progress only for catalog
 * puzzles, so a custom one is never synced. */
export const CUSTOM_PUZZLE_ID = 'custom';
/** Health re-probe backoff: the first retry waits 2 s, and each further miss
 * doubles the wait up to 5 minutes. */
const PROBE_FIRST_MS = 2000,
  PROBE_MAX_MS = 300_000;
/** How long a health probe waits for /api/health. Part of the solve timeout chain; see TIME_MS in
 * crates/server/src/solve.rs. 1.5 s outlasts the server's 900 ms HEALTH_TIMEOUT, which answers
 * even when the database hangs. */
const HEALTH_TIMEOUT_MS = 1500;
/** How long a delayed session save waits for another move: each move restarts the wait, so a run
 * of moves writes browser storage once. */
const SAVE_DELAY_MS = 350;
/** How long a progress read or save waits for the server before it is abandoned. */
const REQUEST_TIMEOUT_MS = 5000;
/** Keeps the selected puzzle's session and verified best in browser storage and, while
 * /api/health reports persistence, syncs solving routes with the server. */
export class ProgressClient {
  /** Whether /api/health last reported PostgreSQL; set by probe(). */
  persistence = false;
  private context: Context | undefined;
  /** The best last passed to `show`, which is always the current context's. */
  private displayed: storage.Best | null = null;
  /** By puzzle id, the last route the server has answered for: it replied to a save of
   * that route, or returned it as the stored best, so posting it again changes nothing. */
  private acknowledged = new Map<string, string>();
  private options: Options;
  private profile: string | null;
  private request: typeof fetch;
  private clock: Scheduler;
  private saveTimer: number | undefined;
  private probeTimer: number | undefined;
  private probeDelay = PROBE_FIRST_MS;
  private probing: Promise<void> | undefined;
  constructor(options: Options) {
    this.options = options;
    this.profile = options.profile === undefined ? storage.profile() : options.profile;
    this.request = unboundFetch(options.fetch);
    this.clock = options.scheduler ?? browserScheduler;
  }
  /** Asks /api/health whether saves reach PostgreSQL and pulls the selected
   * puzzle's server best when persistence turns on. Until an answer says yes, a
   * busy, restarting or database-less server is asked again with backoff instead
   * of being written off for the session; a host without the API (a 404, or an
   * app page instead of JSON) is not. Concurrent calls share one request. */
  probe(): Promise<void> {
    return (this.probing ??= this.checkHealth().finally(() => {
      this.probing = undefined;
    }));
  }
  private async checkHealth() {
    if (this.probeTimer !== undefined) this.clock.clearTimeout(this.probeTimer);
    this.probeTimer = undefined;
    const was = this.persistence;
    let retry = true;
    try {
      const response = await this.request('/api/health', { signal: AbortSignal.timeout(HEALTH_TIMEOUT_MS) });
      const persistence = response.ok ? decodeHealth(await response.json().catch(() => null)) : undefined;
      if (persistence !== undefined) {
        this.persistence = persistence;
        this.options.connected?.(persistence);
      } else if (response.ok || response.status === 404) retry = false;
    } catch {
      /* Offline or timed out: ask again later. */
    }
    if (this.persistence) {
      this.probeDelay = PROBE_FIRST_MS;
      if (!was) void this.pull();
      return;
    }
    if (!retry) return;
    const delay = this.probeDelay;
    this.probeDelay = Math.min(2 * delay, PROBE_MAX_MS);
    this.probeTimer = this.clock.timeout(() => {
      this.probeTimer = undefined;
      void this.probe();
    }, delay);
  }
  /** Switches to puzzle `id` on `rows`: cancels a pending session save, shows the stored best
   * and pulls the server's. */
  select(id: string, rows: string) {
    if (this.saveTimer !== undefined) this.clock.clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
    this.context = { id, rows };
    this.display(this.best());
    void this.pull();
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
  private remote(context: Context | undefined): context is Context {
    return !!context && this.persistence && !!this.profile && context.id !== CUSTOM_PUZZLE_ID;
  }
  /** Posts a solving route unless the server has already answered for exactly this one,
   * as when Replay best or redoing the final move solves the puzzle again. */
  async sync(fullRoute: string) {
    const context = this.context;
    if (!this.remote(context) || this.acknowledged.get(context.id) === fullRoute) return;
    const failed = 'Server save failed. Local progress is still available if browser storage is enabled.';
    try {
      const response = await this.request(`/api/progress/${encodeURIComponent(context.id)}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'x-profile-id': this.profile! },
        body: JSON.stringify({ route: fullRoute }),
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      });
      if (this.context !== context) return;
      if (!response.ok) {
        const error = await errorText(response);
        if (this.context !== context) return;
        this.options.status(
          response.status === 429
            ? 'Server save rate-limited; try again shortly. Local progress is kept.'
            : response.status < 500
              ? `Server save rejected: ${error ?? `HTTP ${response.status}`}`
              : failed,
        );
        return;
      }
      const improved = decodeSaveReply(await response.json());
      if (this.context !== context) return;
      this.acknowledged.set(context.id, fullRoute);
      this.options.status(
        improved
          ? 'Verified best route saved in PostgreSQL for this browser profile.'
          : 'Server already stored an equal or better route for this puzzle.',
      );
    } catch {
      if (this.context === context) this.options.status(failed);
    }
  }
  /** Fetches the server's stored best and keeps it if it replays to the counts the server sent. */
  async pull() {
    const context = this.context;
    if (!this.remote(context)) return;
    try {
      const response = await this.request(`/api/progress/${encodeURIComponent(context.id)}`, {
        headers: { 'x-profile-id': this.profile! },
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      });
      if (!response.ok) return;
      const stored = decodeProgress(await response.json());
      if (this.context !== context || stored.puzzleId !== context.id) return;
      const checked = this.options.verify(context.rows, stored.route);
      if (stored.moves !== checked.moves || stored.pushes !== checked.pushes) return;
      this.acknowledged.set(context.id, stored.route);
      this.keep(stored.route, checked.moves, checked.pushes);
    } catch {
      /* Optional persistence must not interrupt play. */
    }
  }
}
