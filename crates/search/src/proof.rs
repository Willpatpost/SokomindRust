/// Optimality claim for a search run. Only a [`crate::Search`] in
/// [`crate::Mode::Optimal`] makes one, and only from bounds it has
/// established: an admissible frontier and a replayable incumbent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// The optimum lies in `lower_bound..=upper_bound`, where `upper_bound`
    /// is the incumbent's move count.
    Bounded { lower_bound: u32, upper_bound: u32 },
    /// `moves`, the incumbent's length, is the optimum.
    Optimal { moves: u32 },
    /// No route exists.
    Unsolvable,
}
impl Proof {
    /// The wire name: the server's `proof.kind` and the benchmark corpus's
    /// `proof.kind`, which spells a missing proof `"none"`.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Bounded { .. } => "bounded",
            Self::Optimal { .. } => "optimal",
            Self::Unsolvable => "unsolvable",
        }
    }
}
