import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { decodeMetricTuple, decodeSnapshot } from '../web/src/transport.ts';
import { MODES } from './bench-gate.mjs';
import { catalog, catalogHash, diagnosticFields, nativeCorpus } from './corpus.mjs';
import { root } from './toolchain.mjs';

// `--native <file>` checks WASM against native records already on disk instead
// of running the native corpus again. CI passes target/bench/catalog.json right
// after bench:check wrote it, so the corpus runs once per CI run.
// BENCH_FEATURES must match the one `npm run wasm` built web/wasm with.
const FEATURES = process.env.BENCH_FEATURES ?? '';
const { values: options } = parseArgs({ options: { native: { type: 'string' } }, strict: true, allowPositionals: false });
/**
 * The native records in a file bench:check wrote, once they are shown to cover this catalog and build.
 * @param {string} path
 * @returns {import('./corpus.mjs').CorpusRecord[]}
 */
function recorded(path) {
  const file = JSON.parse(readFileSync(resolve(root, path), 'utf8'));
  assert.equal(file.catalogHash, catalogHash, `${path} was recorded for another catalog; rerun npm run bench:check`);
  assert.equal(file.features ?? '', FEATURES, `${path} was measured with cargo features "${file.features ?? ''}", not BENCH_FEATURES="${FEATURES}"`);
  const records = /** @type {unknown} */ (file.records);
  const cases = Array.isArray(records) ? records.map(record => `${record.id}/${record.mode}`) : [];
  assert.deepEqual(cases, catalog.flatMap(puzzle => MODES.map(mode => `${puzzle.id}/${mode}`)),
    `${path} must hold one record per catalog puzzle and mode, as bench:check writes it`);
  return /** @type {import('./corpus.mjs').CorpusRecord[]} */ (records);
}

/**
 * Runs one of the web app's own WASM decoders, naming the case that fails it.
 * @template T
 * @param {string} context
 * @param {() => T} run
 * @returns {T}
 */
function decode(context, run) {
  try { return run(); } catch (error) { throw new Error(`${context}: ${/** @type {Error} */ (error).message}`, { cause: error }); }
}

const { default: init, WasmGame, WasmSearch } = await import(pathToFileURL(resolve(root, 'web/wasm/sokomind.js')).href);
const wasm = await init({ module_or_path: readFileSync(resolve(root, 'web/wasm/sokomind_bg.wasm')) });
const native = options.native ? recorded(options.native) : nativeCorpus([], FEATURES);
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
if (FEATURES) console.log(`*** Checked with cargo features: ${FEATURES} ***`);
console.log(`${native.length} native/WASM cases match including proofs, routes, and diagnostics; ${verified} routes replayed.`);
console.log(`WASM retained linear memory: ${(wasm.memory.buffer.byteLength / 1048576).toFixed(2)} MiB (one reused test instance, not per-worker RSS).`);
