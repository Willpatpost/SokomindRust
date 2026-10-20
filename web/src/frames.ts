import * as storage from './storage.ts';
import * as theme from './theme.ts';
import { ACHIEVEMENTS } from './achievements.ts';
import { aggregates, type Aggregates } from './stats.ts';
import { load, type Journal } from './journal.ts';

/** The board-frame keepsakes: cosmetics an achievement unlocks, chosen in the
 * stats page's showcase and applied to the page root, where tokens.css's
 * data-board-frame rules restyle the board's outer treatment alone — never
 * its tiles, goals, crates, the robot, warnings or focus indicators. */

export type FrameId = 'classic' | 'sage-thread' | 'brass-edge' | 'neon-orbit';

export interface Frame {
  id: FrameId;
  title: string;
  description: string;
  requirement: string;
  achievementId?: string;
  families: readonly theme.ThemeFamily[];
}

export const FRAMES: readonly Frame[] = [
  {
    id: 'classic',
    title: 'Classic frame',
    description: 'The original quiet border and shadow.',
    requirement: 'Available from the start',
    families: theme.THEME_FAMILIES,
  },
  {
    id: 'sage-thread',
    title: 'Sage thread',
    description: 'A slim green outer thread around the board.',
    requirement: 'Earn Getting Started',
    achievementId: 'ten-solved',
    families: theme.THEME_FAMILIES,
  },
  {
    id: 'brass-edge',
    title: 'Brass edge',
    description: 'A warm study-lamp edge with a restrained glow.',
    requirement: 'Earn Graduate',
    achievementId: 'tutorial-complete',
    families: ['cozy-study', 'minimal-ink'],
  },
  {
    id: 'neon-orbit',
    title: 'Neon orbit',
    description: 'A cool double-line frame made for Midnight Neon.',
    requirement: 'Earn Puzzle Enthusiast',
    achievementId: 'fifty-solved',
    families: ['midnight-neon'],
  },
];

function isFrameId(value: unknown): value is FrameId {
  return typeof value === 'string' && FRAMES.some(frame => frame.id === value);
}

/** The frame chosen in browser storage, or the classic one. */
export function preference(): FrameId {
  const value = storage.read('frame');
  return isFrameId(value) ? value : 'classic';
}

export function save(id: FrameId): void {
  storage.write('frame', id);
}

export interface FrameState extends Frame {
  unlocked: boolean;
  compatible: boolean;
  selected: boolean;
  active: boolean;
}

function isUnlocked(frame: Frame, a: Aggregates, book: Journal): boolean {
  if (frame.achievementId === undefined) return true;
  return ACHIEVEMENTS.find(def => def.id === frame.achievementId)?.check(a, book) ?? false;
}

/** Every frame's state under a choice and a theme: earned, fitting, chosen,
 * and the one the board actually wears. */
export function states(pick: FrameId, family: theme.ThemeFamily, a: Aggregates, book: Journal): FrameState[] {
  const chosen = FRAMES.find(frame => frame.id === pick) ?? FRAMES[0]!;
  const chosenWears = isUnlocked(chosen, a, book) && chosen.families.includes(family);
  const wearing = chosenWears ? chosen.id : 'classic';
  return FRAMES.map(frame => ({
    ...frame,
    unlocked: isUnlocked(frame, a, book),
    compatible: frame.families.includes(family),
    selected: pick === frame.id,
    active: wearing === frame.id,
  }));
}

/** The frame the board wears: the chosen one when it is earned and fits the
 * theme, the classic one otherwise. */
export function active(pick: FrameId, family: theme.ThemeFamily, a: Aggregates, book: Journal): FrameId {
  return states(pick, family, a, book).find(state => state.active)?.id ?? 'classic';
}

/** Applies the frame the board wears to the page root, where tokens.css
 * reads it. */
export function apply(): void {
  const book = load();
  document.documentElement.dataset.boardFrame = active(preference(), theme.preferences().themeFamily, aggregates(book), book);
}
