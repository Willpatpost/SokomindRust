//! Floor topology with one cell removed. The sides of a floor cell `x` are
//! the parts its floor component falls into once `x` is blocked; Blocks
//! tells which side holds a cell, whether `x` shares the keeper's
//! component, and where each side lies in DFS order.
//!
//! One iterative Tarjan DFS per floor component, the keeper's first, does
//! the work of a flood per blocked cell in `O(cells)`. In an undirected DFS
//! every non-tree edge joins a cell to one of its ancestors. So a child of
//! `x` whose low-link, the least entry time its subtree reaches through one
//! back edge, is at least `x`'s entry time has no edge out of its subtree
//! but to `x`: the subtree is a whole side, called separated. Everything
//! else joins up through the parent, so the parent's side with every other
//! child's subtree is one more side, the rest. A component's root has no
//! parent: every child of it is separated and it has no rest. Counting the
//! edge to the parent in a low-link lowers it to at most the parent's entry
//! time, which the separation test still passes.
//!
//! A side's label is the direction of its first neighbor of `x` in `U D L R`
//! order, so labels do not depend on the DFS. A separated child in
//! direction `d` has label `d`: an earlier neighbor on its side would be
//! neither an ancestor of `x` nor in an earlier child's subtree, so `x`
//! would enter it first and reach the child inside its subtree.
//!
//! Layout, 6 bytes per cell: entry and exit times as `u16` (a cell lies
//! below `x` iff its entry time is in `x`'s span), a 2-bit side label per
//! neighbor direction, and a links byte: bits 0 to 3 the directions of
//! separated children, bit 4 set when there is a rest side, bits 5 and 6
//! its label, bit 7 set in the keeper's component. Entry times count on
//! across components, so they are a bijection from floor cells onto
//! `0..floor`, and the keeper's component holds the lowest ones. The DFS
//! stack, low-links and direction cursors, 5 bytes per cell, are lent by
//! the caller.
use sokomind_core::{Board, Cell, MAX_CELLS, NONE, WALL};
use std::{collections::TryReserveError, mem::size_of, ops::Range};

/// Entry time of a cell no DFS has reached: a wall, or floor not yet
/// searched during the build.
const UNSEEN: u16 = u16::MAX;
/// Links bits 0 to 3: the directions of separated children.
const SEPARATED: u8 = 0b1111;
/// Links bit 4: the cell has a rest side.
const HAS_REST: u8 = 1 << 4;
/// Links bits 5 and 6 hold the rest side's label.
const REST_SHIFT: u32 = 5;
/// Links bit 7: the cell is in the keeper's floor component.
const IN_KEEPER: u8 = 1 << 7;

// Cell indices are below `MAX_CELLS` and DFS times at most it, one past the
// last entry time, so both fit `u16` and no time ever equals `UNSEEN`.
const _: () = assert!(MAX_CELLS < u16::MAX as usize);

/// One cell's DFS record.
#[derive(Clone, Copy)]
struct Vertex {
    /// Entry time.
    tin: u16,
    /// Exit time: the entry time plus the size of the cell's subtree.
    tout: u16,
    /// The side label of the neighbor in direction `d` at bits `2d` and
    /// `2d + 1`, zero for a missing neighbor.
    labels: u8,
    /// Separated-child directions, the rest side and the keeper flag.
    links: u8,
}

/// The sides of every floor cell; see the module docs.
pub(crate) struct Blocks {
    /// Board width, which locates a separated child from its direction.
    width: usize,
    /// One record per cell, walls included.
    vertices: Vec<Vertex>,
}

impl Blocks {
    /// Heap bytes `build` reserves for a board of `cells` cells.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) const fn bytes_for(cells: usize) -> usize {
        cells * size_of::<Vertex>()
    }
    /// Searches every floor component, the keeper's at `player` first. The
    /// records are the only allocation, reserved once at their final size;
    /// `stack`, `low` and `next` are scratch of at least one entry per cell,
    /// left holding garbage.
    pub(crate) fn build(
        board: &Board,
        player: Cell,
        stack: &mut [Cell],
        low: &mut [u16],
        next: &mut [u8],
    ) -> Result<Self, TryReserveError> {
        let (tiles, neighbors) = (board.tiles(), board.neighbors());
        let cells = tiles.len();
        debug_assert!(stack.len().min(low.len()).min(next.len()) >= cells);
        debug_assert_ne!(tiles[player as usize], WALL);
        let mut vertices = Vec::new();
        vertices.try_reserve_exact(cells)?;
        let unseen = Vertex {
            tin: UNSEEN,
            tout: UNSEEN,
            labels: 0,
            links: 0,
        };
        vertices.resize(cells, unseen);
        let mut blocks = Self {
            width: board.width(),
            vertices,
        };
        let keeper_end = blocks.dfs(neighbors, player as usize, 0, stack, low, next);
        let mut time = keeper_end;
        for (root, &tile) in tiles.iter().enumerate() {
            if tile != WALL && blocks.vertices[root].tin == UNSEEN {
                time = blocks.dfs(neighbors, root, time, stack, low, next);
            }
        }
        for (x, (&tile, around)) in tiles.iter().zip(neighbors).enumerate() {
            if tile != WALL {
                blocks.finish(x, around, keeper_end);
            }
        }
        Ok(blocks)
    }
    /// Iterative Tarjan DFS of `root`'s floor component from entry time
    /// `time`, returning the next unused time. Sets each cell's times and
    /// the separated bits of its links. `low` holds low-links and `next`
    /// each open cell's next direction to try, so the parent of a popped
    /// cell reached it in direction `next[parent] - 1`.
    fn dfs(
        &mut self,
        neighbors: &[[Cell; 4]],
        root: usize,
        mut time: u16,
        stack: &mut [Cell],
        low: &mut [u16],
        next: &mut [u8],
    ) -> u16 {
        self.vertices[root].tin = time;
        low[root] = time;
        next[root] = 0;
        stack[0] = root as Cell;
        let mut depth = 1;
        time += 1;
        while depth > 0 {
            let x = usize::from(stack[depth - 1]);
            let d = next[x];
            if d == 4 {
                depth -= 1;
                self.vertices[x].tout = time;
                if depth > 0 {
                    let parent = usize::from(stack[depth - 1]);
                    low[parent] = low[parent].min(low[x]);
                    if low[x] >= self.vertices[parent].tin {
                        let d = next[parent] - 1;
                        debug_assert_eq!(neighbors[parent][usize::from(d)], x as Cell);
                        self.vertices[parent].links |= 1 << d;
                    }
                }
                continue;
            }
            next[x] = d + 1;
            let y = neighbors[x][usize::from(d)];
            if y == NONE {
                continue;
            }
            let y = usize::from(y);
            let seen = self.vertices[y].tin;
            if seen == UNSEEN {
                self.vertices[y].tin = time;
                low[y] = time;
                next[y] = 0;
                stack[depth] = y as Cell;
                depth += 1;
                time += 1;
            } else {
                low[x] = low[x].min(seen);
            }
        }
        time
    }
    /// Labels floor cell `x`'s neighbors and records its rest side and
    /// keeper flag, once every DFS is done. A neighbor is on the side of the
    /// separated child whose span holds it, else on the rest side, and each
    /// side takes the direction of the first neighbor found on it.
    fn finish(&mut self, x: usize, around: &[Cell; 4], keeper_end: u16) {
        let mut first = [None; 5];
        let mut labels = 0;
        for (d, &y) in around.iter().enumerate() {
            if y != NONE {
                let key = self.child_holding(x, self.vertices[usize::from(y)].tin);
                let label = *first[key.unwrap_or(4)].get_or_insert(d as u8);
                labels |= label << (2 * d);
            }
        }
        let vertex = &mut self.vertices[x];
        vertex.labels = labels;
        if let Some(rest) = first[4] {
            vertex.links |= HAS_REST | (rest << REST_SHIFT);
        }
        if vertex.tin < keeper_end {
            vertex.links |= IN_KEEPER;
        }
    }
    /// The label of the side of floor cell `x` that holds `c`, a floor cell
    /// of `x`'s component other than `x`: at most four span tests.
    pub(crate) fn side(&self, x: Cell, c: Cell) -> u8 {
        let vertex = self.vertices[x as usize];
        let tin = self.vertices[c as usize].tin;
        debug_assert!(c != x && tin != UNSEEN);
        match self.child_holding(x as usize, tin) {
            Some(d) => (vertex.labels >> (2 * d)) & 3,
            None => {
                debug_assert!(vertex.links & HAS_REST != 0, "{c} is on no side of {x}");
                (vertex.links >> REST_SHIFT) & 3
            }
        }
    }
    /// Whether floor cell `x` is in the keeper's floor component.
    pub(crate) fn in_keeper(&self, x: Cell) -> bool {
        self.vertices[x as usize].links & IN_KEEPER != 0
    }
    /// Floor cell `x`'s DFS span, from its entry time to its exit time: a
    /// floor cell lies below `x` iff its own entry time is in the span.
    pub(crate) fn span(&self, x: Cell) -> Range<u16> {
        let vertex = self.vertices[x as usize];
        vertex.tin..vertex.tout
    }
    /// Each separated side of floor cell `x` as its label and span, in
    /// direction order: the side is exactly the cells whose entry times lie
    /// in the span, which starts at the child in the label's direction.
    pub(crate) fn separated(&self, x: Cell) -> impl Iterator<Item = (u8, Range<u16>)> {
        let labels = self.vertices[x as usize].labels;
        let children = self.children(x as usize);
        children.map(move |(d, span)| ((labels >> (2 * d)) & 3, span))
    }
    /// The label of floor cell `x`'s rest side: every cell of its component
    /// outside its span, with those below it in no separated span. `None`
    /// at a component's root, whose sides are all separated.
    pub(crate) fn rest(&self, x: Cell) -> Option<u8> {
        let links = self.vertices[x as usize].links;
        let label = (links >> REST_SHIFT) & 3;
        (links & HAS_REST != 0).then_some(label)
    }
    /// The set of floor cell `x`'s side labels, bit `l` for label `l`. Each
    /// side touches `x`, so the separated sides and the rest are them all.
    pub(crate) fn labels(&self, x: Cell) -> u8 {
        let mut set = self.rest(x).map_or(0, |label| 1 << label);
        for (label, _) in self.separated(x) {
            set |= 1 << label;
        }
        set
    }
    /// `x`'s separated children as each one's direction and span, in
    /// direction order, which is also the order of their spans.
    fn children(&self, x: usize) -> impl Iterator<Item = (usize, Range<u16>)> {
        let bits = self.vertices[x].links & SEPARATED;
        let directions = (0..4).filter(move |&d| (bits >> d) & 1 != 0);
        directions.map(move |d| (d, self.span(self.neighbor(x, d))))
    }
    /// `x`'s neighbor in direction `d`, by index arithmetic. No neighbor
    /// table is kept, which is sound because only real neighbors, the
    /// separated children, are ever asked for.
    fn neighbor(&self, x: usize, d: usize) -> Cell {
        let y = match d {
            0 => x - self.width,
            1 => x + self.width,
            2 => x - 1,
            _ => x + 1,
        };
        y as Cell
    }
    /// The direction of `x`'s separated child whose span holds entry time
    /// `tin`, if any.
    fn child_holding(&self, x: usize, tin: u16) -> Option<usize> {
        let mut children = self.children(x);
        children.find_map(|(d, span)| span.contains(&tin).then_some(d))
    }
}

#[cfg(test)]
mod tests {
    use super::Vertex;
    use crate::reach::Blocks;
    use crate::testkit::{Lcg, adjacent, catalog, components, random_room};
    use sokomind_core::{Board, Cell, MAX_CELLS, WALL};
    use std::mem::size_of;

    /// Blocks against floods on every catalog board, 200 random rooms and
    /// 300 random grids. The grids have no outer wall, so edge cells test
    /// the neighbor arithmetic, and many split into several floor
    /// components. Each build's capacity must match `bytes_for`, its entry
    /// times must number the floor `0..floor`, and every floor cell `x`
    /// gets `in_keeper` checked against a flood from the keeper and the
    /// other accessors against floods with `x` blocked (`check_sides`). One
    /// set of junk scratch serves every build, so stale cursors or
    /// low-links would show.
    #[test]
    fn blocks_match_floods() {
        assert_eq!(size_of::<Vertex>(), 6);
        let mut rng = Lcg(0xb10c);
        let mut boards = catalog();
        for n in 0..200 {
            let board = Board::parse(&random_room(&mut rng)).unwrap();
            boards.push((format!("room {n}"), board));
        }
        for n in 0..300 {
            let board = Board::parse(&random_grid(&mut rng)).unwrap();
            boards.push((format!("grid {n}"), board));
        }
        let mut stack = vec![0xbeef; MAX_CELLS];
        let mut low = vec![0xbeef; MAX_CELLS];
        let mut next = vec![0xee; MAX_CELLS];
        let mut split = 0;
        for (id, board) in &boards {
            let (tiles, player) = (board.tiles(), board.initial().player);
            let built = Blocks::build(board, player, &mut stack, &mut low, &mut next);
            let blocks = built.unwrap();
            let bytes = blocks.vertices.capacity() * size_of::<Vertex>();
            assert_eq!(bytes, Blocks::bytes_for(tiles.len()), "{id}");
            let tin = |c: usize| blocks.span(c as Cell).start;
            let floor: Vec<usize> = (0..tiles.len()).filter(|&c| tiles[c] != WALL).collect();
            let mut tins: Vec<u16> = floor.iter().map(|&c| tin(c)).collect();
            tins.sort_unstable();
            assert!(tins.into_iter().eq(0..floor.len() as u16), "{id}");
            let whole = components(board, None);
            let keeper = whole[player as usize];
            split += usize::from(floor.iter().any(|&c| whole[c] != keeper));
            for &x in &floor {
                let inside = whole[x] == keeper;
                assert_eq!(blocks.in_keeper(x as Cell), inside, "{id} x {x}");
                check_sides(id, board, &blocks, &whole, x);
            }
        }
        assert!(split >= 60, "{split} boards split");
    }

    /// Checks every accessor at floor cell `x` against `whole`, the board's
    /// floor components, and `components` with `x` removed. A side's
    /// canonical label is the first direction whose neighbor it holds, so
    /// distinct sides get distinct labels, and `side(x, c)` matching it for
    /// every other cell `c` of the component is the pairwise property: two
    /// cells share a label iff one flood joins them. Each separated span
    /// must hold exactly its side, starting at the child in the label's
    /// direction, and the rest side, when there is one, everything else.
    fn check_sides(id: &str, board: &Board, blocks: &Blocks, whole: &[usize], x: usize) {
        let (at, cell) = (format!("{id} x {x}"), x as Cell);
        let near = adjacent(board, x);
        let sides = components(board, Some(x));
        let mut canonical = vec![None; whole.len()];
        for (d, y) in near.into_iter().enumerate() {
            if let Some(y) = y {
                let first = &mut canonical[sides[y]];
                if first.is_none() {
                    *first = Some(d as u8);
                }
            }
        }
        let label = |c: usize| canonical[sides[c]].unwrap();
        let tin = |c: usize| blocks.span(c as Cell).start;
        let members: Vec<usize> = (0..whole.len())
            .filter(|&c| c != x && whole[c] == whole[x])
            .collect();
        let mut set: u8 = 0;
        for &c in &members {
            let side = blocks.side(cell, c as Cell);
            assert_eq!(side, label(c), "{at} c {c}");
            set |= 1 << side;
        }
        assert_eq!(blocks.labels(cell), set, "{at}");
        let span = blocks.span(cell);
        let below = members.iter().filter(|&&c| span.contains(&tin(c)));
        assert_eq!(below.count() + 1, span.len(), "{at}");
        let count = |side: u8| members.iter().filter(|&&c| label(c) == side).count();
        let mut last = span.start + 1;
        let mut inside = 0;
        let mut taken: u8 = 0;
        for (side, times) in blocks.separated(cell) {
            assert!(last <= times.start && times.end <= span.end, "{at}");
            let child = near[usize::from(side)].map(tin);
            assert_eq!(child, Some(times.start), "{at} side {side}");
            for &c in &members {
                if times.contains(&tin(c)) {
                    assert_eq!(label(c), side, "{at} c {c}");
                }
            }
            assert_eq!(count(side), times.len(), "{at} side {side}");
            last = times.end;
            inside += times.len();
            taken |= 1 << side;
        }
        match blocks.rest(cell) {
            Some(side) => {
                assert_eq!(taken & (1 << side), 0, "{at}");
                assert!(span.len() <= members.len(), "{at}");
                assert_eq!(count(side), members.len() - inside, "{at}");
            }
            None => {
                assert_eq!(inside, members.len(), "{at}");
                assert_eq!(span.len(), members.len() + 1, "{at}");
            }
        }
    }

    /// A grid of 3 to 10 columns by 2 to 8 rows with no outer wall: the
    /// robot, an `X` box and its `S` goal on distinct cells, and every other
    /// cell a wall with probability 2/5, which often splits the floor.
    fn random_grid(rng: &mut Lcg) -> String {
        let (width, height) = (3 + rng.below(8), 2 + rng.below(7));
        let mut cells = vec![b' '; width * height];
        let mut free: Vec<usize> = (0..cells.len()).collect();
        for &symbol in b"RXS" {
            cells[free.swap_remove(rng.below(free.len()))] = symbol;
        }
        for i in free {
            if rng.below(5) < 2 {
                cells[i] = b'O';
            }
        }
        let rows: Vec<&[u8]> = cells.chunks(width).collect();
        String::from_utf8(rows.join(&b'\n')).unwrap()
    }
}
