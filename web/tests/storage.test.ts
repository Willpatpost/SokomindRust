import { test } from 'node:test';
import assert from 'node:assert/strict';
import { best, profile, session, write } from '../src/storage.ts';
import { installMemoryStorage } from './fakes.ts';
const storage = installMemoryStorage();
const PREFIX = 'sokomind-rust.v1.';
const PROFILE_ID = /^[a-f0-9]{32}$/;
test('session round-trips and is null when missing or malformed', () => {
  storage.clear();
  assert.equal(session(), null);
  const saved = { id: 'p', rows: 'rows', actions: 'UD' };
  assert.equal(write('session', saved), true);
  assert.deepEqual(session(), saved);
  for (const value of [null, 'session', { id: 'p', rows: 'rows' }, { ...saved, id: 1 }, { ...saved, actions: null }]) {
    write('session', value);
    assert.equal(session(), null);
  }
  storage.set(PREFIX + 'session', 'not json');
  assert.equal(session(), null);
});
test('best accepts a record stored for the same rows and rejects anything else', () => {
  storage.clear();
  const record = { rows: 'rows', route: 'DD', moves: 2, pushes: 1 };
  write('best.p', record);
  assert.deepEqual(best('p', 'rows'), record);
  assert.equal(best('p', 'other rows'), null);
  assert.equal(best('q', 'rows'), null);
  for (const patch of [{ rows: undefined }, { route: 1 }, { moves: 1.5 }, { moves: '2' }, { pushes: null }]) {
    write('best.p', { ...record, ...patch });
    assert.equal(best('p', 'rows'), null);
  }
  storage.set(PREFIX + 'best.p', '{"rows":');
  assert.equal(best('p', 'rows'), null);
});
test('profile creates a random id once and then returns the stored one', () => {
  storage.clear();
  const id = profile();
  assert.match(id ?? '', PROFILE_ID);
  assert.equal(storage.get(PREFIX + 'profile'), JSON.stringify(id));
  assert.equal(profile(), id);
});
test('profile replaces a malformed stored id with a new one', () => {
  for (const stored of ['"XYZ"', JSON.stringify('A'.repeat(32)), JSON.stringify('a'.repeat(33)), '42', 'not json']) {
    storage.set(PREFIX + 'profile', stored);
    const id = profile();
    assert.match(id ?? '', PROFILE_ID);
    assert.equal(storage.get(PREFIX + 'profile'), JSON.stringify(id));
  }
});
test('reads are null and writes fail when browser storage throws', () => {
  // A browser that blocks site data throws a SecurityError on any localStorage access.
  const saved = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')!;
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    get() {
      throw new DOMException('The operation is insecure.', 'SecurityError');
    },
  });
  try {
    assert.equal(write('session', { id: 'p', rows: 'rows', actions: '' }), false);
    assert.equal(session(), null);
    assert.equal(best('p', 'rows'), null);
    assert.equal(profile(), null);
  } finally {
    Object.defineProperty(globalThis, 'localStorage', saved);
  }
});
