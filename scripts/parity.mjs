import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { catalog, diagnosticFields, nativeCorpus } from './corpus.mjs';
import { root } from './toolchain.mjs';

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
    const [expanded, generated, reserved, best, kind, lower] = search.metrics();
    const proof = kind === 2 ? { kind: 'optimal', moves: best }
      : kind === 1 ? { kind: 'bounded', lower, upper: best }
      : kind === 3 ? { kind: 'unsolvable' } : { kind: 'none' };
    assert.deepEqual({ status: search.status(), expanded, generated, best: best === 0xffffffff ? null : best,
      lower: lower === 0xffffffff ? null : lower, proof },
    { status: reference.status, expanded: reference.expanded, generated: reference.generated,
      best: reference.moves, lower: reference.lower_bound, proof: reference.proof }, context);
    assert(reserved <= reference.memory_mib * 1048576, `${context}: WASM budget exceeded`);
    assert.deepEqual(Array.from(search.diagnostics()), diagnosticFields.map(field => reference.stats[field]), `${context}: diagnostics`);
    const route = search.solution();
    assert.equal(route ?? null, reference.route, `${context}: route differs`);
    if (route !== undefined) {
      const game = new WasmGame(rows);
      try {
        game.replay(route);
        const snapshot = game.snapshot();
        assert.equal(snapshot[3], 1, context);
        assert.equal(snapshot[1], reference.moves, context);
        assert.equal(snapshot[2], reference.pushes, context);
        for (let i = 0; i < game.labels().length; i++) assert(game.on_goal(i), context);
        verified++;
      } finally { game.free(); }
    }
  } finally { search.free(); }
}
console.log(`${native.length} native/WASM cases match including proofs, routes, and diagnostics; ${verified} routes replayed.`);
console.log(`WASM retained linear memory: ${(wasm.memory.buffer.byteLength / 1048576).toFixed(2)} MiB (one reused test instance, not per-worker RSS).`);
