import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DEFAULT_PREFERENCES } from '../src/theme.ts';
import * as audio from '../src/audio.ts';

test('the cues and variants name what the pages ask for', () => {
  assert.equal(audio.CUES.length, 8);
  assert.ok(audio.CUES.includes('goal-enter'));
  assert.equal(audio.VARIANTS.length, 9);
  assert.ok(audio.VARIANTS.includes('deadlock'));
  // Node offers no window, so no Web Audio either.
  assert.equal(audio.supportsAudio(), false);
});

test('each direction lends its pitch, clamped to five semitones', () => {
  assert.deepEqual([...audio.DIRECTION_PITCH], [1.5, -1.5, -0.5, 0.5]);
  assert.equal(audio.clampPitchOffset(4), 4);
  assert.equal(audio.clampPitchOffset(-4), -4);
  assert.equal(audio.clampPitchOffset(9), 5);
  assert.equal(audio.clampPitchOffset(-9), -5);
  assert.equal(audio.clampPitchOffset(undefined), 0);
  assert.equal(audio.clampPitchOffset(Number.NaN), 0);
});

test("one move's audio, off the engine's own verdict", () => {
  assert.deepEqual(audio.moveCue('blocked', 2, 0, 0), { cue: 'blocked', pitchOffset: -0.5 });
  assert.deepEqual(audio.moveCue('step', 0, 0, 0), { cue: 'step', pitchOffset: 1.5 });
  assert.deepEqual(audio.moveCue('push', 3, 0, 0), { cue: 'push', pitchOffset: 0.5 });
  // A box landing on its goal reads the room's progress into the pitch.
  assert.deepEqual(audio.moveCue('goal-enter', 1, 1, 4), { cue: 'goal-enter', pitchOffset: -1, variant: 'progress' });
  assert.deepEqual(audio.moveCue('goal-enter', 0, 1, 1), { cue: 'goal-enter', pitchOffset: 1.5, variant: 'progress' });
  assert.deepEqual(audio.moveCue('goal-enter', 0, 2, 4), { cue: 'goal-enter', pitchOffset: 2.5, variant: 'progress' });
  assert.deepEqual(audio.moveCue('goal-leave', 2, 0, 4), { cue: 'goal-leave', pitchOffset: -0.5, variant: 'progress' });
});

test('which solve the celebration plays', () => {
  assert.equal(audio.solveVariant(null, 10, false), 'first-clear');
  assert.equal(audio.solveVariant({ moves: 12 }, 10, false), 'personal-best');
  assert.equal(audio.solveVariant({ moves: 12 }, 12, false), 'default');
  assert.equal(audio.solveVariant({ moves: 12 }, 15, false), 'default');
  // The engine's certification outranks the record.
  assert.equal(audio.solveVariant(null, 10, true), 'optimal');
  assert.equal(audio.solveVariant({ moves: 5 }, 10, true), 'optimal');
});

test('the controller stays quiet without a browser', async () => {
  const controller = new audio.AudioController(DEFAULT_PREFERENCES);
  assert.equal(controller.supported, false);
  assert.equal(controller.ready, false);
  assert.equal(await controller.unlock(), false);
  assert.equal(await controller.playCue('step'), false);
  assert.equal(await controller.previewEffects(), false);
  assert.equal(await controller.previewMusic(), false);
  controller.dispose();
  assert.equal(await controller.playCue('step'), false);
});
