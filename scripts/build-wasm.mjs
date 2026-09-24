import { run } from './toolchain.mjs';
run('cargo', ['build', '-p', 'sokomind-wasm', '--target', 'wasm32-unknown-unknown', '--profile', 'wasm-release']);
run('wasm-bindgen', ['--target', 'web', '--out-dir', 'web/wasm', '--out-name', 'sokomind', 'target/wasm32-unknown-unknown/wasm-release/sokomind_wasm.wasm']);
