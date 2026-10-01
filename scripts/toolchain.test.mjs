import { test } from 'node:test';
import assert from 'node:assert/strict';
import { childEnv, env } from './toolchain.mjs';

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
