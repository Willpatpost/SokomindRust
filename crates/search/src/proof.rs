/// Optimality claim for a search run. Only [`crate::ExactSearch`] makes one,
/// and only from bounds it has established: an admissible frontier and a
/// replayable incumbent.
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
