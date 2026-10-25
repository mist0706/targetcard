//! Player cards: which entities are player characters, and what to show for them.
//!
//! AION 2 announces player characters through masked identity records
//! (`33 36` for yourself, `44 36` / `45 36` for every other player). Mob and
//! summon spawns arrive through `40 36` / `41 36` and never enter
//! `known_player_ids`, so membership in that set is the player/NPC test.
//!
//! Everything here is pure: it takes snapshots of the storage maps and returns
//! plain values, which keeps it inside the parser core (no `desktop` feature,
//! no wasm-hostile dependencies) and unit-testable without a game or a UI.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::entity::job_class::JobClass;

pub mod score;
pub mod snapshot;

#[cfg(feature = "desktop")]
pub mod score_service;

/// One row of the target card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlayerCard {
    pub entity_id: i32,
    pub name: String,
    pub class: Option<JobClass>,
    pub is_self: bool,
    /// True when the party roster names this player. A party member stays in a
    /// table even while standing still, because being in the group is the point.
    pub in_party: bool,
    /// Filled in from the party roster when the game states it, so a party
    /// member's card needs no web lookup at all.
    pub level: Option<u32>,
    pub gear_score: Option<i32>,
    pub combat_power: Option<i64>,
}

/// Build the card list for every known player character.
///
/// Ids in `player_ids` without a nickname yet (a player announced before the
/// name record lands) are skipped rather than shown as `#12345`: a card with no
/// name is noise, and the name arrives within the same zone load.
///
/// Sorted by name so the output is stable between polls, which keeps the UI
/// from shuffling rows and makes the unit tests deterministic.
pub fn build_cards(
    nicknames: &HashMap<i32, String>,
    player_ids: &HashSet<i32>,
    jobs: &HashMap<i32, JobClass>,
    self_id: Option<i64>,
) -> Vec<PlayerCard> {
    let self_id = self_id.map(|id| id as i32);
    let mut cards: Vec<PlayerCard> = player_ids
        .iter()
        .filter_map(|&id| {
            let name = nicknames.get(&id)?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(PlayerCard {
                entity_id: id,
                name,
                class: jobs.get(&id).copied(),
                is_self: self_id == Some(id),
                in_party: false,
                level: None,
                gear_score: None,
                combat_power: None,
            })
        })
        .collect();
    cards.sort_by(|a, b| a.name.cmp(&b.name).then(a.entity_id.cmp(&b.entity_id)));
    cards
}

/// The card for the entity a player most likely wants to look at right now.
///
/// Until the game's target-selection opcode is identified, the newest named
/// player wins, excluding yourself.
pub fn focus_card(cards: &[PlayerCard]) -> Option<&PlayerCard> {
    cards
        .iter()
        .filter(|card| !card.is_self)
        .max_by_key(|card| card.entity_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nicknames() -> HashMap<i32, String> {
        let mut map = HashMap::new();
        map.insert(4715, "Miku".to_string());
        map.insert(12, "David".to_string());
        map.insert(900, "NamedMob".to_string());
        map
    }

    fn players() -> HashSet<i32> {
        [4715, 12].into_iter().collect()
    }

    #[test]
    fn named_ids_outside_the_player_set_are_excluded() {
        let cards = build_cards(&nicknames(), &players(), &HashMap::new(), None);
        assert!(cards.iter().all(|c| c.name != "NamedMob"));
    }

    #[test]
    fn cards_cover_only_players_and_mark_self() {
        let cards = build_cards(&nicknames(), &players(), &HashMap::new(), Some(12));
        assert_eq!(cards.len(), 2);
        let david = cards.iter().find(|c| c.name == "David").unwrap();
        assert!(david.is_self);
        assert!(cards.iter().any(|c| c.name == "Miku" && !c.is_self));
    }

    #[test]
    fn class_is_attached_when_known() {
        let mut jobs = HashMap::new();
        jobs.insert(4715, JobClass::Gladiator);
        let cards = build_cards(&nicknames(), &players(), &jobs, None);
        let miku = cards.iter().find(|c| c.entity_id == 4715).unwrap();
        assert_eq!(miku.class, Some(JobClass::Gladiator));
    }

    #[test]
    fn player_without_a_name_yet_is_skipped() {
        let mut ids = players();
        ids.insert(4242);
        let cards = build_cards(&nicknames(), &ids, &HashMap::new(), None);
        assert!(cards.iter().all(|c| c.entity_id != 4242));
    }

    #[test]
    fn output_is_sorted_and_stable() {
        let cards = build_cards(&nicknames(), &players(), &HashMap::new(), None);
        let names: Vec<&str> = cards.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["David", "Miku"]);
    }

    #[test]
    fn focus_skips_self() {
        let cards = build_cards(&nicknames(), &players(), &HashMap::new(), Some(4715));
        let focus = focus_card(&cards).unwrap();
        assert_eq!(focus.name, "David");
    }
}
