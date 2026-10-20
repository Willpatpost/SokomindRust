/** The home page: the dashboard the app opens on — the way back into the
 * saved session, the catalog's cleared progress, the streaks, the daily
 * challenge, the guided journey, and the recently cleared. */
import { DIFFICULTIES, ENTRIES, daily, filter as filterEntries, solved, type Entry } from './catalog.ts';
import * as journal from './journal.ts';
import * as journey from './journey.ts';
import * as storage from './storage.ts';
import { playHash } from './shell.ts';
import { $ } from './dom.ts';

/** Renders the home dashboard and aims its primary action: the cleared
 * progress with its bar, the tier row, the solving streak, today's daily
 * challenge with its run and history, the recently cleared, and the saved
 * session's puzzle as the way back in. */
function syncHome() {
  const saved = storage.session();
  const id = saved && ENTRIES.some(p => p.id === saved.id) ? saved.id : ENTRIES[0]!.id;
  const start = $<HTMLAnchorElement>('start');
  start.href = playHash(id);
  start.textContent = saved ? 'Resume playing' : 'Start playing';
  const cleared = ENTRIES.filter(solved).length;
  const summary = document.createElement('p');
  summary.className = 'muted';
  summary.textContent = `${cleared} of ${ENTRIES.length} rooms cleared`;
  const bar = document.createElement('div');
  bar.className = 'progress-track';
  const fill = document.createElement('div');
  fill.className = 'progress-fill';
  fill.style.width = `${Math.round((cleared / ENTRIES.length) * 100)}%`;
  bar.append(fill);
  const tiers = document.createElement('div');
  tiers.className = 'tier-row';
  for (const difficulty of DIFFICULTIES) {
    const tier = filterEntries(ENTRIES, { difficulty });
    const link = document.createElement('a');
    link.href = `#/puzzles/${encodeURIComponent(difficulty)}`;
    const name = document.createElement('strong');
    name.textContent = difficulty;
    const count = document.createElement('span');
    count.textContent = `${tier.filter(solved).length}/${tier.length}`;
    link.append(name, count);
    tiers.append(link);
  }
  const book = journal.load();
  const parts: HTMLElement[] = [summary, bar, tiers];
  const streak = journal.solvingStreak(book);
  if (streak.current > 0) {
    const banner = document.createElement('div');
    banner.className = 'streak-banner';
    const flame = document.createElement('span');
    flame.className = 'flame';
    flame.setAttribute('aria-hidden', 'true');
    flame.textContent = '🔥';
    const detail = document.createElement('div');
    const current = document.createElement('strong');
    current.textContent = `${streak.current}-day streak`;
    detail.append(current);
    if (streak.longest > streak.current) {
      const longest = document.createElement('span');
      longest.textContent = `Best: ${streak.longest} days`;
      detail.append(longest);
    }
    banner.append(flame, detail);
    parts.push(banner);
  }
  const challenge = daily();
  const view = journal.dailyView(book);
  const card = document.createElement('div');
  card.className = 'card daily';
  card.dataset.state = view.state;
  const topline = document.createElement('p');
  topline.className = 'daily-top';
  const label = document.createElement('span');
  label.textContent = 'Daily challenge';
  const run = document.createElement('span');
  run.textContent = view.streak > 0 ? `${view.streak}-day run` : 'New room daily';
  topline.append(label, run);
  const title = document.createElement('h2');
  title.textContent = challenge.title;
  const framing = document.createElement('p');
  framing.className = 'muted';
  framing.textContent =
    view.state === 'cleared'
      ? 'Today is recorded. Replay the room to refine your route, or return tomorrow for a new one.'
      : view.state === 'recovery'
        ? 'A missed day does not close the path. This room starts a fresh run today.'
        : 'One shared room each day. Solve it before the day rolls over to add today to your run.';
  const play = document.createElement('a');
  play.className = 'button';
  play.href = playHash(challenge.id);
  play.textContent = view.state === 'cleared' ? 'Replay today’s room' : 'Play today’s room';
  card.append(topline, title, framing, play);
  const history = document.createElement('ol');
  history.className = 'daily-history';
  history.setAttribute('aria-label', 'Seven-day daily history');
  for (const day of view.history) {
    const item = document.createElement('li');
    item.dataset.outcome = day.outcome;
    item.setAttribute('aria-label', `${day.label}: ${day.outcome === 'today' ? 'open today' : day.outcome}`);
    const dot = document.createElement('span');
    dot.className = 'dot';
    dot.setAttribute('aria-hidden', 'true');
    dot.textContent = day.outcome === 'cleared' ? '✓' : day.outcome === 'today' ? '•' : '';
    const name = document.createElement('span');
    name.textContent = day.label;
    item.append(dot, name);
    history.append(item);
  }
  card.append(history);
  parts.push(card);
  parts.push(journeyCard(book));
  const recent = journal
    .recentlyCleared(book)
    .map(clear => ({ clear, entry: ENTRIES.find(p => p.id === clear.id) }))
    .filter((row): row is { clear: journal.RecentClear; entry: Entry } => row.entry !== undefined);
  if (recent.length > 0) {
    const strip = document.createElement('div');
    strip.className = 'recent';
    const heading = document.createElement('p');
    heading.className = 'muted';
    heading.textContent = 'Recently cleared';
    const list = document.createElement('div');
    list.className = 'recent-list';
    for (const { clear, entry } of recent) {
      const link = document.createElement('a');
      link.href = playHash(entry.id);
      const name = document.createElement('strong');
      name.textContent = entry.title;
      const meta = document.createElement('span');
      meta.textContent = `${clear.moves}m / ${clear.pushes}p · ${journal.timeAgo(clear.clearedAt)}`;
      link.append(name, meta);
      list.append(link);
    }
    strip.append(heading, list);
    parts.push(strip);
  }
  $('home-dash').replaceChildren(...parts);
}

/** The home page's guided journey card: five concept chapters with their
 * rooms, the next suggestion, and the pause that folds it away. */
function journeyCard(book: journal.Journal): HTMLElement {
  const card = document.createElement('section');
  card.className = 'card journey';
  if (journey.dismissed()) {
    const eyebrow = document.createElement('p');
    eyebrow.className = 'eyebrow';
    eyebrow.textContent = 'GUIDED PATH PAUSED';
    const title = document.createElement('h2');
    title.textContent = 'Explore your own way';
    const note = document.createElement('p');
    note.className = 'muted';
    note.textContent = 'Your chapter progress is safe. Resume whenever you want a suggestion.';
    const resume = document.createElement('button');
    resume.textContent = 'Resume guided path';
    resume.onclick = () => {
      journey.setDismissed(false);
      syncHome();
    };
    card.append(eyebrow, title, note, resume);
    return card;
  }
  const chapters = journey.chapterProgress(book);
  const done = chapters.reduce((sum, chapter) => sum + chapter.solved, 0);
  const total = chapters.reduce((sum, chapter) => sum + chapter.total, 0);
  const header = document.createElement('div');
  header.className = 'journey-head';
  const lead = document.createElement('div');
  const eyebrow = document.createElement('p');
  eyebrow.className = 'eyebrow';
  eyebrow.textContent = `Optional journey · ${done}/${total}`;
  const title = document.createElement('h2');
  title.textContent = 'Build your Sokoban instincts';
  const note = document.createElement('p');
  note.className = 'muted';
  note.textContent = 'Five concept chapters offer direction, never gates.';
  lead.append(eyebrow, title, note);
  const pause = document.createElement('button');
  pause.className = 'journey-pause';
  pause.textContent = 'Pause';
  pause.setAttribute('aria-label', 'Pause guided path');
  pause.onclick = () => {
    journey.setDismissed(true);
    syncHome();
  };
  header.append(lead, pause);
  card.append(header);
  const suggestion = journey.recommendation(book);
  const row = document.createElement('div');
  row.className = 'journey-next';
  const copy = document.createElement('div');
  const where = document.createElement('span');
  const room = document.createElement('strong');
  const why = document.createElement('p');
  why.className = 'muted';
  const go = document.createElement('a');
  go.className = 'button';
  if (suggestion !== null) {
    where.textContent = `Suggested next · ${suggestion.chapterTitle}`;
    room.textContent = ENTRIES.find(entry => entry.id === suggestion.roomId)?.title ?? suggestion.roomId;
    why.textContent = suggestion.reason;
    go.href = playHash(suggestion.roomId);
    go.textContent = 'Play suggestion';
  } else {
    where.textContent = 'Journey complete';
    room.textContent = 'Every chapter cleared';
    why.textContent = 'Your next direction is yours: revisit a favorite route or explore the full catalog.';
    go.href = '#/puzzles';
    go.textContent = 'Browse catalog';
  }
  copy.append(where, room, why);
  row.append(copy, go);
  card.append(row);
  const map = document.createElement('ol');
  map.className = 'journey-map';
  map.setAttribute('aria-label', 'Guided journey chapters');
  for (const chapter of chapters) {
    const item = document.createElement('li');
    item.className = 'chapter';
    if (chapter.complete) item.dataset.complete = 'true';
    const heading = document.createElement('div');
    heading.className = 'chapter-head';
    const number = document.createElement('span');
    number.className = 'chapter-number';
    number.textContent = String(chapter.number);
    const named = document.createElement('div');
    const name = document.createElement('h3');
    name.textContent = chapter.title;
    const concept = document.createElement('p');
    concept.textContent = chapter.concept;
    named.append(name, concept);
    const count = document.createElement('span');
    count.className = 'chapter-count';
    count.textContent = `${chapter.solved}/${chapter.total}`;
    heading.append(number, named, count);
    const explanation = document.createElement('p');
    explanation.className = 'muted';
    explanation.textContent = chapter.explanation;
    item.append(heading, explanation);
    if (chapter.requires.length > 0 && !chapter.ready) {
      const wait = document.createElement('p');
      wait.className = 'chapter-wait';
      const required = chapter.requires[0]!;
      wait.textContent = `Recommended first: ${chapters.find(entry => entry.id === required)?.title ?? required}`;
      item.append(wait);
    }
    const nodes = document.createElement('div');
    nodes.className = 'chapter-nodes';
    nodes.setAttribute('aria-label', `${chapter.title} rooms`);
    for (const [index, id] of chapter.rooms.entries()) {
      const node = document.createElement('a');
      node.className = 'node';
      const entry = ENTRIES.find(candidate => candidate.id === id);
      const cleared = book.cleared[id] !== undefined || (entry !== undefined && solved(entry));
      if (cleared) node.dataset.complete = 'true';
      node.href = playHash(id);
      node.textContent = cleared ? '✓' : String(index + 1);
      node.title = entry?.title ?? id;
      node.setAttribute('aria-label', `${entry?.title ?? id}${cleared ? ', cleared' : ', not cleared'}`);
      nodes.append(node);
    }
    item.append(nodes);
    map.append(item);
  }
  card.append(map);
  const browse = document.createElement('a');
  browse.className = 'journey-browse';
  browse.href = '#/puzzles';
  browse.textContent = `Browse all ${ENTRIES.length} rooms`;
  card.append(browse);
  return card;
}

export { syncHome };
