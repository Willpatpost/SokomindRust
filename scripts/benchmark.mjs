import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { catalogHash, nativeCorpus, summarize } from './corpus.mjs';
import { root } from './toolchain.mjs';

const args = process.argv.slice(2);
const check = args[0] === '--check';
if (check) args.shift();
assert(!check || args.length === 0, 'Baseline checks always use the reviewed fixed-node configuration');
const baseline = check ? JSON.parse(readFileSync(resolve(root, 'benchmarks/catalog-baseline.json'), 'utf8')) : null;
const records = nativeCorpus(check ? ['--states', String(baseline.maxStates), '--memory', String(baseline.memoryMiB)] : args);
mkdirSync(resolve(root, 'target/bench'), { recursive: true });
writeFileSync(resolve(root, 'target/bench/catalog.json'), JSON.stringify({ catalogHash, records }, null, 2) + '\n');
console.table(summarize(records));
if (baseline) {
  assert.equal(catalogHash, baseline.catalogHash, 'Catalog changed: review and update the benchmark baseline');
  const actual = new Map(records.map(record => [record.id + ':' + record.mode, record]));
  assert.equal(actual.size, baseline.cases.length);
  for (const previous of baseline.cases) {
    const key = previous.id + ':' + previous.mode;
    const result = actual.get(key);
    assert(result, `Missing ${key}`);
    if (previous.moves !== null) {
      assert(result.moves !== null && result.moves <= previous.moves, `${key}: lost or worsened the reviewed route`);
    }
    if (previous.proven) assert.equal(result.proof.kind, 'optimal', `${key}: lost exact proof`);
    assert(result.reserved_bytes <= previous.reserved_bytes, `${key}: accounted memory regression`);
    assert(result.expanded <= Math.max(previous.expanded + 32, Math.ceil(previous.expanded * 1.2)), `${key}: expansion regression >20%`);
    assert(result.generated <= Math.max(previous.generated + 32, Math.ceil(previous.generated * 1.2)), `${key}: generation regression >20%`);
  }
  console.log(`Reviewed performance gates passed for ${records.length} cases (routes, proofs, node counts, accounted memory).`);
}
console.log('Raw measurements: target/bench/catalog.json; timings are observational and exclude compilation.');
