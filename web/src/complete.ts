import type { Best } from './storage.ts';

/** What the completion dialog shows: the heading tier, the summary line and
 * the milestones the solve earned. */
export interface Completion {
  eyebrow: string;
  title: string;
  summary: string;
  milestones: string[];
}

/** Presents a solve against the best that stood before it: a first clear
 * leads, a personal best follows, and a matching solve is a plain room
 * cleared. The optimal-clear milestone waits for the proven-optimal cache the
 * progress phase adds. */
export function present(now: { moves: number; pushes: number }, before: Best | null): Completion {
  if (before === null)
    return {
      eyebrow: 'FIRST CLEAR',
      title: 'Room cleared',
      summary: `Solved in ${now.moves} moves and ${now.pushes} pushes — your first route here.`,
      milestones: ['First clear'],
    };
  if (now.moves < before.moves || (now.moves === before.moves && now.pushes < before.pushes)) {
    const fewerMoves = now.moves < before.moves;
    return {
      eyebrow: 'PERSONAL MILESTONE',
      title: 'New personal best',
      summary: fewerMoves
        ? `${now.moves} moves — ${before.moves - now.moves} fewer than your best.`
        : `${now.pushes} pushes — ${before.pushes - now.pushes} fewer at your best moves.`,
      milestones: [fewerMoves ? 'Fewer moves than your best' : 'Fewer pushes at your best moves'],
    };
  }
  return {
    eyebrow: 'ROOM CLEARED',
    title: 'Room cleared',
    summary: `Solved in ${now.moves} moves and ${now.pushes} pushes, matching your best.`,
    milestones: [],
  };
}
