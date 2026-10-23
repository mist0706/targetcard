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
}
