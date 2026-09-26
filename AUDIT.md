# Project audit — 2026-09-26

Audited revision: `240517d`. Scope: all four Rust crates, TypeScript frontend and worker, build scripts, migrations, Docker/NGINX configuration, and existing tests. No application source was changed. Audit probes and raw measurements are under ignored `target/audit-probe/`.

The architecture is a good foundation for the stated goals. The highest-value work is to strengthen API invariants and automated validation, then improve search effectiveness under bounded memory. The measurements do not establish a speedup over the JS/TS reference: no matched reference benchmark was run.

## Evidence and validation

| Check | Result |
| --- | --- |
| Existing Rust suite | 83 tests passed: 11 core, 54 search, 18 server |
| Rust workspace check | Passed |
| Rust formatting check | Passed |
| Full WASM + TypeScript + Vite production build | Passed |
| Clippy | Not run successfully: component is absent from the pinned local toolchain |
| Native catalog probe | 57 puzzles × 3 modes; all 96 returned routes replayed successfully |
| Native/WASM parity probe | 57 puzzles × 3 modes at 20,000 nodes; status, expanded/generated counts, and best move counts matched; all 87 returned WASM routes replayed successfully |
| Live PostgreSQL, Docker, browser DOM/worker integration, dependency advisory scan | Not exercised |

The initial TypeScript check found stale generated WASM declarations (`moves`, `on_goal`, and the return type of `advance`). Regenerating bindings resolved all three errors. The first WASM link was blocked by sandbox permissions; the authorized build retry passed. Neither issue is evidence of a current Rust/TypeScript source incompatibility.

### Native catalog snapshot

One sequential release-mode pass, starting from each initial position, with 500,000 arena records, 64 MiB accounted budget, and 250 ms elapsed budget including search setup. Timings are local observations, not repeated statistical benchmarks or user-facing latency guarantees.

| Mode | Runs | Returned route | Finished solved | State limit | Time limit |
| --- | ---: | ---: | ---: | ---: | ---: |
| Fast | 57 | 35 | 35 | 10 | 12 |
| Quality | 57 | 36 | 25 | 27 | 5 |
| Optimal | 57 | 25 | 25 | 32 | 0 |

Quality can retain a verified route when a limit ends the run, explaining the difference between returned routes and finished runs.

All 32 state-limited Optimal runs stopped between approximately 101 and 207 ms. Grand Hall (`huge`) reached 500,000 records in every mode, without a route:

| Mode | Expanded | Generated | Search/setup elapsed |
| --- | ---: | ---: | ---: |
| Fast | 71,549 | 500,000 | 161.106 ms |
| Quality | 56,958 | 500,000 | 126.093 ms |
| Optimal | 25,647 | 500,000 | 115.743 ms |

Grand Hall accounted storage was 52,470,736 bytes. “Generated” counts arena records, including cheaper replacements of previously known states; it is not a unique-state count. Increasing the time budget alone cannot help these runs because they have already stopped at the node cap.

## What should be preserved

- Dependency-free core rules and search; platform-specific dependencies stay in adapters.
- One policy-driven expansion engine, with distinct exact-proof behavior.
- Dense cell IDs, precomputed neighbors, inline states, a reserved arena, index-based transposition table, and reusable reachability buffers.
- Label-compatible assignment estimates and incremental assignment repair.
- Immutable parent records for route reconstruction; replacing records in place could corrupt existing descendant paths.
- Strict replay, private live Game state, same-label canonicalization restricted to search, and replay checks before routes leave the solver.
- Native solve admission control, immediate busy responses, cooperative cancellation, worker termination, and throttled telemetry.
- Independent primitive-move BFS comparisons, seeded boards, interrupted-bound checks, and 42 frozen reference fixtures.

## Findings and recommended changes

### 1. P2 — Exact-search invariants can be bypassed through the public API

Sources: [Engine::stop](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/engine.rs:143), [ExactSearch mutable dereference](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/exact.rs:22), [Search mutable dereference](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/lib.rs:118), [proof construction](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/exact.rs:48).

Two concrete reproductions succeeded on a board solvable in one move:

1. In a release build, calling `exact.stop(Status::Exhausted)` immediately produced `Some(Unsolvable)`. The restriction on stop reasons is only a `debug_assert!`.
2. Safe `std::mem::swap(&mut *exact, &mut *fast)` replaces the exact wrapper's engine with a weighted engine. Its “certified” lower bound became 5 while the optimum remained 1.

These require misuse by a Rust caller. The current HTTP/WASM adapters use valid stop reasons, and no incorrect proof was observed through their normal execution paths. Nevertheless, the API does not enforce the invariant its exact type claims.

**Change:** replace `stop(Status)` with a restricted `StopReason` type or explicit `cancel()`/`timeout()` methods. Remove public mutable dereferencing to Engine and forward the small supported API explicitly. Keep policy mutation and engine replacement inaccessible.

**Acceptance:** release-mode regression for stop reasons, a compile-fail check for engine replacement, and the existing exact/BFS/bound suite.

### 2. P2 — Public geometry and unchecked start states weaken the reusable crate boundary

Sources: [Board and State](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/core/src/board.rs:9), [step](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/core/src/board.rs:175), [search construction](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/engine.rs:79).

Every Board field and State cell can be changed independently. A Rust caller can provide out-of-range cells, overlapping boxes, inconsistent geometry, unsorted labels, or a player on a box. Search construction returns Result but does not validate those invariants; later indexing assumes them.

The current adapters construct these values through parsing and replay, so this is a library maintainability issue rather than a demonstrated remotely reachable fault.

**Change:** make Board geometry private and expose read-only accessors. Validate an externally supplied State once at the public search boundary, including inactive box slots if they remain part of equality. Preserve compact internal states and unchecked-by-contract internal operations after validation. Consider a validated Position constructor rather than duplicating checks in every expansion.

**Acceptance:** malformed state inputs return a structured error; valid existing callers retain their behavior and performance.

### 3. High-value performance work — Reduce arena growth before tuning small operations

Sources: [state representation](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/core/src/board.rs:9), [arena](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/arena.rs:53), [successor generation](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/engine.rs:158).

The catalog experiment shows a capacity bottleneck for many puzzles, including Grand Hall. Faster expansions alone would reach the same cap sooner.

Start by adding unique-state count, duplicate improvements, reopenings, stale queue pops, peak queue length, and prune counts. Those distinguish excessive branching from repeated path improvements and weak pruning.

Evaluate these changes separately:

- Stronger admissible lower bounds and additional proven deadlock rules.
- Forced tunnel macros only when their conditions preserve legal alternatives, exact walking cost, and route reconstruction.
- An immutable board-analysis object for reverse distances and label groups. Reuse it only where repeated searches justify its retained memory.
- Compact per-board arena storage. Every node currently carries all 32 box cells, while the catalog has 1–22 boxes (mean approximately 8.32). At 500,000 nodes, unused box slots alone average roughly 22.6 MiB across catalog layouts. This is a theoretical storage opportunity, not a measured achievable saving; alignment, metadata, and access cost affect the result.
- Occupancy refresh that clears only previous box cells, or uses stamps, if profiling identifies [the full-board clear](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/src/deadlock.rs:26) as material.

Keep parent records stable when handling improved duplicates. Also preserve the player's exact position for the current total-move objective: merging all positions in a reachable region can lose relevant walking costs.

Do not simply raise every cap. First make the relationship between unique states, arena versions, memory budget, and search quality visible. Raising a configured node cap within a measured memory budget can then be evaluated as a separate tradeoff.

### 4. High priority — Make correctness and performance checks repeatable

Sources: [scripts](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/package.json:8), [pinned toolchain](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/rust-toolchain.toml:1), [search tests](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/search/tests/search.rs:46).

There is substantial solver testing, but no checked-in CI workflow, benchmark runner/baseline, browser integration suite, or live database integration suite. Clippy is also absent from the toolchain component list. The native/WASM parity probe in this audit is temporary, not a permanent guard.

**Change:** add a small CI pipeline that runs formatting, Clippy, Rust tests, a release proof regression, and the full generated WASM build. Preserve a fixed-node native/WASM parity test. Add browser scenarios for worker startup failure, cancellation, stale replies, playback/undo/reload, storage failures, and optional-server failure.

Add PostgreSQL integration checks for concurrent better/worse saves, fingerprint isolation, migration application, and locked/unavailable database behavior. There are already router and extractor tests; extend those with valid/invalid solve requests and busy-slot behavior.

For performance, preserve both deterministic node-budget cases and repeated timed cases. Record time to first route, final route length, proof gap, expanded/generated/unique counts, accounted bytes, and measured process/browser memory. Compare Rust native, WASM, and the reference on identical boards and positions. Accept an optimization only when it improves the intended measure without breaking replay or exact proof checks.

Ensure a fresh build generates WASM declarations before TypeScript validation. The existing full build already orders this correctly; a standalone `npm run check` does not.

### 5. Maintainability — Separate frontend lifecycles and validate transport data

Sources: [main.ts](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/web/src/main.ts:18), [solve](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/web/src/main.ts:316), [HTTP proof decoding](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/web/src/main.ts:387), [worker metrics decoding](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/web/src/solver.worker.ts:17).

The 616-line entry point owns puzzle loading, mutable game state, replay timers, worker/fetch cancellation, progress persistence, rendering updates, and input bindings. Its optional Run fields allow many invalid combinations, such as a busy run without a transport.

Extract a small solver client that normalizes browser/native replies; a playback controller; and a progress client. Leave orchestration and DOM bindings in a smaller entry point. Use a discriminated union for idle, browser-running, native-running, and completed states. Preserve the existing run-identity checks.

HTTP JSON currently becomes unchecked `any`, and every unknown proof kind falls through to “unsolvable.” Reject unknown proof kinds/statuses and invalid counters explicitly. Centralize the WASM metric tuple contract and use adapter tests to detect schema drift. A new framework or a large serialization dependency is not necessary.

Wrap worker construction and initial postMessage in the same failure lifecycle as asynchronous errors. A synchronous startup exception currently occurs after the UI becomes busy but before a watchdog is installed.

### 6. Deployment scalability — Bound progress work and database execution

Sources: [progress save](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/server/src/progress.rs:91), [database pool](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/server/src/main.rs:161), [retention sweep](C:/Users/Willp/Code/GitHub/Sokomind/SokomindRust/crates/server/src/main.rs:184).

Native solving has a concurrency semaphore. Progress replay uses the shared blocking pool without a separate global admission bound. Its per-address rate limit is helpful but does not bound aggregate concurrent work from many addresses. Tokio documents that blocking tasks queue once its large thread limit is reached and recommends limiting CPU concurrency; see [spawn_blocking documentation](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html).

The configured database timeout bounds connection acquisition. The progress SELECT/UPSERT and retention DELETE have no explicit application execution timeout or configured PostgreSQL statement/lock timeout. A lock wait or long query could occupy a connection well beyond the frontend's timeout. This was established by code inspection, not a live lock test.

**Change:** add a small separate admission budget for progress replay, and explicit database statement/lock timeouts with tested error mapping. Bound total waiting requests if traffic warrants it. Preserve the current solve semaphore and cancellation flag.

Retention is disabled by default, anonymous profiles can create new rows, and enabled retention uses one DELETE without an updated_at index. For public deployment, define storage policy, inspect the query plan on realistic data, add an appropriate index if justified, and delete in bounded batches.

Per-process rate limits and solve slots multiply with replicas. Document the intended aggregate CPU/memory/database budget before adding replicas. Add shared enforcement only when a deployment actually needs a global quota.

## Suggested implementation order

1. Enforce proof and position invariants; cover them with focused regressions.
2. Add CI and promote the parity/performance probes into maintained tooling.
3. Collect state-growth metrics and improve pruning/heuristics on the hard catalog cases.
4. Benchmark a compact arena independently of search-policy changes.
5. Extract frontend lifecycle modules with browser regression coverage.
6. Add database and progress backpressure before expanding public traffic.

This ordering preserves the project's small design while making its correctness, performance, and operating limits easier to verify.

## Reproducing the temporary audit probes

Run from the repository root:

```powershell
node scripts/rust.mjs run --release --offline --manifest-path target/audit-probe/Cargo.toml > target/audit-probe/results.jsonl
node scripts/rust.mjs run --release --offline --manifest-path target/audit-probe/Cargo.toml -- --parity > target/audit-probe/parity-native.jsonl
node target/audit-probe/wasm-parity.mjs
```

The WASM parity probe expects bindings from `npm.cmd run build`. Probe source and output are ignored build artifacts and will not survive deleting `target/`. They are evidence for this audit, not a replacement for committed regression tooling.

