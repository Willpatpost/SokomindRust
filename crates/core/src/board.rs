/// A cell of one board, numbered row-major: `row * width + column`. Every
/// cell is below [`MAX_CELLS`], so a u16 holds it and leaves [`NONE`] free.
pub type Cell = u16;
/// No cell: a missing neighbor in [`Board::neighbors`] and an unused slot
/// in [`State::boxes`].
pub const NONE: Cell = u16::MAX;
/// The most boxes a board may hold, and the length of [`State::boxes`].
pub const MAX_BOXES: usize = 32;
/// The most cells a board may have, counting the walls that pad short rows.
/// [`Board::parse`] also caps the text at twice this many bytes (8 KiB),
/// enough for a one-column board of this many rows.
pub const MAX_CELLS: usize = 4096;
/// The tile value of a wall; floor is 0 and a goal is its label.
pub const WALL: u8 = 255;

/// A position on one parsed [`Board`], meaningful only with that board.
///
/// The fields are public for compact copies, so these invariants are a
/// contract rather than enforced by the type. [`Board::initial`],
/// [`Board::step`] and `Game` preserve them, and [`Board::validate_state`]
/// checks a state built any other way (search runs it on every start state):
///
/// - `player` is an in-bounds floor or goal cell, never a wall or a box.
/// - `boxes[i]` for `i < board.labels().len()` is an in-bounds non-wall cell,
///   and no two boxes share a cell. Box `i` carries label
///   `board.labels()[i]`, so labels stay grouped in slot order and a push
///   never changes which slot holds which label.
/// - Every slot at or past `board.labels().len()` is [`NONE`], so the derived
///   equality depends only on the active boxes.
/// - The order of equal-label boxes is free. A live `Game` keeps its own
///   order, because undo records box indices; search sorts each label group
///   of its own copies so equal positions compare equal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State {
    /// The robot's cell.
    pub player: Cell,
    /// Each box's cell in [`Board::labels`] slot order, then [`NONE`].
    pub boxes: [Cell; MAX_BOXES],
}

/// What one legal primitive step did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    /// The player moved onto a free cell.
    Walk,
    /// The player pushed the box in this slot of [`State::boxes`] one cell
    /// ahead; it came from the cell the player now stands on.
    Push(usize),
}

/// A parsed puzzle: its walls, goals and start position, fixed once
/// [`Board::parse`] returns. Cells cover the rectangle that padding short
/// rows with walls makes.
#[derive(Clone)]
pub struct Board {
    width: usize,
    height: usize,
    /// `puzzle-v1:{fnv1a}` over the canonical row form, matching the reference.
    fingerprint: String,
    neighbors: Vec<[Cell; 4]>,
    /// 0 = floor, `WALL` = wall, A..Z = matching goal label.
    tiles: Vec<u8>,
    /// Boxes are grouped by label; equal-label boxes are interchangeable in search.
    labels: Vec<u8>,
    goals: Vec<(Cell, u8)>,
    initial: State,
}

/// An external position violates the parsed board's geometry or occupancy.
/// It displays as `Invalid state:` and the broken rule, naming the cell and
/// any box slot involved, as in `Invalid state: box 1 is on a wall at cell 0`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateError {
    /// The player's cell is past the board's last cell.
    PlayerOutOfBounds {
        /// The player's cell.
        cell: Cell,
    },
    /// The player stands on a wall.
    PlayerOnWall {
        /// The player's cell.
        cell: Cell,
    },
    /// A box's cell is past the board's last cell.
    BoxOutOfBounds {
        /// The box's slot in [`State::boxes`].
        index: usize,
        /// The box's cell.
        cell: Cell,
    },
    /// A box sits on a wall.
    BoxOnWall {
        /// The box's slot in [`State::boxes`].
        index: usize,
        /// The box's cell.
        cell: Cell,
    },
    /// The player stands on a box.
    PlayerOnBox {
        /// The box's slot in [`State::boxes`].
        index: usize,
        /// The shared cell.
        cell: Cell,
    },
    /// Two boxes share a cell.
    OverlappingBoxes {
        /// The lower of the two slots.
        first: usize,
        /// The higher of the two slots.
        second: usize,
        /// The shared cell.
        cell: Cell,
    },
    /// A slot at or past the board's box count holds a cell, not [`NONE`].
    InactiveBox {
        /// The slot in [`State::boxes`].
        index: usize,
        /// The cell it holds.
        cell: Cell,
    },
}
impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid state: ")?;
        match *self {
            Self::PlayerOutOfBounds { cell } => write!(f, "player is off the board at cell {cell}"),
            Self::PlayerOnWall { cell } => write!(f, "player is on a wall at cell {cell}"),
            Self::BoxOutOfBounds { index, cell } => {
                write!(f, "box {index} is off the board at cell {cell}")
            }
            Self::BoxOnWall { index, cell } => write!(f, "box {index} is on a wall at cell {cell}"),
            Self::PlayerOnBox { index, cell } => {
                write!(f, "player is on box {index} at cell {cell}")
            }
            Self::OverlappingBoxes {
                first,
                second,
                cell,
            } => write!(f, "boxes {first} and {second} share cell {cell}"),
            Self::InactiveBox { index, cell } => {
                write!(f, "unused box slot {index} holds cell {cell}")
            }
        }
    }
}
impl std::error::Error for StateError {}

/// Why [`Board::parse`] rejected the text. The first violation found wins:
/// the text and row checks run first, then each cell in row-major order,
/// then the robot, box and goal counts. Each variant displays the message
/// the server and the web app show for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The text is longer than twice [`MAX_CELLS`] bytes (8 KiB).
    TooLarge,
    /// The text contains a carriage return; rows end with `\n` alone.
    CarriageReturn,
    /// A row is empty, as in empty text or a blank line.
    EmptyRow,
    /// The padded rectangle has more than [`MAX_CELLS`] cells.
    TooManyCells,
    /// The byte here is not in the grammar, or is a reserved goal letter.
    UnsupportedSymbol {
        /// The 1-based row.
        row: usize,
        /// The 1-based column, counted in bytes.
        column: usize,
    },
    /// There is no robot `R`, or a second one; the scan stops at the second.
    RobotCount,
    /// There are no boxes, or more than [`MAX_BOXES`].
    BoxCount,
    /// Some label has more boxes than goals, or more goals than boxes.
    UnmatchedGoals,
}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("Board text is too large"),
            Self::CarriageReturn => f.write_str("Board rows cannot contain carriage returns"),
            Self::EmptyRow => f.write_str("Board rows cannot be empty"),
            Self::TooManyCells => write!(f, "Board must contain 1..{MAX_CELLS} cells"),
            Self::UnsupportedSymbol { row, column } => {
                write!(f, "Unsupported symbol at row {row}, column {column}")
            }
            Self::RobotCount => f.write_str("Exactly one robot R is required"),
            Self::BoxCount => write!(f, "Use 1..{MAX_BOXES} boxes"),
            Self::UnmatchedGoals => {
                f.write_str("Each box label must have the same number of matching goals")
            }
        }
    }
}
impl std::error::Error for ParseError {}

impl Board {
    /// Columns: the longest row's length in bytes.
    pub fn width(&self) -> usize {
        self.width
    }
    /// Rows.
    pub fn height(&self) -> usize {
        self.height
    }
    /// The layout revision id: `puzzle-v1:` then eight hex digits of an
    /// FNV-1a hash of the rows and box count, byte-identical to the
    /// reference's. The server keys saved progress on it, so a route saved
    /// for one layout never replays on an edited one.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    /// Each cell's neighbor in each direction, indexed like
    /// [`ACTIONS`](crate::ACTIONS), or [`NONE`] where that neighbor is a
    /// wall or off the board. Walls have no neighbors at all.
    pub fn neighbors(&self) -> &[[Cell; 4]] {
        &self.neighbors
    }
    /// One byte per cell: 0 for floor, [`WALL`] for a wall or row padding,
    /// or a goal's label, the uppercase letter its boxes carry (`X` for an
    /// `S` goal). The robot and the boxes are not tiles; a [`State`] holds
    /// them.
    pub fn tiles(&self) -> &[u8] {
        &self.tiles
    }
    /// Each box slot's label, an uppercase ASCII letter, sorted so equal
    /// labels are adjacent. Its length is the box count, and box `i` of
    /// every [`State`] on this board carries `labels()[i]`.
    pub fn labels(&self) -> &[u8] {
        &self.labels
    }
    /// Every goal cell with its label, in row-major order.
    pub fn goals(&self) -> &[(Cell, u8)] {
        &self.goals
    }
    /// The start position, with equal-label boxes in row-major order.
    pub fn initial(&self) -> State {
        self.initial
    }

    /// Validate once before handing a caller-created state to search. Box order
    /// need not be canonical: equal-label boxes are sorted by search afterward.
    /// Reports the first broken invariant of [`State`], checking the player,
    /// then each slot of [`State::boxes`] in order.
    pub fn validate_state(&self, state: &State) -> Result<(), StateError> {
        if state.player as usize >= self.tiles.len() {
            return Err(StateError::PlayerOutOfBounds { cell: state.player });
        }
        if self.tiles[state.player as usize] == WALL {
            return Err(StateError::PlayerOnWall { cell: state.player });
        }
        for (index, &cell) in state.boxes.iter().enumerate() {
            if index >= self.labels.len() {
                if cell != NONE {
                    return Err(StateError::InactiveBox { index, cell });
                }
                continue;
            }
            if cell as usize >= self.tiles.len() {
                return Err(StateError::BoxOutOfBounds { index, cell });
            }
            if self.tiles[cell as usize] == WALL {
                return Err(StateError::BoxOnWall { index, cell });
            }
            if cell == state.player {
                return Err(StateError::PlayerOnBox { index, cell });
            }
            if let Some(first) = state.boxes[..index].iter().position(|&other| other == cell) {
                return Err(StateError::OverlappingBoxes {
                    first,
                    second: index,
                    cell,
                });
            }
        }
        Ok(())
    }

    /// Parses puzzle text, one line per row:
    ///
    /// - `O` is a wall, a space is floor and `R` is the robot, of which there
    ///   must be exactly one.
    /// - `X` is a box whose goals are `S`. Any other uppercase letter is a box
    ///   whose goals are the same letter in lowercase. `O`, `R` and `S` mean
    ///   something else, so no box carries them and `o`, `r` and `s` could
    ///   never be matched; `x` is spelled `S`. All four are rejected.
    /// - Rows are separated by `\n`, and one trailing newline is ignored.
    ///   Carriage returns and empty rows are rejected.
    /// - Short rows are padded with walls to the longest row's width, and the
    ///   board's edge blocks like a wall, so no border is needed.
    ///
    /// The text may be at most twice [`MAX_CELLS`] bytes (8 KiB) and the
    /// padded board at most [`MAX_CELLS`] cells. It needs between 1 and
    /// [`MAX_BOXES`] boxes, and each label as many goals as boxes.
    /// [`ParseError`] names the first rule the text breaks.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        // A one-column board of MAX_CELLS rows needs a newline after every cell.
        if text.len() > MAX_CELLS * 2 {
            return Err(ParseError::TooLarge);
        }
        if text.contains('\r') {
            return Err(ParseError::CarriageReturn);
        }
        // A single trailing newline is a paste artifact; empty rows are not boards.
        let text = text.strip_suffix('\n').unwrap_or(text);
        let rows: Vec<&str> = text.split('\n').collect();
        if rows.iter().any(|row| row.is_empty()) {
            return Err(ParseError::EmptyRow);
        }
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let height = rows.len();
        // Both are nonzero, and 8 KiB of text cannot overflow their product.
        let size = width * height;
        if size > MAX_CELLS {
            return Err(ParseError::TooManyCells);
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
                            return Err(ParseError::RobotCount);
                        }
                        player = cell;
                    }
                    // Goals come before boxes: S is X's goal, not an S box.
                    // No box carries O, R or S, and X's goal is spelled S, so
                    // o, r, s and x fall through to the positioned error
                    // instead of surfacing later as unmatched goals.
                    b'S' | b'a'..=b'z' if !matches!(symbol, b'o' | b'r' | b's' | b'x') => {
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
                        return Err(ParseError::UnsupportedSymbol {
                            row: y + 1,
                            column: x + 1,
                        });
                    }
                }
            }
        }
        if player == NONE {
            return Err(ParseError::RobotCount);
        }
        if boxes.is_empty() || boxes.len() > MAX_BOXES {
            return Err(ParseError::BoxCount);
        }
        if counts.iter().any(|&n| n != 0) {
            return Err(ParseError::UnmatchedGoals);
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
                if let Some(n) = next
                    && tiles[n] != WALL
                {
                    neighbors[i][d] = n as Cell;
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

    /// Whether every box sits on a goal of its own label.
    pub fn solved(&self, state: &State) -> bool {
        (0..self.labels.len()).all(|i| self.on_goal(i, state.boxes[i]))
    }

    /// Whether box `index` at `cell` sits on a goal of its own label. Search
    /// calls it in hot loops, so it does not check its arguments: it panics
    /// unless `index` is below the box count and `cell` is on the board.
    #[inline]
    pub fn on_goal(&self, index: usize, cell: Cell) -> bool {
        self.tiles[cell as usize] == self.labels[index]
    }

    /// One legal primitive step. Returns `None` and leaves `state` unchanged
    /// when the direction is not 0..4 or a wall or an unpushable box blocks it.
    pub fn step(&self, state: &mut State, direction: usize) -> Option<Step> {
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
        Some(index.map_or(Step::Walk, Step::Push))
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
