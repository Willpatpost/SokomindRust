import { ENTRIES, solved } from './catalog.ts';
import * as storage from './storage.ts';
import type { Journal } from './journal.ts';

/** The guided journey: five concept chapters over the catalog's rooms,
 * offered as direction rather than gates — every room stays playable at any
 * time, and the card can be paused. */

export interface Chapter {
  id: string;
  number: number;
  title: string;
  concept: string;
  explanation: string;
  requires: readonly string[];
  rooms: readonly string[];
}

export const CHAPTERS: readonly Chapter[] = [
  {
    id: 'push-fundamentals',
    number: 1,
    title: 'Push fundamentals',
    concept: 'Position before pressure',
    explanation: 'Learn to approach a box from the useful side and keep a route behind it before committing to a push.',
    requires: [],
    rooms: ['ultra-tiny', 'tutorial-push', 'tiny', 'tutorial-around'],
  },
  {
    id: 'space-and-corners',
    number: 2,
    title: 'Space and corners',
    concept: 'Protect escape routes',
    explanation: 'Read corners, walls, and turning space early so a harmless-looking push does not become permanent.',
    requires: ['push-fundamentals'],
    rooms: ['beginner-detour', 'box-5x5-a', 'beginner-three', 'beginner-typed-line'],
  },
  {
    id: 'routes-and-order',
    number: 3,
    title: 'Routes and order',
    concept: 'Sequence the work',
    explanation: 'Coordinate several boxes by opening traffic lanes and solving goals in an order that preserves access.',
    requires: ['space-and-corners'],
    rooms: ['workshop-1', 'classic-1', 'theme-kitchen', 'garden-2'],
  },
  {
    id: 'constraint-control',
    number: 4,
    title: 'Constraint control',
    concept: 'Create room before spending it',
    explanation: 'Use staging squares, rotations, and temporary displacement to manage tighter boards without sealing a lane.',
    requires: ['routes-and-order'],
    rooms: ['medium', 'large', 'adv-gallery', 'theme-parking'],
  },
  {
    id: 'search-frontiers',
    number: 5,
    title: 'Search frontiers',
    concept: 'Compare plans, not just pushes',
    explanation: 'Tackle branching positions where progress comes from evaluating alternatives and recognizing future deadlocks.',
    requires: ['constraint-control'],
    rooms: ['open-field', 'expert-maze', 'huge'],
  },
];

/** A room counts cleared by either signal — the journal's dated record or a
 * stored best — as everywhere else in the app. */
function clearedRooms(book: Journal): Set<string> {
  return new Set(ENTRIES.filter(entry => book.cleared[entry.id] !== undefined || solved(entry)).map(entry => entry.id));
}

export interface ChapterProgress extends Chapter {
  solved: number;
  total: number;
  complete: boolean;
  ready: boolean;
}

/** Each chapter's progress over the rooms the catalog knows, with the
 * prerequisite chain marked ready as the chapters before it complete. */
export function chapterProgress(book: Journal): ChapterProgress[] {
  const done = clearedRooms(book);
  const known = new Set(ENTRIES.map(entry => entry.id));
  const completeIds = new Set<string>();
  return CHAPTERS.map(chapter => {
    const rooms = chapter.rooms.filter(id => known.has(id));
    const cleared = rooms.filter(id => done.has(id)).length;
    const complete = rooms.length > 0 && cleared === rooms.length;
    const ready = chapter.requires.every(id => completeIds.has(id));
    if (complete) completeIds.add(chapter.id);
    return { ...chapter, rooms, solved: cleared, total: rooms.length, complete, ready };
  });
}

export interface Recommendation {
  roomId: string;
  chapterTitle: string;
  reason: string;
}

/** The next room to try: the first uncleared room of the first unfinished
 * chapter, with the chapter's case for it. */
export function recommendation(book: Journal): Recommendation | null {
  const done = clearedRooms(book);
  const titles = new Map(ENTRIES.map(entry => [entry.id, entry.title] as const));
  for (const chapter of chapterProgress(book)) {
    const roomId = chapter.rooms.find(id => !done.has(id));
    if (roomId === undefined) continue;
    const title = titles.get(roomId) ?? roomId;
    const position = chapter.rooms.indexOf(roomId) + 1;
    const reason = chapter.ready
      ? `${title} is the next unsolved room in ${chapter.title} (${position} of ${chapter.total}). It practices ${chapter.concept.toLowerCase()}.`
      : `${title} fills an earlier foundation in ${chapter.title}. You may still play any later room at any time.`;
    return { roomId, chapterTitle: chapter.title, reason };
  }
  return null;
}

/** The journey card's paused state, kept in browser storage. */
export function dismissed(): boolean {
  return storage.read('journey') === true;
}

export function setDismissed(value: boolean): void {
  storage.write('journey', value);
}
