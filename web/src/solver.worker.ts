import init, { WasmSearch } from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import type { WorkerReply, WorkerRequest } from './protocol';
let cancelled = false;
let active = false;
const post = (message: WorkerReply) => self.postMessage(message);
self.onmessage = async ({ data }: MessageEvent<WorkerRequest>) => {
  if (data.type === 'cancel') { cancelled = true; return; }
  if (active) return;
  active = true;
  let search: WasmSearch | undefined;
  const started = performance.now();
  try {
    await init({ module_or_path: wasmUrl });
    const r = data.request;
    search = new WasmSearch(r.rows, r.actions, r.mode, r.maxStates, r.memoryMiB);
    let reportedBest = 0xffffffff;
    let lastReport = 0;
    while (true) {
      const slice = performance.now();
      while (performance.now() - slice < 8 && search.status() === 'running') {
        if (cancelled || performance.now() - started >= r.timeMs) { search.stop(!cancelled); break; }
        search.advance(8);
      }
      const elapsedMs = performance.now() - started;
      const metrics = search.metrics();
      const status = search.status();
      const improved = metrics[3] < reportedBest;
      if (improved || status !== 'running' || elapsedMs - lastReport >= 150) {
        // Serialize only tiny telemetry and a newly improved, Rust-verified route.
        const route = improved ? search.solution() : undefined;
        if (route !== undefined) reportedBest = metrics[3];
        post({ type: status === 'running' ? 'progress' : 'done', status, metrics, elapsedMs, route });
        lastReport = elapsedMs;
      }
      if (status !== 'running') break;
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
  } catch (error) { post({ type: 'error', message: String(error) }); }
  finally { search?.free(); active = false; }
};
