pub type Cell = u16;
pub const NONE: Cell = u16::MAX;
pub const MAX_BOXES: usize = 32;
pub const MAX_CELLS: usize = 4096;
/// The tile value of a wall; floor is 0 and a goal is its label.
pub const WALL: u8 = 255;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State {
    pub player: Cell,
    pub boxes: [Cell; MAX_BOXES],
}

#[derive(Clone)]
pub struct Board {
    pub width: usize,
    pub height: usize,
    /// `puzzle-v1:{fnv1a}` over the canonical row form, matching the reference.
    pub fingerprint: String,
    pub neighbors: Vec<[Cell; 4]>,
    /// 0 = floor, `WALL` = wall, A..Z = matching goal label.
    pub tiles: Vec<u8>,
    /// Boxes are grouped by label; equal-label boxes are interchangeable in search.
    pub labels: Vec<u8>,
    pub goals: Vec<(Cell, u8)>,
    pub initial: State,
}

impl Board {
    pub fn parse(text: &str) -> Result<Self, String> {
        // A one-column board of MAX_CELLS rows needs a newline after every cell.
        if text.len() > MAX_CELLS * 2 {
            return Err("Board text is too large".into());
        }
        if text.contains('\r') {
            return Err("Board rows cannot contain carriage returns".into());
        }
        // A single trailing newline is a paste artifact; empty rows are not boards.
        let text = text.strip_suffix('\n').unwrap_or(text);
        let rows: Vec<&str> = text.split('\n').collect();
        if rows.iter().any(|row| row.is_empty()) {
            return Err("Board rows cannot be empty".into());
        }
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let height = rows.len();
        // Both are nonzero, and 8 KiB of text cannot overflow their product.
        let size = width * height;
        if size > MAX_CELLS {
            return Err(format!("Board must contain 1..{MAX_CELLS} cells"));
        }
        let mut tiles = vec![WALL; size];
        let mut player = NONE;
        let mut boxes = Vec::new();
        let mut goals = Vec::new();
        let mut counts = [0i16; 26];
        for (y, row) in rows.iter().enumerate() {
            for (x, symbol) in row.bytes().enumerate() {
                let cell = (y * width + x) as Cell;
                let tile = &mut tiles[cell as usize];
                *tile = 0;
                match symbol {
                    b'O' => *tile = WALL,
                    b' ' => (),
                    b'R' => {
                        if player != NONE {
                            return Err("Exactly one robot R is required".into());
                        }
                        player = cell;
                    }
                    // Goals come before boxes: S is X's goal, not an S box.
                    b'S' | b'a'..=b'z' if symbol != b'x' => {
                        let label = if symbol == b'S' {
                            b'X'
                        } else {
                            symbol.to_ascii_uppercase()
                        };
                        *tile = label;
                        goals.push((cell, label));
                        counts[(label - b'A') as usize] -= 1;
                    }
                    b'A'..=b'Z' => {
                        boxes.push((symbol, cell));
                        counts[(symbol - b'A') as usize] += 1;
                    }
                    _ => {
                        return Err(format!(
                            "Unsupported symbol at row {}, column {}",
                            y + 1,
                            x + 1
                        ));
                    }
                }
            }
        }
        if player == NONE {
            return Err("Exactly one robot R is required".into());
        }
        if boxes.is_empty() || boxes.len() > MAX_BOXES {
            return Err(format!("Use 1..{MAX_BOXES} boxes"));
        }
        if counts.iter().any(|&n| n != 0) {
            return Err("Each box label must have the same number of matching goals".into());
        }
        boxes.sort_unstable();
        let mut initial = State {
            player,
            boxes: [NONE; MAX_BOXES],
        };
        let labels = boxes
            .iter()
            .enumerate()
            .map(|(i, &(label, cell))| {
                initial.boxes[i] = cell;
                label
            })
            .collect();
        let mut neighbors = vec![[NONE; 4]; size];
        for i in 0..size {
            if tiles[i] == WALL {
                continue;
            }
            let (x, y) = (i % width, i / width);
            let candidates = [
                y.checked_sub(1).map(|ny| ny * width + x),
                (y + 1 < height).then_some(i + width),
                x.checked_sub(1).map(|_| i - 1),
                (x + 1 < width).then_some(i + 1),
            ];
            for (d, next) in candidates.into_iter().enumerate() {
                if let Some(n) = next {
                    if tiles[n] != WALL {
                        neighbors[i][d] = n as Cell;
                    }
                }
            }
        }
        Ok(Self {
            width,
            height,
            fingerprint: fingerprint(&boxes, &rows),
            neighbors,
            tiles,
            labels,
            goals,
            initial,
        })
    }

    pub fn solved(&self, state: &State) -> bool {
        (0..self.labels.len()).all(|i| self.on_goal(i, state.boxes[i]))
    }

    /// Whether box `index` at `cell` sits on a goal of its own label.
    #[inline]
    pub fn on_goal(&self, index: usize, cell: Cell) -> bool {
        self.tiles[cell as usize] == self.labels[index]
    }

    /// Sorts interchangeable same-label boxes so states compare canonically.
    /// Only call this on search-owned state copies: a live `Game` must keep
    /// its box order, because undo records box indices.
    pub fn canonicalize(&self, state: &mut State) {
        let mut begin = 0;
        while begin < self.labels.len() {
            let mut end = begin + 1;
            while end < self.labels.len() && self.labels[end] == self.labels[begin] {
                end += 1;
            }
            state.boxes[begin..end].sort_unstable();
            begin = end;
        }
    }

    /// One legal primitive step; returns pushed box index, or MAX_BOXES for a walk.
    pub fn step(&self, state: &mut State, direction: usize) -> Option<usize> {
        if direction >= 4 {
            return None;
        }
        let next = self.neighbors[state.player as usize][direction];
        if next == NONE {
            return None;
        }
        let index = state.boxes[..self.labels.len()]
            .iter()
            .position(|&p| p == next);
        if let Some(i) = index {
            let target = self.neighbors[next as usize][direction];
            if target == NONE || state.boxes[..self.labels.len()].contains(&target) {
                return None;
            }
            state.boxes[i] = target;
        }
        state.player = next;
        Some(index.unwrap_or(MAX_BOXES))
    }
}

/// Layout revision hash, byte-identical to the reference's
/// `puzzleRevisionFingerprint`: FNV-1a over
/// `boxes:{n}\nrows:{count}\n{length}:{row}` joined rows, as `puzzle-v1:{hex}`.
fn fingerprint(boxes: &[(u8, Cell)], rows: &[&str]) -> String {
    let mut canonical = format!("boxes:{}\nrows:{}\n", boxes.len(), rows.len());
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            canonical.push('\n');
        }
        canonical.push_str(&format!("{}:{}", row.len(), row));
    }
    let mut hash = 0x811c_9dc5u32;
    for byte in canonical.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("puzzle-v1:{hash:08x}")
}
