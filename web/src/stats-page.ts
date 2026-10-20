/** The stats page: the aggregates over the journal and the stored bests, the
 * activity heat map, the progress tools, and the progression showcase —
 * achievements, milestones, and the board-frame keepsakes. The replay study
 * opens through the opener handed in by the composition root, so the pages
 * never import each other. */
import { ENTRIES, type Entry } from './catalog.ts';
import { navigate } from './router.ts';
import * as journal from './journal.ts';
import * as stats from './stats.ts';
import * as achievements from './achievements.ts';
import * as frames from './frames.ts';
import * as theme from './theme.ts';
import * as storage from './storage.ts';
import * as replays from './replays.ts';
import { idb } from './idb.ts';
import { $ } from './dom.ts';
import { queueReplay, verifyRoute } from './play.ts';

/** Renders the stats route: the aggregates over the journal and the stored
 * bests, the activity heat map, and the progress tools — copy, export,
 * import with its merge summary, and the reset behind three steps. */
function renderStats(openStudy: (entry: Entry) => void) {
  const body = $('stats-body');
  const book = journal.load();
  const a = stats.aggregates(book);
  // The tools below can change what the board wears; keep the frame current.
  frames.apply();
  const note = (text: string) => {
    const live = document.getElementById('stats-status');
    if (live) live.textContent = text;
  };

  const tools = document.createElement('div');
  tools.className = 'stats-tools';
  const copy = document.createElement('button');
  copy.id = 'stats-copy';
  copy.textContent = 'Copy stats';
  copy.onclick = async () => {
    try {
      await navigator.clipboard.writeText(stats.statsText(a));
      copy.textContent = 'Copied!';
      setTimeout(() => {
        copy.textContent = 'Copy stats';
      }, 2000);
    } catch {
      note('Clipboard is unavailable in this browser.');
    }
  };
  const exportProgress = document.createElement('button');
  exportProgress.id = 'stats-export';
  exportProgress.textContent = 'Export progress';
  exportProgress.onclick = () => {
    const blob = new Blob([JSON.stringify(book, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `sokomind-progress-${journal.dayKey(new Date())}.json`;
    link.click();
    URL.revokeObjectURL(url);
  };
  const importLabel = document.createElement('button');
  importLabel.id = 'stats-import';
  importLabel.textContent = 'Import progress';
  const file = document.createElement('input');
  file.id = 'stats-file';
  file.type = 'file';
  file.accept = '.json,application/json';
  file.hidden = true;
  importLabel.onclick = () => file.click();
  file.onchange = async () => {
    const chosen = file.files?.[0];
    file.value = '';
    if (!chosen) return;
    const result = journal.imported(await chosen.text());
    if (!result.ok) {
      note(result.error);
      return;
    }
    const current = journal.load();
    const summary = journal.summarizeMerge(current, result.journal);
    storage.write('journal', journal.merge(current, result.journal));
    renderStats(openStudy);
    note(
      `Imported: ${summary.added} added, ${summary.improved} improved, ${summary.unchanged} unchanged, `
        + `${summary.rejected + result.rejected} rejected, ${result.invalid} invalid.`,
    );
  };
  const status = document.createElement('p');
  status.id = 'stats-status';
  status.setAttribute('role', 'status');
  tools.append(copy, exportProgress, importLabel, file, status);

  if (a.solved === 0) {
    const empty = document.createElement('p');
    empty.className = 'muted';
    empty.textContent = 'Solve some rooms to fill in your statistics. An import still works.';
    body.replaceChildren(tools, empty, showcase(a, book, openStudy));
    return;
  }

  const card = (label: string, big: string, sub?: string, bigId?: string): HTMLElement => {
    const cardEl = document.createElement('div');
    cardEl.className = 'card stat-card';
    const eyebrow = document.createElement('p');
    eyebrow.className = 'eyebrow';
    eyebrow.textContent = label;
    const value = document.createElement('span');
    value.className = 'stat-big';
    if (bigId !== undefined) value.id = bigId;
    value.textContent = big;
    cardEl.append(eyebrow, value);
    if (sub !== undefined) {
      const line = document.createElement('span');
      line.className = 'stat-sub';
      line.textContent = sub;
      cardEl.append(line);
    }
    return cardEl;
  };
  const grid = document.createElement('div');
  grid.className = 'stats-grid';
  grid.append(
    card('Rooms solved', String(a.solved), `of ${a.total} (${Math.round(a.completion)}%)`, 'stats-solved'),
    card('Total moves', a.moves.toLocaleString(), `${a.pushes.toLocaleString()} pushes`),
    card('Avg pushes per room', a.averagePushes.toFixed(1)),
    card(
      'Streak',
      String(a.streak.current),
      `${a.streak.current === 1 ? 'day' : 'days'} current${a.streak.longest > 0 ? ` · ${a.streak.longest} best` : ''}`,
    ),
    a.elapsedMs > 0
      ? card('Time played', stats.formatDuration(a.elapsedMs))
      : card('Estimated time played', stats.formatDuration(a.recorded * 90_000), '~90s per room average'),
  );
  if (a.efficiency)
    grid.append(card('Best efficiency', `${a.efficiency.pushes}/${a.efficiency.boxes}`, `pushes/boxes on ${a.efficiency.title}`));

  // The donut: one arc per solved tier, the count at its center, a legend beside it.
  const donutCard = document.createElement('div');
  donutCard.className = 'card stat-card wide';
  const donutLabel = document.createElement('p');
  donutLabel.className = 'eyebrow';
  donutLabel.textContent = 'Solved by difficulty';
  const donutRow = document.createElement('div');
  donutRow.className = 'donut-row';
  const donut = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  donut.setAttribute('viewBox', '0 0 100 100');
  donut.classList.add('donut');
  const circumference = 2 * Math.PI * 40;
  const solvedTiers = a.byDifficulty.filter(tier => tier.solved > 0);
  let offset = 0;
  for (const tier of solvedTiers) {
    const dash = (tier.solved / a.solved) * circumference;
    const segment = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
    segment.setAttribute('cx', '50');
    segment.setAttribute('cy', '50');
    segment.setAttribute('r', '40');
    segment.setAttribute('fill', 'none');
    segment.setAttribute('stroke', stats.TIER_COLORS[tier.difficulty] ?? 'var(--ink-700)');
    segment.setAttribute('stroke-width', '16');
    segment.setAttribute('stroke-dasharray', `${dash} ${circumference - dash}`);
    segment.setAttribute('stroke-dashoffset', String(-offset));
    segment.setAttribute('transform', 'rotate(-90 50 50)');
    offset += dash;
    donut.append(segment);
  }
  const count = document.createElementNS('http://www.w3.org/2000/svg', 'text');
  count.setAttribute('x', '50');
  count.setAttribute('y', '47');
  count.setAttribute('text-anchor', 'middle');
  count.setAttribute('class', 'donut-center');
  count.textContent = String(a.solved);
  const word = document.createElementNS('http://www.w3.org/2000/svg', 'text');
  word.setAttribute('x', '50');
  word.setAttribute('y', '59');
  word.setAttribute('text-anchor', 'middle');
  word.setAttribute('class', 'donut-label');
  word.textContent = 'solved';
  donut.append(count, word);
  const legend = document.createElement('div');
  legend.className = 'donut-legend';
  for (const tier of solvedTiers) {
    const row = document.createElement('div');
    row.className = 'legend-item';
    const dot = document.createElement('span');
    dot.className = 'legend-dot';
    dot.style.background = stats.TIER_COLORS[tier.difficulty] ?? 'var(--ink-700)';
    const name = document.createElement('span');
    name.textContent = tier.difficulty;
    const value = document.createElement('span');
    value.className = 'legend-value';
    value.textContent = String(tier.solved);
    row.append(dot, name, value);
    legend.append(row);
  }
  donutRow.append(donut, legend);
  donutCard.append(donutLabel, donutRow);
  grid.append(donutCard);

  // The bars: each tier's completion, the track sized by the tier's room count.
  const barsCard = document.createElement('div');
  barsCard.className = 'card stat-card wide';
  const barsLabel = document.createElement('p');
  barsLabel.className = 'eyebrow';
  barsLabel.textContent = 'Completion by difficulty';
  const bars = document.createElement('div');
  bars.className = 'bar-chart';
  const maxTotal = Math.max(...a.byDifficulty.map(tier => tier.total), 1);
  for (const tier of a.byDifficulty) {
    const row = document.createElement('div');
    row.className = 'bar-row';
    const name = document.createElement('span');
    name.className = 'bar-label';
    name.textContent = tier.difficulty;
    const track = document.createElement('div');
    track.className = 'bar-track';
    track.style.width = `${Math.round((tier.total / maxTotal) * 100)}%`;
    const fill = document.createElement('span');
    fill.className = 'bar-fill';
    fill.style.width = `${tier.total === 0 ? 0 : Math.round((tier.solved / tier.total) * 100)}%`;
    fill.style.background = stats.TIER_COLORS[tier.difficulty] ?? 'var(--ink-700)';
    track.append(fill);
    const value = document.createElement('span');
    value.className = 'bar-value';
    value.textContent = `${tier.solved}/${tier.total}`;
    row.append(name, track, value);
    bars.append(row);
  }
  barsCard.append(barsLabel, bars);
  grid.append(barsCard);

  // The heat map: the last 90 days, one cell per day, five levels deep.
  const heatCard = document.createElement('div');
  heatCard.className = 'card stat-card wide';
  const heatLabel = document.createElement('p');
  heatLabel.className = 'eyebrow';
  heatLabel.textContent = 'Activity — last 90 days';
  const cells = journal.heat(book);
  const heatGrid = document.createElement('div');
  heatGrid.className = 'heat-grid';
  heatGrid.setAttribute('role', 'img');
  heatGrid.setAttribute('aria-label', `Activity over the last 90 days: ${cells.reduce((sum, cell) => sum + cell.count, 0)} rooms cleared`);
  for (const cell of cells) {
    const day = document.createElement('span');
    day.className = 'heat-cell';
    const level = stats.heatLevel(cell.count);
    if (level > 0) day.dataset.level = String(level);
    day.title = `${cell.key}: ${cell.count} cleared`;
    heatGrid.append(day);
  }
  const heatLegend = document.createElement('div');
  heatLegend.className = 'heat-legend';
  const less = document.createElement('span');
  less.className = 'heat-word';
  less.textContent = 'Less';
  heatLegend.append(less);
  for (let level = 0; level <= 4; level++) {
    const step = document.createElement('span');
    step.className = 'heat-cell';
    if (level > 0) step.dataset.level = String(level);
    heatLegend.append(step);
  }
  const more = document.createElement('span');
  more.className = 'heat-word';
  more.textContent = 'More';
  heatLegend.append(more);
  heatCard.append(heatLabel, heatGrid, heatLegend);
  grid.append(heatCard);

  // The improvement lab: recent personal-best routes, replay-verified, each
  // playable again from its fresh board.
  const lab = document.createElement('div');
  lab.className = 'card stat-card wide';
  const labEyebrow = document.createElement('p');
  labEyebrow.className = 'eyebrow';
  labEyebrow.textContent = 'IMPROVEMENT LAB';
  const labTitle = document.createElement('h3');
  labTitle.textContent = 'Personal-best replays';
  const labCount = document.createElement('span');
  labCount.className = 'showcase-tally';
  const labState = document.createElement('p');
  labState.className = 'muted';
  labState.textContent = 'Finding recent verified routes…';
  lab.append(labEyebrow, labTitle, labCount, labState);
  grid.append(lab);
  void (async () => {
    const found = await replays.index();
    const items: { entry: Entry; routes: replays.Route[] }[] = [];
    for (const id of found.ids) {
      if (items.length >= 6) break;
      const entry = ENTRIES.find(candidate => candidate.id === id);
      if (entry === undefined) continue;
      const read = await replays.read(idb, id, entry.rows.join('\n'), verifyRoute);
      if (read.status === 'ready' && read.routes.length > 0) items.push({ entry, routes: read.routes });
    }
    if (!found.available) {
      labState.textContent = 'Replay storage is unavailable. Your summary statistics still work normally.';
      return;
    }
    if (items.length === 0) {
      labState.textContent = 'No replay routes are available yet. Clear a room again to capture its route.';
      return;
    }
    labCount.textContent = `${items.length} ready`;
    labState.hidden = true;
    const labList = document.createElement('ul');
    labList.className = 'shelf-list';
    for (const { entry, routes } of items) {
      const best = routes[0];
      if (best === undefined) continue;
      const item = document.createElement('li');
      const detail = document.createElement('div');
      const name = document.createElement('strong');
      name.textContent = entry.title;
      const meta = document.createElement('span');
      meta.textContent = `${best.moves} moves · ${best.pushes} pushes · ${routes.length} ${routes.length === 1 ? 'route' : 'routes'}`;
      const saved = document.createElement('small');
      saved.textContent = `Saved ${achievements.milestoneLabel(best.completedAt)}`;
      detail.append(name, meta, saved);
      const play = document.createElement('button');
      play.textContent = 'Replay';
      play.onclick = () => {
        queueReplay(entry.id, best.actionLog);
        navigate({ kind: 'play', puzzleId: entry.id });
      };
      const studyButton = document.createElement('button');
      studyButton.textContent = 'Study';
      studyButton.onclick = () => openStudy(entry);
      item.append(detail, play, studyButton);
      labList.append(item);
    }
    lab.append(labList);
  })();

  // The danger zone: the reset asks three times, the last by name.
  const danger = document.createElement('div');
  danger.className = 'card stat-card wide danger';
  const dangerLabel = document.createElement('p');
  dangerLabel.className = 'eyebrow';
  dangerLabel.textContent = 'Danger zone';
  const zone = document.createElement('div');
  zone.className = 'danger-zone';
  const draw = (step: 'idle' | 'confirm' | 'typing') => {
    zone.replaceChildren();
    if (step === 'idle') {
      const start = document.createElement('button');
      start.id = 'stats-reset';
      start.textContent = 'Reset all progress';
      start.onclick = () => draw('confirm');
      zone.append(start);
      return;
    }
    if (step === 'confirm') {
      const warning = document.createElement('p');
      warning.className = 'muted';
      warning.textContent =
        'This permanently deletes your solved rooms, statistics and daily history from this browser. Your theme, favorites and ratings stay.';
      const go = document.createElement('button');
      go.textContent = 'I understand, continue';
      go.onclick = () => draw('typing');
      const cancel = document.createElement('button');
      cancel.textContent = 'Cancel';
      cancel.onclick = () => draw('idle');
      zone.append(warning, go, cancel);
      return;
    }
    const ask = document.createElement('p');
    ask.className = 'muted';
    const strong = document.createElement('strong');
    strong.textContent = 'RESET';
    ask.append('Type ', strong, ' to confirm:');
    const input = document.createElement('input');
    input.id = 'stats-reset-input';
    input.type = 'text';
    input.spellcheck = false;
    const go = document.createElement('button');
    go.id = 'stats-reset';
    go.textContent = 'Permanently reset progress';
    go.disabled = true;
    input.oninput = () => {
      go.disabled = input.value !== 'RESET';
    };
    go.onclick = () => {
      journal.reset();
      for (const name of storage.names()) if (name.startsWith('best.')) storage.remove(name);
      // The replay shelf follows the solved record down; the optimal cache
      // stays, being the engine's knowledge rather than progress.
      void replays.clear();
      // The reset can unearn the frame the board wears.
      frames.apply();
      navigate({ kind: 'home' });
    };
    const cancel = document.createElement('button');
    cancel.textContent = 'Cancel';
    cancel.onclick = () => draw('idle');
    zone.append(ask, input, go, cancel);
  };
  draw('idle');
  danger.append(dangerLabel, zone);
  grid.append(danger);

  body.replaceChildren(tools, grid, showcase(a, book, openStudy));
}

/** The stats page's progression showcase: the achievements in their
 * collections with their progress, the milestones behind them, and the
 * board-frame keepsakes an achievement unlocks. */
function showcase(a: stats.Aggregates, book: journal.Journal, openStudy: (entry: Entry) => void): HTMLElement {
  const section = document.createElement('section');
  section.className = 'showcase';
  const intro = document.createElement('div');
  intro.className = 'showcase-intro';
  const lead = document.createElement('div');
  const eyebrow = document.createElement('p');
  eyebrow.className = 'eyebrow';
  eyebrow.textContent = 'LONG-TERM PROGRESS';
  const title = document.createElement('h2');
  title.textContent = 'Achievements and keepsakes';
  const note = document.createElement('p');
  note.className = 'muted';
  note.textContent = 'Every requirement is visible and evaluated from your saved solves. Rewards never lock rooms or change board rules.';
  lead.append(eyebrow, title, note);
  const earned = achievements.unlocked(a, book);
  const tally = document.createElement('strong');
  tally.textContent = `${earned.length}/${achievements.ACHIEVEMENTS.length} earned`;
  intro.append(lead, tally);
  section.append(intro);

  // Recent milestones: the path behind you, dated by the records.
  const marks = achievements.milestones(a, book);
  const recent = document.createElement('div');
  recent.className = 'showcase-block';
  const recentHead = document.createElement('div');
  const recentEyebrow = document.createElement('p');
  recentEyebrow.className = 'eyebrow';
  recentEyebrow.textContent = 'RECENT MILESTONES';
  const recentTitle = document.createElement('h3');
  recentTitle.textContent = 'The path behind you';
  recentHead.append(recentEyebrow, recentTitle);
  const recentNote = document.createElement('span');
  recentNote.className = 'muted';
  recentNote.textContent = 'Based on saved completion dates';
  recent.append(recentHead, recentNote);
  if (marks.length > 0) {
    const list = document.createElement('ol');
    list.className = 'milestone-list';
    for (const mark of marks) {
      const item = document.createElement('li');
      const tick = document.createElement('span');
      tick.className = 'milestone-mark';
      tick.setAttribute('aria-hidden', 'true');
      tick.textContent = '✓';
      const detail = document.createElement('div');
      const name = document.createElement('strong');
      name.textContent = mark.title;
      const when = document.createElement('span');
      when.textContent = `${mark.collectionTitle} · ${achievements.milestoneLabel(mark.earnedAt)}`;
      const what = document.createElement('small');
      what.textContent = mark.description;
      detail.append(name, when, what);
      item.append(tick, detail);
      list.append(item);
    }
    recent.append(list);
  } else {
    const none = document.createElement('p');
    none.className = 'muted';
    none.textContent = 'Your first solved room will place a milestone here.';
    recent.append(none);
  }
  section.append(recent);

  // The collections, one block per group of achievements.
  const earnedIds = new Set(earned.map(def => def.id));
  const collections = document.createElement('div');
  collections.className = 'showcase-collections';
  for (const collection of achievements.COLLECTIONS) {
    const defs = achievements.ACHIEVEMENTS.filter(def => def.collection === collection.id);
    const block = document.createElement('section');
    const head = document.createElement('div');
    const name = document.createElement('h3');
    name.textContent = collection.title;
    const blurb = document.createElement('p');
    blurb.className = 'muted';
    blurb.textContent = collection.description;
    head.append(name, blurb);
    const count = document.createElement('span');
    count.className = 'showcase-tally';
    count.textContent = `${defs.filter(def => earnedIds.has(def.id)).length}/${defs.length}`;
    block.append(head, count);
    const list = document.createElement('ul');
    list.className = 'achievement-list';
    for (const def of defs) {
      const progress = def.progress(a, book);
      const item = document.createElement('li');
      if (progress.complete) item.dataset.earned = 'true';
      const icon = document.createElement('span');
      icon.className = 'achievement-icon';
      icon.setAttribute('aria-hidden', 'true');
      icon.textContent = progress.complete ? '✓' : def.icon;
      const detail = document.createElement('div');
      const top = document.createElement('div');
      const label = document.createElement('strong');
      label.textContent = def.title;
      const standing = document.createElement('span');
      standing.textContent = progress.complete ? 'Earned' : 'In progress';
      top.append(label, standing);
      const what = document.createElement('p');
      what.textContent = def.description;
      const track = document.createElement('div');
      track.className = 'progress-track';
      track.setAttribute('role', 'progressbar');
      track.setAttribute('aria-label', `${def.title}: ${progress.label}`);
      track.setAttribute('aria-valuemin', '0');
      track.setAttribute('aria-valuemax', String(progress.target));
      track.setAttribute('aria-valuenow', String(progress.current));
      const fill = document.createElement('span');
      fill.className = 'progress-fill';
      fill.style.width = `${Math.round((progress.current / progress.target) * 100)}%`;
      track.append(fill);
      const label2 = document.createElement('small');
      label2.textContent = progress.label;
      detail.append(top, what, track, label2);
      item.append(icon, detail);
      list.append(item);
    }
    block.append(list);
    collections.append(block);
  }
  section.append(collections);

  // The board-frame keepsakes: choose a quiet finishing touch.
  const keepsakes = document.createElement('div');
  keepsakes.className = 'showcase-block';
  const keepsakeHead = document.createElement('div');
  const keepsakeEyebrow = document.createElement('p');
  keepsakeEyebrow.className = 'eyebrow';
  keepsakeEyebrow.textContent = 'BOARD-FRAME KEEPSAKES';
  const keepsakeTitle = document.createElement('h3');
  keepsakeTitle.textContent = 'Choose a quiet finishing touch';
  keepsakeHead.append(keepsakeEyebrow, keepsakeTitle);
  const keepsakeNote = document.createElement('span');
  keepsakeNote.className = 'muted';
  keepsakeNote.textContent = 'Frame and shadow only';
  keepsakes.append(keepsakeHead, keepsakeNote);
  const never = document.createElement('p');
  never.className = 'muted';
  never.textContent = 'These never recolor tiles, goals, crates, the robot, warnings, or focus indicators.';
  keepsakes.append(never);
  const picks = document.createElement('div');
  picks.className = 'keepsake-grid';
  picks.setAttribute('role', 'group');
  picks.setAttribute('aria-label', 'Board frame cosmetic');
  for (const state of frames.states(frames.preference(), theme.preferences().themeFamily, a, book)) {
    const usable = state.unlocked && state.compatible;
    const pick = document.createElement('button');
    pick.className = 'keepsake';
    pick.dataset.frame = state.id;
    if (state.active) pick.dataset.active = 'true';
    if (!usable) pick.dataset.locked = 'true';
    pick.setAttribute('aria-pressed', String(state.active));
    pick.disabled = !usable;
    pick.onclick = () => {
      frames.save(state.id);
      renderStats(openStudy);
    };
    const preview = document.createElement('span');
    preview.className = 'keepsake-preview';
    preview.setAttribute('aria-hidden', 'true');
    const chip = document.createElement('span');
    preview.append(chip);
    const copy = document.createElement('span');
    copy.className = 'keepsake-copy';
    const name = document.createElement('strong');
    name.textContent = state.title;
    const blurb = document.createElement('small');
    blurb.textContent = state.description;
    const standing = document.createElement('em');
    standing.textContent = !state.unlocked
      ? state.requirement
      : !state.compatible
        ? `Use with ${state.families.map(family => family.replaceAll('-', ' ')).join(' or ')}`
        : state.active
          ? 'Active'
          : 'Available';
    copy.append(name, blurb, standing);
    pick.append(preview, copy);
    picks.append(pick);
  }
  keepsakes.append(picks);
  section.append(keepsakes);
  return section;
}

export { renderStats };
