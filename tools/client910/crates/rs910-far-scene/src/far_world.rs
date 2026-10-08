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

#[cfg(test)]
mod tests {
    use super::*;

    /// Level 1: a 3 x 3 ring.
    fn at(x: i32, z: i32) -> FarView {
        FarView {
            focus: [x * 32_768 + 16_000, z * 32_768 + 30_000],
            level: FarLevel::new(1).unwrap(),
        }
    }

    /// The ring is planned nearest first; squares outside the box plus one
    /// hysteresis ring are evicted, those inside kept; the focus square is
    /// class 0.
    #[test]
    fn plans_nearest_first_and_evicts_with_hysteresis() {
        let mut world: FarWorld<()> = FarWorld::default();
        let plan = world.plan(&at(50, 50));
        assert_eq!(plan.missing.len(), 9);
        assert_eq!(plan.missing[0], (50, 50));
        for &sq in &plan.missing[..4] {
            world.start(sq, &at(50, 50), ());
        }
        // The rest next frame, in ring order.
        let plan = world.plan(&at(50, 50));
        assert_eq!(plan.missing.len(), 5);
        for sq in plan.missing {
            world.start(sq, &at(50, 50), ());
        }
        // One square east: the far west column stays (hysteresis), the new
        // east column is missing.
        let plan = world.plan(&at(51, 50));
        assert!(plan.evicted.is_empty());
        assert_eq!(plan.missing.len(), 3);
        for sq in plan.missing {
            world.start(sq, &at(51, 50), ());
        }
        // Two more: the old west columns go.
        let plan = world.plan(&at(53, 50));
        assert_eq!(plan.evicted.len(), 6);
        assert!(world.squares().all(|s| within((53, 50), s.id, 2)));
        for sq in plan.missing {
            world.start(sq, &at(53, 50), ());
        }
        assert_eq!(world.square((53, 50)).unwrap().class, 0);
    }

    /// A result is accepted only for the start it belongs to: an evicted,
    /// invalidated or restarted square's outstanding result is stale.
    #[test]
    fn results_of_evicted_or_restarted_squares_are_stale() {
        let mut world: FarWorld<u8> = FarWorld::default();
        let view = at(50, 50);
        let first = world.start((50, 50), &view, 0);
        let east = world.start((51, 50), &view, 0);
        assert!(world.current_mut((50, 50), first).is_some());
        // Invalidated (the classic window moved over it), then restarted.
        world.invalidate(&|sq| sq == (50, 50));
        assert!(world.current_mut((50, 50), first).is_none());
        let second = world.start((50, 50), &view, 1);
        assert!(world.current_mut((50, 50), first).is_none());
        assert_eq!(world.current_mut((50, 50), second).unwrap().value, 1);
        // Evicted by a far move.
        world.plan(&at(60, 50));
        assert!(world.current_mut((51, 50), east).is_none());
    }
}
