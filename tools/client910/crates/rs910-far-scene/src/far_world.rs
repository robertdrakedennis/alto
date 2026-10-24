//! The resident far squares (design §6.2 "`FarWorld`", following the modern
//! client's build loop): each frame the ring
//! around the focus at the level's radius; the squares missing from it
//! named nearest first so the owner can start their builds (the modern
//! client starts at most 9 a frame); squares outside the ring's box plus one ring of
//! hysteresis evicted; each square's distance class kept.
//!
//! What a square *is* for the renderer is its business: [`FarWorld`] holds
//! one owner value `T` per square the owner started (its build state and
//! products; milestone F5 builds them on worker threads, `far_jobs`), and
//! each start has a ticket, so a result that comes back for a square that
//! was evicted, invalidated or restarted meanwhile is recognised as stale.

use std::collections::BTreeMap;

use crate::far_level::{distance_class, FarLevel};
use crate::far_ring::{distance_to_square, ring, square_of, within, SquareId};

/// Squares started per frame outside sync mode (the modern client allows 9
/// per frame; the builds themselves run on workers).
pub const FRAME_BUDGET: usize = 9;

/// One frame's view of the far world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FarView {
    /// The camera focus, absolute fine x and z.
    pub focus: [i32; 2],
    pub level: FarLevel,
}

/// One started square.
#[derive(Debug)]
pub struct FarSquare<T> {
    pub id: SquareId,
    /// The distance class at the last update ([`distance_class`]).
    pub class: u8,
    /// This start's ticket ([`FarWorld::start`]).
    pub ticket: u64,
    /// The owner's value.
    pub value: T,
}

/// What one plan changed and asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FarPlan {
    /// Squares dropped (outside the ring's box plus the hysteresis ring).
    pub evicted: Vec<SquareId>,
    /// Ring squares not started, nearest first.
    pub missing: Vec<SquareId>,
}

/// See the module docs.
pub struct FarWorld<T> {
    squares: BTreeMap<SquareId, FarSquare<T>>,
    ring: Vec<SquareId>,
    tickets: u64,
}

impl<T> Default for FarWorld<T> {
    fn default() -> Self {
        Self {
            squares: BTreeMap::new(),
            ring: Vec::new(),
            tickets: 0,
        }
    }
}

impl<T> FarWorld<T> {
    /// Bring the world to `view`: evict what left the ring (with one ring of
    /// hysteresis), refresh the classes, and name the ring squares not
    /// started yet, nearest first.
    pub fn plan(&mut self, view: &FarView) -> FarPlan {
        let radius = view.level.ring_radius();
        let centre = square_of(view.focus);
        self.ring = ring(view.focus, radius);
        let evicted: Vec<SquareId> = self
            .squares
            .keys()
            .copied()
            .filter(|&sq| !within(centre, sq, radius + 1))
            .collect();
        for sq in &evicted {
            self.squares.remove(sq);
        }
        for sq in self.squares.values_mut() {
            sq.class = distance_class(distance_to_square(view.focus, sq.id));
        }
        let missing = self
            .ring
            .iter()
            .copied()
            .filter(|sq| !self.squares.contains_key(sq))
            .collect();
        FarPlan { evicted, missing }
    }

    /// Start square `sq` with the owner's `value` (replacing a started one);
    /// returns its ticket.
    pub fn start(&mut self, sq: SquareId, view: &FarView, value: T) -> u64 {
        self.tickets += 1;
        self.squares.insert(
            sq,
            FarSquare {
                id: sq,
                class: distance_class(distance_to_square(view.focus, sq)),
                ticket: self.tickets,
                value,
            },
        );
        self.tickets
    }

    /// Square `sq` if it is still the start `ticket` names (a result for
    /// any other is stale).
    pub fn current_mut(&mut self, sq: SquareId, ticket: u64) -> Option<&mut FarSquare<T>> {
        self.squares.get_mut(&sq).filter(|s| s.ticket == ticket)
    }

    /// Started square `sq`, whatever its ticket.
    pub fn square_mut(&mut self, sq: SquareId) -> Option<&mut FarSquare<T>> {
        self.squares.get_mut(&sq)
    }

    /// The started squares, in square order.
    pub fn squares(&self) -> impl Iterator<Item = &FarSquare<T>> {
        self.squares.values()
    }

    /// Mutable access to the started squares.
    pub fn squares_mut(&mut self) -> impl Iterator<Item = &mut FarSquare<T>> {
        self.squares.values_mut()
    }

    /// Started square `sq`.
    #[must_use]
    pub fn square(&self, sq: SquareId) -> Option<&FarSquare<T>> {
        self.squares.get(&sq)
    }

    /// The last plan's ring, nearest first.
    #[must_use]
    pub fn ring(&self) -> &[SquareId] {
        &self.ring
    }

    /// Forget the squares `stale` names: the next plans name them missing
    /// again, and their outstanding results are stale.
    pub fn invalidate(&mut self, stale: &dyn Fn(SquareId) -> bool) -> usize {
        let before = self.squares.len();
        self.squares.retain(|&sq, _| !stale(sq));
        before - self.squares.len()
    }

    /// Drop everything (a new scene kind or a switch-off).
    pub fn clear(&mut self) {
        self.squares.clear();
        self.ring.clear();
    }
}
