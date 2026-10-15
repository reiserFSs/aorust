//! MSVCR100's per-thread `rand`/`srand`, shared by native game and rendering consumers.
use std::cell::Cell;

thread_local! {
    static STATE: Cell<u32> = const { Cell::new(1) };
}

fn next(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(214013).wrapping_add(2531011);
    (*state >> 16) & 0x7fff
}

/// Draw from this thread's native CRT stream; an unseeded thread starts at 1.
pub fn crt_rand() -> u32 {
    STATE.with(|state| {
        let mut value = state.get();
        let result = next(&mut value);
        state.set(value);
        result
    })
}

/// Reseed this thread's CRT stream, including every existing shared handle.
pub fn crt_srand(seed: u32) { STATE.with(|state| state.set(seed)); }

/// An isolated fixture stream or an allocation-free handle to this thread's CRT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrtRand(Option<u32>);

impl Default for CrtRand {
    fn default() -> Self { Self::new(1) }
}

impl CrtRand {
    /// Explicitly seeded, isolated stream for deterministic fixtures.
    pub const fn new(seed: u32) -> Self { Self(Some(seed)) }
    /// The native thread stream. Constructing a handle does not reseed it.
    pub const fn thread() -> Self { Self(None) }
    pub fn reseed(&mut self, seed: u32) {
        match &mut self.0 { Some(state) => *state = seed, None => crt_srand(seed) }
    }
    pub fn rand(&mut self) -> u32 {
        match &mut self.0 { Some(state) => next(state), None => crt_rand() }
    }
    pub fn unit(&mut self) -> f64 { f64::from(self.rand()) / 32767.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_reseed_and_isolated_fixture_follow_native_thread_semantics() {
        let mut local = CrtRand::new(1);
        assert_eq!([local.rand(), local.rand(), local.rand(), local.rand()], [41, 18467, 6334, 26500]);
        let mut a = CrtRand::thread();
        let mut b = CrtRand::thread();
        a.reseed(1);
        assert_eq!((a.rand(), b.rand()), (41, 18467));
        b.reseed(42);
        let mut expected = CrtRand::new(42);
        assert_eq!(a.rand(), expected.rand());
        assert_eq!(crt_rand(), expected.rand());
        assert_eq!(std::thread::spawn(crt_rand).join().unwrap(), 41, "another native thread starts with its own default state");
        let mut isolated = CrtRand::new(1);
        assert_eq!(isolated.rand(), 41, "explicit fixtures ignore shared reseeds");
    }
}
