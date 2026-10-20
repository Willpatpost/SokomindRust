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
  assert.deepEqual(preferences(), { ...DEFAULT_PREFERENCES, themeFamily: 'minimal-ink', appearance: 'dark', motion: 'reduced' });
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

test('the sound choices ride the theme record', () => {
  // A record the pre-audio app stored keeps its theme and takes the sound defaults.
  storage.write('experience', { themeFamily: 'minimal-ink', appearance: 'dark', motion: 'reduced' });
  assert.deepEqual(preferences(), { ...DEFAULT_PREFERENCES, themeFamily: 'minimal-ink', appearance: 'dark', motion: 'reduced' });
  // Malformed sound fields fall back to the defaults; the theme survives.
  storage.write('experience', {
    themeFamily: 'minimal-ink',
    appearance: 'dark',
    motion: 'reduced',
    effectsVolume: 'loud',
    soundEnabled: 'yes',
  });
  assert.deepEqual(preferences(), { ...DEFAULT_PREFERENCES, themeFamily: 'minimal-ink', appearance: 'dark', motion: 'reduced' });
  // A volume outside its range clamps into it.
  storage.write('experience', { ...DEFAULT_PREFERENCES, musicVolume: 7 });
  assert.equal(preferences().musicVolume, 1);
  storage.write('experience', { ...DEFAULT_PREFERENCES, effectsVolume: -3 });
  assert.equal(preferences().effectsVolume, 0);
  // Explicit choices round-trip over the defaults.
  storage.write('experience', DEFAULT_PREFERENCES);
  update({ soundEnabled: false, musicVolume: 0.25 });
  assert.deepEqual(preferences(), { ...DEFAULT_PREFERENCES, soundEnabled: false, musicVolume: 0.25 });
});
