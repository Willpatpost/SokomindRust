import init, { WasmSearch } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import { MAX_ROUTE, PAST_LIMIT_MESSAGE, errorMessage, type WorkerReply, type WorkerRequest } from './protocol.ts';
import { decodeMetricTuple } from './transport.ts';
/** Search time between yields: a 'cancel' message is only read while the loop is yielded. */
const SLICE_MS = 8;
/** Pops per advance() between deadline checks, as POPS_PER_CHECK in crates/server/src/solve.rs:
 * the time limit overshoots by at most this many pops, and the clock reads between batches cost
 * little beside them. */
const POPS_PER_ADVANCE = 8;
/** How often a running search posts its metrics, which keep the counters on screen moving. */
const REPORT_MS = 150;
/** The least time between two improved routes posted while the search runs. */
const ROUTE_SAMPLE_MS = 500;
// Set by 'cancel' and never cleared: SolverClient starts a fresh worker for each
// search and terminates it when the search ends, so a worker runs one search.
let cancelled = false;
const post = (message: WorkerReply) => self.postMessage(message);
// Browsers clamp nested setTimeout(0) to >= 4ms, which would idle a third
// of every time budget; a MessageChannel yield is a macrotask without clamp.
const yieldChannel = new MessageChannel();
const yieldToEventLoop = () =>
  new Promise<void>(resolve => {
    yieldChannel.port1.onmessage = () => resolve();
    yieldChannel.port2.postMessage(0);
  });
self.onmessage = async ({ data }: MessageEvent<WorkerRequest>) => {
  if (data.type === 'cancel') {
    cancelled = true;
    return;
  }
  let search: WasmSearch | undefined;
  const started = performance.now();
  try {
    await init({ module_or_path: wasmUrl });
    const r = data.request;
    search = new WasmSearch(r.rows, r.actions, r.mode, r.maxStates, r.memoryMiB);
    // The longest route the position leaves room for. A longer best is never rebuilt, since no
    // game could replay it after the position's moves; a search that ends with one fails with the
    // text POST /api/solve sends for it, and a Quality run may still improve below the limit.
    const routeLimit = MAX_ROUTE - r.actions.length;
    let reportedBest: number | undefined;
    let lastReport = 0;
    let lastRoute = 0;
    let running = true;
    while (true) {
      const slice = performance.now();
      while (running && performance.now() - slice < SLICE_MS) {
        if (cancelled) {
          search.cancel();
          running = false;
        } else if (performance.now() - started >= r.timeMs) {
          search.stop_time_limit();
          running = false;
        } else running = search.advance(POPS_PER_ADVANCE);
      }
      const elapsedMs = performance.now() - started;
      // Only a finished search needs its status string from Rust.
      const metrics = decodeMetricTuple(search.metrics(), running ? 'running' : search.status());
      const best = metrics.best;
      if (!running && best !== undefined && best > routeLimit) throw new Error(PAST_LIMIT_MESSAGE);
      // Rebuilding a route walks every push, so a stream of Quality improvements is
      // sampled every ROUTE_SAMPLE_MS; the first route and the final best always go out.
      const improved =
        best !== undefined
        && best <= routeLimit
        && (reportedBest === undefined || (best < reportedBest && (!running || elapsedMs - lastRoute >= ROUTE_SAMPLE_MS)));
      if (improved || !running || elapsedMs - lastReport >= REPORT_MS) {
        // Serialize only tiny telemetry and a newly improved, Rust-verified route.
        const route = improved ? search.solution() : undefined;
        if (improved) lastRoute = elapsedMs;
        if (route !== undefined) reportedBest = best;
        post({ type: running ? 'progress' : 'done', metrics, elapsedMs, route });
        lastReport = elapsedMs;
      }
      if (!running) break;
      await yieldToEventLoop();
    }
  } catch (error) {
    post({ type: 'error', message: errorMessage(error) });
  } finally {
    search?.free();
  }
};
