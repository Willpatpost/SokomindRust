import type { Scheduler, WorkerPort } from '../src/solver-client.ts';
import type { WorkerRequest } from '../src/protocol.ts';
export class Clock implements Scheduler {
  time = 0;
  next = 1;
  tasks = new Map<number, { callback: () => void; at: number; interval?: number }>();
  now() { return this.time; }
  interval(callback: () => void, ms: number) {
    const id = this.next++; this.tasks.set(id, { callback, at: this.time + ms, interval: ms }); return id;
  }
  timeout(callback: () => void, ms: number) {
    const id = this.next++; this.tasks.set(id, { callback, at: this.time + ms }); return id;
  }
  clearInterval(id: number) { this.tasks.delete(id); }
  clearTimeout(id: number) { this.tasks.delete(id); }
  advance(ms: number) {
    const end = this.time + ms;
    while (true) {
      const next = [...this.tasks].filter(([, task]) => task.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      const [id, task] = next; this.time = task.at;
      if (task.interval) task.at += task.interval; else this.tasks.delete(id);
      task.callback();
    }
    this.time = end;
  }
}
export class Worker implements WorkerPort {
  onmessage: WorkerPort['onmessage'] = null;
  onerror: WorkerPort['onerror'] = null;
  terminated = false;
  messages: WorkerRequest[] = [];
  failPost = false;
  postMessage(message: WorkerRequest) {
    if (this.failPost) throw new Error('post failed');
    this.messages.push(message);
  }
  terminate() { this.terminated = true; }
  reply(data: unknown) { this.onmessage?.({ data } as MessageEvent<unknown>); }
}
export const progress = (route?: string) => ({
  type: 'progress', elapsedMs: 12.5, route,
  metrics: { expanded: 2, generated: 4, reservedBytes: 128,
    best: route?.length, status: 'running', proof: { kind: 'none' } },
});
export const native = () => ({ status: 'solved', route: 'D', moves: 1, pushes: 1,
  expanded: 2, generated: 3, reserved_bytes: 128, elapsed_ms: 10,
  proof: { kind: 'optimal', lower_bound: 1, upper_bound: 1 } });
export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
