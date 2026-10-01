import { test } from 'node:test';
import assert from 'node:assert/strict';
import { attempt, childEnv, env } from './toolchain.mjs';

/** @param {string} key */
const isPath = key => key.toUpperCase() === 'PATH';

test("childEnv sets a command's variables over env, keeps env's path entry and leaves env unchanged", () => {
  // toolchain.mjs always sets the path: Path on Windows, PATH elsewhere.
  const pathKey = Object.keys(env).find(isPath);
  assert.ok(pathKey, 'env has no PATH or Path entry');
  const before = { ...env };
  // validate.mjs's doc:rust step env, which reaches cargo only through attempt and childEnv.
  const child = childEnv({ RUSTDOCFLAGS: '-D warnings' });
  assert.equal(child.RUSTDOCFLAGS, '-D warnings');
  assert.equal(child[pathKey], env[pathKey]);
  assert.deepEqual(Object.keys(child).filter(isPath), Object.keys(env).filter(isPath), 'no second path entry under another spelling');
  // A variable env already has is overridden.
  assert.equal(childEnv({ [pathKey]: 'elsewhere' })[pathKey], 'elsewhere');
  assert.deepEqual(childEnv(), env);
  assert.notEqual(childEnv(), env, 'a copy, not env itself');
  assert.deepEqual(env, before, 'env is unchanged');
});

test('attempt hands a command its extra variables, for that command only', () => {
  // Node itself as the command, so no cargo is needed; the probe name is one nobody sets.
  const probe = "process.exit(process.env.SOKOMIND_ATTEMPT_PROBE === 'set' ? 0 : 3)";
  assert.equal(attempt(process.execPath, ['-e', probe], { SOKOMIND_ATTEMPT_PROBE: 'set' }), 0);
  assert.equal(attempt(process.execPath, ['-e', probe]), 3);
});
