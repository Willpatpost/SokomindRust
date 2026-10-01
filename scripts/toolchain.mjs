import { existsSync } from 'node:fs';
import { resolve, delimiter } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
export const root = fileURLToPath(new URL('../', import.meta.url));
export const env = { ...process.env };
// Windows names the variable "Path"; writing env.PATH there would add a
// second, case-sensitive key and drop the system path from child processes.
const pathKey = Object.keys(env).find(key => key.toUpperCase() === 'PATH') ?? 'PATH';
const localCargo = resolve(root, '.tools/cargo');
if (existsSync(localCargo)) {
  env.CARGO_HOME = localCargo;
  env.RUSTUP_HOME = resolve(root, '.tools/rustup');
  env[pathKey] = `${resolve(localCargo, 'bin')}${delimiter}${env[pathKey]}`;
}
env[pathKey] = `${resolve(root, '.tools/bin')}${delimiter}${env[pathKey]}`;

/**
 * The environment for one command: a copy of env with `extra`'s variables set
 * over it, so env itself never keeps a command's variables.
 * @param {Readonly<Record<string, string>>} [extra]
 * @returns {NodeJS.ProcessEnv}
 */
export const childEnv = extra => ({ ...env, ...extra });

/**
 * Runs a command in the repo root, with the project-local tools on the path and
 * output shared with this process. Returns its exit code, or 1 when it could not
 * start or was killed by a signal.
 * @param {string} command
 * @param {readonly string[]} args
 * @param {Readonly<Record<string, string>>} [extra] Variables set for this command only, over env.
 * @returns {number}
 */
export function attempt(command, args, extra) {
  const result = spawnSync(command, args, { cwd: root, env: childEnv(extra), stdio: 'inherit', shell: false });
  if (result.error) {
    console.error(`${command}: ${result.error.message}. See "Run locally" in README.md.`);
    return 1;
  }
  return result.status ?? 1;
}

/**
 * Like attempt, but a failure ends this process with the command's exit code.
 * @param {string} command
 * @param {readonly string[]} args
 */
export function run(command, args) {
  const code = attempt(command, args);
  if (code !== 0) process.exit(code);
}
