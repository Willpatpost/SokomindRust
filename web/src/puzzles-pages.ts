/** The puzzles pages: the tier grid, a tier's collections, and the
 * filterable, paginated list of a collection's rooms. */
import {
  DIFFICULTIES,
  ENTRIES,
  favorites,
  filter as filterEntries,
  minimap,
  paginate,
  PER_PAGE,
  rating,
  solved,
  toggleFavorite,
  type Entry,
} from './catalog.ts';
import * as storage from './storage.ts';
import { navigate, route, type Route } from './router.ts';
import { playHash } from './shell.ts';
import { $ } from './dom.ts';

/** The list's local filter state, kept across the family's re-renders. */
let listSearch = '';
let listStatus: 'all' | 'solved' | 'open' | 'favorites' = 'all';

/** Renders the puzzles route family into its body: the tier grid, a tier's
 * collections — or its single collection's list outright — and the list a
 * collection names, with its filters, rows and pagination. An unknown tier or
 * collection lands on the grid. */
function renderPuzzles(next: Route) {
  if (next.kind !== 'puzzles' && next.kind !== 'puzzles-difficulty' && next.kind !== 'puzzles-collection') return;
  const body = $('puzzles-body');
  if (next.kind === 'puzzles') {
    body.replaceChildren(tierGrid());
    return;
  }
  if (!DIFFICULTIES.includes(next.difficulty)) {
    navigate({ kind: 'puzzles' });
    return;
  }
  const tier = filterEntries(ENTRIES, { difficulty: next.difficulty });
  const collections = [...new Set(tier.map(entry => entry.collection))];
  if (next.kind === 'puzzles-difficulty') {
    // A tier of one collection is that collection's list already.
    if (collections.length === 1) {
      body.replaceChildren(puzzleList({ difficulty: next.difficulty, collection: collections[0]! }, next.page, '#/puzzles'));
      return;
    }
    body.replaceChildren(collectionGrid(next.difficulty, collections));
    return;
  }
  if (!collections.includes(next.collection)) {
    navigate({ kind: 'puzzles-difficulty', difficulty: next.difficulty, page: 1 });
    return;
  }
  body.replaceChildren(
    puzzleList({ difficulty: next.difficulty, collection: next.collection }, next.page, `#/puzzles/${encodeURIComponent(next.difficulty)}`),
  );
}

/** The difficulty grid: one card per tier with its progress ring, plus the
 * next unsolved puzzle as a way in. */
function tierGrid(): HTMLElement {
  const grid = document.createElement('div');
  grid.className = 'selector-grid';
  for (const difficulty of DIFFICULTIES) {
    const tier = filterEntries(ENTRIES, { difficulty });
    const cleared = tier.filter(solved).length;
    const card = document.createElement('a');
    card.className = 'grid-card';
    card.href = `#/puzzles/${encodeURIComponent(difficulty)}`;
    card.append(ring(cleared, tier.length));
    const detail = document.createElement('div');
    const name = document.createElement('strong');
    name.textContent = difficulty;
    const line = document.createElement('span');
    line.textContent = `${cleared} of ${tier.length} cleared`;
    const meta = document.createElement('span');
    meta.textContent = `${new Set(tier.map(entry => entry.collection)).size} collections`;
    detail.append(name, line, meta);
    card.append(detail);
    grid.append(card);
  }
  const unsolved = ENTRIES.find(entry => !solved(entry)) ?? ENTRIES[0]!;
  const play = document.createElement('a');
  play.className = 'button primary';
  play.href = playHash(unsolved.id);
  play.textContent = 'Play next unsolved';
  const wrap = document.createElement('div');
  wrap.className = 'grid-wrap';
  wrap.append(grid, play);
  return wrap;
}

/** A tier's collections, one card each, with the way back to the tiers. */
function collectionGrid(difficulty: string, collections: string[]): HTMLElement {
  const grid = document.createElement('div');
  grid.className = 'selector-grid';
  for (const collection of collections) {
    const set = filterEntries(ENTRIES, { difficulty, collection });
    const cleared = set.filter(solved).length;
    const card = document.createElement('a');
    card.className = 'grid-card';
    card.href = `#/puzzles/${encodeURIComponent(difficulty)}/${encodeURIComponent(collection)}`;
    card.append(ring(cleared, set.length));
    const detail = document.createElement('div');
    const name = document.createElement('strong');
    name.textContent = collection;
    const line = document.createElement('span');
    line.textContent = `${cleared} of ${set.length} cleared`;
    detail.append(name, line);
    card.append(detail);
    grid.append(card);
  }
  const wrap = document.createElement('div');
  wrap.className = 'grid-wrap';
  wrap.append(backLink('#/puzzles', 'All difficulties'), grid);
  return wrap;
}

/** The filterable, paginated list of a tier and collection's puzzles. */
function puzzleList(by: { difficulty: string; collection: string }, page: number, back: string): HTMLElement {
  const wrap = document.createElement('div');
  wrap.className = 'list-wrap';
  wrap.append(backLink(back, 'Back'));
  const search = document.createElement('input');
  search.type = 'search';
  search.placeholder = 'Search puzzles';
  search.value = listSearch;
  search.id = 'puzzle-search';
  const chips = document.createElement('div');
  chips.className = 'chips';
  for (const status of ['all', 'solved', 'open', 'favorites'] as const) {
    const chip = document.createElement('button');
    chip.textContent = status === 'all' ? 'All' : status === 'solved' ? 'Cleared' : status === 'open' ? 'Open' : 'Favorites';
    chip.classList.toggle('on', status === listStatus);
    chip.onclick = () => {
      listStatus = status;
      navigate({ kind: 'puzzles-collection', ...by, page: 1 });
      // The same-route navigate emits nothing, so the list re-renders here.
      renderPuzzles(route());
    };
    chips.append(chip);
  }
  const bar = document.createElement('div');
  bar.className = 'list-bar';
  bar.append(search, chips);
  wrap.append(bar);
  const kept = filterEntries(ENTRIES, { ...by, search: listSearch, status: listStatus });
  const at = paginate(kept, page);
  const summary = document.createElement('p');
  summary.className = 'muted list-summary';
  const first = kept.length === 0 ? 0 : (at.page - 1) * PER_PAGE + 1;
  summary.textContent =
    kept.length === 0 ? 'No puzzles match.' : `Showing ${first}–${first + at.slice.length - 1} of ${kept.length} puzzles`;
  wrap.append(summary);
  const list = document.createElement('div');
  list.className = 'puzzle-list';
  for (const entry of at.slice) list.append(puzzleRow(entry));
  wrap.append(list);
  if (at.pages > 1) {
    const pages = document.createElement('div');
    pages.className = 'chips pages';
    for (let number = 1; number <= at.pages; number++) {
      const link = document.createElement('a');
      link.className = `chip${number === at.page ? ' on' : ''}`;
      link.href = `#/puzzles/${encodeURIComponent(by.difficulty)}/${encodeURIComponent(by.collection)}${number === 1 ? '' : `?page=${number}`}`;
      link.textContent = String(number);
      pages.append(link);
    }
    wrap.append(pages);
  }
  // The search re-renders the list as it narrows; the focus returns to it.
  search.oninput = () => {
    listSearch = search.value;
    renderPuzzles(route());
    $('puzzle-search')?.focus();
  };
  return wrap;
}

/** One list row: the minimap, the names and numbers, the best, the heart. */
function puzzleRow(entry: Entry): HTMLElement {
  const row = document.createElement('a');
  row.className = 'puzzle-row';
  row.href = playHash(entry.id);
  row.append(minimap(entry));
  const detail = document.createElement('div');
  detail.className = 'row-detail';
  const name = document.createElement('strong');
  name.textContent = entry.title;
  const meta = document.createElement('span');
  const best = storage.best(entry.id, entry.rows.join('\n'));
  meta.textContent = `${entry.width}×${entry.height} · ${entry.boxes} boxes${best === null ? '' : ` · best ${best.moves} moves`}`;
  detail.append(name, meta);
  row.append(detail);
  // The felt-difficulty mark the rating store holds, colored by its verdict.
  const felt = rating(entry.id);
  if (felt !== undefined) {
    const dot = document.createElement('span');
    dot.className = 'rating-dot';
    dot.dataset.rating = felt;
    dot.title = felt === 'easy' ? 'Felt easy' : felt === 'right' ? 'Just right' : 'Felt hard';
    row.append(dot);
  }
  const heart = document.createElement('button');
  const favorite = favorites().has(entry.id);
  heart.className = `heart${favorite ? ' on' : ''}`;
  heart.textContent = favorite ? '♥' : '♡';
  heart.title = 'Favorite';
  heart.onclick = event => {
    event.preventDefault();
    event.stopPropagation();
    toggleFavorite(entry.id);
    renderPuzzles(route());
  };
  row.append(heart);
  return row;
}

/** The progress ring a grid card shows: the cleared share of a donut. */
function ring(cleared: number, total: number): SVGSVGElement {
  const radius = 19;
  const circ = 2 * Math.PI * radius;
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 44 44');
  svg.classList.add('ring');
  const arcs: [string, number][] = [
    ['ring-back', 0],
    ['ring-arc', circ * (1 - (total === 0 ? 0 : cleared / total))],
  ];
  for (const [kind, offset] of arcs) {
    const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
    circle.setAttribute('cx', '22');
    circle.setAttribute('cy', '22');
    circle.setAttribute('r', String(radius));
    circle.setAttribute('class', kind);
    circle.style.strokeDasharray = String(circ);
    circle.style.strokeDashoffset = String(offset);
    svg.append(circle);
  }
  return svg;
}

/** A small way-back link. */
function backLink(href: string, text: string): HTMLElement {
  const link = document.createElement('a');
  link.className = 'back';
  link.href = href;
  link.textContent = `← ${text}`;
  return link;
}

export { renderPuzzles };
