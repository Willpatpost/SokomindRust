// The repo installs no @types/node, so this declares just the Node APIs the
// unit tests and playwright.config.ts use, loosely enough for type checking.
// Delete it and list "node" in tsconfig.json's types if @types/node is added.
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
