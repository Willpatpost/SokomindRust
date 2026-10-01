import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { env, root, run } from './toolchain.mjs';

/**
 * The catalog fields these scripts read.
 * @typedef {{ id: string, rows: string[] }} Puzzle
 */

/**
 * One JSON line from crates/search/examples/catalog.rs: one search of one puzzle in one mode.
 * @typedef {object} CorpusRecord
 * @property {string} id
 * @property {string} fingerprint
 * @property {string} mode
 * @property {number} sample
 * @property {number} max_states
 * @property {number} memory_mib
 * @property {string} status
 * @property {number | null} moves
 * @property {number | null} pushes
 * @property {string | null} route
 * @property {{ kind: string, moves?: number, lower?: number, upper?: number }} proof
 * @property {number | null} lower_bound
 * @property {number} expanded
 * @property {number} generated
 * @property {number} reserved_bytes
 * @property {number | null} first_route_expanded
 * @property {number | null} first_route_generated
 * @property {number} setup_us
 * @property {number | null} first_route_us
 * @property {number} search_us
 * @property {number} reconstruct_us
 * @property {Record<string, number>} stats
 */

// Bytes per MiB: budgets are set in MiB, while the records and the WASM metrics count bytes.
export const MIB = 1024 * 1024;

const catalogText = readFileSync(resolve(root, 'data/puzzles.json'), 'utf8');
/** @type {Puzzle[]} */
export const catalog = JSON.parse(catalogText);
// Windows checkouts hold CRLF working copies; hash normalized text so the
// benchmark baseline is stable on every platform.
export const catalogHash = createHash('sha256').update(catalogText.replace(/\r\n/g, '\n')).digest('hex');
// sokomind_search::SearchStats::FIELDS, in order. parity.mjs catches drift: it
// compares the WASM diagnostics with the native stats read in this order.
export const diagnosticFields = [
  'unique_states', 'duplicate_improvements', 'reopened_states', 'stale_pops', 'peak_queue',
  'pruned_dead_cells', 'pruned_deadlocks', 'pruned_duplicates', 'pruned_assignment', 'pruned_bound',
];

let built = false;
/**
 * Compile once per process, then return the example's verified JSON-line records.
 * @param {readonly string[]} [args] Options for the catalog example.
 * @returns {CorpusRecord[]}
 */
export function nativeCorpus(args = []) {
  if (!built) run('cargo', ['build', '--locked', '--release', '-p', 'sokomind-search', '--example', 'catalog']);
  built = true;
  const binary = resolve(root, 'target/release/examples/catalog' + (process.platform === 'win32' ? '.exe' : ''));
  const result = spawnSync(binary, args, { cwd: root, env, encoding: 'utf8', maxBuffer: 64 * MIB });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr || `Corpus exited ${result.status}`);
  return result.stdout.trim().split(/\r?\n/).map(line => JSON.parse(line));
}

/** Short HEAD sha, marked +dirty when tracked files outside benchmarks/ differ from it. */
export function sourceRevision() {
  /** @param {string[]} args */
  const git = args => spawnSync('git', args, { cwd: root, encoding: 'utf8' });
  const head = git(['rev-parse', '--short=12', 'HEAD']);
  const dirty = git(['status', '--porcelain', '--untracked-files=no', '--', '.', ':(exclude)benchmarks']);
  if (head.error || head.status !== 0 || dirty.error || dirty.status !== 0) {
    console.warn('git is unavailable; recording sourceRevision "unknown"');
    return 'unknown';
  }
  return head.stdout.trim() + (dirty.stdout.trim() ? '+dirty' : '');
}
