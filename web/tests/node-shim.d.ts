// @types/node is installed for scripts/ only: tsconfig.json's empty types keeps it
// out of here, so Node globals never type-check in the src modules these tests
// import. This declares just the Node APIs the unit tests and playwright.config.ts
// use, loosely enough for type checking.
declare module 'node:test' {
  export function test(name: string, fn: () => void | Promise<void>): Promise<void>;
}
declare module 'node:assert/strict' {
  interface Assert {
    equal(actual: unknown, expected: unknown, message?: string): void;
    deepEqual(actual: unknown, expected: unknown, message?: string): void;
    ok(value: unknown, message?: string): void;
    match(value: string, pattern: RegExp, message?: string): void;
    throws(fn: () => unknown, error?: RegExp, message?: string): void;
    rejects(promise: Promise<unknown> | (() => Promise<unknown>), error?: RegExp, message?: string): Promise<void>;
  }
  const assert: Assert;
  export default assert;
}
declare function setImmediate(callback: (...args: unknown[]) => void): unknown;
declare const process: { env: Record<string, string | undefined> };
