use sokomind_core::{Board, MAX_BOXES, NONE, OPPOSITE, State};

/// Per-goal reverse-push distances; shared by dead-square pruning and assignment.
pub struct Heuristic {
    distances: Vec<Vec<u16>>,
}
impl Heuristic {
    pub fn new(board: &Board) -> Self {
        let mut queue = Vec::with_capacity(board.tiles.len());
        let distances = board
            .goals
            .iter()
            .map(|&(goal, _)| {
                let mut dist = vec![NONE; board.tiles.len()];
                queue.clear();
                queue.push(goal);
                dist[goal as usize] = 0;
                let mut head = 0;
                while head < queue.len() {
                    let cell = queue[head];
                    head += 1;
                    for direction in OPPOSITE {
                        let previous = board.neighbors[cell as usize][direction];
                        if previous == NONE {
                            continue;
                        }
                        let support = board.neighbors[previous as usize][direction];
                        if support != NONE && dist[previous as usize] == NONE {
                            dist[previous as usize] = dist[cell as usize] + 1;
                            queue.push(previous);
                        }
                    }
                }
                dist
            })
            .collect();
        Self { distances }
    }

    /// Minimum-cost label-compatible box/goal matching (Hungarian, O(boxes^3)).
    /// Push distances ignore other boxes, so the sum is a lower bound on moves.
    pub fn estimate(&self, board: &Board, state: &State) -> Option<u32> {
        const INF: i32 = 1_000_000;
        let n = board.labels.len();
        let mut u = [0i32; MAX_BOXES + 1];
        let mut v = [0i32; MAX_BOXES + 1];
        let mut p = [0usize; MAX_BOXES + 1];
        let mut way = [0usize; MAX_BOXES + 1];
        for i in 1..=n {
            p[0] = i;
            let mut j0 = 0;
            let mut minv = [INF; MAX_BOXES + 1];
            let mut used = [false; MAX_BOXES + 1];
            loop {
                used[j0] = true;
                let i0 = p[j0];
                let mut delta = INF;
                let mut j1 = 0;
                for j in 1..=n {
                    if used[j] {
                        continue;
                    }
                    let d = self.distances[j - 1][state.boxes[i0 - 1] as usize];
                    let cost = if board.labels[i0 - 1] == board.goals[j - 1].1 && d != NONE {
                        d as i32
                    } else {
                        INF
                    };
                    let current = cost - u[i0] - v[j];
                    if current < minv[j] {
                        minv[j] = current;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
                if delta >= INF {
                    return None;
                }
                for j in 0..=n {
                    if used[j] {
                        u[p[j]] += delta;
                        v[j] -= delta;
                    } else {
                        minv[j] -= delta;
                    }
                }
                j0 = j1;
                if p[j0] == 0 {
                    break;
                }
            }
            loop {
                let j1 = way[j0];
                p[j0] = p[j1];
                j0 = j1;
                if j0 == 0 {
                    break;
                }
            }
        }
        let total = -v[0];
        (total < INF).then_some(total as u32)
    }
}
