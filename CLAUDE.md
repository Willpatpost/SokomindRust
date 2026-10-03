# CLAUDE.md

## Purpose of this repository

SokomindRust exists primarily to build the Sokomind solver as well as it can reasonably be built.

This project is a Rust/WASM port and redesign of an older project- SokomindSolver. The older project accumulated architectural drift, duplicated ideas, experimental leftovers, and implementation decisions that were no longer close to the cleanest or most efficient design.

This port is an opportunity to correct that.

The goal is **not merely to reproduce the old solver in Rust**. The goal is to build a solver whose architecture, algorithms, memory use, execution speed, and maintainability are deliberately optimized.

When working in this repository, treat the solver as the product.

The UI exists to expose and exercise the solver, but UI/UX refinement is not currently a priority unless a frontend change is required to support, test, measure, or correctly expose solver behavior.

---

# The nine engineering priorities

Every meaningful change should be evaluated against these nine traits.

They are not superficial style preferences. They are core project goals.

## 1. Code cleanliness

Code should be easy to read, reason about, audit, and modify.

Prefer:

- clear ownership of responsibilities;
- direct data flow;
- explicit invariants;
- small and well-defined interfaces;
- meaningful names;
- minimal duplication;
- minimal hidden state;
- minimal unnecessary indirection;
- removal of obsolete code after an approach is replaced;
- code that explains itself structurally before comments are needed.

Avoid accumulating compatibility layers, abandoned experiments, temporary abstractions, duplicated algorithms, or alternate implementations without a strong reason.

Do not preserve bad structure merely because it already exists.

At the same time, do not refactor performance-sensitive code purely for aesthetic reasons if the resulting abstraction measurably harms performance. Clean code and fast code should normally reinforce one another, but measured performance takes precedence over arbitrary stylistic purity in hot paths.

---

## 2. Maintainability

A future contributor should be able to change one part of the solver without having to rediscover the entire system.

Maintain strong boundaries between:

- game rules;
- board representation;
- search state;
- reachability;
- heuristics;
- deadlock detection;
- queueing;
- transposition/state tables;
- memory management;
- proof logic;
- WASM transport;
- server transport;
- benchmarking and validation.

Avoid creating multiple sources of truth.

Important constants, wire formats, mirrored values, and invariants should either:

1. come from a shared source, or
2. have automated tests that ensure their copies stay synchronized.

Prefer designs that make invalid states difficult to construct and incorrect behavior difficult to express.

When complexity is unavoidable, localize it.

Large files or subsystems are acceptable when cohesion and performance justify them, but continuously consider whether independent concepts can be separated without adding runtime cost or obscuring their interaction.

---

## 3. Modularity

Each part of the solver should be replaceable, measurable, and testable on its own.

Maintainability names the boundaries. Modularity is how they are built and kept: which way dependencies point, how narrow each interface is, and what enforces both.

Dependencies point one way, as described under Architectural direction. The same rule holds inside `search`. Reachability, the heuristic, deadlock and corral detection, and the arena do not depend on the engine that composes them, except in tests, and their imports of one another form no cycle. Shared public types, such as statuses, errors, statistics, limits, and proofs, are exported from the crate root for any module to use.

Transports adapt; they do not decide. The WASM bindings, the server, and the web worker pass a mode and limits, capping the limits where they must, then drive the search and encode or decode its results. They never choose weights or phases, and never derive a bound or a proof.

Place new code where its concept is owned:

- heuristics, prunes, and tables in the `search` module that owns the concept, or in a new one;
- mode differences in policy data, mapped from the mode once in the public search type, and proof logic only in the exact search;
- each shared wire name once in the crate that owns its concept, such as statuses, modes, proof kinds, and counters in `search`, and each encoding in the transport or tool that emits it, so the `search` library needs no serialization.

Let the compiler enforce the seams.

Prefer:

- private modules behind a small public facade, with the narrowest visibility that works;
- types that make a forbidden call impossible, such as a public search type that hides its engine;
- exhaustive matches and destructuring, so a new variant or counter breaks the build at every site that matches or destructures it;
- a test or const assert for each hand-written list the compiler cannot check, such as an array of every mode, that fails when the list misses an entry;
- doctests that must fail to compile, pinning what callers must never write.

The compiler does not see copies outside Rust, such as those in `web`, the scripts, the docs, the migrations, and the deployment files. A clean build still needs the checks under Changes across mirrored layers and Native/WASM parity.

Replaceable does not mean **dynamic**. Search composes concrete types and constant policy data. Add a trait, generic parameter, or `dyn` object at a component boundary only when a second real implementation exists and the seam's hot-path cost is measured.

Tightly coupled structures, such as the node store, queue, and state table, may share one type or allocation when cohesion and performance justify it, as Maintainability allows. Keep each part's operations distinct, so one can change without rewriting the others.

Avoid:

- a dependency against the one-way direction, between crates, from a component to the engine, or in a cycle between components;
- a component that reaches into a sibling's fields or layout when a narrow accessor would do;
- a test or measurement switch stored in policy data, a public type, or a wire format.

---

## 4. Scalability

Design for puzzles substantially larger and more difficult than the easy catalog cases.

A solution that performs well only because the current test boards are small is not enough.

Consider how an implementation behaves as these increase:

- board cells;
- box count;
- label groups;
- reachable cells;
- generated states;
- duplicate states;
- queue size;
- transposition-table size;
- memory budget;
- search duration;
- solver concurrency;
- WASM linear memory pressure.

Prefer algorithms whose costs grow predictably and whose memory requirements can be bounded.

Do not introduce an optimization whose hidden complexity becomes pathological on large boards without documenting and measuring that tradeoff.

---

## 5. Lightweight

Keep the whole system small. That includes what it depends on, what it ships, what it reserves up front, and what it takes to build, run, and deploy.

Efficiency is about the work a search performs. Lightweight is about the weight the system carries regardless of the search: the cost paid before the first state is expanded, and the cost every user, build, and deployment pays whether or not a feature is used.

Pay attention to:

- third-party dependencies (crates and npm packages), including their transitive trees;
- WASM module size and web bundle size;
- native binary and container image size;
- startup and initialization cost;
- memory reserved before or independently of search (tables, arenas, buffers);
- per-solve setup cost on small boards;
- build time and toolchain requirements;
- services and moving parts required to run or deploy.

Prefer:

- the standard library or a small local implementation over a large dependency when the needed functionality is narrow;
- dependencies with small transitive trees, with only the features actually used;
- lazy allocation and precomputation sized to the board and the requested limits;
- small boards that cost little: memory and setup time should scale with the puzzle, not with the maximum limit;
- removing features, flags, tooling, and services that no longer earn their keep.

A dependency or subsystem must justify its weight. Before adding one, ask:

- What does it replace?
- What does it add to the WASM bundle, binary, image, build time, and audit surface?
- Would a small, well-tested local implementation do?

Lightweight does not mean reinventing well-solved, security-sensitive, or correctness-critical machinery. A mature dependency is often lighter overall than an unproven local replacement. Weigh the full cost.

When a change affects footprint, record it the same way as performance. Note WASM and bundle size, reserved and fixed memory, and dependency changes.

---

## 6. Efficiency

Treat unnecessary work, allocation, copying, hashing, scanning, and memory movement as things to eliminate.

Pay close attention to:

- asymptotic complexity;
- data structure size;
- cache locality;
- allocation frequency;
- temporary allocations;
- repeated scans;
- repeated flood fills;
- unnecessary cloning;
- unnecessary string creation;
- memory fragmentation;
- WASM linear-memory behavior;
- table load factors;
- queue-entry size;
- state-record size;
- duplicated calculations;
- work repeated across sibling states.

Reuse buffers and allocations when doing so materially reduces work or memory use.

Prefer precomputation when:

- it can be reused enough to justify its cost;
- its memory footprint is controlled;
- it reduces work in the search hot path.

Prefer incremental updates when they are simpler or materially cheaper than recomputation.

Do **not** interpret "reuse" as forcing unrelated local variables to share storage simply to save a few bytes. Reuse should target meaningful allocations, buffers, tables, and repeated computation without reducing clarity or correctness.

Memory is part of performance.

A change that runs faster but dramatically increases memory consumption is not automatically an improvement.

A change that saves memory but slows important searches dramatically is not automatically an improvement either.

Measure both.

---

## 7. Speed

Search speed and development-time execution speed both matter.

### Search/runtime speed

Optimize the operations that dominate real searches.

Examples include:

- state expansion;
- reachability;
- heuristic evaluation;
- deadlock checks;
- duplicate detection;
- queue operations;
- state-table lookup;
- state insertion;
- route reconstruction;
- memory growth.

Do not assume a theoretically clever optimization is useful. Measure it against realistic workloads.

A pruning rule that costs more than the work it prevents can make the solver worse.

A stronger heuristic that reduces expansions but takes too long to calculate can make the solver worse.

The relevant result is total useful solver performance under realistic limits.

### Workflow speed

When investigating or implementing changes:

- run independent investigations in parallel when practical;
- perform cheap checks before expensive ones;
- avoid rebuilding components unnecessarily;
- reuse benchmark output when valid;
- test the narrowest relevant layer first;
- run broader validation after local correctness is established.

The repository already contains tooling intended to avoid redundant work. Preserve and improve that behavior.

---

## 8. Documentation

Documentation is part of the implementation.

Important behavior should be understandable without reverse-engineering it from code.

Maintain documentation at two levels.

### Code documentation

Comments should explain:

- invariants;
- why an algorithm is sound;
- why an optimization exists;
- why an unusual implementation was chosen;
- memory-layout assumptions;
- performance-sensitive decisions;
- proof conditions;
- non-obvious edge cases;
- relationships between mirrored constants or protocols.

Avoid comments that merely restate the code.

When code looks strange because the obvious implementation was slower, unsafe, unsound, or more memory-hungry, document why.

### Project documentation

README and dedicated documentation should explain:

- architecture;
- solver behavior;
- search modes;
- benchmark methodology;
- memory limits;
- algorithmic decisions;
- important rejected approaches;
- deployment assumptions where relevant;
- how correctness and performance are validated.

If a subsystem becomes too complicated to explain comfortably inside source comments, create or expand a dedicated design document.

Rejected optimizations with meaningful experimental results are worth documenting. Future work should not repeatedly rediscover that the same idea was already measured and rejected.

---

## 9. Optimality of implementation

Outside the specific meaning of the solver's `Optimal` mode, "optimality" in this document means:

> Continuously seek a better implementation of the work the program already needs to perform.

Question whether operations can be:

- removed;
- combined;
- reordered;
- amortized;
- cached;
- made incremental;
- represented more compactly;
- performed fewer times;
- performed on less data;
- performed with better locality;
- deferred until necessary;
- avoided entirely for common cases.

Do not accept an implementation simply because it works.

Ask whether it is the right implementation.

However, avoid speculative complexity.

A simpler implementation that is equally fast or faster is preferable to a complicated theoretical optimization.

A complicated implementation is justified when its correctness is clear and its benefit is meaningful and measured.

---

# Correctness comes before optimization

No optimization is valuable if it makes the solver incorrect.

The hierarchy is:

1. correctness and proof soundness;
2. preservation of required behavior;
3. measurable solver performance;
4. memory efficiency;
5. architectural quality and maintainability;
6. secondary conveniences.

For `Optimal` mode especially:

**Never trade proof soundness for speed.**

False optimality or false unsolvability is a critical failure.

A slower correct proof is preferable to a faster incorrect proof.

---

# This is typed Sokoban

This project is not ordinary single-type Sokoban.

Boxes have types/labels.

Goals have corresponding types/labels.

A box is solved only when it is on a goal compatible with that box's label.

The current text representation follows the project's existing parser rules, including the special representation where `X` boxes use `S` goals.

Do not introduce algorithms that silently assume all boxes are interchangeable.

Equal-label boxes may be interchangeable where the solver deliberately canonicalizes them, but boxes belonging to different label groups are not interchangeable.

Any:

- assignment heuristic;
- dead-cell computation;
- deadlock rule;
- corral rule;
- pattern database;
- canonicalization;
- matching algorithm;
- goal-distance calculation

must respect box labels.

A technique that is sound for ordinary Sokoban is not automatically sound for typed Sokoban.

---

# The optimization objective is moves

The primary search objective is **total moves**, not pushes.

Walking matters.

A route with fewer pushes can still be worse if it requires more total movement.

Any technique borrowed from a push-optimal Sokoban solver must be re-evaluated before being used here.

In particular, do not assume that:

- push-optimal successor restriction;
- push-only dominance;
- push-based corral ordering;
- push-only lower bounds used incorrectly;
- irreversible goal commitments

remain sound when the objective is total moves.

The existing project has already encountered techniques that are safe under a push objective but unsafe under a move objective.

Be conservative here.

---

# The three search modes

The project has three distinct search goals.

They should not gradually collapse into one another.

## Fast

Goal:

> Find any valid solution as quickly as possible.

This mode is particularly important for very large or difficult puzzles where obtaining a route matters more than proving or heavily optimizing it.

Fast should prioritize:

- time to first route;
- low overhead;
- useful pruning;
- low memory waste;
- high expansion throughput.

It does **not** claim optimality.

Fast must never emit a proof that its route is optimal or that a puzzle is unsolvable unless the architecture is explicitly redesigned to make such a proof valid.

The current implementation uses weighted A* behavior for Fast. The exact weight is an implementation choice, not a sacred constant. Change it only with evidence.

---

## Quality

Goal:

> Find a solution quickly, then use the available time and memory to improve it as much as reasonably possible.

Quality should preserve Fast's ability to produce an early route while continuing to seek shorter routes.

The desired behavior is:

1. obtain a viable route quickly;
2. retain that route as an incumbent;
3. continue searching for improvements;
4. return the best verified route found when time or memory expires.

Quality should not become so expensive before the first route that it loses its reason for existing.

Quality also does **not** claim optimality unless it is eventually backed by the same kind of sound proof machinery required by Optimal mode.

The current implementation transitions from a more aggressive Fast phase to a lighter weighted search and can restart its arena while retaining its incumbent. These are implementation details that may be improved if measurements justify the change.

---

## Optimal

Goal:

> Find the shortest valid solution in total moves as efficiently as possible and prove that it is optimal.

Optimal mode has the strictest correctness requirements.

A returned optimal route must be proven.

An unsolvable result must be proven.

If limits stop the search before a proof is complete, the solver may return:

- a verified incumbent;
- a sound lower bound;
- a bounded result;

but it must not pretend the result is proven optimal.

Optimal search must use only sound:

- heuristics;
- pruning;
- dominance rules;
- deadlock detection;
- canonicalization;
- lower bounds.

Anything that could eliminate a valid shorter route is forbidden from the exact search.

---

# Proof separation is intentional

Only exact/Optimal search should be allowed to produce proofs unless another mode is explicitly redesigned around equivalent soundness guarantees.

Preserve this architectural separation.

Do not add a convenient `proof = optimal` result to Fast or Quality merely because a route looks good, matches a known result, or no better route was found within the budget.

"Nothing better was found" is not proof.

Likewise:

- timing out is not proof;
- reaching a memory limit is not proof;
- matching the benchmark's best route is not proof;
- matching another solver is not proof.

---

# Every reported route must be valid

Routes returned by a search should be replay-verifiable under the actual Rust game rules.

Preserve the existing philosophy that routes crossing trust boundaries are checked rather than blindly trusted.

The browser, server, persistence layer, and solver should not rely on claimed route metadata when the route itself can be replayed.

If the search reports:

- move count;
- push count;
- solved state;
- proof information;

those must remain consistent with the route and the game rules.

---

# Rust is the source of truth for game logic

Do not duplicate Sokoban rules in TypeScript or another layer.

The core Rust crate should remain the authoritative implementation of:

- parsing;
- state transitions;
- legal moves;
- pushes;
- solved-state checks;
- route replay;
- labels and goals;
- board invariants.

WASM and server layers should be thin interfaces around the same core behavior.

Frontend code may render, transport, schedule, and validate wire formats, but it should not independently implement the puzzle rules.

---

# Architectural direction

The intended dependency direction is approximately:

`core -> search -> wasm/server -> web`

Where:

- `core` owns the puzzle and game rules;
- `search` owns the solver;
- `wasm` exposes the solver/game to the browser;
- `server` exposes native solving and persistence;
- `web` drives the UI and transport.

Keep solver algorithms platform-independent when possible.

Do not put browser-specific or HTTP-specific behavior into the search engine.

Do not make the solver depend on frontend details.

---

# Solver-first scope

Current development emphasis is the solver.

Do not spend significant effort redesigning:

- visual styling;
- animations;
- layout;
- cosmetic controls;
- general UX polish;
- frontend framework architecture;

unless the change is necessary to:

- expose a solver capability;
- debug the solver;
- test the solver;
- benchmark the solver;
- correctly represent solver state;
- improve development or validation infrastructure.

A solver improvement is generally more valuable than cosmetic polish at this stage.

---

# Performance work must be evidence-driven

Do not keep an optimization because it sounds good.

Measure it.

When evaluating a solver change, consider at least:

- time to first route;
- final route length;
- lower bound;
- proof status;
- expanded states;
- generated states;
- duplicate behavior;
- pruning counters;
- peak queue;
- reserved memory;
- actual memory behavior where measurable;
- search time;
- behavior under different state limits;
- behavior under different memory limits;
- behavior across puzzle sizes;
- Fast;
- Quality;
- Optimal.

A change may help one mode and hurt another.

That is not automatically bad, but it must be understood.

A Fast-specific optimization does not need to improve Optimal if it remains isolated from Optimal.

A heuristic or prune shared by all modes needs stronger justification.

---

# Do not optimize from one puzzle

Never judge a solver change using a single favorable board.

Use representative sets.

Include:

- trivial boards;
- small boards;
- medium boards;
- large boards;
- memory-bound boards;
- difficult boards;
- typed/multi-label boards;
- cases where the optimization is expected to trigger;
- cases where it should not trigger.

Look for aggregate results and worst regressions.

A 5% aggregate improvement that causes a catastrophic regression on an important class of puzzles may not be an improvement.

---

# Benchmark discipline

Benchmark baselines are evidence, not obstacles.

Do not "fix" benchmark failures by blindly rebaselining.

When a baseline changes:

1. identify why;
2. verify route/proof correctness;
3. understand performance movement;
4. determine whether the change is intentional;
5. document meaningful algorithmic changes;
6. only then update the baseline.

Correctness regressions must never be accepted merely to make a benchmark gate green.

A performance regression may sometimes be justified by a larger correctness or capability improvement, but it must be explicit.

---

# Native/WASM parity

The native and WASM implementations expose the same underlying solver and should remain behaviorally consistent where their limits and environments permit comparison.

Preserve parity checks.

Be especially careful with:

- integer width;
- memory ceilings;
- linear-memory growth;
- route reconstruction;
- wire formats;
- search statistics;
- proof kinds;
- status values.

An optimization that is excellent natively but disastrous under WASM linear memory should be treated as incomplete.

---

# Memory is a first-class constraint

Search memory must remain budgetable and predictable.

Avoid designs that accidentally allocate far beyond the declared memory budget.

Pay particular attention to:

- temporary peak allocation during resizing;
- multiple simultaneous hash tables;
- queues sized above useful capacity;
- vectors that retain unnecessarily large capacity;
- WASM memory that cannot shrink;
- allocations repeated per state;
- structures whose per-state cost grows unexpectedly with box count.

Prefer bounded structures.

Where possible, calculate and document important per-state and fixed memory costs.

---

# Hot-path rules

Search hot paths deserve special scrutiny.

Before adding work to every generated or expanded state, ask:

- How often does this run?
- How much does it prune or improve the heuristic?
- Can it be gated cheaply?
- Can it be incremental?
- Can part of it be precomputed?
- Can a common case return early?
- Can scratch memory be reused?
- Does it improve total wall-clock performance?

An operation that costs only a few microseconds can still dominate a search when executed millions of times.

---

# Allocation rules

Avoid per-node heap allocation whenever practical.

Prefer:

- compact records;
- indices instead of owning pointers where suitable;
- reusable scratch buffers;
- preallocated vectors;
- arenas;
- flat arrays;
- typed integer representations sized to the actual domain.

Do not reduce integer widths blindly. Ensure the complete supported range fits and retain explicit sentinel space where required.

Use checked, saturating, or otherwise intentional arithmetic when limits could be approached.

---

# Canonicalization

Canonicalization is valuable when it safely reduces duplicate search states.

For typed Sokoban:

- only boxes with equivalent labels may be freely canonicalized together;
- different labels must remain distinct;
- any canonicalization must preserve all information necessary for the move objective.

Do not canonicalize away keeper/player information if the remaining move cost depends on it.

State identity must be strong enough for the cost model being optimized.

---

# Deadlocks and pruning

Pruning can provide enormous gains, but an unsound prune can destroy the solver.

Any new prune intended for Optimal mode must have a clear soundness argument.

Prefer:

- simple local proofs;
- monotonic conditions;
- well-defined conservative approximations.

Be suspicious of rules that depend on assumptions such as:

- another box will never move;
- the keeper cannot later reach an area;
- a box committed to a goal never needs to leave it;
- a corral boundary is permanent;
- push-optimal reasoning remains valid for move-optimal search.

If soundness is uncertain, do not enable the rule in Optimal mode.

It may still be possible to experiment with an explicitly non-proof mode, but that distinction must be clear.

---

# Heuristics

For Optimal mode, a heuristic used in A* must remain admissible.

If consistency is not guaranteed, the search architecture must correctly support reopenings.

Heuristic strength alone is not the goal.

Evaluate:

`total search time = states explored × cost per state`

A stronger heuristic that is disproportionately expensive can lose overall.

When possible, update heuristics incrementally after a push rather than recomputing unaffected information.

---

# Fast and Quality may be more aggressive

Fast and Quality do not provide proofs, so they may use strategies that are inappropriate for Optimal mode if those strategies preserve route validity.

However:

- do not knowingly make them incomplete without explicitly documenting the tradeoff;
- do not let heuristic-mode shortcuts leak into proof-producing code;
- keep policy differences structurally clear.

Shared infrastructure is good.

Shared unsound assumptions are not.

---

# Prefer measured special cases

Common-case fast paths are encouraged when:

- they are simple;
- their result is exactly equivalent to the general algorithm;
- they meaningfully reduce hot-path work;
- they are tested against the general implementation.

For example, if a common configuration can be answered without running a general fixpoint or assignment algorithm, a verified shortcut may be worthwhile.

These optimizations are often preferable to large new subsystems.

---

# Parallelism

Use parallelism where it genuinely improves throughput or development speed.

Do not introduce parallel search merely because more cores exist.

Parallel solver architecture must account for:

- duplicated search memory;
- synchronization cost;
- cache contention;
- nondeterminism;
- proof correctness;
- duplicate work;
- WASM/browser constraints.

Parallelize independent benchmarking, investigation, preprocessing, or tooling tasks when practical.

For the solver itself, require evidence that parallelism improves the relevant workload enough to justify its complexity and memory cost.

---

# Testing expectations

A solver change should normally have tests at the lowest layer capable of proving its behavior.

Use:

- unit tests for local invariants;
- regression fixtures for known bugs;
- boundary tests for limits;
- differential tests when two implementations should agree;
- replay verification for routes;
- exact small-board checks when exhaustive verification is feasible;
- catalog/corpus checks for broad regressions;
- native/WASM parity tests where appropriate;
- release-mode tests for behavior sensitive to overflow or debug assertions.

When fixing a bug, add a test that would have caught it.

---

# Validation workflow

Prefer this progression while developing:

1. inspect the relevant code and existing tests;
2. make the smallest coherent change;
3. run the narrowest relevant test;
4. run formatting/linting/type checks as appropriate;
5. run relevant search tests;
6. run benchmark/parity checks for solver changes;
7. run the broader validation suite before considering the work complete.

Use the repository's existing validation tooling rather than inventing redundant command sequences.

Do not skip expensive validation merely because a change "looks safe" when it affects:

- search behavior;
- memory layout;
- proofs;
- pruning;
- heuristics;
- state identity;
- WASM ABI;
- benchmark output.

---

# Documentation of experiments

Performance experiments should leave enough information to reproduce the conclusion.

Record:

- what changed;
- which baseline was used;
- which puzzles were tested;
- relevant limits;
- whether runs were repeated;
- important aggregate results;
- meaningful regressions;
- why the change was accepted or rejected.

Do not leave experimental implementations permanently in production code after they have been rejected unless they are intentionally guarded for future research.

The documentation can preserve the idea without preserving dead code.

---

# Avoid algorithmic drift

One reason for this port is that the previous project drifted away from a clean, optimal implementation.

Prevent that from happening again.

Before adding a new mechanism, ask:

- Is this solving a demonstrated problem?
- Does equivalent machinery already exist?
- Can the same result be achieved by simplifying something?
- Does this duplicate another code path?
- Will future contributors know which version is authoritative?
- Does it create long-term state that must now be maintained?
- Can the idea be measured before becoming architecture?

Prefer deleting complexity over adding compensating complexity.

---

# Do not optimize obsolete architecture

If a subsystem is fundamentally structured poorly, do not spend excessive effort micro-optimizing around that structure.

Consider whether the underlying representation or algorithm should change.

Examples:

- shrinking fields inside a state representation may matter more than optimizing a minor loop;
- eliminating an entire repeated flood may matter more than shaving instructions from the flood;
- changing lookup structure may matter more than micro-optimizing its hash;
- avoiding an allocation is usually better than making the allocation slightly faster.

Look for leverage.

---

# Backward compatibility

This project is still evolving.

Do not preserve internal APIs, benchmark schemas, wire formats, or architectural mistakes indefinitely merely for backward compatibility unless they are already externally relied upon.

When breaking an internal interface produces a materially better design:

1. update all consumers;
2. update tests;
3. update documentation;
4. remove the obsolete interface.

Do not leave duplicate old/new paths without necessity.

User data and persisted formats require greater care than internal APIs.

---

# Changes across mirrored layers

When changing a value or protocol shared across Rust, WASM, TypeScript, server configuration, documentation, Docker, or NGINX, identify every mirror.

Examples include:

- route limits;
- memory limits;
- statuses;
- proof kinds;
- search modes;
- metrics layouts;
- timeout chains;
- toolchain versions.

Where feasible, add or preserve automated mirror checks.

Do not rely solely on comments saying "change these together."

---

# UI changes

Keep UI changes functional and minimal unless explicitly asked to work on UX.

Good UI work at this stage includes:

- exposing a new solver option;
- showing useful solver diagnostics;
- representing proof status correctly;
- enabling benchmarking/debugging;
- preventing invalid configuration;
- improving testability.

Avoid spending solver-development time on cosmetic redesign.

---

# What not to do

Do not:

- duplicate game rules in JavaScript/TypeScript;
- claim optimality without proof;
- claim unsolvability without proof;
- use an unsound prune in Optimal mode;
- assume ordinary Sokoban rules when typed labels matter;
- assume push-optimal reasoning is valid for move-optimal search;
- rebaseline benchmarks to hide regressions;
- optimize only one convenient puzzle;
- add per-state allocation casually;
- ignore WASM memory behavior;
- retain dead experimental code without reason;
- add abstraction solely for abstraction's sake;
- sacrifice readability for meaningless micro-optimizations;
- sacrifice meaningful performance for cosmetic cleanliness;
- change solver behavior without tests;
- allow documentation describing the solver to drift away from reality.

---

# How to judge a proposed change

Before accepting a meaningful solver change, be able to answer:

### Correctness
- Is it correct for typed Sokoban?
- Is it correct under a total-move objective?
- If used by Optimal, is it sound?
- Can it affect proofs?

### Performance
- Does it improve real wall-clock behavior?
- Does it improve first-route time, final route quality, proof speed, or memory?
- On which puzzle classes?
- What does it make worse?

### Efficiency
- Does it reduce repeated work?
- Does it improve allocation behavior or locality?
- What is its fixed and per-state memory cost?

### Lightweight
- Does it add dependencies, and are they worth their transitive weight?
- What does it do to WASM, bundle, and binary size?
- Does it reserve memory or do setup work that small boards do not need?

### Maintainability
- Is there now one clear implementation?
- Are the invariants understandable?
- Is the complexity localized?

### Modularity
- Do dependencies still point one way, with none of the parts the engine composes depending on it?
- Does the change live in the modules that own its concepts, with each new item as private as it can be?
- Can the component be tested and measured on its own, and its internals changed without editing its callers?

### Scalability
- What happens on large boards and high state counts?
- Does its cost explode with box count, goal count, or label count?

### Documentation
- Will a future contributor understand why this exists?
- Were meaningful measurements and tradeoffs recorded?

If these questions cannot be answered, the change probably needs more investigation.

---

# Definition of done for solver work

A solver change is not complete merely because it compiles.

It should normally be:

- correct;
- replay-verified;
- tested;
- formatted;
- lint-clean;
- documented where non-obvious;
- benchmarked when performance-sensitive;
- checked for memory impact;
- checked for footprint impact (dependencies, WASM size) when it adds or removes code paths;
- checked against Fast, Quality, and Optimal as relevant;
- checked for native/WASM implications;
- placed in the modules that own its concepts, with dependencies still pointing one way;
- free of abandoned implementation paths;
- reflected accurately in project documentation.

The final result should leave the solver not only more capable, but also easier to understand and improve next time.

---

# Guiding principle

When choosing between two correct designs, prefer the one that does less work, carries less weight, uses less unnecessary memory, exposes clearer invariants, scales better, and is easier to verify.

When those goals conflict, measure the tradeoff.

The purpose of SokomindRust is not simply to make Sokomind work in Rust.

The purpose is to build the solver deliberately, cleanly, and as close to the best implementation we can justify with correctness and evidence.