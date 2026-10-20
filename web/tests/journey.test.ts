import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../src/storage.ts';
import * as journal from '../src/journal.ts';
import * as journey from '../src/journey.ts';
import { ENTRIES } from '../src/catalog.ts';
import { installMemoryStorage } from './fakes.ts';
installMemoryStorage();

const empty = (): journal.Journal => ({ cleared: {}, daily: {}, activity: {} });
const titles = new Map(ENTRIES.map(entry => [entry.id, entry.title] as const));
const bestFor = (id: string) => {
  const entry = ENTRIES.find(room => room.id === id)!;
  storage.write(`best.${id}`, { rows: entry.rows.join('\n'), route: 'D', moves: 1, pushes: 1 });
};

test('every chapter room is a catalog room', () => {
  const ids = new Set(ENTRIES.map(entry => entry.id));
  for (const chapter of journey.CHAPTERS) for (const id of chapter.rooms) assert.ok(ids.has(id), `${chapter.id}: ${id}`);
  assert.equal(
    journey.CHAPTERS.reduce((sum, chapter) => sum + chapter.rooms.length, 0),
    19,
  );
});

test('chapter progress counts cleared rooms by either signal', () => {
  const progress = journey.chapterProgress(empty());
  assert.equal(progress[0]!.solved, 0);
  assert.equal(progress[0]!.ready, true);
  assert.equal(progress[1]!.ready, false);
  // Four chapter-one bests complete the chapter, dates or none.
  for (const id of journey.CHAPTERS[0]!.rooms) bestFor(id);
  const done = journey.chapterProgress(empty());
  assert.equal(done[0]!.solved, 4);
  assert.equal(done[0]!.complete, true);
  assert.equal(done[1]!.ready, true);
});

test('the recommendation names the first uncleared room of the first unfinished chapter', () => {
  // An earlier test left chapter-one bests behind; the recommendation reads
  // either solved signal, so clear them for a clean catalog.
  for (const name of storage.names()) if (name.startsWith('best.')) storage.remove(name);
  const first = journey.recommendation(empty());
  assert.equal(first?.roomId, 'ultra-tiny');
  assert.equal(first?.chapterTitle, 'Push fundamentals');
  assert.ok(first?.reason.includes(titles.get('ultra-tiny')!));
  // Chapter one cleared moves the suggestion into chapter two.
  for (const id of journey.CHAPTERS[0]!.rooms) bestFor(id);
  const second = journey.recommendation(empty());
  assert.equal(second?.roomId, 'beginner-detour');
  assert.equal(second?.chapterTitle, 'Space and corners');
  // Every room cleared leaves nothing to suggest.
  for (const entry of ENTRIES) bestFor(entry.id);
  assert.equal(journey.recommendation(empty()), null);
});

test('the paused state reads and writes through browser storage', () => {
  assert.equal(journey.dismissed(), false);
  storage.write('journey', 'garbage');
  assert.equal(journey.dismissed(), false);
  journey.setDismissed(true);
  assert.equal(journey.dismissed(), true);
  journey.setDismissed(false);
  assert.equal(journey.dismissed(), false);
});
