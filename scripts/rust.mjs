// Runs cargo with toolchain.mjs's environment, for package.json's Rust
// scripts: node scripts/rust.mjs <cargo arguments>.
import { run } from './toolchain.mjs';
run('cargo', process.argv.slice(2));
