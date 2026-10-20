/** The sound menu: the theme menu's choices over the synthesizer the play core
 * owns; the step wrapper every move's cue rides lives there with it. */
import { $, button } from './dom.ts';
import * as theme from './theme.ts';
import * as storage from './storage.ts';
import { sound } from './play.ts';

/** Binds the sound choices into the theme menu, keeps the controller following
 * the stored record — this tab's menu and another tab's — and wakes the
 * browser's audio on the first gesture. */
function bindAudio() {
  const master = $<HTMLInputElement>('sound-enabled');
  const music = $<HTMLInputElement>('music-enabled');
  const effects = $<HTMLInputElement>('effects-volume');
  const effectsOut = $<HTMLOutputElement>('effects-volume-out');
  const musicVolume = $<HTMLInputElement>('music-volume');
  const musicOut = $<HTMLOutputElement>('music-volume-out');
  const note = $('audio-note');
  const sync = () => {
    const prefs = theme.preferences();
    sound.setPreferences(prefs);
    master.checked = prefs.soundEnabled;
    music.checked = prefs.musicEnabled;
    music.disabled = !prefs.soundEnabled;
    effects.value = String(prefs.effectsVolume);
    musicVolume.value = String(prefs.musicVolume);
    effectsOut.textContent = `${Math.round(prefs.effectsVolume * 100)}%`;
    musicOut.textContent = `${Math.round(prefs.musicVolume * 100)}%`;
  };
  master.addEventListener('change', () => {
    theme.set({ soundEnabled: master.checked });
    sync();
  });
  music.addEventListener('change', () => {
    theme.set({ musicEnabled: music.checked });
    sync();
  });
  effects.addEventListener('input', () => {
    theme.set({ effectsVolume: Number(effects.value) });
    sync();
  });
  musicVolume.addEventListener('input', () => {
    theme.set({ musicVolume: Number(musicVolume.value) });
    sync();
  });
  button('preview-effects').onclick = () => {
    void sound.previewEffects().then(played => {
      note.textContent = played ? 'Effects preview played.' : 'Effects preview could not play.';
    });
  };
  button('preview-music').onclick = () => {
    void sound.previewMusic().then(played => {
      note.textContent = played ? 'Music preview played.' : 'Music preview could not play.';
    });
  };
  sync();
  // Another tab's menu writes the same record.
  addEventListener('storage', event => {
    if (event.key === storage.key(theme.EXPERIENCE_KEY)) sync();
  });
  // The browser needs a gesture before sound; the first one anywhere wakes it.
  const wake = () => void sound.unlock();
  document.addEventListener('pointerdown', wake, { once: true, capture: true });
  document.addEventListener('keydown', wake, { once: true, capture: true });
  // A hidden page goes quiet, whatever the menu says.
  document.addEventListener('visibilitychange', () => sound.setPageVisible(document.visibilityState !== 'hidden'));
  // M mutes or restores, anywhere a dialog is not open and a field is not
  // taking the key.
  document.addEventListener('keydown', event => {
    if (event.key.toLowerCase() !== 'm' || event.ctrlKey || event.metaKey || event.altKey) return;
    if (document.querySelector('dialog[open]')) return;
    if (event.target instanceof Element && event.target.matches('input,textarea,select')) return;
    theme.set({ soundEnabled: !theme.preferences().soundEnabled });
    sync();
    note.textContent = theme.preferences().soundEnabled ? 'Audio restored.' : 'Audio muted.';
  });
}

export { bindAudio };
