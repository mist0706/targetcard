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
//! Pure: no I/O, so it stays inside the parser core and under the wasm check.

use serde::Serialize;

/// What a card shows about one character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GearScore {
    pub name: String,
    pub server: String,
    pub class_name: Option<String>,
    pub level: Option<u32>,
    pub item_level: Option<u32>,
    pub combat_power: Option<i64>,
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
}
