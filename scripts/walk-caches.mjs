// Generates the web's walk-term dual cache: runs the native engine over
// the named catalog boards at the app's declaration with the build's
// dual-cache directory armed, collects the SWT1 records the solves store,
// and writes them with a rows-hash manifest into web/public/walk-caches/,
// where the worker fetches them for its optimal searches. The records are
// keyed to the initial position and the tables the capped scale builds;
// the engine's own name, dimension and objective checks decide whether a
// fetched record is trusted, so a stale manifest is a missed cache, never
// a wrong bound.
//   node scripts/walk-caches.mjs -- --boards large,huge
import { createHash } from 'node:crypto';
import { copyFileSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { catalog, nativeCorpus } from './corpus.mjs';
import { root } from './toolchain.mjs';

const OUT = resolve(root, 'web/public/walk-caches');
// The app's declaration: the state ceiling the page always sends and the
// memory select's top. The room tables' cap clamps to 2^24 wherever the
// walk term arms at all, so records generated here serve every budget
// that arms the term, not only this one.
const STATES = 256_000_000;
const MEMORY_MIB = 1024;
// The term arms after a million expansions and the LP pair follows;
// huge's pair outlasts this budget, so its record wants a longer run.
const TIME_MS = 1_800_000;

const args = process.argv.slice(2);
const at = args.indexOf('--boards');
if (at < 0 || at + 1 >= args.length) {
  console.error('Usage: node scripts/walk-caches.mjs -- --boards id1,id2');
  process.exitCode = 2;
} else {
  const wanted = new Set(args[at + 1].split(','));
  const puzzles = catalog.filter(puzzle => wanted.has(puzzle.id));
  const missing = [...wanted].filter(id => !puzzles.some(puzzle => puzzle.id === id));
  if (missing.length) throw new Error(`No catalog puzzle ${missing.join(', ')}`);
  mkdirSync(OUT, { recursive: true });
  // A record regenerated replaces its manifest row; others' rows stay.
  const manifestPath = resolve(OUT, 'manifest.json');
  /** @type {{ rowsHash: string, name: string }[]} */
  let entries = [];
  try {
    entries = JSON.parse(readFileSync(manifestPath, 'utf8')).entries;
  } catch {
    /* No manifest yet. */
  }
  const scratch = resolve(root, 'target/walk-caches');
  for (const puzzle of puzzles) {
    rmSync(scratch, { recursive: true, force: true });
    mkdirSync(scratch, { recursive: true });
    const records = nativeCorpus(
      [
        '--states',
        String(STATES),
        '--memory',
        String(MEMORY_MIB),
        '--time-ms',
        String(TIME_MS),
        '--mode',
        'optimal',
        '--puzzle',
        puzzle.id,
      ],
      { env: { SOKOMIND_DUAL_CACHE: scratch, SOKOMIND_WALK_TERM_LOG: '1' } },
    );
    const stored = readdirSync(scratch).filter(name => name.endsWith('.swt'));
    for (const name of stored) {
      copyFileSync(resolve(scratch, name), resolve(OUT, name));
      entries = entries.filter(entry => entry.name !== name);
      entries.push({
        rowsHash: createHash('sha256').update(puzzle.rows.join('\n')).digest('hex'),
        name,
      });
    }
    const record = records[0];
    if (record === undefined) throw new Error(`${puzzle.id}: the corpus run printed no record`);
    console.log(`${puzzle.id}: ${record.status}, ${record.expanded.toLocaleString()} expanded, ${stored.length} record(s) stored.`);
  }
  rmSync(scratch, { recursive: true, force: true });
  writeFileSync(manifestPath, JSON.stringify({ entries }, null, 2) + '\n');
  console.log(`Wrote ${entries.length} record(s) and the manifest to web/public/walk-caches/.`);
}
