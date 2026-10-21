import { formatHash, type Route, route, subscribe } from './router.ts';

/** Every page's section in index.html, by its data-page attribute. */
const PAGES = ['home', 'puzzles', 'play', 'editor', 'stats', 'solver-lab'] as const;
export type Page = (typeof PAGES)[number];

/** How deep each page sits: a deeper page slides in from the right, a shallower
 * one from the left, the ported design's page transitions. */
const DEPTH: Record<Page, number> = { home: 0, puzzles: 1, play: 2, editor: 1, stats: 1, 'solver-lab': 1 };

/** The section a route shows: the puzzles family shares one, its drill-downs
 * re-rendering inside it. */
function pageOf(next: Route): Page {
  if (next.kind === 'puzzles-difficulty' || next.kind === 'puzzles-collection') return 'puzzles';
  return next.kind;
}

/** What the route announcer calls each page. */
const TITLES: Record<Page, string> = {
  home: 'Home',
  puzzles: 'Puzzles',
  play: 'Play',
  editor: 'The puzzle editor',
  stats: 'Statistics',
  'solver-lab': 'Solver Lab',
};

/** A page's mount, called with the route every time the page shows. Pages that
 * are static markup register no hook. */
type Hook = (route: Route) => void;
const hooks = new Map<Page, Hook>();

/** Registers `hook` as `page`'s mount. */
export function onPage(page: Page, hook: Hook): void {
  hooks.set(page, hook);
}

/** Whether the resolved theme reduces motion, which the theme module keeps on
 * the html element the token sheet reads. */
function reducedMotion(): boolean {
  return document.documentElement.dataset.motion === 'reduced';
}

/** Announces through the page's live region, leaving identical text alone so a
 * repeat is not re-announced. */
function announce(text: string): void {
  const region = document.getElementById('announcer');
  if (region && region.textContent !== text) region.textContent = text;
}

/** Runs the entering section's directional slide. tokens.css styles the classes;
 * they come off once the transition has had its window, so the next navigation
 * can apply a fresh entering state. */
function slide(section: HTMLElement, direction: 'forward' | 'back'): void {
  section.classList.add(`page-enter-${direction}`);
  // Read a layout property so the browser applies the entering state first.
  void section.offsetWidth;
  section.classList.add('page-enter-active');
  setTimeout(() => section.classList.remove(`page-enter-${direction}`, 'page-enter-active'), 260);
}

/** Wires the frame around the pages: each route shows its section and hides the
 * others, announces the page and focuses its heading, syncs the navigation's
 * active tab, runs the page's hook, and Escape walks history back unless an open
 * menu or dialog owns the key. The to-top button appears once the page scrolls. */
export function startShell(): void {
  let previous: Page | undefined;
  subscribe(next => {
    const page = pageOf(next);
    const entering = document.querySelector<HTMLElement>(`[data-page="${page}"]`);
    if (!entering) return;
    for (const name of PAGES) document.querySelector(`[data-page="${name}"]`)?.toggleAttribute('hidden', name !== page);
    if (previous !== page) {
      if (previous !== undefined) {
        slide(entering, DEPTH[page] >= DEPTH[previous] ? 'forward' : 'back');
        scrollTo({ top: 0, behavior: reducedMotion() ? 'auto' : 'smooth' });
      }
      announce(`Navigated to ${TITLES[page]}`);
      entering.querySelector('h1')?.focus({ preventScroll: true });
    }
    for (const control of document.querySelectorAll('[data-nav]')) {
      if (control.getAttribute('data-nav') === page) control.setAttribute('aria-current', 'page');
      else control.removeAttribute('aria-current');
    }
    previous = page;
    hooks.get(page)?.(next);
  });
  document.addEventListener('keydown', event => {
    if (event.key !== 'Escape' || route().kind === 'home') return;
    const open = document.querySelector('details[open], dialog[open]');
    if (open) {
      // A dialog must close through close(): removing the open attribute
      // hides it without ending the modal session, and the page behind
      // stays inert — no click lands until the next navigation.
      if (open instanceof HTMLDialogElement) open.close();
      else open.removeAttribute('open');
      return;
    }
    history.back();
  });
  const top = document.getElementById('to-top');
  if (top) {
    const toggle = () => top.classList.toggle('shown', scrollY > 400);
    addEventListener('scroll', toggle, { passive: true });
    toggle();
    top.onclick = () => scrollTo({ top: 0, behavior: reducedMotion() ? 'auto' : 'smooth' });
  }
}

/** The hash a puzzle plays at, for links the shell's pages build. */
export function playHash(puzzleId: string): string {
  return formatHash({ kind: 'play', puzzleId });
}
