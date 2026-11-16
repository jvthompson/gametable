//! Booster pack generation. Port of `src/core/pack_generator.lua`.

use std::collections::HashSet;

use crate::models::{CardDefinition, PackSetting};
use crate::tag_match::parse_tag_entry;
use crate::util::{RandomIndexSource, shuffle_in_place};

/// Cards in a pack when no setting guarantees any.
pub const DEFAULT_PACK_CARD_COUNT: usize = 10;

/// Resolution used to turn a fractional chance into an integer roll.
const BONUS_ROLL_RESOLUTION: usize = 1_000_000;

/// Picks a pack from `set_cards`. Never repeats a card and never pads a short pack.
///
/// Each setting's integer part guarantees that many cards with its tag; the fractional part is
/// the chance of one extra card with that tag. With no guaranteed counts at all, the pack is
/// [`DEFAULT_PACK_CARD_COUNT`] random cards. Random numbers are drawn in the same order as the
/// original, so a seeded source picks the same cards.
pub fn select_pack_cards<'cards>(
    set_cards: &'cards [CardDefinition],
    pack_settings: &[PackSetting],
    random: &mut dyn RandomIndexSource,
) -> Vec<&'cards CardDefinition> {
    let mut picked: Vec<&CardDefinition> = Vec::new();
    let mut picked_ids: HashSet<&str> = HashSet::new();

    /// Shuffles the pool and adds up to `count` of its cards that are not already picked.
    fn take_random<'cards>(
        mut pool: Vec<&'cards CardDefinition>,
        count: usize,
        picked: &mut Vec<&'cards CardDefinition>,
        picked_ids: &mut HashSet<&'cards str>,
        random: &mut dyn RandomIndexSource,
    ) {
        shuffle_in_place(&mut pool, random);
        for card in pool.into_iter().take(count) {
            if picked_ids.insert(card.id.as_str()) {
                picked.push(card);
            }
        }
    }

    let pool_for_tag = |tag: &str, picked_ids: &HashSet<&str>| -> Vec<&'cards CardDefinition> {
        set_cards
            .iter()
            .filter(|card| !picked_ids.contains(card.id.as_str()))
            .filter(|card| card.types.iter().any(|entry| parse_tag_entry(entry).1 == tag))
            .collect()
    };

    // Phase 1: the guaranteed counts, or the default random fill when there are none.
    let any_guaranteed = pack_settings.iter().any(|setting| setting.count.floor() >= 1.0);
    if !any_guaranteed {
        take_random(set_cards.iter().collect(), DEFAULT_PACK_CARD_COUNT, &mut picked, &mut picked_ids, random);
    } else {
        for setting in pack_settings {
            let guaranteed_count = setting.count.floor();
            if guaranteed_count >= 1.0 {
                let pool = pool_for_tag(&setting.tag, &picked_ids);
                take_random(pool, guaranteed_count as usize, &mut picked, &mut picked_ids, random);
            }
        }
    }

    // Phase 2: each fractional remainder is the chance of one more card with that tag.
    for setting in pack_settings {
        let remainder = setting.count - setting.count.floor();
        if remainder > 0.0 {
            let roll = random.random_from_one_to(BONUS_ROLL_RESOLUTION);
            let threshold = (remainder * BONUS_ROLL_RESOLUTION as f64).floor() as usize;
            if roll <= threshold {
                let pool = pool_for_tag(&setting.tag, &picked_ids);
                take_random(pool, 1, &mut picked, &mut picked_ids, random);
            }
        }
    }
    picked
}
