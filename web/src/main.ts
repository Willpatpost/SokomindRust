import './styles/tokens.css';
import './style.css';
import init from '../wasm/sokomind';
import wasmUrl from '../wasm/sokomind_bg.wasm?url';
import { bindInput, ignoresKey } from './input.ts';
import { ENTRIES, filter as filterEntries, rate } from './catalog.ts';
import * as storage from './storage.ts';
import * as theme from './theme.ts';
import { navigate, route, startRouter, subscribe } from './router.ts';
import { onPage, startShell } from './shell.ts';
import * as frames from './frames.ts';
import { closeDialog, openDialog } from './dialog.ts';
import { errorMessage } from './protocol.ts';
import { decodeHealth } from './transport.ts';
import { $, button, choice, message, select, setText } from './dom.ts';
import {
  animate,
  askHint,
  board,
  changed,
  CUSTOM_PUZZLE_ID,
  customPuzzle,
  engineArrived,
  engineReady,
  enterPlay,
  hint,
  invalidate,
  load,
  loaded,
  markRating,
  move,
  paused,
  playback,
  progress,
  RATE_BUTTONS,
  render,
  saveSession,
  session,
  share,
  solve,
  solver,
  sound,
  tap,
  timer,
  togglePause,
  toggleZen,
  updateButtons,
  zoom,
} from './play.ts';
import { registerServiceWorker } from './offline-shell.ts';
import { bindAudio } from './audio-page.ts';
import { bindEditor, flushEditorSave, leaveEditor, renderEditor } from './editor-page.ts';
import { bindGenerator } from './generator-page.ts';
import { bindLab, renderLab } from './lab-page.ts';
import { bindStudy, openStudy } from './replay-study.ts';
import { renderStats } from './stats-page.ts';
import { syncHome } from './home-page.ts';
import { renderPuzzles } from './puzzles-pages.ts';

/** The theme menu's labels; the values are theme.ts's lists. */
const FAMILY_LABELS: Record<theme.ThemeFamily, string> = {
  'cozy-study': 'Cozy study',
  'midnight-neon': 'Midnight neon',
  'minimal-ink': 'Minimal ink',
};
const APPEARANCE_LABELS: Record<theme.Appearance, string> = { system: 'System', light: 'Light', dark: 'Dark' };
const MOTION_LABELS: Record<theme.Motion, string> = { system: 'System', full: 'Full', reduced: 'Reduced' };

/** Shows the stored preferences in the theme menu's selects. */
function syncThemeMenu() {
  const current = theme.preferences();
  select('theme-family').value = current.themeFamily;
  select('appearance').value = current.appearance;
  select('motion').value = current.motion;
}

/** Populates the theme menu from theme.ts's lists and applies every change; a
 * preference another tab stores arrives as a storage event, which re-syncs. */
function bindThemeMenu() {
  const family = select('theme-family');
  for (const value of theme.THEME_FAMILIES) family.add(new Option(FAMILY_LABELS[value], value));
  const appearance = select('appearance');
  for (const value of theme.APPEARANCES) appearance.add(new Option(APPEARANCE_LABELS[value], value));
  const motion = select('motion');
  for (const value of theme.MOTIONS) motion.add(new Option(MOTION_LABELS[value], value));
  syncThemeMenu();
  family.onchange = () => {
    theme.set({ themeFamily: choice('theme-family', theme.THEME_FAMILIES) });
    // The family decides which frames fit.
    frames.apply();
  };
  appearance.onchange = () => theme.set({ appearance: choice('appearance', theme.APPEARANCES) });
  motion.onchange = () => theme.set({ motion: choice('motion', theme.MOTIONS) });
  addEventListener('storage', event => {
    if (event.key === storage.key(theme.EXPERIENCE_KEY)) syncThemeMenu();
  });
  // A menu closes when a click lands anywhere else: the theme menu is a
  // details element, which otherwise stays open until its own summary is
  // clicked again.
  document.addEventListener('click', event => {
    const menu = document.querySelector('details.theme-menu[open]');
    if (menu && !(event.target instanceof Node && menu.contains(event.target))) menu.removeAttribute('open');
  });
}

async function start() {
  theme.start();
  bindThemeMenu();
  // The board wears whatever frame the journal has earned.
  frames.apply();
  const mode = select('mode');
  const syncModeHelp = () => setText('mode-help', mode.selectedOptions[0]?.dataset.help ?? '');
  mode.onchange = syncModeHelp;
  // The browser can restore the select's value on reload; match the help text.
  syncModeHelp();
  // The shell shows the first route before the engine is ready: a play route loads
  // once WebAssembly is, and every later one through the router.
  onPage('play', next => {
    if (engineReady) enterPlay(next);
  });
  onPage('home', syncHome);
  onPage('puzzles', renderPuzzles);
  onPage('stats', () => renderStats(openStudy));
  onPage('solver-lab', renderLab);
  onPage('editor', renderEditor);
  // The lab's, the editor's, the generator's, the sound's and the study's
  // controls bind before the router reports its first route, so a page that
  // opens cold is interactive at once.
  bindLab();
  bindEditor();
  bindGenerator();
  bindAudio();
  bindStudy();
  // Leaving the play page stops its search and playback; the session keeps
  // saving. Leaving the editor flushes its autosave and ends any playtest.
  subscribe(next => {
    if (next.kind !== 'play') invalidate();
    if (next.kind !== 'editor') leaveEditor();
  });
  startShell();
  startRouter();
  // The offline shell registers once the page has loaded — called before the
  // engine loads, so the load listener never races the wasm fetch; dev
  // builds skip it.
  registerServiceWorker();
  await init({ module_or_path: wasmUrl });
  engineArrived();
  if (route().kind === 'play') enterPlay(route());
  // The editor already renders without the engine; this pass brings its
  // readiness verdict, which only the engine can give.
  if (route().kind === 'editor') renderEditor(route());
  const selector = select('puzzles');
  for (const puzzle of ENTRIES) selector.add(new Option(`${puzzle.title} · ${puzzle.difficulty}`, puzzle.id));
  selector.add(new Option('Custom puzzle', CUSTOM_PUZZLE_ID));
  selector.onchange = () => {
    if (selector.value === CUSTOM_PUZZLE_ID) {
      selector.value = session?.puzzle.id ?? ENTRIES[0].id;
      $<HTMLTextAreaElement>('rows').closest('details')!.open = true;
      return;
    }
    const puzzle = ENTRIES.find(p => p.id === selector.value);
    if (puzzle) navigate({ kind: 'play', puzzleId: puzzle.id });
    // Arrow keys must move the robot, not walk the dropdown after a change.
    selector.blur();
  };
  const step = (delta: number) => {
    const id = session?.puzzle.id ?? ENTRIES[0].id;
    const index = ENTRIES.findIndex(p => p.id === id);
    navigate({ kind: 'play', puzzleId: ENTRIES[(index + delta + ENTRIES.length) % ENTRIES.length].id });
  };
  button('prev').onclick = () => step(-1);
  button('next').onclick = () => step(1);
  button('undo').onclick = () => {
    invalidate();
    if (loaded().game.undo()) {
      void sound.playCue('undo');
      changed();
    }
  };
  button('undo-5').onclick = () => {
    invalidate();
    const { game } = loaded();
    for (let i = 0; i < 5 && game.moves() > 0; i++) game.undo();
    void sound.playCue('undo');
    changed();
  };
  button('copy-moves').onclick = async () => {
    try {
      await navigator.clipboard.writeText(loaded().game.actions());
      message('Move sequence copied as U/D/L/R.');
    } catch {
      message('Clipboard is unavailable in this browser.');
    }
  };
  const performReset = () => {
    invalidate();
    loaded().game.reset();
    // A restart is a fresh attempt: its clock starts over on the next move.
    timer.reset(loaded().puzzle.id);
    void sound.playCue('reset');
    changed();
  };
  // The ported guard: a restart with moves on the board asks first, since it
  // throws the attempt away; an empty board restarts at once.
  const requestReset = () => {
    const state = session?.state;
    if (!state || state.moves === 0) {
      performReset();
      return;
    }
    setText(
      'reset-message',
      `Restarting removes ${state.moves} ${state.moves === 1 ? 'move' : 'moves'} in this attempt. Your best route is not affected.`,
    );
    openDialog('reset-confirm');
  };
  button('reset').onclick = requestReset;
  button('reset-keep').onclick = () => closeDialog('reset-confirm');
  button('reset-now').onclick = () => {
    closeDialog('reset-confirm');
    performReset();
  };
  button('best').onclick = () => {
    const best = progress.best();
    if (!best) return;
    invalidate();
    loaded().game.reset();
    render();
    animate(best.route);
  };
  // The play page's own quick load: an engine-verified paste, and the source of the
  // custom-puzzle share flow — the editor's import dialog serves compose-and-iterate.
  button('load-custom').onclick = () => {
    try {
      load(
        customPuzzle(
          $<HTMLTextAreaElement>('rows')
            .value.replace(/\r/g, '')
            .replace(/^\n|\n$/g, ''),
        ),
      );
    } catch (error) {
      message(errorMessage(error));
    }
  };
  button('load-route').onclick = () => {
    if (paused) return;
    const actions = $<HTMLTextAreaElement>('route-input').value.toUpperCase().replace(/\s/g, '');
    if (!actions) {
      message('Route is empty.');
      return;
    }
    try {
      // Rust validates the whole replay atomically before replacing the current state.
      loaded().game.replay(actions);
      invalidate();
      changed();
    } catch (error) {
      message(errorMessage(error));
    }
  };
  bindInput($('board'), {
    // The keys are document-wide, so they drive only the page they belong to:
    // on the editor route its grid and playtest own them.
    move: direction => {
      if (route().kind === 'play') move(direction);
    },
    // A click does nothing while the Undo button is disabled.
    undo: () => {
      if (route().kind === 'play') button('undo').click();
    },
    swipeable: () => board.fits && !zoom.zoomed,
    tap,
  });
  button('exit-zen').onclick = () => toggleZen();
  button('complete-replay').onclick = () => {
    closeDialog('complete');
    button('best').click();
  };
  button('complete-next').onclick = () => {
    closeDialog('complete');
    step(1);
  };
  button('complete-close').onclick = () => closeDialog('complete');
  button('deadlock-undo').onclick = () => {
    closeDialog('deadlock');
    button('undo').click();
  };
  button('deadlock-restart').onclick = () => {
    closeDialog('deadlock');
    // The deadlock dialog already asked; no second confirmation.
    performReset();
  };
  button('deadlock-keep').onclick = () => closeDialog('deadlock');
  // The search laboratory: the play page's docked solver panel, and its door to the lab.
  button('open-solver').onclick = () => {
    // A hint asked before the panel opened would play its nudge beside it.
    hint.reset();
    $('solver').hidden = !$('solver').hidden;
  };
  button('solver-close').onclick = () => {
    $('solver').hidden = true;
  };
  button('solver-lab-link').onclick = () => {
    $('solver').hidden = true;
    const actions = session?.game.actions() ?? '';
    navigate(
      actions === ''
        ? { kind: 'solver-lab', puzzleId: loaded().puzzle.id }
        : { kind: 'solver-lab', puzzleId: loaded().puzzle.id, actionLog: actions },
    );
  };
  button('hint').onclick = askHint;
  button('share').onclick = () => void share();
  button('random').onclick = () => {
    const pool = filterEntries(ENTRIES, { status: 'open' });
    const from = pool.length === 0 ? ENTRIES : pool;
    navigate({ kind: 'play', puzzleId: from[Math.floor(Math.random() * from.length)]!.id });
  };
  for (const [id, value] of RATE_BUTTONS) {
    button(id).onclick = () => {
      rate(loaded().puzzle.id, value);
      markRating();
      message('Rating saved.');
    };
  }
  /** Walks to the next puzzle with no stored best, wrapping the ENTRIES. */
  const stepToUnsolved = () => {
    const from = session?.puzzle.id ?? ENTRIES[0].id;
    const index = ENTRIES.findIndex(p => p.id === from);
    for (let ahead = 1; ahead <= ENTRIES.length; ahead++) {
      const candidate = ENTRIES[(index + ahead) % ENTRIES.length]!;
      if (storage.best(candidate.id, candidate.rows.join('\n')) === null) {
        navigate({ kind: 'play', puzzleId: candidate.id });
        return;
      }
    }
    navigate({ kind: 'play', puzzleId: ENTRIES[(index + 1) % ENTRIES.length]!.id });
  };
  // The page-level keys the movement layer does not own. While a dialog is up,
  // its keys are the dialog's.
  document.addEventListener('keydown', event => {
    if (document.querySelector('dialog[open]')) return;
    if (route().kind !== 'play') return;
    if (ignoresKey(event, event.target instanceof Element && event.target.matches('input,textarea,select'))) return;
    switch (event.key.toLowerCase()) {
      // The plain Z is zen; its chord undoes in the movement layer.
      case 'z':
        if (!event.ctrlKey && !event.metaKey) toggleZen();
        break;
      case 'h':
        askHint();
        break;
      case 'r':
        if (!paused) button('reset').click();
        break;
      case 'p':
        togglePause();
        break;
      case '[':
        step(-1);
        break;
      case ']':
        step(1);
        break;
      case 'n':
        stepToUnsolved();
        break;
      case '?':
        openDialog('shortcuts');
        break;
    }
  });
  button('solve').onclick = solve;
  button('cancel').onclick = () => {
    if (playback.active) {
      playback.end();
      message('Playback stopped.');
    } else solver.cancel();
  };
  button('play').onclick = () => {
    playback.toggle(solver.route);
    updateButtons();
    // The route plays on the board: center it in view before the first step.
    // 'instant' is deliberate: 'auto' would inherit the page's smooth
    // scrolling, which later UI can cancel mid-flight and strand the board
    // off-screen on the widths where the dock sits below it.
    document.querySelector('.board-wrap')?.scrollIntoView({ block: 'center', behavior: 'instant' });
  };
  button('copy').onclick = async () => {
    if (solver.route === undefined) return;
    try {
      await navigator.clipboard.writeText(solver.prefix + solver.route);
      message('Full route copied as U/D/L/R.');
    } catch {
      message('Clipboard is unavailable in this browser.');
    }
  };
  // The skip button bypasses the header's controls; it must not touch the hash,
  // which the router owns.
  button('skip').onclick = () => {
    $<HTMLElement>('page').focus();
    scrollTo({ top: 0 });
  };
  addEventListener('resize', () => render());
  addEventListener('pagehide', () => {
    saveSession();
    flushEditorSave();
  });
  // Mobile browsers often skip pagehide, so hiding the page flushes the pending save too.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') {
      saveSession();
      timer.pause();
    } else if (session !== undefined && !session.state.solved && session.state.moves > 0) timer.run();
  });
  void probeServer();
}
/** Labels the page when /api/health answers, so the native solver can run. Static hosting has no
 * API: a 404, an app page instead of JSON, or no answer leaves Browser mode on show. */
async function probeServer() {
  try {
    const response = await fetch('/api/health');
    if (response.ok && decodeHealth(await response.json())) setText('connection', 'Native solver connected');
  } catch {
    /* No API here; the browser solver still works. */
  }
}
start().catch(error => message(`Could not start WebAssembly: ${errorMessage(error)}. Run npm run wasm and reload.`));
