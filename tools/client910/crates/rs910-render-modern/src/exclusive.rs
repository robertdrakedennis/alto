//! [`Exclusive`]: a value only `&mut` reaches, so a shared borrow of its
//! owner can cross threads although the value is not `Sync` (like the
//! standard library's unstable `std::sync::Exclusive`). The renderer's
//! frame state is prepared through `&mut ModernRenderer` and encoded on
//! worker threads through `&ModernRenderer` (`frame::jobs`); the few
//! prepare-only values that are not `Sync` (an `Rc` kept for its identity,
//! a config store with a `Cell`) live in one.

/// See the module docs.
#[derive(Default)]
pub struct Exclusive<T>(T);

impl<T> Exclusive<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

// SAFETY: no method reaches the value through `&Exclusive<T>`, so sharing
// one across threads shares nothing; it is moved or dropped only by its
// owner.
unsafe impl<T> Sync for Exclusive<T> {}
