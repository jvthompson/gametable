//! Small shared helpers. Port of the parts of `src/core/util.lua` that Rust does not already provide.
//!
//! Most of `util.lua` (`map`, `filter`, `find`, `copyWith`, ...) is covered by iterators and
//! `Clone`; what remains here is behaviour that must match Lua exactly: whitespace trimming,
//! Lua's floored modulo, and the shuffle algorithm with its random-number source.

/// The characters Lua's `%s` pattern class matches (C `isspace` in the "C" locale).
const LUA_WHITESPACE: [char; 6] = [' ', '\t', '\n', '\u{0B}', '\u{0C}', '\r'];

/// Trims the same characters as `util.trim` in Lua (`^%s+` and `%s+$`).
pub fn trim_lua_whitespace(text: &str) -> &str {
    text.trim_matches(&LUA_WHITESPACE[..])
}

/// Lua's `%` operator on numbers: the result has the sign of the divisor.
pub fn lua_modulo(dividend: f64, divisor: f64) -> f64 {
    dividend - (dividend / divisor).floor() * divisor
}

/// A source of random integers in `1..=upper_inclusive`, the contract of the `rng(n)` functions
/// the Lua code passes around.
pub trait RandomIndexSource {
    fn random_from_one_to(&mut self, upper_inclusive: usize) -> usize;
}

/// Production randomness, seeded from the operating system.
///
/// The original never seeded Lua's `math.random`; the port deliberately uses OS entropy.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRandom;

impl RandomIndexSource for SystemRandom {
    fn random_from_one_to(&mut self, upper_inclusive: usize) -> usize {
        rand::random_range(1..=upper_inclusive.max(1))
    }
}

/// Deterministic Park-Miller generator, identical to `util.seededRng` in Lua, so tests can
/// reproduce the exact orders the original tests expect.
#[derive(Debug, Clone)]
pub struct ParkMillerRandom {
    state: i64,
}

impl ParkMillerRandom {
    const MODULUS: i64 = 2_147_483_647;
    const MULTIPLIER: i64 = 16_807;

    pub fn with_seed(seed: i64) -> Self {
        Self { state: seed.rem_euclid(Self::MODULUS - 1) + 1 }
    }
}

impl RandomIndexSource for ParkMillerRandom {
    fn random_from_one_to(&mut self, upper_inclusive: usize) -> usize {
        self.state = (self.state * Self::MULTIPLIER) % Self::MODULUS;
        (self.state as usize % upper_inclusive) + 1
    }
}

/// Fisher-Yates shuffle in place, visiting positions in the same order as `util.shuffle` so a
/// seeded generator produces the same permutation as the Lua code.
pub fn shuffle_in_place<T>(items: &mut [T], random: &mut dyn RandomIndexSource) {
    for one_based_position in (2..=items.len()).rev() {
        let one_based_swap_partner = random.random_from_one_to(one_based_position);
        items.swap(one_based_position - 1, one_based_swap_partner - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_lua_whitespace_only() {
        assert_eq!(trim_lua_whitespace(" \t\u{0B}a b\r\n"), "a b");
        assert_eq!(trim_lua_whitespace("\u{A0}a"), "\u{A0}a");
    }

    #[test]
    fn lua_modulo_follows_the_divisor_sign() {
        assert_eq!(lua_modulo(-1.0, 6.0), 5.0);
        assert_eq!(lua_modulo(7.0, 6.0), 1.0);
    }

    #[test]
    fn park_miller_matches_lua_sequence() {
        // util.seededRng(42) in Lua: state starts at 43, then state = state * 16807 % 2147483647.
        let mut random = ParkMillerRandom::with_seed(42);
        let expected_first_state: i64 = 43 * 16807 % 2_147_483_647;
        assert_eq!(random.random_from_one_to(1000), (expected_first_state % 1000) as usize + 1);
    }

    #[test]
    fn shuffle_is_a_permutation_and_seeded_shuffles_repeat() {
        let mut first: Vec<u32> = (1..=20).collect();
        let mut second = first.clone();
        shuffle_in_place(&mut first, &mut ParkMillerRandom::with_seed(7));
        shuffle_in_place(&mut second, &mut ParkMillerRandom::with_seed(7));
        assert_eq!(first, second);
        let mut sorted = first.clone();
        sorted.sort();
        assert_eq!(sorted, (1..=20).collect::<Vec<_>>());
    }
}
