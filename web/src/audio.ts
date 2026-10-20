/** The page's procedural sound: a tiny dependency-free Web Audio synthesizer.
 * Construction is side-effect free — the AudioContext is allocated only when
 * unlock() or playCue() runs, so importing this module is safe during builds
 * and tests. The cue recipes, the cooldowns that keep a burst of moves reading
 * as motion rather than noise, and the quiet soundscape loop are all the
 * original app's, ported as they were. Pure helpers — the pitch each direction
 * lends its cues, and which solve the celebration plays — sit beside the
 * controller for the tests to read. */
import type { Preferences } from './theme.ts';

/** The cues the pages ask for. */
export const CUES = ['step', 'push', 'goal-enter', 'goal-leave', 'blocked', 'undo', 'reset', 'solve'] as const;
export type Cue = (typeof CUES)[number];

/** The semantic variations a cue may take. */
export const VARIANTS = [
  'default',
  'progress',
  'multi',
  'warning',
  'deadlock',
  'first-clear',
  'personal-best',
  'optimal',
  'preview',
] as const;
export type Variant = (typeof VARIANTS)[number];

export interface CueOptions {
  /** Small transposition used to distinguish direction and goal progress. */
  pitchOffset?: number;
  /** Semantic variation interpreted by the selected cue. */
  variant?: Variant;
  /** Reserved for intentional settings previews. */
  bypassRateLimit?: boolean;
}

/** How long a cue holds its silence after playing. */
const COOLDOWNS: Partial<Record<Cue, number>> = {
  step: 0.028,
  push: 0.045,
  'goal-enter': 0.055,
  'goal-leave': 0.055,
  blocked: 0.09,
  undo: 0.055,
};

/** The soundscape's loop: a slow arpeggio with a bass octave under every
 * fourth note. */
const MUSIC_NOTES = [220, 261.63, 293.66, 329.63, 392, 329.63, 293.66, 261.63] as const;
const MUSIC_STEP_SECONDS = 1.35;
const MUSIC_LOOKAHEAD_SECONDS = 0.35;
const MUSIC_PREVIEW_COOLDOWN_SECONDS = 0.4;

/** A small transposition, clamped to ±5 semitones. */
export function clampPitchOffset(value: number | undefined): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(-5, Math.min(5, value ?? 0));
}

function pitchMultiplier(offset: number): number {
  return 2 ** (offset / 12);
}

/** How much louder a variant reads than its base recipe. */
function variantVolumeMultiplier(variant: Variant): number {
  switch (variant) {
    case 'multi':
      return 0.94;
    case 'warning':
      return 1.12;
    case 'deadlock':
      return 1.2;
    case 'personal-best':
    case 'optimal':
      return 1.08;
    default:
      return 1;
  }
}

type AudioContextConstructor = new () => AudioContext;

function audioContextConstructor(): AudioContextConstructor | undefined {
  if (typeof window === 'undefined') return undefined;
  const extended = window as typeof window & { webkitAudioContext?: AudioContextConstructor };
  return window.AudioContext ?? extended.webkitAudioContext;
}

/** Whether this browser offers Web Audio at all. */
export function supportsAudio(): boolean {
  return audioContextConstructor() !== undefined;
}

interface ToneOptions {
  frequency: number;
  endFrequency?: number;
  start: number;
  duration: number;
  volume: number;
  type?: OscillatorType;
  music?: boolean;
}

/** The Web Audio the controller drives: master, effects and music gains, the
 * cue cooldowns, and the soundscape's scheduler. */
export class AudioController {
  private context: AudioContext | null = null;
  private masterGain: GainNode | null = null;
  private effectsGain: GainNode | null = null;
  private musicGain: GainNode | null = null;
  private preferences: Preferences;
  private visible = true;
  private disposed = false;
  private unlockPromise: Promise<boolean> | null = null;
  private musicTimer: number | null = null;
  private musicStep = 0;
  private nextMusicTime = 0;
  private lastMusicPreviewTime = Number.NEGATIVE_INFINITY;
  private readonly lastCueTimes = new Map<Cue, number>();
  private readonly musicSources = new Set<OscillatorNode>();

  constructor(preferences: Preferences) {
    this.preferences = preferences;
  }

  get supported(): boolean {
    return supportsAudio();
  }

  get ready(): boolean {
    return this.context?.state === 'running';
  }

  /** Follows the menu's choices, mixing live. */
  setPreferences(preferences: Preferences): void {
    this.preferences = preferences;
    this.applyMix();
    this.syncMusic();
  }

  /** A hidden page goes quiet, whatever the menu says. */
  setPageVisible(visible: boolean): void {
    this.visible = visible;
    this.applyMix();
    this.syncMusic();
  }

  /** Call from a click, pointer, or keyboard handler before relying on sound:
   * browsers may reject resume() outside a user activation. */
  unlock(): Promise<boolean> {
    if (this.disposed || !this.supported) return Promise.resolve(false);
    if (this.context?.state === 'running') {
      this.syncMusic();
      return Promise.resolve(true);
    }
    if (this.unlockPromise) return this.unlockPromise;
    this.unlockPromise = this.createOrResumeContext()
      .catch(() => false)
      .finally(() => {
        this.unlockPromise = null;
      });
    return this.unlockPromise;
  }

  /** Plays one cue under its options; false when muted, hidden, locked by the
   * browser, or cooling down. */
  async playCue(cue: Cue, options: CueOptions = {}): Promise<boolean> {
    if (this.disposed || !this.preferences.soundEnabled || this.preferences.effectsVolume <= 0 || !this.visible) return false;
    if (!(await this.unlock())) return false;
    const context = this.context;
    if (!context || context.state !== 'running') return false;
    if (!this.canPresentCue(cue, context.currentTime, options)) return false;

    const now = context.currentTime + 0.006;
    const variant = options.variant ?? 'default';
    const pitch = pitchMultiplier(clampPitchOffset(options.pitchOffset));
    const volume = variantVolumeMultiplier(variant);
    const tone = (options: ToneOptions) =>
      this.tone({
        ...options,
        frequency: options.frequency * pitch,
        ...(options.endFrequency === undefined ? {} : { endFrequency: options.endFrequency * pitch }),
        volume: options.volume * volume,
      });

    try {
      switch (cue) {
        case 'step':
          tone({ frequency: 145, endFrequency: 112, start: now, duration: 0.045, volume: 0.055, type: 'triangle' });
          break;
        case 'push':
          tone({ frequency: 112, endFrequency: 66, start: now, duration: 0.13, volume: 0.14, type: 'sine' });
          tone({ frequency: 218, endFrequency: 154, start: now, duration: 0.07, volume: 0.035, type: 'triangle' });
          break;
        case 'goal-enter':
          tone({ frequency: 392, start: now, duration: 0.16, volume: 0.075, type: 'sine' });
          tone({ frequency: 587.33, start: now + 0.075, duration: 0.22, volume: 0.065, type: 'sine' });
          break;
        case 'goal-leave':
          tone({ frequency: 392, endFrequency: 293.66, start: now, duration: 0.16, volume: 0.055, type: 'sine' });
          break;
        case 'blocked':
          tone({ frequency: 92, endFrequency: 74, start: now, duration: 0.075, volume: 0.045, type: 'square' });
          if (variant === 'warning' || variant === 'deadlock')
            tone({
              frequency: variant === 'deadlock' ? 61.74 : 123.47,
              endFrequency: variant === 'deadlock' ? 49 : 98,
              start: now + 0.052,
              duration: variant === 'deadlock' ? 0.14 : 0.09,
              volume: variant === 'deadlock' ? 0.045 : 0.028,
              type: 'triangle',
            });
          break;
        case 'undo':
          tone({ frequency: 329.63, endFrequency: 246.94, start: now, duration: 0.15, volume: 0.055, type: 'triangle' });
          break;
        case 'reset':
          tone({ frequency: 196, endFrequency: 110, start: now, duration: 0.22, volume: 0.065, type: 'sine' });
          break;
        case 'solve':
          (variant === 'optimal'
            ? [261.63, 329.63, 392, 523.25, 659.25, 783.99]
            : variant === 'personal-best'
              ? [293.66, 369.99, 440, 587.33, 659.25]
              : variant === 'first-clear'
                ? [261.63, 329.63, 392, 523.25, 659.25]
                : [261.63, 329.63, 392, 523.25]
          ).forEach((frequency, index, notes) => {
            tone({
              frequency,
              start: now + index * 0.085,
              duration: 0.42,
              volume: index === notes.length - 1 ? 0.07 : 0.052,
              type: index % 2 === 0 ? 'sine' : 'triangle',
            });
          });
          break;
      }
    } catch {
      return false;
    }

    this.lastCueTimes.set(cue, context.currentTime);
    return true;
  }

  /** One goal-enter at the current volume, from the menu's Preview button. */
  async previewEffects(): Promise<boolean> {
    return this.playCue('goal-enter', { variant: 'preview', bypassRateLimit: true });
  }

  /** A chord of the soundscape, from the menu's Preview button. */
  async previewMusic(): Promise<boolean> {
    if (
      this.disposed
      || !this.visible
      || !this.preferences.soundEnabled
      || !this.preferences.musicEnabled
      || this.preferences.musicVolume <= 0
    )
      return false;
    if (!(await this.unlock())) return false;
    const context = this.context;
    if (!context || context.state !== 'running') return false;
    if (context.currentTime - this.lastMusicPreviewTime < MUSIC_PREVIEW_COOLDOWN_SECONDS) return false;

    const now = context.currentTime + 0.006;
    try {
      [261.63, 329.63, 392].forEach((frequency, index) => {
        this.tone({
          frequency,
          start: now + index * 0.11,
          duration: 0.48,
          volume: index === 2 ? 0.065 : 0.05,
          type: index % 2 === 0 ? 'sine' : 'triangle',
          music: true,
        });
      });
    } catch {
      return false;
    }

    this.lastMusicPreviewTime = context.currentTime;
    return true;
  }

  /** Closes the context and stops everything. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.stopMusic();
    const context = this.context;
    this.context = null;
    this.masterGain = null;
    this.effectsGain = null;
    this.musicGain = null;
    if (context && context.state !== 'closed') void context.close().catch(() => undefined);
  }

  private async createOrResumeContext(): Promise<boolean> {
    if (!this.context) {
      const AudioContextClass = audioContextConstructor();
      if (!AudioContextClass) return false;
      const context = new AudioContextClass();
      const masterGain = context.createGain();
      const effectsGain = context.createGain();
      const musicGain = context.createGain();
      effectsGain.connect(masterGain);
      musicGain.connect(masterGain);
      masterGain.connect(context.destination);
      this.context = context;
      this.masterGain = masterGain;
      this.effectsGain = effectsGain;
      this.musicGain = musicGain;
      this.applyMix(true);
    }
    if (this.context.state === 'suspended')
      await Promise.race([this.context.resume(), new Promise<void>(resolve => setTimeout(resolve, 500))]);
    const ready = this.context.state === 'running';
    if (ready) this.syncMusic();
    return ready;
  }

  private canPresentCue(cue: Cue, now: number, options: CueOptions): boolean {
    if (options.bypassRateLimit) return true;
    const cooldown = COOLDOWNS[cue];
    if (cooldown === undefined) return true;
    const previous = this.lastCueTimes.get(cue);
    return previous === undefined || now - previous >= cooldown;
  }

  private applyMix(immediate = false): void {
    const context = this.context;
    if (!context) return;
    const master = this.preferences.soundEnabled && this.visible ? 0.9 : 0;
    const music = this.preferences.soundEnabled && this.preferences.musicEnabled && this.visible ? this.preferences.musicVolume : 0;
    this.setGain(this.masterGain, master, immediate);
    this.setGain(this.effectsGain, this.preferences.effectsVolume, immediate);
    this.setGain(this.musicGain, music, immediate);
  }

  private setGain(node: GainNode | null, value: number, immediate: boolean): void {
    if (!node || !this.context) return;
    const now = this.context.currentTime;
    node.gain.cancelScheduledValues(now);
    if (immediate) node.gain.setValueAtTime(value, now);
    else node.gain.setTargetAtTime(value, now, 0.025);
  }

  /** One note: an oscillator with a quick attack and decay, sliding toward its
   * end frequency when it has one. */
  private tone(options: ToneOptions): OscillatorNode | null {
    const context = this.context;
    const output = options.music ? this.musicGain : this.effectsGain;
    if (!context || !output || context.state !== 'running') return null;
    const oscillator = context.createOscillator();
    const envelope = context.createGain();
    const attack = Math.min(0.025, options.duration * 0.22);
    const end = options.start + options.duration;
    const peak = Math.max(0.0001, options.volume);
    oscillator.type = options.type ?? 'sine';
    oscillator.frequency.setValueAtTime(options.frequency, options.start);
    if (options.endFrequency !== undefined && options.endFrequency !== options.frequency)
      oscillator.frequency.exponentialRampToValueAtTime(Math.max(1, options.endFrequency), end);
    envelope.gain.setValueAtTime(0.0001, options.start);
    envelope.gain.exponentialRampToValueAtTime(peak, options.start + attack);
    envelope.gain.exponentialRampToValueAtTime(0.0001, end);
    oscillator.connect(envelope);
    envelope.connect(output);
    oscillator.start(options.start);
    oscillator.stop(end + 0.01);
    if (options.music) this.musicSources.add(oscillator);
    oscillator.addEventListener(
      'ended',
      () => {
        this.musicSources.delete(oscillator);
        oscillator.disconnect();
        envelope.disconnect();
      },
      { once: true },
    );
    return oscillator;
  }

  private syncMusic(): void {
    const shouldPlay =
      !this.disposed
      && this.visible
      && this.preferences.soundEnabled
      && this.preferences.musicEnabled
      && this.preferences.musicVolume > 0
      && this.context?.state === 'running';
    if (shouldPlay) this.startMusic();
    else this.stopMusic();
  }

  private startMusic(): void {
    if (this.musicTimer !== null || !this.context) return;
    this.nextMusicTime = this.context.currentTime + 0.08;
    this.scheduleMusic();
    this.musicTimer = window.setInterval(() => this.scheduleMusic(), 100);
  }

  /** Keeps a third of a second of the loop scheduled ahead of the clock. */
  private scheduleMusic(): void {
    const context = this.context;
    if (!context || context.state !== 'running') return;
    const horizon = context.currentTime + MUSIC_LOOKAHEAD_SECONDS;
    while (this.nextMusicTime < horizon) {
      const frequency = MUSIC_NOTES[this.musicStep % MUSIC_NOTES.length];
      this.tone({
        frequency,
        start: this.nextMusicTime,
        duration: 1.55,
        volume: 0.07,
        type: this.musicStep % 3 === 0 ? 'sine' : 'triangle',
        music: true,
      });
      if (this.musicStep % 4 === 0)
        this.tone({ frequency: frequency / 2, start: this.nextMusicTime, duration: 2.2, volume: 0.04, type: 'sine', music: true });
      this.musicStep = (this.musicStep + 1) % MUSIC_NOTES.length;
      this.nextMusicTime += MUSIC_STEP_SECONDS;
    }
  }

  private stopMusic(): void {
    if (this.musicTimer !== null && typeof window !== 'undefined') {
      window.clearInterval(this.musicTimer);
      this.musicTimer = null;
    }
    for (const source of this.musicSources) {
      try {
        source.stop();
      } catch {
        /* An already-ended oscillator is harmless. */
      }
    }
    this.musicSources.clear();
    this.nextMusicTime = 0;
  }
}

/** The pitch each direction lends its cues, indexed as ACTIONS is: up, down,
 * left, right. Up and down read an octave-ish apart; left and right a step. */
export const DIRECTION_PITCH = [1.5, -1.5, -0.5, 0.5] as const;

/** How far into the room the goal progress has come, as pitch. */
function progressPitch(matched: number, total: number): number {
  if (total <= 1) return 0;
  return (matched / total) * 2;
}

/** One move's audio, off the engine's own verdict: blocked, a step, a push, or
 * a push that changed how many boxes stand on their goals. */
export type MoveKind = 'blocked' | 'step' | 'push' | 'goal-enter' | 'goal-leave';

export interface Presentation {
  cue: Cue;
  pitchOffset: number;
  variant?: Variant;
}

export function moveCue(kind: MoveKind, direction: number, matchedGoals: number, totalGoals: number): Presentation {
  const pitchOffset = DIRECTION_PITCH[direction] ?? 0;
  switch (kind) {
    case 'blocked':
      return { cue: 'blocked', pitchOffset };
    case 'step':
      return { cue: 'step', pitchOffset };
    case 'push':
      return { cue: 'push', pitchOffset };
    case 'goal-enter':
      return { cue: 'goal-enter', pitchOffset: pitchOffset + progressPitch(matchedGoals, totalGoals), variant: 'progress' };
    case 'goal-leave':
      return { cue: 'goal-leave', pitchOffset, variant: 'progress' };
  }
}

/** Which solve the celebration plays: the engine's certification outranks the
 * record, and a first clear outranks a personal best. */
export function solveVariant(previousBest: { moves: number } | null, moves: number, optimal: boolean): Variant {
  if (optimal) return 'optimal';
  if (previousBest === null) return 'first-clear';
  if (moves < previousBest.moves) return 'personal-best';
  return 'default';
}
