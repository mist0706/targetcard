//! Gear score extraction from NC's official global character API.
//!
//! Verified live 2026-10-24 against the European server Phernos (serverId 1313):
//!
//! ```text
//! GET https://aion2.plaync.com/api/character/info
//!     ?lang=en-US&characterId=<urlencoded>&serverId=1313&region=eu
//!     headers: Referer: https://aion2.plaync.com/en-us
//!              Origin:  https://aion2.plaync.com
//! ```
//!
//! One request returns both numbers the card needs:
//!
//! - gear score: `stat.statList[]` entry whose `type` is `ItemLevel` (its display
//!   name is localised, Korean `아이템레벨`, so match on `type`, never on `name`)
//! - combat power: `profile.combatPower`
//!
//! Unknown character: HTTP 404 `resource.not_found`. Missing `region`/`localeInfo`
//! on the search endpoint: HTTP 400 `InvalidSearchParamException`.
//!
//! Everything here is pure: URL building, the server table and response parsing.
//! The HTTP client and the cache live in `score_service.rs`, which is desktop-only,
//! so this file stays inside the parser core and under the wasm check.

use serde::{Deserialize, Serialize};

pub const SEARCH_BASE: &str = "https://api-search.plaync.com/aion2global/search/v2/character";
pub const INFO_BASE: &str = "https://aion2.plaync.com/api/character/info";
pub const REFERER: &str = "https://aion2.plaync.com/en-us";
pub const ORIGIN: &str = "https://aion2.plaync.com";
pub const LANG: &str = "en-US";
pub const REGION_EU: &str = "eu";

/// EU server ids, derived empirically from the search endpoint on 2026-10-24.
/// Elyos occupy 13xx and Asmodian 23xx. The numbering is not guessable, so a new
/// region has to be derived the same way: search a broad keyword and collect the
/// distinct `serverId`/`serverName` pairs.
pub const EU_SERVERS: &[(&str, u16)] = &[
    ("Siel", 1301), ("Nezekan", 1302), ("Vaizel", 1303), ("Kaisinel", 1304),
    ("Yustiel", 1305), ("Ariel", 1306), ("Fregion", 1307), ("Meslamtaeda", 1308),
    ("Hithanya", 1309), ("Nania", 1310), ("Tahavatha", 1311), ("Luteros", 1312),
    ("Phernos", 1313), ("Daminu", 1314), ("Kasaka", 1315), ("Bakarma", 1316),
    ("Tsenka", 1317), ("Kochi", 1318), ("Ishtar", 1319), ("Tiamat", 1320),
    ("Gauss", 1321), ("Lamuatan", 1322), ("Nathara", 1323),
    ("Israphel", 2301), ("Zikel", 2302), ("Triniel", 2303), ("Lumiel", 2304),
    ("Marchutan", 2305), ("Azphel", 2306), ("Ereshkigal", 2307), ("Beritra", 2308),
    ("Nemon", 2309), ("Hadala", 2310), ("Ludra", 2311), ("Ulgorn", 2312),
    ("Munin", 2313), ("Odar", 2314), ("Zemurru", 2315), ("Kromede", 2316),
    ("Quai", 2317), ("Baba", 2318), ("Fafnir", 2319), ("Indnath", 2320),
    ("Agnita", 2321), ("Atiel", 2322), ("Tassin", 2323),
];

/// What a card shows about one character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GearScore {
    pub name: String,
    pub server: String,
    /// Faction: 1 Elyos, 2 Asmodian, as the API states it. The card uses it to
    /// show only hostiles in the Abyss.
    pub race: Option<u8>,
    pub class_name: Option<String>,
    pub level: Option<u32>,
    pub item_level: Option<u32>,
    pub combat_power: Option<i64>,
}

/// Server id for a server name, case-insensitively.
pub fn server_id(name: &str) -> Option<u16> {
    EU_SERVERS
        .iter()
        .find(|(server, _)| server.eq_ignore_ascii_case(name.trim()))
        .map(|(_, id)| *id)
}

/// Server name for an id, as the game reports it in its own identity records.
/// Returns `None` for ids outside the table, so callers can fall back.
pub fn server_name_for_id(id: u16) -> Option<&'static str> {
    EU_SERVERS.iter().find(|(_, sid)| *sid == id).map(|(name, _)| *name)
}

/// `%XX`-encode everything outside the RFC 3986 unreserved set.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Undo `%XX` encoding. Invalid escapes pass through unchanged, which is what we
/// want for the `characterId` values the search endpoint returns: they arrive
/// already encoded, and a stray `%` should not lose the rest of the id.
pub fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The search endpoint wraps the matched substring in `<strong>` tags.
pub fn strip_tags(value: &str) -> String {
    value.replace("<strong>", "").replace("</strong>", "").trim().to_string()
}

pub fn search_url(keyword: &str, server_id: Option<u16>) -> String {
    let mut url = format!(
        "{SEARCH_BASE}?keyword={}&page=1&size=20&localeInfo={LANG}&region={REGION_EU}",
        encode(keyword)
    );
    if let Some(id) = server_id {
        url.push_str(&format!("&serverId={id}"));
    }
    url
}

/// What a lookup came back with, and why.
///
/// `reason` exists because "no such character" and "several characters and no way
/// to tell them apart" need opposite retry policies: one is final for half an
/// hour, the other should be asked again in a minute, since the class that
/// settles it often arrives once the fight starts.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScoreReply {
    pub score: Option<GearScore>,
    pub reason: &'static str,
}

impl ScoreReply {
    pub fn found(score: GearScore) -> Self {
        Self { score: Some(score), reason: "ok" }
    }
    pub fn missing(reason: &'static str) -> Self {
        Self { score: None, reason }
    }
}

/// One row of a search result, narrowed to what a lookup needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub character_id: String,
    pub server_id: u16,
    pub server: String,
    pub level: u32,
    pub race: Option<u8>,
}

/// Every exact name match in a search response.
///
/// The endpoint is a fuzzy search: asking for `Tails` returns `Sixtails`,
/// `Ninetails` and one `Tails` per EU server. Taking the first row regardless
/// produced a real number for the wrong character, which is worse than no
/// number, so only rows whose name is exactly the one asked for survive.
pub fn exact_candidates(payload: &serde_json::Value, wanted: &str) -> Vec<Candidate> {
    let Some(rows) = payload.get("list").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let wanted = wanted.trim();
    rows.iter()
        .filter_map(|row| {
            let name = strip_tags(row.get("name")?.as_str()?);
            if !name.eq_ignore_ascii_case(wanted) {
                return None;
            }
            Some(Candidate {
                name,
                character_id: decode(row.get("characterId")?.as_str()?),
                server_id: row.get("serverId")?.as_u64()? as u16,
                server: row.get("serverName")?.as_str()?.to_string(),
                level: row.get("level")?.as_u64()? as u32,
                race: row.get("race").and_then(|r| r.as_u64()).map(|r| r as u8),
            })
        })
        .collect()
}

/// Which candidate is the player the card is showing.
///
/// Names are not unique: `Tails` exists on six EU servers at level 45. In order:
///
/// 1. the player's own server, when it has a match
/// 2. otherwise a single candidate at arena level, since a match on several
///    servers with several level 45s is not an identification, it is a guess
/// 3. otherwise a single candidate of any level
///
/// `None` means "ask again another way rather than show a number that might
/// belong to somebody else".
pub fn pick_candidate(candidates: &[Candidate], local_server: Option<u16>) -> Option<Candidate> {
    if candidates.is_empty() {
        return None;
    }
    if let Some(server) = local_server {
        let mut here: Vec<&Candidate> = candidates.iter().filter(|c| c.server_id == server).collect();
        if !here.is_empty() {
            here.sort_by_key(|c| std::cmp::Reverse(c.level));
            return Some(here[0].clone());
        }
    }
    let mut grown: Vec<&Candidate> = candidates.iter().filter(|c| c.level >= 40).collect();
    grown.sort_by_key(|c| std::cmp::Reverse(c.level));
    if grown.len() == 1 {
        return Some(grown[0].clone());
    }
    if !grown.is_empty() {
        return None; // several arena-level players share the name
    }
    if candidates.len() == 1 {
        return Some(candidates[0].clone());
    }
    None
}

/// Are these the same class, given the two vocabularies in play?
///
/// The parser calls one class Elementalist; the game and the API call it
/// Spiritmaster. Everything else is spelled the same.
pub fn same_class(a: &str, b: &str) -> bool {
    let norm = |value: &str| {
        let lower = value.trim().to_lowercase();
        if lower == "elementalist" { "spiritmaster".to_string() } else { lower }
    };
    !a.trim().is_empty() && norm(a) == norm(b)
}

/// What to do with a set of exact name matches.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// One candidate is the player: ask about this index.
    Use(usize),
    /// Several could be: these indexes (arena level) need their class checked.
    ByClass(Vec<usize>),
    /// Nothing sensible to do without more information.
    GiveUp,
}

/// Decide what a set of exact name matches means.
///
/// Names are not unique, and the arena is cross-server, so the order matters:
///
/// 1. the player's own server, highest level first
/// 2. a single candidate at all
/// 3. a single candidate at arena level, since level 45 is the only way into a 5v5
/// 4. several at arena level: only a class can settle it
///
/// `floor` is the lowest level worth considering, normally our own level minus a
/// few. Names are shared across servers, and the one on your own server is often
/// a low-level alt: a level 11 "Betelgeuse" answered for the level 45 Asmodian who
/// killed the player in four hits, because it was on the right server. Candidates
/// below the floor are ignored unless nothing else is left, so a genuinely
/// low-level player still resolves.
///
/// Rule 2 is not a detail. Without it a name with one match anywhere in EU came
/// back as a dash, which is how an entire arena roster turned into dashes.
pub fn plan(candidates: &[Candidate], local: u16, has_class: bool, floor: u32) -> Plan {
    if candidates.is_empty() {
        return Plan::GiveUp;
    }
    // Everything below works on the candidates that are plausible for someone we
    // could actually be fighting. If none are, the name really does belong to a
    // low-level character and the old order applies unchanged.
    let plausible: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.level >= floor)
        .map(|(index, _)| index)
        .collect();
    if plausible.is_empty() {
        return plan(candidates, local, has_class, 0);
    }
    if let Some(index) = plausible
        .iter()
        .copied()
        .filter(|&i| candidates[i].server_id == local)
        .max_by_key(|&i| candidates[i].level)
    {
        return Plan::Use(index);
    }
    if plausible.len() == 1 {
        return Plan::Use(plausible[0]);
    }
    let mut arena: Vec<usize> = plausible
        .into_iter()
        .filter(|&i| candidates[i].level >= 40)
        .collect();
    arena.sort_by_key(|&index| std::cmp::Reverse(candidates[index].level));
    if arena.len() == 1 {
        return Plan::Use(arena[0]);
    }
    if arena.len() > 1 && has_class {
        return Plan::ByClass(arena.into_iter().take(6).collect());
    }
    Plan::GiveUp
}

pub fn info_url(character_id: &str, server_id: u16) -> String {
    format!(
        "{INFO_BASE}?lang={LANG}&characterId={}&serverId={server_id}&region={REGION_EU}",
        encode(character_id)
    )
}

/// Pull a score out of a `/api/character/info` response.
///
/// Returns `None` when the payload is not a character at all (an error body, an
/// empty object), so a bad response degrades to "GS ..." instead of a zero.
pub fn parse_score(payload: &serde_json::Value) -> Option<GearScore> {
    let profile = payload.get("profile")?;
    let name = profile.get("characterName")?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    let server = profile
        .get("serverName")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let item_level = payload
        .get("stat")
        .and_then(|s| s.get("statList"))
        .and_then(|l| l.as_array())
        .and_then(|list| {
            list.iter()
                .find(|entry| entry.get("type").and_then(|t| t.as_str()) == Some("ItemLevel"))
                .and_then(|entry| entry.get("value"))
                .and_then(|v| v.as_u64())
        })
        .map(|v| v as u32);
    let combat_power = profile.get("combatPower").and_then(|v| v.as_i64());
    Some(GearScore {
        name,
        server,
        // The info payload does not carry a faction; the service fills this in
        // from the search row that led here.
        race: None,
        class_name: profile
            .get("className")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        level: profile
            .get("characterLevel")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        item_level,
        combat_power,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real response for `Sins` on Phernos, 2026-10-24.
    const REAL: &str = r#"{
      "profile": {
        "characterName": "Sins", "className": "Assassin", "characterLevel": 45,
        "combatPower": 79658, "serverName": "Phernos", "serverId": 1313,
        "raceName": "Elyos"
      },
      "stat": { "statList": [
        { "name": "Might", "type": "STR", "value": 25 },
        { "name": "아이템레벨", "type": "ItemLevel", "value": 1611 }
      ] }
    }"#;

    #[test]
    fn reads_gear_score_and_combat_power() {
        let payload: serde_json::Value = serde_json::from_str(REAL).unwrap();
        let score = parse_score(&payload).unwrap();
        assert_eq!(score.item_level, Some(1611));
        assert_eq!(score.combat_power, Some(79658));
        assert_eq!(score.class_name.as_deref(), Some("Assassin"));
        assert_eq!(score.level, Some(45));
        assert_eq!(score.server, "Phernos");
    }

    #[test]
    fn matches_item_level_on_type_not_localised_name() {
        // The display name is Korean; a name-based lookup would miss it.
        let payload: serde_json::Value = serde_json::from_str(
            r#"{"profile":{"characterName":"X"},"stat":{"statList":[
                 {"name":"아이템레벨","type":"ItemLevel","value":2673}]}}"#,
        )
        .unwrap();
        assert_eq!(parse_score(&payload).unwrap().item_level, Some(2673));
    }

    #[test]
    fn error_body_yields_none() {
        let payload: serde_json::Value = serde_json::from_str(
            r#"{"detail":"Resource not found","status":404,"code":"resource.not_found"}"#,
        )
        .unwrap();
        assert!(parse_score(&payload).is_none());
    }

    #[test]
    fn level_one_character_still_parses() {
        let payload: serde_json::Value = serde_json::from_str(
            r#"{"profile":{"characterName":"A","className":"Cleric","characterLevel":1,
                 "combatPower":2882,"serverName":"Phernos"},
                "stat":{"statList":[{"name":"아이템레벨","type":"ItemLevel","value":1}]}}"#,
        )
        .unwrap();
        let score = parse_score(&payload).unwrap();
        assert_eq!(score.item_level, Some(1));
        assert_eq!(score.combat_power, Some(2882));
    }

    #[test]
    fn server_table_round_trips() {
        assert_eq!(server_id("Phernos"), Some(1313));
        assert_eq!(server_id("phernos"), Some(1313));
        assert_eq!(server_id("  Munin "), Some(2313));
        assert_eq!(server_id("Nowhere"), None);
        assert_eq!(server_name_for_id(1313), Some("Phernos"));
        assert_eq!(server_name_for_id(9), None);
    }

    #[test]
    fn urls_carry_the_required_parameters() {
        let search = search_url("Sin s", Some(1313));
        assert!(search.contains("keyword=Sin%20s"), "{search}");
        assert!(search.contains("region=eu"), "{search}");
        assert!(search.contains("localeInfo=en-US"), "{search}");
        assert!(search.contains("serverId=1313"), "{search}");

        let info = info_url("tMf1G5JFEh2Qy9U5BXofyxXsFPz28DSR2au5MkolSZ4=", 1313);
        assert!(info.contains("serverId=1313"), "{info}");
        assert!(info.contains("%3D"), "the trailing = must be encoded: {info}");
        assert!(!info.contains("SZ4="), "raw = must not survive: {info}");
    }

    #[test]
    fn character_ids_round_trip_through_encoding() {
        let raw = "tMf1G5JFEh2Qy9U5BXofyxXsFPz28DSR2au5MkolSZ4=";
        assert_eq!(decode(&encode(raw)), raw);
        assert_eq!(decode(&encode("a+b/c==")), "a+b/c==");
    }

    #[test]
    fn search_highlight_tags_are_stripped() {
        assert_eq!(strip_tags("<strong>Sins</strong>"), "Sins");
        assert_eq!(strip_tags(" tch<strong>sins</strong> "), "tchsins");
    }

    /// The rows this endpoint really returns for `Tails`, trimmed to the fields
    /// the lookup reads: one exact match per EU server, six of them level 45.
    fn tails_payload() -> serde_json::Value {
        let rows = [
            ("tails", 17, 1310, "Nania"),
            ("Tails", 45, 2312, "Ulgorn"),
            ("Tails", 33, 2310, "Hadala"),
            ("Tails", 45, 2301, "Israphel"),
            ("Tails", 45, 2302, "Zikel"),
            ("Tails", 45, 2303, "Triniel"),
            ("Tails", 45, 2311, "Ludra"),
            ("Tailss", 1, 2312, "Ulgorn"),
            ("Sixtails", 1, 1313, "Phernos"),
            ("Ninetails", 1, 1313, "Phernos"),
        ];
        serde_json::json!({
            "list": rows.iter().map(|(name, level, server, server_name)| serde_json::json!({
                "name": format!("<strong>{name}</strong>"),
                "characterId": "abc%3D",
                "level": level,
                "serverId": server,
                "serverName": server_name,
            })).collect::<Vec<_>>()
        })
    }

    #[test]
    fn class_names_are_compared_across_vocabularies() {
        assert!(same_class("Templar", "templar"));
        assert!(same_class("Elementalist", "Spiritmaster"));
        assert!(same_class("Spiritmaster", "Elementalist"));
        assert!(!same_class("Templar", "Assassin"));
        assert!(!same_class("", "Templar"));
    }

    #[test]
    fn a_fuzzy_search_only_yields_exact_names() {
        let candidates = exact_candidates(&tails_payload(), "Tails");
        assert_eq!(candidates.len(), 7, "one exact match per server, no Sixtails");
        assert!(candidates.iter().all(|c| c.name.eq_ignore_ascii_case("Tails")));
    }

    #[test]
    fn the_players_own_server_wins() {
        let candidates = exact_candidates(&tails_payload(), "Tails");
        let picked = pick_candidate(&candidates, Some(2310)).expect("Hadala Tails");
        assert_eq!(picked.server, "Hadala");
        assert_eq!(picked.level, 33);
    }

    #[test]
    fn several_arena_level_matches_elsewhere_are_not_a_guess() {
        // This is the report that started it: five level 45 players named Tails
        // on other servers, and Phernos had nobody of that name.
        let candidates = exact_candidates(&tails_payload(), "Tails");
        assert_eq!(pick_candidate(&candidates, Some(1313)), None);

        // With no idea which server we are on, it is no better.
        assert_eq!(pick_candidate(&candidates, None), None);
    }

    #[test]
    fn one_arena_level_match_is_enough_for_a_cross_server_opponent() {
        let payload = serde_json::json!({"list": [
            {"name": "<strong>Mikuwu</strong>", "characterId": "x", "level": 45, "serverId": 2302, "serverName": "Zikel"},
            {"name": "<strong>Mikuwuu</strong>", "characterId": "y", "level": 12, "serverId": 1313, "serverName": "Phernos"}
        ]});
        let candidates = exact_candidates(&payload, "Mikuwu");
        let picked = pick_candidate(&candidates, Some(1313)).expect("the only level 45 Mikuwu");
        assert_eq!(picked.server, "Zikel");
    }

    #[test]
    fn a_lone_low_level_match_is_still_the_answer() {
        let payload = serde_json::json!({"list": [
            {"name": "<strong>Lowbie</strong>", "characterId": "x", "level": 8, "serverId": 1313, "serverName": "Phernos"}
        ]});
        let candidates = exact_candidates(&payload, "Lowbie");
        assert_eq!(pick_candidate(&candidates, Some(1313)).map(|c| c.level), Some(8));
    }

    fn candidate(name: &str, level: u32, server: u16, server_name: &str) -> Candidate {
        Candidate {
            name: name.to_string(),
            character_id: format!("id-{server_name}"),
            server_id: server,
            server: server_name.to_string(),
            level,
            race: None,
        }
    }

    #[test]
    fn one_match_anywhere_is_the_answer_even_with_no_class() {
        let only = vec![candidate("7rameee", 45, 2321, "Agnita")];
        assert_eq!(plan(&only, 1313, false, 0), Plan::Use(0));
    }

    #[test]
    fn one_arena_level_match_settles_a_shared_name() {
        let names = vec![
            candidate("Merum", 12, 1310, "Nania"),
            candidate("Merum", 45, 2313, "Munin"),
        ];
        assert_eq!(plan(&names, 1313, false, 0), Plan::Use(1));
    }

    #[test]
    fn our_own_server_wins_over_a_higher_level_elsewhere() {
        let names = vec![
            candidate("Kiria", 45, 2312, "Ulgorn"),
            candidate("Kiria", 41, 1313, "Phernos"),
        ];
        assert_eq!(plan(&names, 1313, false, 0), Plan::Use(1));
    }

    #[test]
    fn an_alt_on_our_server_does_not_answer_for_the_opponent_who_killed_us() {
        // The real roster: a level 11 Elyos on our own server, and five level 45
        // Asmodians elsewhere. The wrong answer was 5,333 combat power.
        let names = vec![
            candidate("Betelgeuse", 11, 1313, "Phernos"),
            candidate("Betelgeuse", 45, 2315, "Zemurru"),
            candidate("Betelgeuse", 45, 2312, "Ulgorn"),
            candidate("Betelgeuse", 45, 2304, "Lumiel"),
            candidate("Betelgeuse", 45, 2310, "Hadala"),
            candidate("Betelgeuse", 45, 2301, "Israphel"),
        ];
        assert_eq!(plan(&names, 1313, false, 40), Plan::GiveUp, "five, so no guess");
        assert_eq!(
            plan(&names, 1313, true, 40),
            Plan::ByClass(vec![1, 2, 3, 4, 5])
        );
    }

    #[test]
    fn a_genuinely_low_level_player_still_resolves() {
        let names = vec![candidate("Betelgeuse", 11, 1313, "Phernos")];
        assert_eq!(plan(&names, 1313, false, 40), Plan::Use(0));
    }

    #[test]
    fn several_arena_level_matches_need_a_class() {
        let names = vec![
            candidate("Kar", 45, 2312, "Ulgorn"),
            candidate("Kar", 45, 2302, "Zikel"),
        ];
        assert_eq!(plan(&names, 1313, false, 0), Plan::GiveUp);
        assert_eq!(plan(&names, 1313, true, 0), Plan::ByClass(vec![0, 1]));
    }
}
