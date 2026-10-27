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

/// How many search rows to ask for. Twenty was the upstream default and it was
/// silently truncating: a mixed-case "BeTeLGeuSe" on Munin sat at row 21 and the
/// card answered with a level 11 namesake instead.
pub const SEARCH_SIZE: usize = 100;

pub fn search_url(keyword: &str, server_id: Option<u16>) -> String {
    let mut url = format!(
        "{SEARCH_BASE}?keyword={}&page=1&size={SEARCH_SIZE}&localeInfo={LANG}&region={REGION_EU}",
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
/// Does the game's class name mean the API's class name?
///
/// The card normalises one pair: the game calls the class Elementalist where the
/// API says Spiritmaster, so a comparison has to treat them as the same.
pub fn same_class(a: &str, b: &str) -> bool {
    let norm = |value: &str| {
        let lower = value.trim().to_lowercase();
        if lower == "elementalist" {
            "spiritmaster".to_string()
        } else {
            lower
        }
    };
    !a.trim().is_empty() && norm(a) == norm(b)
}

pub fn exact_candidates(payload: &serde_json::Value, wanted: &str) -> Vec<Candidate> {
    let Some(rows) = payload.get("list").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let wanted = wanted.trim();
    let all: Vec<Candidate> = rows
        .iter()
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
        .collect();

    // The game states the name with its capitals and so does the API, so capitals
    // are evidence, not noise. "BeTeLGeuSe" on Munin is the player who was on
    // screen; "Betelgeuse" on Phernos is somebody else. Only when no candidate
    // matches exactly do we fall back to ignoring case, since a name typed in a
    // different case would otherwise resolve to nothing.
    let same_case: Vec<Candidate> = all
        .iter()
        .filter(|candidate| candidate.name == wanted)
        .cloned()
        .collect();
    if !same_case.is_empty() {
        if same_case.len() < all.len() {
            tracing::debug!(
                "targetcard: {wanted}: {} of {} matches have the same capitals; using those",
                same_case.len(),
                all.len()
            );
        }
        return same_case;
    }
    all
}

/// The combat power below which a level 45 is not really in the arena.
///
/// From the player: you cannot queue before level 45, and by then a character is
/// worth at least about 35,000, so a number far under 40,000 in a match is almost
/// always the wrong namesake. Used only as a tie-break between candidates that are
/// otherwise indistinguishable.
pub const ARENA_CP_FLOOR: i64 = 40_000;

/// Which of these scores is the only one plausible for an arena player?
///
/// Returns an index only when exactly one candidate reaches `floor` and at least one
/// other does not. Anything less clear than that stays a dash: a name with four
/// geared namesakes is still a guess.
pub fn unique_above(scores: &[GearScore], floor: i64) -> Option<usize> {
    let above: Vec<usize> = scores
        .iter()
        .enumerate()
        .filter(|(_, score)| score.combat_power.is_some_and(|cp| cp >= floor))
        .map(|(index, _)| index)
        .collect();
    if above.len() == 1 && scores.len() > 1 {
        Some(above[0])
    } else {
        None
    }
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
/// 2. the server paired with ours, which is where Abyss opponents come from
/// 3. a single candidate at all
/// 4. a single candidate at arena level, since level 45 is the only way into a 5v5
/// 5. several at arena level: only a class can settle it
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
    if let Some(partner) = paired_server(local) {
        if let Some(index) = plausible
            .iter()
            .copied()
            .filter(|&i| candidates[i].server_id == partner)
            .max_by_key(|&i| candidates[i].level)
        {
            return Plan::Use(index);
        }
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

/// One row of the official server list.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Server {
    pub id: u16,
    pub region: String,
    pub race: u8,
    pub name: String,
    pub short: String,
}

static SERVERS: std::sync::LazyLock<Vec<Server>> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Doc {
        servers: Vec<Server>,
    }
    serde_json::from_str::<Doc>(include_str!("../../../src/data/servers.json"))
        .map(|doc| doc.servers)
        .unwrap_or_default()
});

/// The opposing-faction server paired with this one.
///
/// Taken from the official server list rather than guessed: within a region the
/// Elyos and Asmodian servers are listed in a fixed order and pair by position, so
/// Phernos 1313 (PHE) is paired with Munin 2313 (MUN). This is the server an Abyss
/// opponent comes from, which makes it the strongest hint about who a name belongs
/// to.
pub fn paired_server(id: u16) -> Option<u16> {
    let all = SERVERS.as_slice();
    let mine = all.iter().find(|server| server.id == id)?;
    let mut same_region: Vec<&Server> = all
        .iter()
        .filter(|server| server.region == mine.region && server.race == mine.race)
        .collect();
    same_region.sort_by_key(|server| server.id);
    let position = same_region.iter().position(|server| server.id == id)?;
    let mut partners: Vec<&Server> = all
        .iter()
        .filter(|server| server.region == mine.region && server.race != mine.race)
        .collect();
    partners.sort_by_key(|server| server.id);
    partners.get(position).map(|server| server.id)
}

/// The three-letter tag the game prints beside a name, e.g. PHE for Phernos.
pub fn server_short_name(id: u16) -> Option<&'static str> {
    SERVERS
        .iter()
        .find(|server| server.id == id)
        .map(|server| server.short.as_str())
}

pub fn server_display_name(id: u16) -> Option<&'static str> {
    SERVERS
        .iter()
        .find(|server| server.id == id)
        .map(|server| server.name.as_str())
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
    fn a_fuzzy_search_only_yields_exact_names_and_prefers_the_same_capitals() {
        // Six people are spelled Tails, seven if capitals are ignored, and there is a
        // lowercase "tails" on Nania who is somebody else. Exact letters first, no
        // Sixtails either way.
        let candidates = exact_candidates(&tails_payload(), "Tails");
        assert_eq!(candidates.len(), 6, "the lowercase tails is a different spelling");
        assert!(candidates.iter().all(|c| c.name == "Tails"));

        // Asked for in lowercase, the lowercase name is the answer.
        let lower = exact_candidates(&tails_payload(), "tails");
        assert_eq!(lower.len(), 1);
        assert_eq!(lower[0].server, "Nania");
    }

    fn chosen(candidates: &[Candidate], plan_result: Plan) -> Candidate {
        match plan_result {
            Plan::Use(index) => candidates[index].clone(),
            other => panic!("expected a choice, got {other:?}"),
        }
    }

    #[test]
    fn the_players_own_server_wins() {
        let candidates = exact_candidates(&tails_payload(), "Tails");
        let picked = chosen(&candidates, plan(&candidates, 2310, false, 0));
        assert_eq!(picked.server, "Hadala");
        assert_eq!(picked.level, 33);
    }

    #[test]
    fn several_arena_level_matches_elsewhere_are_not_a_guess() {
        // This is the report that started it: five level 45 players named Tails
        // on other servers, and Phernos had nobody of that name.
        let candidates = exact_candidates(&tails_payload(), "Tails");
        assert_eq!(plan(&candidates, 1313, false, 0), Plan::GiveUp);

        // With no idea which server we are on, it is no better.
        assert_eq!(plan(&candidates, 9999, false, 0), Plan::GiveUp);
    }

    #[test]
    fn one_arena_level_match_is_enough_for_a_cross_server_opponent() {
        let payload = serde_json::json!({"list": [
            {"name": "<strong>Mikuwu</strong>", "characterId": "x", "level": 45, "serverId": 2302, "serverName": "Zikel"},
            {"name": "<strong>Mikuwuu</strong>", "characterId": "y", "level": 12, "serverId": 1313, "serverName": "Phernos"}
        ]});
        let candidates = exact_candidates(&payload, "Mikuwu");
        let picked = chosen(&candidates, plan(&candidates, 1313, false, 0));
        assert_eq!(picked.server, "Zikel");
    }

    #[test]
    fn a_lone_low_level_match_is_still_the_answer() {
        let payload = serde_json::json!({"list": [
            {"name": "<strong>Lowbie</strong>", "characterId": "x", "level": 8, "serverId": 1313, "serverName": "Phernos"}
        ]});
        let candidates = exact_candidates(&payload, "Lowbie");
        assert_eq!(chosen(&candidates, plan(&candidates, 1313, false, 40)).level, 8);
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

    fn search_result(name: &str, level: u32, server: u16, server_name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": format!("<strong>{name}</strong>"),
            "characterId": "abc%3D",
            "serverId": server,
            "serverName": server_name,
            "level": level,
            "race": 2
        })
    }

    #[test]
    fn capitals_narrow_a_name_that_exists_in_several_case_variants() {
        // The exact roster: the game said BeTeLGeuSe, and only Munin spells it that
        // way. Ignoring case let a level 11 alt on our own server answer instead.
        let payload = serde_json::json!({
            "list": [
                search_result("Betelgeuse", 11, 1313, "Phernos"),
                search_result("BeTeLGeuSe", 45, 2313, "Munin"),
                search_result("BETELGEUSE", 45, 2301, "Israphel")
            ]
        });
        let found = exact_candidates(&payload, "BeTeLGeuSe");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].server, "Munin");
    }

    #[test]
    fn a_name_with_no_capitals_in_common_still_matches() {
        let payload = serde_json::json!({
            "list": [search_result("Betelgeuse", 45, 2313, "Munin")]
        });
        let found = exact_candidates(&payload, "betelgeuse");
        assert_eq!(found.len(), 1, "case is evidence, not a requirement");
    }

    fn geared(name: &str, server: &str, class: &str, combat_power: i64, item_level: u32) -> GearScore {
        GearScore {
            name: name.into(),
            server: server.into(),
            race: Some(2),
            class_name: Some(class.into()),
            level: Some(45),
            item_level: Some(item_level),
            combat_power: Some(combat_power),
        }
    }

    #[test]
    fn the_one_arena_geared_candidate_wins_a_shared_name() {
        // Leu, the match that prompted this: a level 45 Chanter on Agnita at 37,031
        // and a level 45 Cleric on Ludra at 77,671. Nothing else separated them.
        let chanter = geared("Leu", "Agnita", "Chanter", 37_031, 740);
        let cleric = geared("Leu", "Ludra", "Cleric", 77_671, 1699);

        assert_eq!(
            unique_above(&[chanter.clone(), cleric], ARENA_CP_FLOOR),
            Some(1),
            "Ludra, the only one above the arena floor"
        );

        // The rule is about gear, not about the server: one under-geared name and
        // one geared one means the geared one is the arena player.
        let other = geared("Leu", "Agnita", "Chanter", 55_000, 900);
        assert_eq!(unique_above(&[geared("Leu", "Kasaka", "Gladiator", 4_666, 24), other], ARENA_CP_FLOOR), Some(1));

        // Two geared namesakes stay a guess.
        let first = geared("Leu", "Agnita", "Chanter", 55_000, 900);
        let second = geared("Leu", "Ludra", "Cleric", 77_671, 1699);
        assert_eq!(unique_above(&[first, second], ARENA_CP_FLOOR), None);
    }

    #[test]
    fn the_official_list_pairs_phernos_with_munin() {
        assert_eq!(paired_server(1313), Some(2313), "Phernos with Munin");
        assert_eq!(paired_server(2313), Some(1313), "and back again");
        assert_eq!(paired_server(1310), Some(2310), "Nania with Hadala");
        assert_eq!(server_short_name(2313), Some("MUN"));
        assert_eq!(server_short_name(1313), Some("PHE"));
        assert_eq!(paired_server(1), None, "not a server id");
        assert_eq!(paired_server(9999), None, "not in the list");
    }

    #[test]
    fn a_name_s_server_tag_can_be_looked_up() {
        // The arena prints these, and they are what would settle a shared name.
        assert_eq!(server_display_name(2301), Some("Israphel"));
        assert_eq!(server_short_name(2301), Some("ISR"));
    }

    #[test]
    fn the_paired_server_answers_when_our_own_only_has_an_alt() {
        let names = vec![
            candidate("BeTeLGeuSe", 11, 1313, "Phernos"),
            candidate("BeTeLGeuSe", 45, 2313, "Munin"),
            candidate("BeTeLGeuSe", 45, 2304, "Lumiel"),
        ];
        assert_eq!(plan(&names, 1313, false, 40), Plan::Use(1), "Munin, the Cleric");
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
