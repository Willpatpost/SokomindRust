import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { MODES } from './bench-gate.mjs';
import { env, root, run } from './toolchain.mjs';

export const catalog = JSON.parse(readFileSync(resolve(root, 'data/puzzles.json'), 'utf8'));
// Windows checkouts hold CRLF working copies; hash normalized text so the
// benchmark baseline is stable on every platform.
export const catalogHash = createHash('sha256')
    .update(readFileSync(resolve(root, 'data/puzzles.json'), 'utf8').replace(/\r\n/g, '\n'))
    .digest('hex');
// sokomind_search::SearchStats::FIELDS, in order (pinned by crates/search/tests/boundary.rs).
export const diagnosticFields = [
  'unique_states', 'duplicate_improvements', 'reopened_states', 'stale_pops', 'peak_queue',
  'pruned_dead_cells', 'pruned_deadlocks', 'pruned_duplicates', 'pruned_assignment', 'pruned_bound',
];

let built = false;
/** Compile once per process, then return the example's verified JSON-line records. */
export function nativeCorpus(args = [], features = '') {
  if (!built) {
    run('cargo', ['build', '--locked', '--release', '-p', 'sokomind-search', '--example', 'catalog',
      ...(features ? ['--features', features] : [])]);
  }
  built = true;
  const binary = resolve(root, 'target/release/examples/catalog' + (process.platform === 'win32' ? '.exe' : ''));
  const result = spawnSync(binary, args, { cwd: root, env, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr || `Corpus exited ${result.status}`);
  return result.stdout.trim().split(/\r?\n/).map(line => JSON.parse(line));
}

/** Short HEAD sha, marked +dirty when tracked files outside benchmarks/ differ from it. */
export function sourceRevision() {
  const git = args => spawnSync('git', args, { cwd: root, encoding: 'utf8' });
  const head = git(['rev-parse', '--short=12', 'HEAD']);
  const dirty = git(['status', '--porcelain', '--untracked-files=no', '--', '.', ':(exclude)benchmarks']);
  if (head.error || head.status !== 0 || dirty.error || dirty.status !== 0) {
    console.warn('git is unavailable; recording sourceRevision "unknown"');
    return 'unknown';
  }
  return head.stdout.trim() + (dirty.stdout.trim() ? '+dirty' : '');
}

export function summarize(records) {
  return MODES.map(mode => {
    const cases = records.filter(r => r.mode === mode);
    const times = cases.map(r => r.search_us / 1000).sort((a, b) => a - b);
    return {
      mode, runs: cases.length, withRoute: cases.filter(r => r.route !== null).length,
      solved: cases.filter(r => r.status === 'solved').length,
      stateLimit: cases.filter(r => r.status === 'state_limit').length,
      timeLimit: cases.filter(r => r.status === 'time_limit').length,
      medianMs: times.length ? times[Math.floor(times.length / 2)] : null,
      expanded: cases.reduce((sum, r) => sum + r.expanded, 0),
      generated: cases.reduce((sum, r) => sum + r.generated, 0),
      peakReservedMiB: Math.max(0, ...cases.map(r => r.reserved_bytes)) / 1048576,
    };
  });
}
