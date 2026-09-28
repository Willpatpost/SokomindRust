import * as storage from './storage.ts';
import { counter, errorText, route, unboundFetch } from './transport.ts';
import { browserScheduler, type Scheduler } from './solver-client.ts';

interface Context { id: string; rows: string }
interface Score { moves: number; pushes: number }
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
/** Health re-probe backoff: the first retry waits 2 s, and each further miss
 * doubles the wait up to 5 minutes. */
const PROBE_FIRST_MS = 2000, PROBE_MAX_MS = 300_000;
export class ProgressClient {
  /** Whether /api/health last reported PostgreSQL; set by probe(). */
  persistence = false;
  private context: Context | undefined;
  private options: Options;
  private profile: string | null;
  private request: typeof fetch;
  private clock: Scheduler;
  private saveTimer: number | undefined;
  private probeTimer: number | undefined;
  private probeDelay = PROBE_FIRST_MS;
  private probing: Promise<void> | undefined;
  constructor(options: Options) {
    this.options = options; this.profile = options.profile === undefined ? storage.profile() : options.profile;
    this.request = unboundFetch(options.fetch); this.clock = options.scheduler ?? browserScheduler;
  }
  /** Asks /api/health whether saves reach PostgreSQL and pulls the selected
   * puzzle's server best when persistence turns on. Until an answer says yes, a
   * busy, restarting or database-less server is asked again with backoff instead
   * of being written off for the session; a host without the API (a 404, or an
   * app page instead of JSON) is not. Concurrent calls share one request. */
  probe(): Promise<void> {
    return this.probing ??= this.checkHealth().finally(() => { this.probing = undefined; });
  }
  private async checkHealth() {
    if (this.probeTimer !== undefined) this.clock.clearTimeout(this.probeTimer);
    this.probeTimer = undefined;
    const was = this.persistence;
    let retry = true;
    try {
      const response = await this.request('/api/health', { signal: AbortSignal.timeout(1500) });
      const health: unknown = response.ok ? await response.json().catch(() => null) : null;
      if (health && typeof health === 'object') {
        this.persistence = (health as { persistence?: unknown }).persistence === true;
        this.options.connected?.(this.persistence);
      } else if (response.ok || response.status === 404) retry = false;
    } catch { /* Offline or timed out: ask again later. */ }
    if (this.persistence) {
      this.probeDelay = PROBE_FIRST_MS;
      if (!was) void this.pull();
      return;
    }
    if (!retry) return;
    const delay = this.probeDelay;
    this.probeDelay = Math.min(2 * delay, PROBE_MAX_MS);
    this.probeTimer = this.clock.timeout(() => { this.probeTimer = undefined; void this.probe(); }, delay);
  }
  select(id: string, rows: string) {
    if (this.saveTimer !== undefined) this.clock.clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
    this.context = { id, rows };
    this.options.show(this.best());
    void this.pull();
  }
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
    if (delayed) this.saveTimer = this.clock.timeout(save, 350);
    else save();
  }
  best(): storage.Best | null {
    if (!this.context) return null;
    const best = storage.best(this.context.id, this.context.rows);
    if (!best) return null;
    try {
      const score = this.options.verify(this.context.rows, best.route);
      return score.moves === best.moves && score.pushes === best.pushes ? best : null;
    } catch { return null; }
  }
  keep(fullRoute: string, moves: number, pushes: number) {
    if (!this.context) return;
    const old = this.best();
    let best = old;
    if (!old || moves < old.moves || (moves === old.moves && pushes < old.pushes)) {
      const next = { rows: this.context.rows, route: fullRoute, moves, pushes };
      if (storage.write('best.' + this.context.id, next)) best = next;
      else this.options.status('Could not save the best route in this browser.');
    }
    this.options.show(best);
  }
  private remote(context: Context | undefined): context is Context {
    return !!context && this.persistence && !!this.profile && context.id !== 'custom';
  }
  async sync(fullRoute: string) {
    const context = this.context;
    if (!this.remote(context)) return;
    const failed = 'Server save failed. Local progress is still available if browser storage is enabled.';
    try {
      const response = await this.request(`/api/progress/${encodeURIComponent(context.id)}`, {
        method: 'POST', headers: { 'content-type': 'application/json', 'x-profile-id': this.profile! },
        body: JSON.stringify({ route: fullRoute }), signal: AbortSignal.timeout(5000),
      });
      if (this.context !== context) return;
      if (!response.ok) {
        const error = await errorText(response);
        if (this.context !== context) return;
        this.options.status(response.status === 429 ? 'Server save rate-limited; try again shortly. Local progress is kept.'
          : response.status < 500 ? `Server save rejected: ${error ?? `HTTP ${response.status}`}` : failed);
        return;
      }
      const value: unknown = await response.json();
      if (this.context !== context) return;
      if (!value || typeof value !== 'object' || typeof (value as { improved?: unknown }).improved !== 'boolean')
        throw new Error('Invalid progress response');
      this.options.status((value as { improved: boolean }).improved
        ? 'Verified best route saved in PostgreSQL for this browser profile.'
        : 'Server already stored an equal or better route for this puzzle.');
    } catch { if (this.context === context) this.options.status(failed); }
  }
  async pull() {
    const context = this.context;
    if (!this.remote(context)) return;
    try {
      const response = await this.request(`/api/progress/${encodeURIComponent(context.id)}`, {
        headers: { 'x-profile-id': this.profile! }, signal: AbortSignal.timeout(5000),
      });
      if (!response.ok) return;
      const value: unknown = await response.json();
      if (this.context !== context || !value || typeof value !== 'object') return;
      const result = value as Record<string, unknown>, fullRoute = route(result.route);
      if (fullRoute === undefined || result.puzzle_id !== context.id) return;
      const checked = this.options.verify(context.rows, fullRoute);
      if (counter(result.moves, 'progress moves') !== checked.moves || counter(result.pushes, 'progress pushes') !== checked.pushes) return;
      this.keep(fullRoute, checked.moves, checked.pushes);
    } catch { /* Optional persistence must not interrupt play. */ }
  }
}
