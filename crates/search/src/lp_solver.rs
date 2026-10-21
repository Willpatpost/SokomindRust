//! A revised primal simplex over a product-form inverse, built for the
//! walk LPs: sparse integral data, costs and rhs nonnegative. The
//! two-phase start sits on the artificial identity basis, whose
//! inverse is empty, so the eta file begins at zero length and grows
//! one sparse eta per pivot. Dantzig pricing over incrementally
//! maintained reduced costs with a Bland window as anti-cycle
//! insurance; an exact-minimum ratio test that breaks ties toward
//! the largest pivot — the anti-drift rule the dense tableau
//! died without; periodic refreshes of the basic values and the
//! reduced costs through the eta chains, so no more than `REFRESH`
//! pivots of update error can accumulate. The solver errs out rather
//! than returning a wrong number: unboundedness, a stability check,
//! or the duals guard (`y·b` must reproduce the objective) all fail
//! loud. The duals come out as a fresh BTRAN of the basic costs — the
//! row potentials the dual-transfer term carries.
//!
//! A solve is resumable: [`SolveBuild::step`] spends a caller-paid
//! budget of units — one per pass, a pivot with the refresh due beside
//! it — and returns between batches, so a caller that must answer a
//! deadline between its own batches can carry a solve that runs
//! minutes (see `walk_term::WalkBuild`); [`solve`], the one-shot
//! reference the tests pin the stepping against, runs the same build
//! to its end, pivoting exactly as the resumable one does.

/// A sparse LP: minimize `c·x` over `A x = b`, `x ≥ 0`, with `b ≥ 0`.
/// Columns are CSC; the rows are equalities.
pub(crate) struct Lp {
    pub(crate) m: usize,
    pub(crate) n: usize,
    /// CSC column starts, length `n + 1`.
    pub(crate) starts: Vec<usize>,
    /// CSC row indices and values, column-major.
    pub(crate) index: Vec<usize>,
    pub(crate) value: Vec<f64>,
    pub(crate) b: Vec<f64>,
    pub(crate) c: Vec<f64>,
}

/// A solved LP: the optimal objective, the row duals, the primal
/// solution over the structural columns, and the pivots the solve took
/// across both phases — the grind's own meter, carried out so a slow
/// build reads off its run instead of being inferred from memory
/// growth.
pub(crate) struct Solved {
    pub(crate) objective: f64,
    pub(crate) duals: Vec<f64>,
    pub(crate) x: Vec<f64>,
    pub(crate) pivots: u64,
}

/// The smallest pivot element the ratio test accepts.
const PIVOT_TOL: f64 = 1e-7;
/// The reduced-cost negativity that still prices in.
const DUAL_TOL: f64 = 1e-9;
/// Pivots between full refreshes of the basic values and duals.
const REFRESH: usize = 100;
/// Degenerate pivots before the Bland window opens, and its length.
const STALL_LIMIT: usize = 5_000;
const BLAND_WINDOW: usize = 2_000;

struct Solver {
    /// The rhs the solver runs on: the caller's `b` plus a tiny
    /// distinct perturbation per row. The walk LPs are massively
    /// degenerate — most rows carry a zero rhs — and the ties stall
    /// the primal ratio test at zero steps; distinct microdeltas in
    /// the 1e-8 band break them above the refined values' precision.
    /// Sound because the dual feasible region never involves b: the
    /// duals this returns stay feasible for the unperturbed LP, and
    /// the objective damage is `y·δ`, measured against the certified
    /// optima.
    b: Vec<f64>,
    m: usize,
    n: usize,
    /// The basis: column ids, artificials being `n + row`.
    basis: Vec<usize>,
    /// The eta file: the leaving row, its pivot value, and the FTRAN'd
    /// entering column's nonzeros (the pivot row included).
    eta_row: Vec<usize>,
    eta_pivot: Vec<f64>,
    eta_index: Vec<Vec<usize>>,
    eta_value: Vec<Vec<f64>>,
    /// The basic values and the row duals.
    x_b: Vec<f64>,
    y: Vec<f64>,
    /// The reduced costs of the structural and artificial columns.
    cbar: Vec<f64>,
    /// 1 while the artificial sum is minimized, 2 on the real costs.
    phase: u8,
    /// Pivots since the last refresh, and the Bland window's end.
    since_refresh: usize,
    bland_until: usize,
    pivots: usize,
    /// The degenerate-pivot streak, held across `pivot_until` calls so
    /// a pause at a unit boundary never resets the Bland window's
    /// timing: a resumable solve pivots exactly where the one-shot
    /// one does.
    degenerate: usize,
    /// Scratch: the FTRAN'd entering column and the BTRAN'd pivot row.
    alpha: Vec<f64>,
}

impl Solver {
    fn new(lp: &Lp) -> Self {
        let m = lp.m;
        let n = lp.n;
        let b: Vec<f64> =
            lp.b.iter()
                .enumerate()
                .map(|(i, &value)| {
                    let hash = (i as u64).wrapping_mul(2654435761).wrapping_add(1);
                    value + 1e-8 * (1.0 + (hash % 1024) as f64 / 10240.0)
                })
                .collect();
        Solver {
            b: b.clone(),
            m,
            n,
            basis: (n..n + m).collect(),
            eta_row: Vec::new(),
            eta_pivot: Vec::new(),
            eta_index: Vec::new(),
            eta_value: Vec::new(),
            x_b: b,
            y: vec![1.0; m],
            cbar: vec![1.0; n + m],
            phase: 1,
            since_refresh: 0,
            bland_until: 0,
            pivots: 0,
            degenerate: 0,
            alpha: vec![0.0; m],
        }
    }

    /// A column's cost in the current phase: phase 1 minimizes the
    /// artificial sum alone — structural columns carry zero there —
    /// and phase 2 the real costs, artificials nothing once more.
    fn cost(&self, lp: &Lp, column: usize) -> f64 {
        if column < self.n {
            if self.phase == 1 { 0.0 } else { lp.c[column] }
        } else if self.phase == 1 {
            1.0
        } else {
            0.0
        }
    }

    /// Solve `B x = v` through the eta chain, in place.
    fn ftran(&self, v: &mut [f64]) {
        for k in 0..self.eta_row.len() {
            let r = self.eta_row[k];
            let pivot = self.eta_pivot[k];
            let x_r = v[r] / pivot;
            v[r] = x_r;
            let (index, value) = (&self.eta_index[k], &self.eta_value[k]);
            for (position, &i) in index.iter().enumerate() {
                if i != r {
                    v[i] -= value[position] * x_r;
                }
            }
        }
    }

    /// Solve `Bᵀ x = v` through the eta chain in reverse, in place.
    fn btran(&self, v: &mut [f64]) {
        for k in (0..self.eta_row.len()).rev() {
            let r = self.eta_row[k];
            let pivot = self.eta_pivot[k];
            let mut sum = v[r];
            let (index, value) = (&self.eta_index[k], &self.eta_value[k]);
            for (position, &i) in index.iter().enumerate() {
                if i != r {
                    sum -= value[position] * v[i];
                }
            }
            v[r] = sum / pivot;
        }
    }

    /// Solve `B x = b`, refined: one chain solve, then a residual
    /// correction computed against the basis columns themselves. The
    /// chain accumulates dust as it lengthens; the residual is exact
    /// (sparse dot products), and one more chain pass on it — where
    /// relative dust on a small correction is harmless — keeps the
    /// values at the matrix's precision instead of the chain's.
    fn solve_values(&self, lp: &Lp) -> Vec<f64> {
        let mut values = self.b.clone();
        self.ftran(&mut values);
        let mut residual = self.b.clone();
        for (r, &value) in values.iter().enumerate() {
            if value == 0.0 {
                continue;
            }
            let column = self.basis[r];
            if column < self.n {
                for position in lp.starts[column]..lp.starts[column + 1] {
                    residual[lp.index[position]] -= lp.value[position] * value;
                }
            } else {
                residual[column - self.n] -= value;
            }
        }
        self.ftran(&mut residual);
        for (value, correction) in values.iter_mut().zip(residual) {
            *value += correction;
        }
        values
    }

    /// Solve `Bᵀ y = c_B`, refined the same way: the residual is the
    /// basic costs less each basis column's product with the duals.
    fn solve_duals(&self, lp: &Lp, basic_costs: &[f64]) -> Vec<f64> {
        let mut duals = basic_costs.to_vec();
        self.btran(&mut duals);
        let mut residual = basic_costs.to_vec();
        for (r, &column) in self.basis.iter().enumerate() {
            let mut product = 0.0;
            if column < self.n {
                for position in lp.starts[column]..lp.starts[column + 1] {
                    product += lp.value[position] * duals[lp.index[position]];
                }
            } else {
                product = duals[column - self.n];
            }
            residual[r] -= product;
        }
        self.btran(&mut residual);
        for (value, correction) in duals.iter_mut().zip(residual) {
            *value += correction;
        }
        duals
    }

    /// Recompute the basic values, the duals, and every reduced cost
    /// from scratch through the chains — the drift corrector. Fails if
    /// the refreshed basic values sit below feasibility.
    fn refresh(&mut self, lp: &Lp) -> Result<(), String> {
        let mut values = self.solve_values(lp);
        for (i, &value) in values.iter().enumerate() {
            // The guard sits between two scales: the eta chain's own
            // noise on a fifteen-thousand-row LP runs near 1e-7, and
            // real drift — the corruption that killed the dense
            // tableau's Dantzig run — shows up at O(1). Anything past
            // the 1e-5 band is the latter, and the solver fails loud.
            if value < -1e-5 * (1.0 + value.abs()) {
                return Err(format!(
                    "the basic values drifted infeasible: row {i} at {value}"
                ));
            }
        }
        for value in &mut values {
            if *value < 0.0 {
                *value = 0.0;
            }
        }
        self.x_b = values;
        let basic_costs: Vec<f64> = (0..self.m).map(|r| self.cost(lp, self.basis[r])).collect();
        let duals = self.solve_duals(lp, &basic_costs);
        self.y = duals;
        for j in 0..self.n {
            let mut reduced = self.cost(lp, j);
            for position in lp.starts[j]..lp.starts[j + 1] {
                reduced -= self.y[lp.index[position]] * lp.value[position];
            }
            self.cbar[j] = reduced;
        }
        for i in 0..self.m {
            self.cbar[self.n + i] = self.cost(lp, self.n + i) - self.y[i];
        }
        self.since_refresh = 0;
        Ok(())
    }

    /// The FTRAN'd entering column, in `alpha`, with its nonzeros
    /// collected for the eta push. The vector travels out and back:
    /// the chain solve borrows the solver immutably, so `alpha` cannot
    /// sit in `self` while it turns.
    fn ftran_column(&mut self, lp: &Lp, column: usize) -> Vec<(usize, f64)> {
        let mut alpha = std::mem::take(&mut self.alpha);
        alpha.fill(0.0);
        if column < self.n {
            for position in lp.starts[column]..lp.starts[column + 1] {
                alpha[lp.index[position]] = lp.value[position];
            }
        } else {
            alpha[column - self.n] = 1.0;
        }
        self.ftran(&mut alpha);
        let mut nonzeros = Vec::new();
        for (i, &value) in alpha.iter().enumerate() {
            // Dust below a trillionth is dropped: it bloats the eta
            // file without moving any value, and the pivot row's entry
            // sits above PIVOT_TOL, so it never goes.
            if value.abs() > 1e-12 {
                nonzeros.push((i, value));
            }
        }
        self.alpha = alpha;
        nonzeros
    }

    /// One pivot: enter `column`, leave `row` (chosen by the ratio
    /// test or forced by the drive-out). Updates the eta file, the
    /// basis, the basic values, and the reduced costs.
    fn pivot(&mut self, lp: &Lp, column: usize, row: usize) -> Result<(), String> {
        let nonzeros = self.ftran_column(lp, column);
        let pivot_value = self.alpha[row];
        if pivot_value.abs() <= PIVOT_TOL {
            return Err(format!(
                "the pivot element vanished: column {column} row {row} at {pivot_value}"
            ));
        }
        // The pivot row of the current tableau, for the reduced-cost
        // update: pi = B⁻ᵀ e_row, then pi·A over every column.
        let mut pi = vec![0.0; self.m];
        pi[row] = 1.0;
        self.btran(&mut pi);
        let step = self.cbar[column] / pivot_value;
        for j in 0..self.n {
            let mut alpha_r = 0.0;
            for position in lp.starts[j]..lp.starts[j + 1] {
                alpha_r += pi[lp.index[position]] * lp.value[position];
            }
            if alpha_r != 0.0 {
                self.cbar[j] -= step * alpha_r;
            }
        }
        for (i, &pi_i) in pi.iter().enumerate() {
            self.cbar[self.n + i] -= step * pi_i;
        }
        for (i, &pi_i) in pi.iter().enumerate() {
            self.y[i] += step * pi_i;
        }
        // The primal step: the leaving row's exact ratio, clamped at
        // zero against numerical dust.
        let mut primal = self.x_b[row] / pivot_value;
        if primal < 0.0 {
            primal = 0.0;
        }
        for &(i, value) in &nonzeros {
            if i != row {
                self.x_b[i] -= primal * value;
            }
        }
        self.x_b[row] = primal;
        self.eta_row.push(row);
        self.eta_pivot.push(pivot_value);
        self.eta_index
            .push(nonzeros.iter().map(|&(i, _)| i).collect());
        self.eta_value
            .push(nonzeros.iter().map(|&(_, v)| v).collect());
        self.basis[row] = column;
        self.pivots += 1;
        self.since_refresh += 1;
        Ok(())
    }

    /// The entering column: Dantzig's most negative reduced cost, or
    /// Bland's first within the anti-cycle window. Artificials price
    /// only in phase 1.
    fn entering(&self) -> Option<usize> {
        let limit = if self.phase == 1 {
            self.n + self.m
        } else {
            self.n
        };
        if self.pivots < self.bland_until {
            return (0..limit).find(|&j| self.cbar[j] < -DUAL_TOL);
        }
        let mut best = -DUAL_TOL;
        let mut pick = None;
        for j in 0..limit {
            if self.cbar[j] < best {
                best = self.cbar[j];
                pick = Some(j);
            }
        }
        pick
    }

    /// The ratio test: the exact minimum ratio, and the largest pivot
    /// among the rows tied at it. No admission slack at all — the
    /// chosen row is a true minimum-ratio row, so every basic with a
    /// positive column entry stays nonnegative to pure rounding, and
    /// no infeasibility accumulates between refreshes. The
    /// perturbation makes exact ties rare, which is what keeps the
    /// largest-pivot tie-break meaningful rather than degenerate.
    fn leaving(&self, alpha: &[f64]) -> Result<usize, String> {
        let mut minimum = f64::INFINITY;
        for (&a, &x) in alpha.iter().zip(&self.x_b) {
            if a > PIVOT_TOL {
                minimum = minimum.min(x / a);
            }
        }
        if minimum == f64::INFINITY {
            return Err("the LP is unbounded or the column numerically empty".to_owned());
        }
        let bound = minimum.max(0.0);
        let mut best = None;
        let mut best_pivot = 0.0;
        for (i, &a) in alpha.iter().enumerate() {
            if a > PIVOT_TOL && self.x_b[i] / a <= bound && a > best_pivot {
                best_pivot = a;
                best = Some(i);
            }
        }
        best.ok_or_else(|| "the ratio test found no stable pivot".to_owned())
    }

    /// The phase loop: price, pivot, refresh, until the phase reaches
    /// its optimum or `units` runs out — false while more pivots
    /// remain. One unit buys one pass — a pivot, with the refresh due
    /// beside it sharing the pass — so a resumable caller bounds the
    /// work of every call it makes.
    fn pivot_until(&mut self, lp: &Lp, units: &mut usize) -> Result<bool, String> {
        loop {
            if *units == 0 {
                return Ok(false);
            }
            *units -= 1;
            if self.since_refresh >= REFRESH {
                self.refresh(lp)?;
            }
            let Some(column) = self.entering() else {
                return Ok(true);
            };
            self.ftran_column(lp, column);
            let row = self.leaving(&self.alpha)?;
            let before = self.x_b[row];
            self.pivot(lp, column, row)?;
            if self.x_b[row] <= 1e-12 && before <= 1e-12 {
                self.degenerate += 1;
                if self.degenerate >= STALL_LIMIT && self.pivots >= self.bland_until {
                    self.bland_until = self.pivots + BLAND_WINDOW;
                    self.degenerate = 0;
                }
            } else {
                self.degenerate = 0;
            }
            if self.pivots > 2_000_000 {
                return Err("the pivot budget exhausted".to_owned());
            }
        }
    }
}

/// One LP's solve, resumable: two phases from the artificial basis,
/// each unit one pass of its loop, so a caller that must return
/// between batches can. `step` runs a caller-paid budget of units at
/// a time; `finish` — only once `step` said true — takes the duals
/// through the guards the one-shot solve always ran.
pub(crate) struct SolveBuild {
    solver: Solver,
    /// Whether the phase-1-to-2 transition already ran.
    phase2: bool,
}

impl SolveBuild {
    /// Starts the solve: the rhs check, the artificial basis, and
    /// phase 1's first refresh.
    pub(crate) fn new(lp: &Lp) -> Result<Self, String> {
        if lp.b.iter().any(|&value| value < 0.0) {
            return Err("the rhs must be nonnegative".to_owned());
        }
        let mut solver = Solver::new(lp);
        // The first refresh turns the placeholder duals and reduced costs
        // into phase 1's real ones (the artificial basis's y is the all-ones
        // vector through the empty chain).
        solver.refresh(lp)?;
        Ok(Self {
            solver,
            phase2: false,
        })
    }

    /// Runs up to `units` pivots: false while the solve has more to
    /// do, true once the second phase reached its optimum. Between the
    /// phases the artificials' sum is checked and the costs switch; no
    /// drive-out — a forced pivot on a row whose artificial carries a
    /// real residual takes a step the ratio test never sanctioned, and
    /// the damage lands on the other rows' feasibility. Artificials
    /// stay basic at their residuals instead — phase 2 prices them at
    /// nothing and never enters them, their mass is the perturbation's
    /// own scale, and the duals the solver returns are feasible over
    /// the structural columns regardless, which is all the transfer
    /// carries.
    pub(crate) fn step(&mut self, lp: &Lp, units: &mut usize) -> Result<bool, String> {
        if !self.solver.pivot_until(lp, units)? {
            return Ok(false);
        }
        if self.phase2 {
            return Ok(true);
        }
        self.phase2 = true;
        let artificial_sum: f64 = (0..self.solver.m)
            .filter(|&r| self.solver.basis[r] >= self.solver.n)
            .map(|r| self.solver.x_b[r])
            .sum();
        // The perturbed rhs leaves the phase-1 optimum a microsum of
        // artificials — the perturbation's own scale times the active
        // rows, which on a fifteen-thousand-row LP reaches ~3e-4 at the
        // 1e-8 band; anything past five thousandths is real infeasibility.
        if artificial_sum > 5e-3 {
            return Err(format!(
                "the LP is infeasible: {artificial_sum} of artificials remain"
            ));
        }
        self.solver.phase = 2;
        self.solver.refresh(lp)?;
        self.solver.pivot_until(lp, units)
    }

    /// The solved LP, once `step` ran true: the final duals are a
    /// fresh BTRAN checked against the objective.
    pub(crate) fn finish(self, lp: &Lp) -> Result<Solved, String> {
        debug_assert!(self.phase2, "finish before the phases ran");
        let solver = self.solver;
        let basic_costs: Vec<f64> = (0..solver.m)
            .map(|r| solver.cost(lp, solver.basis[r]))
            .collect();
        let duals = solver.solve_duals(lp, &basic_costs);
        let objective: f64 = (0..solver.m)
            .map(|r| solver.cost(lp, solver.basis[r]) * solver.x_b[r])
            .sum();
        let check: f64 = (0..solver.m).map(|r| duals[r] * solver.b[r]).sum();
        if (check - objective).abs() > 1e-4 * (1.0 + objective.abs()) {
            return Err(format!(
                "the duals guard failed: y·b {check} against the objective {objective}"
            ));
        }
        let mut x = vec![0.0; solver.n];
        for r in 0..solver.m {
            let column = solver.basis[r];
            if column < solver.n {
                x[column] = solver.x_b[r];
            }
        }
        Ok(Solved {
            objective,
            duals,
            x,
            pivots: solver.pivots as u64,
        })
    }
}

/// Solve the LP in one call — the resumable build run to its end. The
/// product paths step the build; this is the one-shot reference the
/// tests pin the stepping against.
#[cfg(test)]
pub(crate) fn solve(lp: &Lp) -> Result<Solved, String> {
    let mut build = SolveBuild::new(lp)?;
    let mut units = usize::MAX;
    while !build.step(lp, &mut units)? {}
    build.finish(lp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_lp_solves_with_duals() {
        // min x + 2y + 3z over x + y = 3 and x + z = 1, all nonnegative:
        // the equations force y = 3 - x and z = 1 - x with x <= 1, so
        // the cost is 9 - 4x, minimized at x = 1 (y = 2, z = 0) for 5.
        // The duals price row one at 2 and row two at -1: the second
        // row's equality overprices z, and x's column caps their sum.
        let lp = Lp {
            m: 2,
            n: 3,
            starts: vec![0, 2, 3, 4],
            index: vec![0, 1, 0, 1],
            value: vec![1.0, 1.0, 1.0, 1.0],
            b: vec![3.0, 1.0],
            c: vec![1.0, 2.0, 3.0],
        };
        let solved = solve(&lp).expect("the tiny LP solves");
        // The rhs perturbation leaves the objective within its duals'
        // damage y·δ of the unperturbed optimum, while the duals
        // themselves price the unperturbed rows exactly — the
        // invariance the transfer term's soundness rides.
        assert!(
            (solved.objective - 5.0).abs() < 1e-6,
            "{}",
            solved.objective
        );
        assert!(
            (solved.duals[0] - 2.0).abs() < 1e-9 && (solved.duals[1] - (-1.0)).abs() < 1e-9,
            "{:?}",
            solved.duals
        );
        // Both phases pivot off the artificial basis at least once.
        assert!(solved.pivots > 0);
    }

    #[test]
    fn an_infeasible_lp_errors() {
        // x = 1 and x = 2 cannot both hold.
        let lp = Lp {
            m: 2,
            n: 1,
            starts: vec![0, 2],
            index: vec![0, 1],
            value: vec![1.0, 1.0],
            b: vec![1.0, 2.0],
            c: vec![1.0],
        };
        assert!(solve(&lp).is_err());
    }

    #[test]
    fn a_degenerate_transportation_solves() {
        // A small transportation problem with a tie: two sources, two
        // demands, equal costs — the optimum is 4 either way.
        let lp = Lp {
            m: 4,
            n: 4,
            starts: vec![0, 2, 4, 6, 8],
            index: vec![0, 2, 0, 3, 1, 2, 1, 3],
            value: vec![1.0; 8],
            b: vec![1.0, 1.0, 1.0, 1.0],
            c: vec![1.0, 2.0, 2.0, 1.0],
        };
        let solved = solve(&lp).expect("the transportation solves");
        // The perturbation's y·δ objective damage again.
        assert!(
            (solved.objective - 2.0).abs() < 1e-6,
            "{}",
            solved.objective
        );
    }

    /// A solve stepped one pivot at a time lands exactly where the
    /// one-shot call does: the same duals, the same objective, the
    /// same pivot count — the resumable build must never pivot
    /// differently than `solve` would have.
    #[test]
    fn a_stepped_solve_matches_the_one_shot() {
        let lp = Lp {
            m: 4,
            n: 4,
            starts: vec![0, 2, 4, 6, 8],
            index: vec![0, 2, 0, 3, 1, 2, 1, 3],
            value: vec![1.0; 8],
            b: vec![1.0, 1.0, 1.0, 1.0],
            c: vec![1.0, 2.0, 2.0, 1.0],
        };
        let one_shot = solve(&lp).expect("the one-shot solve");
        let mut build = SolveBuild::new(&lp).expect("the stepped solve starts");
        let mut units = 1;
        while !build.step(&lp, &mut units).expect("the stepped solve runs") {
            units = 1;
        }
        let stepped = build.finish(&lp).expect("the stepped solve finishes");
        assert_eq!(stepped.pivots, one_shot.pivots);
        assert!((stepped.objective - one_shot.objective).abs() < 1e-12);
        for (stepped, one_shot) in stepped.duals.iter().zip(&one_shot.duals) {
            assert!((stepped - one_shot).abs() < 1e-12);
        }
    }
}
