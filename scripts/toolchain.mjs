import { existsSync } from 'node:fs';
import { resolve, delimiter } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
export const root = fileURLToPath(new URL('../', import.meta.url));
export const env = { ...process.env };
const localCargo = resolve(root, '.tools/cargo');
if (existsSync(localCargo)) {
  env.CARGO_HOME = localCargo;
  env.RUSTUP_HOME = resolve(root, '.tools/rustup');
  env.PATH = `${resolve(localCargo, 'bin')}${delimiter}${env.PATH}`;
}
env.PATH = `${resolve(root, '.tools/bin')}${delimiter}${env.PATH}`;
export function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, env, stdio: 'inherit', shell: false });
  if (result.error) { console.error(`${command}: ${result.error.message}. See README setup.`); process.exit(1); }
  if (result.status !== 0) process.exit(result.status || 1);
}
