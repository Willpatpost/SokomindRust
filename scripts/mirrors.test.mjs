import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Limits that one language copies from another by hand, each beside a comment
// naming the original. The checks read the source text, so no build is needed.

/**
 * The number a repo file's first match of `pattern` captures, digit separators removed.
 * @param {string} path Relative to the repo root.
 * @param {RegExp} pattern One capture group around the number.
 * @returns {number}
 */
function read(path, pattern) {
  const value = pattern.exec(readFileSync(new URL(`../${path}`, import.meta.url), 'utf8'))?.[1];
  assert.ok(value, `${path} no longer matches ${pattern}`);
  return Number(value.replaceAll('_', ''));
}

test("nginx's client_max_body_size equals the server's BODY_LIMIT, so nginx passes every body the API accepts", () => {
  assert.equal(
    read('deploy/nginx.conf', /client_max_body_size (\d+)k;/),
    read('crates/server/src/main.rs', /const BODY_LIMIT: usize = (\d+) \* 1024;/),
  );
});

test("the puzzle textarea's maxlength is Board::parse's text cap, MAX_CELLS * 2 bytes", () => {
  assert.equal(
    read('web/index.html', /id="rows"[^>]*maxlength="(\d+)"/),
    2 * read('crates/core/src/board.rs', /pub const MAX_CELLS: usize = ([\d_]+);/),
  );
});

test("protocol.ts's MAX_ROUTE equals sokomind_core::MAX_ROUTE", () => {
  assert.equal(
    read('web/src/protocol.ts', /export const MAX_ROUTE = ([\d_]+);/),
    read('crates/core/src/game.rs', /pub const MAX_ROUTE: usize = ([\d_]+);/),
  );
});
