import * as storage from './storage.ts';

/** The theme families web/src/styles/tokens.css defines, as html[data-theme-family] values. */
export const THEME_FAMILIES = ['cozy-study', 'midnight-neon', 'minimal-ink'] as const;
export type ThemeFamily = (typeof THEME_FAMILIES)[number];
/** Light or dark, or whichever the OS prefers; the html[data-theme] value. */
export const APPEARANCES = ['system', 'light', 'dark'] as const;
export type Appearance = (typeof APPEARANCES)[number];
/** Full or reduced animation, or whichever the OS prefers; the html[data-motion] value. */
export const MOTIONS = ['system', 'full', 'reduced'] as const;
export type Motion = (typeof MOTIONS)[number];

/** The theme preferences, stored as one record under the app's storage prefix,
 * with the sound choices the audio phase landed. */
export interface Preferences {
  themeFamily: ThemeFamily;
  appearance: Appearance;
  motion: Motion;
  /** The master switch; the music and effects choices survive a mute. */
  soundEnabled: boolean;
  /** The quiet procedural soundscape's own switch. */
  musicEnabled: boolean;
  /** Effect loudness, 0 to 1. */
  effectsVolume: number;
  /** Music loudness, 0 to 1. */
  musicVolume: number;
}

/** What a visitor with no stored choice gets: the default family, following the
 * OS, with sound on at half volume. */
export const DEFAULT_PREFERENCES: Preferences = {
  themeFamily: 'cozy-study',
  appearance: 'system',
  motion: 'system',
  soundEnabled: true,
  musicEnabled: true,
  effectsVolume: 0.5,
  musicVolume: 0.5,
};

/** The storage key the preferences live under; the boot script in index.html
 * reads the same key before this module loads (checked by scripts/mirrors.test.mjs). */
export const EXPERIENCE_KEY = 'experience';

function member<T extends string>(list: readonly T[], value: unknown): value is T {
  return typeof value === 'string' && list.includes(value as T);
}

/** A volume as stored, clamped to its range, or the fallback when absent or malformed. */
function volume(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : fallback;
}

/** A value as preferences, or null unless the theme fields are one of the
 * module's options. A record that fails is discarded whole, so a partly-written
 * theme never applies; the sound fields read leniently, so a record the app
 * stored before the audio phase keeps its theme and takes the sound defaults. */
function parse(value: unknown): Preferences | null {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Partial<Preferences>;
  if (!member(THEME_FAMILIES, candidate.themeFamily) || !member(APPEARANCES, candidate.appearance) || !member(MOTIONS, candidate.motion)) {
    return null;
  }
  return {
    themeFamily: candidate.themeFamily,
    appearance: candidate.appearance,
    motion: candidate.motion,
    soundEnabled: candidate.soundEnabled !== false,
    musicEnabled: candidate.musicEnabled !== false,
    effectsVolume: volume(candidate.effectsVolume, DEFAULT_PREFERENCES.effectsVolume),
    musicVolume: volume(candidate.musicVolume, DEFAULT_PREFERENCES.musicVolume),
  };
}

/** The stored preferences, or the defaults when none are stored or the record is malformed. */
export function preferences(): Preferences {
  return parse(storage.read(EXPERIENCE_KEY)) ?? DEFAULT_PREFERENCES;
}

/** Stores `patch` merged over the stored preferences and returns what is now stored.
 * The merge of two valid records is valid by construction, so no re-validation
 * runs; a storage failure leaves the stored record unchanged and still returns
 * the merged value, which the caller may apply for this visit. */
export function update(patch: Partial<Preferences>): Preferences {
  const next: Preferences = { ...preferences(), ...patch };
  storage.write(EXPERIENCE_KEY, next);
  return next;
}

/** The appearance shown: a `system` choice resolves through the OS's dark preference. */
export function resolveAppearance(preference: Appearance, prefersDark: boolean): 'light' | 'dark' {
  if (preference === 'light' || preference === 'dark') return preference;
  return prefersDark ? 'dark' : 'light';
}

/** The motion shown: a `system` choice resolves through the OS's reduced-motion preference. */
export function resolveMotion(preference: Motion, prefersReduced: boolean): 'full' | 'reduced' {
  if (preference === 'full' || preference === 'reduced') return preference;
  return prefersReduced ? 'reduced' : 'full';
}

/** What the OS currently prefers, read fresh; `start` re-reads it on every change. */
interface System {
  prefersDark: boolean;
  prefersReduced: boolean;
}
function system(): System {
  return {
    prefersDark: matchMedia('(prefers-color-scheme: dark)').matches,
    prefersReduced: matchMedia('(prefers-reduced-motion: reduce)').matches,
  };
}

/** The theme-color meta's content for each family and appearance: the base color
 * of the family's --app-background gradient in tokens.css, which a mirrors test
 * quotes back to that sheet. */
const META_COLORS: Record<ThemeFamily, Record<'light' | 'dark', string>> = {
  'cozy-study': { light: '#eee8da', dark: '#222420' },
  'midnight-neon': { light: '#e9edfb', dark: '#10152a' },
  'minimal-ink': { light: '#f6f6f3', dark: '#111212' },
};

/** Writes the resolved theme onto the document: the three html attributes the
 * token sheet selects on, and the theme-color meta. The boot script in
 * index.html applies the same attributes before first paint; a mirrors test
 * keeps the two in step. */
export function apply(prefs: Preferences, os: System): void {
  const root = document.documentElement;
  root.dataset.themeFamily = prefs.themeFamily;
  const appearance = resolveAppearance(prefs.appearance, os.prefersDark);
  root.dataset.theme = appearance;
  root.dataset.motion = resolveMotion(prefs.motion, os.prefersReduced);
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', META_COLORS[prefs.themeFamily][appearance]);
}

/** Stores and applies `patch` in one step; the controls in main.ts call this. */
export function set(patch: Partial<Preferences>): void {
  apply(update(patch), system());
}

/** Applies the stored preferences and follows the OS and other tabs: the OS
 * media queries re-resolve a `system` choice when it changes, and a storage
 * event re-reads preferences another tab stored. Called once, after start. */
export function start(): void {
  apply(preferences(), system());
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => apply(preferences(), system()));
  matchMedia('(prefers-reduced-motion: reduce)').addEventListener('change', () => apply(preferences(), system()));
  addEventListener('storage', event => {
    if (event.key === storage.key(EXPERIENCE_KEY)) apply(preferences(), system());
  });
}
