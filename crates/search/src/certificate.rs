/// Terminal certificate for a search run. Only the exact kernel constructs
/// these; a resource limit keeps existing bounds but never upgrades one kind
/// into another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// Replay-verified route plus a certified lower bound on the optimum.
    Bounded {
        lower_bound: u32,
        upper_bound: u32,
    },
    /// Certified move-optimal route.
    Optimal { moves: u32 },
    /// Certified that no route exists.
    Unsolvable,
}
impl Proof {
    pub fn bounded(lower_bound: u32, upper_bound: u32) -> Result<Self, String> {
        if lower_bound > upper_bound {
            return Err("Bounded proof requires lower_bound <= upper_bound".into());
        }
        Ok(Self::Bounded {
            lower_bound,
            upper_bound,
        })
    }
    pub fn is_optimal(&self) -> bool {
        matches!(self, Self::Optimal { .. })
    }
}
