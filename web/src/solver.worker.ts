import init, { WasmSearch } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import { errorMessage, type Metrics, type Proof, type SearchStatus, type WorkerReply, type WorkerRequest } from './protocol';
let cancelled = false;
const post = (message: WorkerReply) => self.postMessage(message);
// Browsers clamp nested setTimeout(0) to >= 4ms, which would idle a third
// of every time budget; a MessageChannel yield is a macrotask without clamp.
const yieldChannel = new MessageChannel();
const yieldToEventLoop = () => new Promise<void>((resolve) => {
  yieldChannel.port1.onmessage = () => resolve();
  yieldChannel.port2.postMessage(0);
});
// WasmSearch.metrics(): expanded, generated, reserved bytes, best moves, proof
// kind (0 none, 1 bounded, 2 optimal, 3 unsolvable) and the lower bound, with
// u32::MAX for a missing best or bound. Decoded here, once per slice.
const NONE = 0xffffffff;
function readMetrics(search: WasmSearch, status: SearchStatus): Metrics {
  const [expanded, generated, reservedBytes, best, kind, lower] = search.metrics();
  const proof: Proof = kind === 2 ? { kind: 'optimal', moves: best } : kind === 1 ? { kind: 'bounded', lower, upper: best }
    : kind === 3 ? { kind: 'unsolvable' } : { kind: 'none' };
  return { expanded, generated, reservedBytes, best: best === NONE ? undefined : best, lowerBound: lower === NONE ? undefined : lower, proof, status };
}
self.onmessage = async ({ data }: MessageEvent<WorkerRequest>) => {
  if (data.type === 'cancel') { cancelled = true; return; }
  let search: WasmSearch | undefined;
  const started = performance.now();
  try {
    await init({ module_or_path: wasmUrl });
    const r = data.request;
    search = new WasmSearch(r.rows, r.actions, r.mode, r.maxStates, r.memoryMiB);
    let reportedBest: number | undefined;
    let lastReport = 0;
    let lastRoute = 0;
    let running = true;
    while (true) {
      const slice = performance.now();
      while (running && performance.now() - slice < 8) {
        if (cancelled || performance.now() - started >= r.timeMs) { search.stop(!cancelled); running = false; }
        else running = search.advance(8);
      }
      const elapsedMs = performance.now() - started;
      // Only a finished search needs its status string from Rust.
      const metrics = readMetrics(search, running ? 'running' : search.status() as SearchStatus);
      // Rebuilding a route walks every push, so a stream of Quality improvements
      // is sampled every 500 ms; the first route and the final best always go out.
      const best = metrics.best;
      const improved = best !== undefined
        && (reportedBest === undefined || (best < reportedBest && (!running || elapsedMs - lastRoute >= 500)));
      if (improved || !running || elapsedMs - lastReport >= 150) {
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
  } catch (error) { post({ type: 'error', message: errorMessage(error) }); }
  finally { search?.free(); }
};
