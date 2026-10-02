// Checks the WASM build in web/wasm against the native corpus: each catalog
// search must match its native record's status, counters, bound, proof,
// diagnostics and route and stay within its memory budget, and each route must
// replay to a solved board. Two last cases, each in a fresh instance, check that
// a search's arena grows with its use, not its budget, and that one that fills
// its arena grows memory by at most its memory ceiling plus 2 MiB. npm run
// test:parity builds the WASM first; CI and validate run this file with --native
// after their one WASM build.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { MAX_STATES } from '../web/src/protocol.ts';
import { decodeMetricTuple, decodeSnapshot } from '../web/src/transport.ts';
import { MODES } from './bench-gate.mjs';
import { MIB, catalog, catalogHash, diagnosticFields, nativeCorpus } from './corpus.mjs';
import { root } from './toolchain.mjs';

// `--native <file>` checks WASM against native records already on disk instead
// of running the native corpus again. CI passes target/bench/catalog.json right
// after bench:check wrote it, so the corpus runs once per CI run.
const { values: options } = parseArgs({ options: { native: { type: 'string' } }, strict: true, allowPositionals: false });
/**
 * The native records in a file bench:check wrote, once they are shown to cover this catalog.
 * @param {string} path
 * @returns {import('./corpus.mjs').CorpusRecord[]}
 */
function recorded(path) {
  const file = JSON.parse(readFileSync(resolve(root, path), 'utf8'));
  assert.equal(file.catalogHash, catalogHash, `${path} was recorded for another catalog; rerun npm run bench:check`);
  const records = /** @type {unknown} */ (file.records);
  const cases = Array.isArray(records) ? records.map(record => `${record.id}/${record.mode}`) : [];
  assert.deepEqual(
    cases,
    catalog.flatMap(puzzle => MODES.map(mode => `${puzzle.id}/${mode}`)),
    `${path} must hold one record per catalog puzzle and mode, as bench:check writes it`,
  );
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
  try {
    return run();
  } catch (error) {
    throw new Error(`${context}: ${/** @type {Error} */ (error).message}`, { cause: error });
  }
}

const bindings = pathToFileURL(resolve(root, 'web/wasm/sokomind.js')).href;
const wasmBytes = readFileSync(resolve(root, 'web/wasm/sokomind_bg.wasm'));
const { default: init, WasmGame, WasmSearch } = await import(bindings);
const wasm = await init({ module_or_path: wasmBytes });
const native = options.native ? recorded(options.native) : nativeCorpus();
const puzzles = new Map(catalog.map(puzzle => [puzzle.id, puzzle.rows.join('\n')]));
/** Pops per advance(). parity sets no deadline, so any batch size gives the same records. */
const POPS_PER_ADVANCE = 8;
let verified = 0;
for (const reference of native) {
  const rows = puzzles.get(reference.id);
  const context = `${reference.id}/${reference.mode}`;
  const search = new WasmSearch(rows, '', reference.mode, reference.max_states, reference.memory_mib);
  try {
    while (search.advance(POPS_PER_ADVANCE)) {}
    // The worker's own decoder: parity checks what the UI reads, not a copy of it.
    const metrics = decode(context, () => decodeMetricTuple(search.metrics(), search.status()));
    assert.deepEqual(
      {
        status: metrics.status,
        expanded: metrics.expanded,
        generated: metrics.generated,
        best: metrics.best ?? null,
        lower: metrics.lowerBound ?? null,
        proof: metrics.proof,
      },
      {
        status: reference.status,
        expanded: reference.expanded,
        generated: reference.generated,
        best: reference.moves,
        lower: reference.lower_bound,
        proof: reference.proof,
      },
      context,
    );
    assert(metrics.reservedBytes <= reference.memory_mib * MIB, `${context}: WASM budget exceeded`);
    assert.deepEqual(
      Array.from(search.diagnostics()),
      diagnosticFields.map(field => reference.stats[field]),
      `${context}: diagnostics`,
    );
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
      } finally {
        game.free();
      }
    }
  } finally {
    search.free();
  }
}

/**
 * Runs one catalog puzzle in optimal mode at the full state cap to its end, solved or not, in a fresh
 * WASM instance and measures how far its linear memory grew. Linear memory never shrinks and the
 * instance above held every corpus search, so each call imports the bindings under its own query
 * string, which gives them their own module state and instance.
 * @param {string} query
 * @param {string} id
 * @param {number} memoryMib
 */
async function freshRun(query, id, memoryMib) {
  const fresh = await import(`${bindings}?${query}`);
  const freshWasm = await fresh.default({ module_or_path: wasmBytes });
  const before = freshWasm.memory.buffer.byteLength;
  const search = new fresh.WasmSearch(puzzles.get(id), '', 'optimal', MAX_STATES, memoryMib);
  try {
    while (search.advance(POPS_PER_ADVANCE)) {}
    const metrics = decode(`parity ${query}: ${id}`, () => decodeMetricTuple(search.metrics(), search.status()));
    return { metrics, growth: freshWasm.memory.buffer.byteLength - before };
  } finally {
    search.free();
  }
}
/** @param {number} bytes */
const mib = bytes => (bytes / MIB).toFixed(2);

console.log(`${native.length} native/WASM cases match including proofs, routes, and diagnostics; ${verified} routes replayed.`);
console.log(`WASM retained linear memory: ${mib(wasm.memory.buffer.byteLength)} MiB (one reused test instance, not per-worker RSS).`);

// Ultra-tiny at the full state cap and the web's default 64 MiB budget holds
// 2,097,151 states: the arena reserves their 16 MiB queue up front and grows only
// the records and table it fills, so the instance grows by at most that plus
// 4 MiB, where an arena reserved whole took about 60 MiB.
const TINY_GROWTH_CAP = (16 + 4) * MIB;
const tiny = await freshRun('memory', 'ultra-tiny', 64);
console.log(`parity memory: ultra-tiny growth=${mib(tiny.growth)} limit=${mib(TINY_GROWTH_CAP)}`);
assert.equal(tiny.metrics.status, 'solved', 'parity memory: ultra-tiny/optimal');
assert(tiny.growth <= TINY_GROWTH_CAP, `parity memory: ultra-tiny grew a fresh instance ${tiny.growth} bytes, over ${TINY_GROWTH_CAP}`);

// Large at 16 MiB fills its arena, so every buffer reaches the size
// reserved_bytes charges and the state table grows once, freeing its first
// table, at most 256 KiB, which linear memory keeps. The instance may grow by at
// most reserved_bytes plus 2 MiB for that table, the allocator and the bindings.
const fill = await freshRun('memory-fill', 'large', 16);
const fillCap = fill.metrics.reservedBytes + 2 * MIB;
const fillSizes = `growth=${mib(fill.growth)} reserved=${mib(fill.metrics.reservedBytes)} limit=${mib(fillCap)}`;
console.log(`parity memory fill: large status=${fill.metrics.status} ${fillSizes}`);
assert.equal(fill.metrics.status, 'memory_limit', 'parity memory fill: large/optimal');
assert(fill.growth <= fillCap, `parity memory fill: large grew a fresh instance ${fill.growth} bytes, over ${fillCap}`);
