import { MAX_ROUTE, errorMessage, type SearchUpdate, type SolveRequest, type WorkerRequest } from './protocol.ts';
import { decodeNativeReply, decodeWorkerReply, errorText } from './transport.ts';

export interface WorkerPort {
  onmessage: ((event: MessageEvent<unknown>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  postMessage(message: WorkerRequest): void;
  terminate(): void;
}
export interface Scheduler {
  now(): number;
  interval(callback: () => void, ms: number): number;
  timeout(callback: () => void, ms: number): number;
  clearInterval(handle: number): void;
  clearTimeout(handle: number): void;
}
export const browserScheduler: Scheduler = {
  now: () => performance.now(), interval: (fn, ms) => window.setInterval(fn, ms),
  timeout: (fn, ms) => window.setTimeout(fn, ms),
  clearInterval: handle => window.clearInterval(handle), clearTimeout: handle => window.clearTimeout(handle),
};
interface Result { prefix: string; route?: string }
interface Running extends Result { timer: number; watchdog: number }
export type SolverState =
  | { kind: 'idle' }
  | (Running & { kind: 'browser-running'; worker: WorkerPort })
  | (Running & { kind: 'native-running'; abort: AbortController })
  | (Result & { kind: 'completed' });
type Active = Extract<SolverState, { kind: 'browser-running' | 'native-running' }>;
interface Options {
  worker(): WorkerPort;
  fetch?: typeof fetch;
  scheduler?: Scheduler;
  changed(): void;
  elapsed(ms: number): void;
  update(update: SearchUpdate, route: string | undefined): void;
  status(text: string): void;
  verify(prefix: string, route: string): void;
}
export class SolverClient {
  state: SolverState = { kind: 'idle' };
  private options: Options;
  private clock: Scheduler;
  private request: typeof fetch;
  constructor(options: Options) {
    this.options = options;
    this.clock = options.scheduler ?? browserScheduler;
    this.request = options.fetch ?? fetch;
  }
  get busy() { return this.state.kind === 'browser-running' || this.state.kind === 'native-running'; }
  get route() { return this.state.kind === 'idle' ? undefined : this.state.route; }
  get prefix() { return this.state.kind === 'idle' ? '' : this.state.prefix; }
  private release(active: Active) {
    this.clock.clearInterval(active.timer);
    this.clock.clearTimeout(active.watchdog);
    if (active.kind === 'browser-running') {
      active.worker.onmessage = null;
      active.worker.onerror = null;
      active.worker.terminate();
    } else active.abort.abort();
  }
  reset() {
    if (this.state.kind === 'browser-running' || this.state.kind === 'native-running') this.release(this.state);
    this.state = { kind: 'idle' };
    this.options.changed();
  }
  dropRoute() {
    if (this.state.kind === 'completed') this.state = { kind: 'completed', prefix: this.state.prefix };
    this.options.changed();
  }
  private finish(active: Active) {
    if (this.state !== active) return;
    this.release(active);
    this.state = { kind: 'completed', prefix: active.prefix, route: active.route };
    this.options.changed();
  }
  private fail(active: Active, error: unknown) {
    if (this.state !== active) return;
    this.options.status(errorMessage(error));
    this.finish(active);
  }
  private accept(active: Active, update: SearchUpdate) {
    if (this.state !== active) return;
    if (update.route !== undefined) {
      if (active.prefix.length + update.route.length > MAX_ROUTE)
        throw new Error(`Position and route together exceed the ${MAX_ROUTE}-move replay limit`);
      this.options.verify(active.prefix, update.route);
      active.route = update.route;
    }
    if (update.metrics.status === 'solved' && active.route === undefined) throw new Error('Solved reply has no verified route');
    if (update.metrics.proof.kind === 'unsolvable' && active.route !== undefined) throw new Error('Unsolvable reply conflicts with verified route');
    if (active.route !== undefined && update.metrics.lowerBound !== undefined && update.metrics.lowerBound > active.route.length)
      throw new Error('Solver bound exceeds verified route');
    this.options.update(update, active.route);
    if (update.type === 'done') { this.finish(active); this.options.elapsed(update.elapsedMs); }
    else this.options.changed();
  }
  async solve(engine: string, request: SolveRequest): Promise<void> {
    if (this.busy) return;
    this.reset();
    const started = this.clock.now();
    // Worker construction can throw before any transport exists. Keep that
    // failure in a completed state and release every subsequently created timer.
    if (engine === 'browser') {
      let worker: WorkerPort;
      try { worker = this.options.worker(); }
      catch (error) {
        this.state = { kind: 'completed', prefix: request.actions };
        this.options.status(errorMessage(error)); this.options.changed(); return;
      }
      const active: Active = { kind: 'browser-running', prefix: request.actions, worker, timer: 0, watchdog: 0 };
      this.state = active;
      try {
        active.timer = this.clock.interval(() => {
          if (this.state === active) this.options.elapsed(this.clock.now() - started);
        }, 150);
        active.watchdog = this.clock.timeout(() => {
          if (this.state !== active) return;
          this.options.status(active.route !== undefined ? 'Stopped at deadline. Verified route retained.' : 'Worker deadline reached.');
          this.finish(active);
        }, request.timeMs + 2000);
        worker.onmessage = ({ data }) => {
          if (this.state !== active) return;
          try {
            const reply = decodeWorkerReply(data);
            if (reply.type === 'error') this.fail(active, reply.message);
            else this.accept(active, reply);
          } catch (error) { this.fail(active, error); }
        };
        worker.onerror = event => this.fail(active, event.message || 'Worker failed to start');
        this.options.changed();
        worker.postMessage({ type: 'solve', request });
      } catch (error) { this.fail(active, error); }
      return;
    }
    const abort = new AbortController();
    const active: Active = { kind: 'native-running', prefix: request.actions, abort, timer: 0, watchdog: 0 };
    this.state = active;
    try {
      active.timer = this.clock.interval(() => {
        if (this.state === active) this.options.elapsed(this.clock.now() - started);
      }, 150);
      active.watchdog = this.clock.timeout(() => {
        this.fail(active, `Server search exceeded its ${request.timeMs / 1000} s budget.`);
      }, request.timeMs + 5000);
      this.options.changed();
      const response = await this.request('/api/solve', {
        method: 'POST', headers: { 'content-type': 'application/json' }, signal: abort.signal,
        body: JSON.stringify({ rows: request.rows.split('\n'), actions: request.actions, mode: request.mode,
          time_ms: request.timeMs, max_states: request.maxStates, memory_mib: request.memoryMiB }),
      });
      if (this.state !== active) return;
      if (!response.ok) {
        const error = (await errorText(response)) ?? `Server returned HTTP ${response.status}`;
        throw new Error(response.status === 429 && !/browser/i.test(error)
          ? `${error}. The browser solver still works: set Run on to This browser.` : error);
      }
      const value: unknown = await response.json();
      if (this.state === active) this.accept(active, decodeNativeReply(value));
    } catch (error) { this.fail(active, error); }
  }
  cancel() {
    const active = this.state;
    if (active.kind === 'browser-running') {
      try {
        active.worker.postMessage({ type: 'cancel' });
        if (this.state !== active) return;
        this.clock.clearTimeout(active.watchdog);
        active.watchdog = this.clock.timeout(() => {
          if (this.state !== active) return;
          this.options.status(active.route !== undefined ? 'Stopped. Verified route retained.' : 'Stopped.');
          this.finish(active);
        }, 1000);
      } catch (error) { this.fail(active, error); }
    } else if (active.kind === 'native-running') {
      this.reset();
      this.options.status('Stopped waiting for the native search.');
    }
  }
}
