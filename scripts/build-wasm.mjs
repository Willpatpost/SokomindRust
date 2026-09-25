import { run, env, root } from './toolchain.mjs';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

// The CLI must equal the wasm-bindgen crate that Cargo.lock resolves.
const lock = readFileSync(resolve(root, 'Cargo.lock'), 'utf8');
const PINNED = /name = "wasm-bindgen"\r?\nversion = "([^"]+)"/.exec(lock)?.[1];
if (!PINNED) {
  console.error('Cargo.lock does not list wasm-bindgen.');
  process.exit(1);
}
const version = spawnSync('wasm-bindgen', ['--version'], { env, encoding: 'utf8' });
if (version.error || version.status !== 0) {
  console.error('wasm-bindgen is not runnable. See README setup.');
  process.exit(1);
}
if (!version.stdout.trim().endsWith(PINNED)) {
  console.error(`wasm-bindgen ${PINNED} is required (found: ${version.stdout.trim()}). See README setup.`);
  process.exit(1);
}
run('cargo', ['build', '--locked', '-p', 'sokomind-wasm', '--target', 'wasm32-unknown-unknown', '--profile', 'wasm-release']);
run('wasm-bindgen', ['--target', 'web', '--out-dir', 'web/wasm', '--out-name', 'sokomind', 'target/wasm32-unknown-unknown/wasm-release/sokomind_wasm.wasm']);
