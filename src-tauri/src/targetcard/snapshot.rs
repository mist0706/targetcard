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
    /// The latest arrivals, newest first, capped for the table.
    pub recent: Vec<PlayerCard>,
    /// The card to show right now, if any.
    pub focus_entity_id: Option<i32>,
    /// The local player's server, when the game has told us its id and we know
    /// the name for it. `None` lets the page fall back to its configured server.
    pub server_name: Option<String>,
}

/// How many arrivals the table shows.
pub const RECENT_LIMIT: usize = 10;

/// How long a player stays in the table without producing any traffic.
///
/// The game re-sends state for anyone it can see, several times a second, so a
/// player who is still there keeps refreshing this. Thirty seconds is long
/// enough to survive a quiet moment and short enough that the person who walked
/// past while you were reading a quest text is gone by the time you look up.
pub const PRESENCE_TTL_MS: i64 = 30_000;

/// Is this player still around? Presence is refreshed by the state packets the
/// client sends for anything it can see, so a player who walked away stops
/// refreshing it and drops out after `PRESENCE_TTL_MS`.
fn is_present(storage: &DataStorage, card: &PlayerCard, now: i64) -> bool {
    storage
        .entity_last_seen(card.entity_id)
        .is_some_and(|at| now.saturating_sub(at) <= PRESENCE_TTL_MS)
}

/// The most recently appeared players, newest first.
///
/// Entity ids are handed out in spawn order, so the highest ids are the latest
/// arrivals. `build_cards` already sorts by name for the card list, so this is
/// the one place that ordering gets undone.
pub fn recent_players(cards: &[PlayerCard], limit: usize) -> Vec<PlayerCard> {
    let mut recent: Vec<PlayerCard> = cards.iter().filter(|card| !card.is_self).cloned().collect();
    recent.sort_by_key(|card| std::cmp::Reverse(card.entity_id));
    recent.truncate(limit);
    recent
}

/// Player ids: authoritative identity records plus confirmed player actors.
pub fn player_ids(storage: &DataStorage) -> HashSet<i32> {
    let mut ids = storage.get_known_player_ids();
    ids.extend(storage.get_authoritative_name_ids());
    ids
}

/// Which card to show, in order of confidence:
///
/// 1. the meter's tracked target, when that target is a player character
/// 2. the player whose damage event is the most recent, which covers fighting a
///    player while the meter's own target selection is following a mob
/// 3. the newest named player, which is all we have for someone who has not
///    been fought yet
///
/// The real selection opcode is still unfound; this is the honest interim.
pub fn focus_entity(storage: &DataStorage, cards: &[PlayerCard]) -> Option<i32> {
    let players: HashSet<i32> = cards
        .iter()
        .filter(|card| !card.is_self)
        .map(|card| card.entity_id)
        .collect();

    let tracked = storage.current_target();
    if tracked != 0 && players.contains(&tracked) {
        return Some(tracked);
    }

    if let Some(recent) = storage.most_recently_damaged(&players) {
        return Some(recent);
    }

    focus_card(cards).map(|card| card.entity_id)
}

pub fn snapshot(storage: &DataStorage) -> TargetSnapshot {
    let party = storage.get_party_members();
    // The party roster states each member's level, gear score and combat power,
    // so a party member's card is complete with no web lookup at all. Anything
    // the roster does not cover keeps the fields empty and the page falls back
    // to NC's character API.
    let cards: Vec<PlayerCard> = build_cards(
        &storage.get_nicknames(),
        &player_ids(storage),
        &storage.get_actor_jobs(),
        storage.local_player_id(),
    )
    .into_iter()
    .map(|mut card| {
        if let Some(member) = party.get(&card.name) {
            card.in_party = true;
            card.level = Some(member.level.max(0) as u32);
            card.gear_score = (member.gear_score > 0).then_some(member.gear_score);
            card.combat_power = (member.combat_power > 0).then_some(member.combat_power);
            if card.class.is_none() {
                card.class = member.job;
            }
        }
        card
    })
    .collect();
    let focus_entity_id = focus_entity(storage, &cards);
    let now = crate::clock::now_ms();
    // Party members are the meter's business, not this table's: they are already
    // in its roster with the same numbers, and repeating them here pushes the
    // strangers out of the ten slots.
    let visible: Vec<PlayerCard> = cards
        .iter()
        .filter(|card| !card.in_party)
        .filter(|card| is_present(storage, card, now))
        .cloned()
        .collect();
    let recent = recent_players(&visible, RECENT_LIMIT);
    let server_name = storage
        .local_character_name()
        .map(|name| storage.player_server(&name))
        .filter(|server_id| *server_id != 0)
        .and_then(server_name_for_id)
        .map(str::to_string);
    TargetSnapshot { players: cards, recent, focus_entity_id, server_name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_lists_the_latest_arrivals_newest_first() {
        let storage = DataStorage::new();
        // Ids are handed out in spawn order: 4715 arrived before 4717.
        storage.append_nickname_authoritative(4715, "Miku");
        storage.append_nickname_authoritative(4717, "Sins");
        storage.append_nickname_authoritative(4716, "Alpha");

        let names: Vec<String> = snapshot(&storage)
            .recent
            .into_iter()
            .map(|card| card.name)
            .collect();
        assert_eq!(names, vec!["Sins", "Alpha", "Miku"]);
    }

    #[test]
    fn the_table_stops_at_ten_rows() {
        let storage = DataStorage::new();
        for id in 100..115 {
            storage.append_nickname_authoritative(id, &format!("P{id}"));
        }
        let recent = snapshot(&storage).recent;
        assert_eq!(recent.len(), RECENT_LIMIT);
        // The newest arrival is first, so the oldest five must have fallen off.
        assert_eq!(recent[0].name, "P114");
        assert_eq!(recent[RECENT_LIMIT - 1].name, "P105");
    }

    #[test]
    fn a_player_who_produces_no_traffic_leaves_the_table() {
        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        storage.append_nickname_authoritative(4716, "Alpha");
        assert_eq!(snapshot(&storage).recent.len(), 2);

        // What note_despawn does when the game says the entity is gone.
        storage.forget_entity(4715);

        let names: Vec<String> = snapshot(&storage)
            .recent
            .into_iter()
            .map(|card| card.name)
            .collect();
        assert_eq!(names, vec!["Alpha"]);
    }

    #[test]
    fn party_members_are_left_to_the_meter() {
        use crate::combat::data_storage::PartyMember;

        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        storage.append_nickname_authoritative(4716, "Alpha");
        storage.set_party_roster(
            vec![(
                "Miku".to_string(),
                PartyMember { slot: 1, level: 45, ..Default::default() },
            )],
            true,
        );
        storage.forget_entity(4715);

        // Alpha is a stranger with presence; Miku is in the party, so the table
        // leaves her to the meter's own roster.
        let names: Vec<String> = snapshot(&storage)
            .recent
            .into_iter()
            .map(|card| card.name)
            .collect();
        assert_eq!(names, vec!["Alpha"]);
    }

    #[test]
    fn an_empty_storage_gives_an_empty_snapshot() {
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

    #[test]
    fn a_player_you_fought_wins_over_a_newer_name() {
        use crate::entity::damage_packet::ParsedDamagePacket;

        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        // The fallback would pick this one: newest id, so most recently named.
        storage.append_nickname_authoritative(4716, "Alpha");

        let mut hit = ParsedDamagePacket::new();
        hit.set_actor_id(1001);
        hit.set_target_id(4715);
        hit.set_damage(1200);
        hit.set_skill_code(200101);
        storage.append_damage(hit);
        storage.set_current_target(0); // the meter is tracking a mob, not a player

        assert_eq!(snapshot(&storage).focus_entity_id, Some(4715));
    }

    #[test]
    fn the_party_roster_fills_level_gear_and_power() {
        use crate::combat::data_storage::PartyMember;

        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        storage.set_party_roster(
            vec![(
                "Miku".to_string(),
                PartyMember {
                    slot: 1,
                    level: 45,
                    gear_score: 1611,
                    combat_power: 79_658,
                    ..Default::default()
                },
            )],
            true,
        );

        let card = &snapshot(&storage).players[0];
        assert_eq!(card.level, Some(45));
        assert_eq!(card.gear_score, Some(1611));
        assert_eq!(card.combat_power, Some(79_658));
    }

    #[test]
    fn a_stranger_outside_the_roster_keeps_empty_score_fields() {
        let storage = DataStorage::new();
        storage.append_nickname_authoritative(4715, "Miku");
        let card = &snapshot(&storage).players[0];
        assert_eq!(card.gear_score, None);
        assert_eq!(card.combat_power, None);
    }
}
