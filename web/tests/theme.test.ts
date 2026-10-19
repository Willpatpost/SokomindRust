import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import { DEFAULT_PREFERENCES, preferences, resolveAppearance, resolveMotion, update } from '../src/theme.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

test('preferences are the defaults when nothing is stored or the record is malformed', () => {
  for (const value of [
    null,
    'experience',
    {},
    { themeFamily: 'cozy-study' },
    { ...DEFAULT_PREFERENCES, appearance: 'noir' },
    { ...DEFAULT_PREFERENCES, motion: null },
    [DEFAULT_PREFERENCES],
  ]) {
    storage.write('experience', value);
    assert.deepEqual(preferences(), DEFAULT_PREFERENCES, JSON.stringify(value));
  }
});

test('update merges each patch over the stored preferences', () => {
  assert.deepEqual(update({ appearance: 'dark' }), { ...DEFAULT_PREFERENCES, appearance: 'dark' });
  assert.deepEqual(update({ themeFamily: 'minimal-ink' }), { ...DEFAULT_PREFERENCES, themeFamily: 'minimal-ink', appearance: 'dark' });
  assert.deepEqual(update({ motion: 'reduced' }), {
    ...DEFAULT_PREFERENCES,
    themeFamily: 'minimal-ink',
    appearance: 'dark',
    motion: 'reduced',
  });
  assert.deepEqual(preferences(), { themeFamily: 'minimal-ink', appearance: 'dark', motion: 'reduced' });
});

test('a system appearance or motion resolves through the OS preference; an explicit choice wins', () => {
  assert.equal(resolveAppearance('system', true), 'dark');
  assert.equal(resolveAppearance('system', false), 'light');
  assert.equal(resolveAppearance('light', true), 'light');
  assert.equal(resolveAppearance('dark', false), 'dark');
  assert.equal(resolveMotion('system', true), 'reduced');
  assert.equal(resolveMotion('system', false), 'full');
  assert.equal(resolveMotion('full', true), 'full');
  assert.equal(resolveMotion('reduced', false), 'reduced');
});
