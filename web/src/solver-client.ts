import { MAX_ROUTE, PAST_LIMIT_MESSAGE, errorMessage, type SearchUpdate, type SolveRequest, type WorkerRequest } from './protocol.ts';
import { browserScheduler, type Scheduler } from './scheduler.ts';
import { decodeNativeReply, decodeWorkerReply, encodeSolveRequest, errorText, nativeErrorText, unboundFetch } from './transport.ts';

/** How often a running search's elapsed time is redrawn between its replies. */
const ELAPSED_TICK_MS = 150;
/** How long past its time budget a worker may stay silent before it is written off. The worker
 * enforces the budget itself; the grace covers building and posting its final route. */
const WORKER_GRACE_MS = 2000;
/** How long a cancelled worker has to post its final reply before it is terminated. */
const CANCEL_GRACE_MS = 1000;
/** How long past its time budget the page waits for a native search to answer.
 * Part of the solve timeout chain; see TIME_MS in crates/server/src/solve.rs.
 * The 5 s margin leaves the server time to answer after its own deadline. */
const NATIVE_GRACE_MS = 5000;

/** The part of a Worker that SolverClient uses, so tests can pass a fake. */
export interface WorkerPort {
  onmessage: ((event: MessageEvent<unknown>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  postMessage(message: WorkerRequest): void;
  terminate(): void;
}
/** Where a search runs: the WASM worker in this tab or the server's native
 * solver. The values of the #engine select. */
export const ENGINES = ['browser', 'native'] as const;
export type Engine = (typeof ENGINES)[number];
interface Result {
  prefix: string;
  route?: string;
}
interface Running extends Result {
  timer: number;
  watchdog: number;
}
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
/** Runs one search at a time on either engine, replay-checks every route it reports through
 * `verify` before showing it, and keeps the last verified route after the search ends. */
export class SolverClient {
  state: SolverState = { kind: 'idle' };
  private options: Options;
  private clock: Scheduler;
  private request: typeof fetch;
  constructor(options: Options) {
    this.options = options;
    this.clock = options.scheduler ?? browserScheduler;
    this.request = unboundFetch(options.fetch);
  }
  /** Whether a search is running. */
  get busy() {
    return this.state.kind === 'browser-running' || this.state.kind === 'native-running';
  }
  /** The last verified route, which starts where `prefix` leaves the puzzle. */
  get route() {
    return this.state.kind === 'idle' ? undefined : this.state.route;
  }
  /** The moves played before the search started; '' when idle. */
  get prefix() {
    return this.state.kind === 'idle' ? '' : this.state.prefix;
  }
  private release(active: Active) {
    this.clock.clearInterval(active.timer);
    this.clock.clearTimeout(active.watchdog);
    if (active.kind === 'browser-running') {
      active.worker.onmessage = null;
      active.worker.onerror = null;
      active.worker.terminate();
    } else active.abort.abort();
  }
  /** Ends any running search without a status and forgets its route. */
  reset() {
    if (this.state.kind === 'browser-running' || this.state.kind === 'native-running') this.release(this.state);
    this.state = { kind: 'idle' };
    this.options.changed();
  }
  /** Forgets a completed search's route but keeps its prefix, once the position has moved on from
   * where the route starts (its playback ended or was blocked). A running search is unaffected. */
  dropRoute() {
    if (this.state.kind === 'completed') this.state = { kind: 'completed', prefix: this.state.prefix };
    this.options.changed();
  }
  /** Redraws the elapsed time since `started` while `active` is the running search. */
  private tick(active: Active, started: number): number {
    return this.clock.interval(() => {
      if (this.state === active) this.options.elapsed(this.clock.now() - started);
    }, ELAPSED_TICK_MS);
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
      // The worker and the server both withhold such a route; a faulty one still never reaches verify.
      if (active.prefix.length + update.route.length > MAX_ROUTE) throw new Error(PAST_LIMIT_MESSAGE);
      this.options.verify(active.prefix, update.route);
      active.route = update.route;
    }
    if (update.metrics.status === 'solved' && active.route === undefined) throw new Error('Solved reply has no verified route');
    if (update.metrics.proof.kind === 'unsolvable' && active.route !== undefined)
      throw new Error('Unsolvable reply conflicts with verified route');
    if (active.route !== undefined && update.metrics.lowerBound !== undefined && update.metrics.lowerBound > active.route.length)
      throw new Error('Solver bound exceeds verified route');
    // Progress routes are throttled, but the final reply resends any better route,
    // so a final proof must certify exactly the route that will be displayed.
    const proof = update.metrics.proof.kind;
    if (update.type === 'done' && (proof === 'optimal' || proof === 'bounded') && update.metrics.best !== active.route?.length)
      throw new Error('Final proof does not match the verified route length');
    this.options.update(update, active.route);
    if (update.type === 'done') {
      this.finish(active);
      this.options.elapsed(update.elapsedMs);
    } else this.options.changed();
  }
  /** Starts a search unless one is running. A browser search reports through the callbacks; the
   * promise resolves once a native reply has been handled. */
  async solve(engine: Engine, request: SolveRequest): Promise<void> {
    if (this.busy) return;
    this.reset();
    const started = this.clock.now();
    // Worker construction can throw before any transport exists. Keep that
    // failure in a completed state and release every subsequently created timer.
    if (engine === 'browser') {
      let worker: WorkerPort;
      try {
        worker = this.options.worker();
      } catch (error) {
        this.state = { kind: 'completed', prefix: request.actions };
        this.options.status(errorMessage(error));
        this.options.changed();
        return;
      }
      const active: Active = { kind: 'browser-running', prefix: request.actions, worker, timer: 0, watchdog: 0 };
      this.state = active;
      try {
        active.timer = this.tick(active, started);
        active.watchdog = this.clock.timeout(() => {
          if (this.state !== active) return;
          this.options.status(active.route !== undefined ? 'Stopped at deadline. Verified route retained.' : 'Worker deadline reached.');
          this.finish(active);
        }, request.timeMs + WORKER_GRACE_MS);
        worker.onmessage = ({ data }) => {
          if (this.state !== active) return;
          try {
            const reply = decodeWorkerReply(data);
            if (reply.type === 'error') this.fail(active, reply.message);
            else this.accept(active, reply);
          } catch (error) {
            this.fail(active, error);
          }
        };
        worker.onerror = event => this.fail(active, event.message || 'Worker failed to start');
        this.options.changed();
        worker.postMessage({ type: 'solve', request });
      } catch (error) {
        this.fail(active, error);
      }
      return;
    }
    const abort = new AbortController();
    const active: Active = { kind: 'native-running', prefix: request.actions, abort, timer: 0, watchdog: 0 };
    this.state = active;
    try {
      active.timer = this.tick(active, started);
      active.watchdog = this.clock.timeout(() => {
        this.fail(active, `Server search exceeded its ${request.timeMs / 1000} s budget.`);
      }, request.timeMs + NATIVE_GRACE_MS);
      this.options.changed();
      const response = await this.request('/api/solve', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        signal: abort.signal,
        body: encodeSolveRequest(request),
      });
      if (this.state !== active) return;
      if (!response.ok) throw new Error(nativeErrorText(response.status, await errorText(response)));
      const value: unknown = await response.json();
      if (this.state === active) this.accept(active, decodeNativeReply(value));
    } catch (error) {
      this.fail(active, error);
    }
  }
  /** Asks a browser search to stop and report its best, writing it off after CANCEL_GRACE_MS;
   * stops waiting for a native search at once. */
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
        }, CANCEL_GRACE_MS);
      } catch (error) {
        this.fail(active, error);
      }
    } else if (active.kind === 'native-running') {
      this.reset();
      this.options.status('Stopped waiting for the native search.');
    }
  }
}
