//! The snapshot the card window asks for once a second.
//!
//! Pure data derivation from `DataStorage`: no window, no command, no I/O, so it
//! stays in the parser core and is testable without a running app.
//!
//! Who counts as a player:
//!
//! - an id named by an authoritative `44 36` / `45 36` identity record (the game
//!   only sends those for player characters; mob nicknames are non-authoritative)
//! - an actor that dealt player-class damage and was not flagged as a summon
//!   (`known_player_ids`)
//!
//! Union of the two, because the first catches a player standing next to you who
//! has not fought yet, and the second catches a party member announced only
//! through damage. A mob is in neither.

use std::collections::HashSet;

use serde::Serialize;

use super::score::server_name_for_id;
use super::{build_cards, focus_card, PlayerCard};
use crate::combat::data_storage::DataStorage;

#[derive(Debug, Clone, Serialize)]
pub struct TargetSnapshot {
    /// Every player character currently known, sorted by name.
    pub players: Vec<PlayerCard>,
    /// The card to show right now, if any.
    pub focus_entity_id: Option<i32>,
    /// The local player's server, when the game has told us its id and we know
    /// the name for it. `None` lets the page fall back to its configured server.
    pub server_name: Option<String>,
}

/// Player ids: authoritative identity records plus confirmed player actors.
pub fn player_ids(storage: &DataStorage) -> HashSet<i32> {
    let mut ids = storage.get_known_player_ids();
    ids.extend(storage.get_authoritative_name_ids());
    ids
}

/// Which card to show.
///
/// The meter already tracks a current target from damage events
/// (`DataStorage::current_target`), so prefer that when it is a known player:
/// switching to a player and hitting them then moves the card immediately.
/// Otherwise fall back to the newest named player, which is all we have for a
/// player who has not been fought yet. The real selection opcode is still
/// unfound; this is the honest interim.
pub fn focus_entity(storage: &DataStorage, cards: &[PlayerCard]) -> Option<i32> {
    let current = storage.current_target();
    if current != 0
        && cards
            .iter()
            .any(|card| card.entity_id == current && !card.is_self)
    {
        return Some(current);
    }
    focus_card(cards).map(|card| card.entity_id)
}

pub fn snapshot(storage: &DataStorage) -> TargetSnapshot {
    let cards = build_cards(
        &storage.get_nicknames(),
        &player_ids(storage),
        &storage.get_actor_jobs(),
        storage.local_player_id(),
    );
    let focus_entity_id = focus_entity(storage, &cards);
    let server_name = storage
        .local_character_name()
        .map(|name| storage.player_server(&name))
        .filter(|server_id| *server_id != 0)
        .and_then(server_name_for_id)
        .map(str::to_string);
    TargetSnapshot { players: cards, focus_entity_id, server_name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_storage_gives_an_empty_snapshot() {
        let storage = DataStorage::new();
        let snapshot = snapshot(&storage);
        assert!(snapshot.players.is_empty());
        assert_eq!(snapshot.focus_entity_id, None);
        assert_eq!(snapshot.server_name, None);
    }

    #[test]
    fn an_authoritatively_named_player_is_a_card_and_the_focus() {
        let storage = DataStorage::new();
        // What scan_masked_identity does for a 44/45 36 record.
        storage.append_nickname_authoritative(4715, "Miku");
        let snapshot = snapshot(&storage);
        assert_eq!(snapshot.players.len(), 1);
        assert_eq!(snapshot.players[0].name, "Miku");
        assert_eq!(snapshot.focus_entity_id, Some(4715));
    }

    #[test]
    fn an_unnamed_player_does_not_become_a_card() {
        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4716, "");
        assert!(snapshot(&storage).players.is_empty());
    }

    #[test]
    fn a_combat_target_wins_over_the_newest_name() {
        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        storage.append_nickname_authoritative(4716, "Alpha");
        // The meter sets this from damage events: whoever you are fighting.
        storage.set_current_target(4715);
        assert_eq!(snapshot(&storage).focus_entity_id, Some(4715));
    }

    #[test]
    fn without_a_combat_target_the_newest_name_wins() {
        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        storage.append_nickname_authoritative(4716, "Alpha");
        storage.set_current_target(0);
        assert_eq!(snapshot(&storage).focus_entity_id, Some(4716));
    }

    #[test]
    fn a_combat_target_that_is_not_a_player_is_ignored() {
        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        // A mob id, tracked as a damage target but absent from the card list.
        storage.set_current_target(9001);
        assert_eq!(snapshot(&storage).focus_entity_id, Some(4715));
    }
}
