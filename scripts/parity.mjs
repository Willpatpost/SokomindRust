import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { decodeMetricTuple, decodeSnapshot } from '../web/src/transport.ts';
import { catalog, diagnosticFields, nativeCorpus } from './corpus.mjs';
import { root } from './toolchain.mjs';

/** Runs one of the web app's own WASM decoders, naming the case that fails it. */
function decode(context, run) {
  try { return run(); } catch (error) { throw new Error(`${context}: ${error.message}`, { cause: error }); }
}

const { default: init, WasmGame, WasmSearch } = await import(pathToFileURL(resolve(root, 'web/wasm/sokomind.js')));
const wasm = await init({ module_or_path: readFileSync(resolve(root, 'web/wasm/sokomind_bg.wasm')) });
const native = nativeCorpus();
const puzzles = new Map(catalog.map(puzzle => [puzzle.id, puzzle.rows.join('\n')]));
let verified = 0;
for (const reference of native) {
  const rows = puzzles.get(reference.id);
  const context = `${reference.id}/${reference.mode}`;
  const search = new WasmSearch(rows, '', reference.mode, reference.max_states, reference.memory_mib);
  try {
    while (search.advance(8)) {}
    // The worker's own decoder: parity checks what the UI reads, not a copy of it.
    const metrics = decode(context, () => decodeMetricTuple(search.metrics(), search.status()));
    assert.deepEqual({ status: metrics.status, expanded: metrics.expanded, generated: metrics.generated,
      best: metrics.best ?? null, lower: metrics.lowerBound ?? null, proof: metrics.proof },
    { status: reference.status, expanded: reference.expanded, generated: reference.generated,
      best: reference.moves, lower: reference.lower_bound, proof: reference.proof }, context);
    assert(metrics.reservedBytes <= reference.memory_mib * 1048576, `${context}: WASM budget exceeded`);
    assert.deepEqual(Array.from(search.diagnostics()), diagnosticFields.map(field => reference.stats[field]), `${context}: diagnostics`);
    const route = search.solution();
    assert.equal(route ?? null, reference.route, `${context}: route differs`);
    if (route !== undefined) {
      const game = new WasmGame(rows);
      try {
        game.replay(route);
        const boxes = game.labels().length;
        // main.ts's own snapshot decoder, checked the same way.
        const snapshot = decode(context, () => decodeSnapshot(game.snapshot(), boxes));
        assert.equal(snapshot.solved, true, context);
        assert.equal(snapshot.moves, reference.moves, context);
        assert.equal(snapshot.pushes, reference.pushes, context);
        for (let i = 0; i < boxes; i++) assert(game.on_goal(i), context);
        verified++;
      } finally { game.free(); }
    }
  } finally { search.free(); }
}
console.log(`${native.length} native/WASM cases match including proofs, routes, and diagnostics; ${verified} routes replayed.`);
console.log(`WASM retained linear memory: ${(wasm.memory.buffer.byteLength / 1048576).toFixed(2)} MiB (one reused test instance, not per-worker RSS).`);
