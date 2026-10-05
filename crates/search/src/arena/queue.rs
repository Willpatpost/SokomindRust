//! The search queue: a four-ary min-heap of [`Key`]s whose storage grows a
//! chunk at a time, in step with the arena's records, instead of being
//! reserved whole up front.
use super::{CHUNK, CHUNK_SHIFT, Key};
use std::collections::TryReserveError;

// A group of four siblings starts at a multiple of four, so it fits in one chunk.
const _: () = assert!(CHUNK >= 4);

/// Four-ary min-heap of [`Key`]s. Position `p` lives in chunk
/// `p >> CHUNK_SHIFT` at offset `p & (CHUNK - 1)`, the split the arena's
/// records use. The root, at 0, has children 1 to 3, and every other `p` has
/// `4p` to `4p + 3`, so the parent of any `c` is `c / 4`. Each node's
/// children fill one aligned group of four, which never straddles a chunk and
/// is read with one bounds check, and the heap has half a binary heap's
/// levels. A binary heap over these chunks, two bounds checks and a chunk
/// lookup per access at three accesses a level, ran the observe boards
/// 3-9% slower than the contiguous `BinaryHeap` before it; this one runs the
/// memory-bound huge board at par and the rest 1.5-4.6% slower (job 48154),
/// what the bounds checks cost now that the queue grows with the records.
///
/// The arena adds a chunk here with each record chunk, of the
/// same size, and every enqueue follows an insert, so the queue always has
/// room for one more entry, never reallocates, and holds at most the
/// budget's share for the records reached so far. A `Vec` that doubled
/// would briefly hold its old and new buffers at once, past the budget
/// near the limit; one reserved whole would hold the full share however
/// early the search ends, which wasm32 commits as linear memory.
///
/// The keys queued at once are distinct, each naming its own record, so
/// every correct min-heap pops them in the same order: this one runs the
/// same search as the `std::collections::BinaryHeap` it replaced.
pub(super) struct Queue {
    /// Every chunk before the one holding the last entry is full, and every
    /// chunk after it is empty.
    chunks: Vec<Vec<Key>>,
    len: usize,
}

impl Queue {
    /// An empty queue with no chunks, whose chunk list has room for
    /// `chunks` of them.
    pub(super) fn new(chunks: usize) -> Result<Self, TryReserveError> {
        let mut list = Vec::new();
        list.try_reserve_exact(chunks)?;
        Ok(Self {
            chunks: list,
            len: 0,
        })
    }
    /// Adds a chunk with room for `entries` entries.
    pub(super) fn add_chunk(&mut self, entries: usize) -> Result<(), TryReserveError> {
        let mut chunk = Vec::new();
        chunk.try_reserve_exact(entries)?;
        debug_assert!(self.chunks.len() < self.chunks.capacity());
        self.chunks.push(chunk);
        Ok(())
    }
    pub(super) fn len(&self) -> usize {
        self.len
    }
    /// The lowest key.
    pub(super) fn peek(&self) -> Option<Key> {
        self.chunks.first()?.first().copied()
    }
    /// Queues `key` without allocating.
    pub(super) fn push(&mut self, key: Key) {
        let at = self.len;
        let chunk = &mut self.chunks[at >> CHUNK_SHIFT];
        debug_assert!(
            chunk.len() < chunk.capacity(),
            "the arena adds a queue chunk with each record chunk"
        );
        chunk.push(key);
        self.len += 1;
        self.sift_up(0, at, key);
    }
    /// Removes and returns the lowest key.
    pub(super) fn pop(&mut self) -> Option<Key> {
        let last = self.len.checked_sub(1)?;
        self.len = last;
        // Some: the chunk holding the last entry is never empty.
        let key = self.chunks[last >> CHUNK_SHIFT].pop()?;
        if last == 0 {
            return Some(key);
        }
        let top = self.get(0);
        self.sift_down_to_bottom(0, key);
        Some(top)
    }
    /// Replaces every key with `rekey(key)` in place, then restores the heap
    /// order bottom-up, in time linear in the length.
    pub(super) fn rekey(&mut self, mut rekey: impl FnMut(Key) -> Key) {
        for key in self.chunks.iter_mut().flatten() {
            *key = rekey(*key);
        }
        // Every node from (len - 1) / 4 on is a leaf.
        for at in (0..self.len.div_ceil(4)).rev() {
            self.sift_down_to_bottom(at, self.get(at));
        }
    }
    /// Empties the queue, keeping every chunk's allocation.
    pub(super) fn clear(&mut self) {
        for chunk in &mut self.chunks {
            chunk.clear();
        }
        self.len = 0;
    }
    #[inline]
    fn get(&self, at: usize) -> Key {
        self.chunks[at >> CHUNK_SHIFT][at & (CHUNK - 1)]
    }
    #[inline]
    fn set(&mut self, at: usize, key: Key) {
        self.chunks[at >> CHUNK_SHIFT][at & (CHUNK - 1)] = key;
    }
    /// Fills the hole at `at` with `key` after moving down every ancestor
    /// above `key`, up to `start`.
    fn sift_up(&mut self, start: usize, mut at: usize, key: Key) {
        while at > start {
            let parent = at / 4;
            let above = self.get(parent);
            if above <= key {
                break;
            }
            self.set(at, above);
            at = parent;
        }
        self.set(at, key);
    }
    /// Places `key` in the hole at `start`, whose subtrees are heaps: the
    /// hole moves down to a leaf, always to the lowest child, and `key` rises
    /// back from there. A key refilling the top came from the bottom and
    /// mostly belongs near it, so this saves the comparison with it at every
    /// level a plain sift-down makes, as in `BinaryHeap::pop`.
    fn sift_down_to_bottom(&mut self, start: usize, key: Key) {
        let end = self.len;
        let mut at = start;
        loop {
            let first = (4 * at).max(1);
            if first >= end {
                break;
            }
            let (lowest, below) = self.lowest_child(first, end.min(4 * at + 4));
            self.set(at, below);
            at = lowest;
        }
        self.sift_up(start, at, key);
    }
    /// The position and key of the lowest of the siblings `first..last`,
    /// one group of four or less.
    #[inline]
    fn lowest_child(&self, first: usize, last: usize) -> (usize, Key) {
        let at = first & (CHUNK - 1);
        let siblings = &self.chunks[first >> CHUNK_SHIFT][at..at + (last - first)];
        if let Ok(&[a, b, c, d]) = <&[Key; 4]>::try_from(siblings) {
            let (i, x) = if b < a { (1, b) } else { (0, a) };
            let (j, y) = if d < c { (3, d) } else { (2, c) };
            let (k, z) = if y < x { (j, y) } else { (i, x) };
            return (first + k, z);
        }
        let mut lowest = 0;
        for (i, &key) in siblings.iter().enumerate().skip(1) {
            if key < siblings[lowest] {
                lowest = i;
            }
        }
        (first + lowest, siblings[lowest])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::Lcg;
    use std::{cmp::Reverse, collections::BinaryHeap};

    /// The chunked heap agrees with `BinaryHeap` through interleaved pushes,
    /// pops and re-keys across many of the four-entry chunks unit tests use.
    #[test]
    fn pops_what_a_std_heap_pops() {
        const ENTRIES: usize = 300;
        let mut queue = Queue::new(ENTRIES.div_ceil(CHUNK)).unwrap();
        for _ in 0..ENTRIES.div_ceil(CHUNK) {
            queue.add_chunk(CHUNK).unwrap();
        }
        let mut reference = BinaryHeap::new();
        let mut rng = Lcg(0x9e47);
        let mut id = 0;
        let mut step = 0;
        while id < ENTRIES as u32 {
            if rng.below(3) == 0 {
                assert_eq!(queue.pop(), reference.pop().map(|Reverse(key)| key));
            } else {
                let key = Key::new(rng.below(64) as u64, rng.below(8) as u32, id);
                id += 1;
                queue.push(key);
                reference.push(Reverse(key));
            }
            step += 1;
            if step % 97 == 0 {
                let rekey = |key: Key| Key::new((key.f() * 7 + 3) % 64, key.h(), key.id());
                queue.rekey(rekey);
                reference = reference
                    .into_iter()
                    .map(|Reverse(key)| Reverse(rekey(key)))
                    .collect();
            }
            assert_eq!(queue.len(), reference.len());
            assert_eq!(queue.peek(), reference.peek().map(|&Reverse(key)| key));
        }
        while let Some(Reverse(key)) = reference.pop() {
            assert_eq!(queue.pop(), Some(key));
        }
        assert_eq!(queue.pop(), None);
    }
}
