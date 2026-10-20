import { browserScheduler, type Scheduler } from './scheduler.ts';
import { key } from './storage.ts';

/** How a timer reads: M:SS, or H:MM:SS once an hour has passed. */
export function clock(ms: number): string {
  const total = Math.floor(ms / 1000);
  const seconds = total % 60;
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  const two = (value: number) => String(value).padStart(2, '0');
  return hours === 0 ? `${minutes}:${two(seconds)}` : `${hours}:${two(minutes)}:${two(seconds)}`;
}

/** A puzzle's accumulated play time. The page starts and holds the clock by
 * its attempt policy — first move to solve, replays held, tab hidden — and
 * keeps what it accumulated in sessionStorage under the puzzle, so a reload
 * resumes it. */
export class Timer {
  private scheduler: Scheduler;
  private show: (ms: number) => void;
  private accumulated = 0;
  private resumedAt: number | undefined;
  private puzzleId = '';
  private handle: number;

  constructor(options: { scheduler?: Scheduler; show: (ms: number) => void }) {
    this.scheduler = options.scheduler ?? browserScheduler;
    this.show = options.show;
    this.handle = this.scheduler.interval(() => this.show(this.elapsed()), 1000);
  }

  /** The time played so far, the running stretch included. */
  elapsed(): number {
    return this.accumulated + (this.resumedAt === undefined ? 0 : this.scheduler.now() - this.resumedAt);
  }

  /** Starts the clock, or resumes a paused one. */
  run(): void {
    if (this.resumedAt === undefined) this.resumedAt = this.scheduler.now();
  }

  /** Stops the clock, folding the running stretch into the accumulated time. */
  pause(): void {
    if (this.resumedAt === undefined) return;
    this.accumulated = this.elapsed();
    this.resumedAt = undefined;
    this.store();
  }

  /** Switches to `puzzleId`, storing the old puzzle's time and restoring the
   * new one's. The clock comes up running. */
  select(puzzleId: string): void {
    this.pause();
    this.puzzleId = puzzleId;
    this.accumulated = read(puzzleId);
    this.resumedAt = this.scheduler.now();
    this.show(this.elapsed());
  }

  /** Clears `puzzleId`'s time: a restart is a fresh attempt, whose clock starts
   * over. */
  reset(puzzleId: string): void {
    this.pause();
    if (this.puzzleId === puzzleId) {
      this.accumulated = 0;
      this.show(0);
    }
    try {
      sessionStorage.removeItem(key(`timer.${puzzleId}`));
    } catch {
      /* A blocked session store loses the reset; play continues. */
    }
  }

  /** Stops the clock and the tick; the page teardown path. */
  stop(): void {
    this.pause();
    this.scheduler.clearInterval(this.handle);
  }

  private store(): void {
    try {
      sessionStorage.setItem(key(`timer.${this.puzzleId}`), String(this.accumulated));
    } catch {
      /* A full or blocked session store loses the timer; play continues. */
    }
  }
}

function read(puzzleId: string): number {
  try {
    const value = Number(sessionStorage.getItem(key(`timer.${puzzleId}`)));
    return Number.isFinite(value) && value >= 0 ? value : 0;
  } catch {
    return 0;
  }
}
