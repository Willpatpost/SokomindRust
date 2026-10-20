/** How many particles each celebration tier bursts. */
const COUNTS = { clear: 8, best: 14, first: 20 } as const;

/** Bursts the celebration overlay: particles over the board, sized by how good
 * the solve was — most for a first clear, more for a personal best — each with
 * its own travel, delay, spin and hue, falling away on their own. The token
 * sheet's reduced-motion rules collapse the animation. */
export function celebrate(kind: keyof typeof COUNTS): void {
  const overlay = document.getElementById('celebrate');
  if (!overlay) return;
  overlay.replaceChildren(
    ...Array.from({ length: COUNTS[kind] }, () => {
      const particle = document.createElement('div');
      particle.className = `particle ${['square', 'leaf', 'dot'][Math.floor(Math.random() * 3)]}`;
      particle.style.setProperty('--x', `${Math.floor(Math.random() * 200) - 100}%`);
      particle.style.setProperty('--travel', `${60 + Math.floor(Math.random() * 60)}%`);
      particle.style.setProperty('--delay', `${Math.floor(Math.random() * 200)}ms`);
      particle.style.setProperty('--spin', `${Math.floor(Math.random() * 720) - 360}deg`);
      particle.style.setProperty('--hue', `${70 + Math.floor(Math.random() * 60)}`);
      return particle;
    }),
  );
  setTimeout(() => overlay.replaceChildren(), 1500);
}
