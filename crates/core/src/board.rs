pub type Cell = u16;
pub const NONE: Cell = u16::MAX;
pub const MAX_BOXES: usize = 32;
pub const MAX_CELLS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State {
    pub player: Cell,
    pub boxes: [Cell; MAX_BOXES],
}

#[derive(Clone)]
pub struct Board {
    pub width: usize,
    pub height: usize,
    pub neighbors: Vec<[Cell; 4]>,
    /// 0 = floor, 255 = wall, A..Z = matching goal label.
    pub tiles: Vec<u8>,
    /// Boxes are grouped by label; equal-label boxes are interchangeable in search.
    pub labels: Vec<u8>,
    pub goals: Vec<(Cell, u8)>,
    pub initial: State,
}

impl Board {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_CELLS * 2 {
            return Err("Board text is too large".into());
        }
        let rows: Vec<_> = text.lines().collect();
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let height = rows.len();
        let size = width
            .checked_mul(height)
            .ok_or("Board dimensions overflow")?;
        if size == 0 || size > MAX_CELLS {
            return Err("Board must contain 1..4096 cells".into());
        }
        let mut tiles = vec![255; size];
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
                    b'O' => *tile = 255,
                    b' ' => (),
                    b'R' => {
                        if player != NONE {
                            return Err("Exactly one robot R is required".into());
                        }
                        player = cell;
                    }
                    b'S' => {
                        *tile = b'X';
                        goals.push((cell, b'X'));
                        counts[23] -= 1;
                    }
                    b'A'..=b'Z' => {
                        boxes.push((symbol, cell));
                        counts[(symbol - b'A') as usize] += 1;
                    }
                    b'a'..=b'z' if symbol != b'x' => {
                        let label = symbol.to_ascii_uppercase();
                        *tile = label;
                        goals.push((cell, label));
                        counts[(label - b'A') as usize] -= 1;
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
            return Err("Use 1..32 boxes".into());
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
            if tiles[i] == 255 {
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
                    if tiles[n] != 255 {
                        neighbors[i][d] = n as Cell;
                    }
                }
            }
        }
        Ok(Self {
            width,
            height,
            neighbors,
            tiles,
            labels,
            goals,
            initial,
        })
    }

    pub fn solved(&self, state: &State) -> bool {
        self.labels
            .iter()
            .enumerate()
            .all(|(i, &label)| self.tiles[state.boxes[i] as usize] == label)
    }

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
