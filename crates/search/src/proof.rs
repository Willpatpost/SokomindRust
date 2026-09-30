/// Optimality claim for a search run. Only a [`crate::Search`] in
/// [`crate::Mode::Optimal`] makes one, and only from bounds it has
/// established: an admissible frontier and a replayable incumbent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// The optimum lies in `lower_bound..=upper_bound`, where `upper_bound`
    /// is the incumbent's move count.
    Bounded {
        /// Certified lower bound on the optimal move count.
        lower_bound: u32,
        /// The incumbent's move count.
        upper_bound: u32,
    },
    /// `moves`, the incumbent's length, is the optimum.
    Optimal {
        /// The optimal move count.
        moves: u32,
    },
    /// No route exists.
    Unsolvable,
}
impl Proof {
    /// The wire name: the server's `proof.kind` and the benchmark corpus's
    /// `proof.kind`, which spells a missing proof `"none"`.
    ///
    /// The WASM search's metrics tuple (`WasmSearch::metrics` in
    /// `crates/wasm`) carries only numbers, so it sends the kind as a code in
    /// slot `[4]`. It needs no bound fields: a proof's bounds are always the
    /// search's own [`crate::Search::lower_bound`], in slot `[5]`, and
    /// [`crate::Search::best_moves`], in slot `[3]`, each `u32::MAX` when
    /// absent. `decodeMetricTuple` in `web/src/transport.ts` reads them back.
    ///
    /// | Proof                 | Wire name             | WASM code | WASM bounds            |
    /// |-----------------------|-----------------------|-----------|------------------------|
    /// | none                  | `"none"`, corpus only | 0         | none                   |
    /// | [`Proof::Bounded`]    | `"bounded"`           | 1         | optimum in `[5]..=[3]` |
    /// | [`Proof::Optimal`]    | `"optimal"`           | 2         | `[3]` is the optimum   |
    /// | [`Proof::Unsolvable`] | `"unsolvable"`        | 3         | none                   |
    ///
    /// A new variant needs a new name and code, and every encoder changes
    /// with it: the server's `proof_body`, the corpus's `proof_value`, the
    /// WASM `metrics` and the web decoders. The Rust encoders match
    /// exhaustively, so they stop compiling until updated; the web decoders
    /// reject an unknown kind only at run time.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Bounded { .. } => "bounded",
            Self::Optimal { .. } => "optimal",
            Self::Unsolvable => "unsolvable",
        }
    }
}
