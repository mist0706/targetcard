//! Cached character lookup against NC's official global API.
//!
//! Desktop only: this module owns the HTTP client and the cache file. Parsing and
//! URL building live in `score.rs`, which is pure and covered by the core tests.
//!
//! Etiquette: one search plus one info call per character, cached for six hours.
//! A card that re-queried on every poll would hammer NC's infrastructure and get
//! the IP rate-limited (HTTP 429 on the search endpoint).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::score::{
    exact_candidates, info_url, parse_score, same_class, search_url, server_id,
    Candidate, GearScore, ORIGIN, REFERER,
};

const TTL_SECS: u64 = 6 * 3600;
const TIMEOUT_SECS: u64 = 10;

#[derive(Serialize, Deserialize, Clone)]
struct Cached {
    score: GearScore,
    at: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    entries: HashMap<String, Cached>,
}

pub struct ScoreService {
    path: PathBuf,
    cache: HashMap<String, Cached>,
    client: reqwest::Client,
}

/// Managed Tauri state. `tokio::sync::Mutex` because lookups are async and hold
/// the lock across an await.
pub struct ScoreServiceState(pub tokio::sync::Mutex<ScoreService>);

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl ScoreService {
    pub fn new(config_dir: &Path) -> Self {
        let path = config_dir.join("targetcard-scores.json");
        let cache = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<CacheFile>(&raw).ok())
            .map(|file| file.entries)
            .unwrap_or_default();
        let client = reqwest::Client::builder()
            .user_agent(concat!("targetcard/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        Self { path, cache, client }
    }

    fn key(server: &str, name: &str) -> String {
        format!("{}:{}", server.trim().to_lowercase(), name.trim().to_lowercase())
    }

    /// A cached score, if it is still inside the TTL.
    pub fn cached(&self, name: &str, server: &str) -> Option<GearScore> {
        let hit = self.cache.get(&Self::key(server, name))?;
        (now_secs().saturating_sub(hit.at) < TTL_SECS).then(|| hit.score.clone())
    }

    /// Score for one character: cache, then search, then info.
    ///
    /// Every failure path logs why, because from the outside a missing combat
    /// power is indistinguishable between "the API has no such character", "the
    /// request never completed" and "the response changed shape". Those need
    /// different answers, and the log is the only place to tell them apart.
    pub async fn lookup(&mut self, name: &str, server: &str, class: Option<&str>) -> Option<GearScore> {
        let started = std::time::Instant::now();
        if let Some(hit) = self.cached(name, server) {
            tracing::debug!("targetcard: {name}@{server} cache hit");
            return Some(hit);
        }
        let Some(id) = server_id(server) else {
            tracing::warn!("targetcard: {name}: unknown server '{server}', nothing to look up");
            return None;
        };
        let candidates = self.search(name).await?;
        let Some(score) = self.resolve(name, id, class, started, &candidates).await else {
            return None;
        };
        self.cache.insert(
            Self::key(server, name),
            Cached { score: score.clone(), at: now_secs() },
        );
        self.flush();
        Some(score)
    }

    /// Every exact name match on every EU server.
    ///
    /// The search is fuzzy and names are not unique: `Tails` returns `Sixtails`,
    /// `Ninetails` and one `Tails` per EU server, five of them at level 45.
    async fn search(&self, name: &str) -> Option<Vec<Candidate>> {
        let payload = self.get_json(&search_url(name, None)).await?;
        let rows = payload
            .get("list")
            .and_then(|list| list.as_array())
            .map(|list| list.len())
            .unwrap_or(0);
        let candidates = exact_candidates(&payload, name);
        if candidates.is_empty() {
            tracing::debug!("targetcard: {name}: no exact match among {rows} search rows");
            return None;
        }
        Some(candidates)
    }

    /// Which candidate is the player on the card, and their score.
    ///
    /// 1. the one on the player's own server, when the name exists there
    /// 2. otherwise, when several arena-level players share the name, the one
    ///    whose class matches what the game showed us. Tails at level 45 exists
    ///    on five EU servers; only one of them is a Templar.
    /// 3. nothing, rather than a number that belongs to somebody else
    async fn resolve(
        &self,
        name: &str,
        local: u16,
        class: Option<&str>,
        started: std::time::Instant,
        candidates: &[Candidate],
    ) -> Option<GearScore> {
        // The player's own server first, but not blindly: a level 13 namesake on
        // our server must not shadow the level 45 opponent from another one. When
        // the game showed us a class, the own-server candidate has to be that
        // class to win, which costs one request and settles Althea-type cases.
        if let Some(candidate) = candidates.iter().filter(|c| c.server_id == local).max_by_key(|c| c.level).cloned() {
            if let Some(score) = self.score_of(&candidate, started).await {
                let agrees = match class {
                    Some(wanted) if !wanted.trim().is_empty() => score
                        .class_name
                        .as_deref()
                        .is_some_and(|name| same_class(name, wanted)),
                    _ => true,
                };
                if agrees {
                    tracing::debug!(
                        "targetcard: {name} -> {} ({}) chosen on our own server",
                        candidate.name,
                        candidate.server
                    );
                    return Some(score);
                }
                tracing::debug!(
                    "targetcard: {name}: {} on our server is a {:?}, not a {class:?}; looking across servers",
                    candidate.name,
                    score.class_name
                );
            }
        }

        let Some(class) = class.filter(|c| !c.trim().is_empty()) else {
            tracing::warn!(
                "targetcard: {name}: {} exact matches and no class to tell them apart; not guessing",
                candidates.len()
            );
            return None;
        };

        // Ambiguous name: ask about the arena-level candidates and keep the one
        // whose class is the class the game showed. At most six, once per name
        // per six hours, because the answer is cached like any other.
        let mut matched: Vec<GearScore> = Vec::new();
        for candidate in candidates.iter().filter(|c| c.level >= 40).take(6) {
            let Some(score) = self.score_of(candidate, started).await else {
                continue;
            };
            if score
                .class_name
                .as_deref()
                .is_some_and(|name| same_class(name, class))
            {
                matched.push(score);
            }
        }
        match matched.len() {
            1 => {
                let score = matched.pop()?;
                tracing::debug!(
                    "targetcard: {name} -> {} ({}) chosen by class {class}",
                    score.name,
                    score.server
                );
                Some(score)
            }
            0 => {
                tracing::warn!(
                    "targetcard: {name}: {} exact matches, none of them a {class}",
                    candidates.len()
                );
                None
            }
            n => {
                tracing::warn!(
                    "targetcard: {name}: {n} matches are a {class} as well; not guessing"
                );
                None
            }
        }
    }

    /// The score for one known candidate: one info request, parsed.
    async fn score_of(&self, candidate: &Candidate, started: std::time::Instant) -> Option<GearScore> {
        let payload = self
            .get_json(&info_url(&candidate.character_id, candidate.server_id))
            .await?;
        let score = parse_score(&payload)?;
        tracing::debug!(
            "targetcard: {} ({}) CP {:?} in {:?}",
            candidate.name,
            candidate.server,
            score.combat_power,
            started.elapsed()
        );
        Some(score)
    }

    async fn get_json(&self, url: &str) -> Option<serde_json::Value> {
        // Without Referer and Origin the API answers 403 with an empty body.
        let response = self
            .client
            .get(url)
            .header(reqwest::header::REFERER, REFERER)
            .header(reqwest::header::ORIGIN, ORIGIN)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        // reqwest is built without its `json` feature here (upstream only enables
        // rustls + stream), so parse the text ourselves with serde_json.
        let body = response.text().await.ok()?;
        serde_json::from_str(&body).ok()
    }

    fn flush(&self) {
        let file = CacheFile { entries: self.cache.clone() };
        if let Ok(raw) = serde_json::to_string(&file) {
            let _ = std::fs::write(&self.path, raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(item_level: u32) -> GearScore {
        GearScore {
            name: "Sins".into(),
            server: "Phernos".into(),
            class_name: Some("Assassin".into()),
            level: Some(45),
            item_level: Some(item_level),
            combat_power: Some(79658),
        }
    }

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("targetcard-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn fresh_entry_is_returned_and_expired_one_is_not() {
        let mut service = ScoreService::new(&scratch_dir());
        let key = ScoreService::key("Phernos", "Sins");
        service.cache.insert(key.clone(), Cached { score: score(1611), at: now_secs() });
        assert_eq!(service.cached("sins", "phernos").unwrap().item_level, Some(1611));

        service
            .cache
            .insert(key, Cached { score: score(1611), at: now_secs() - TTL_SECS - 1 });
        assert!(service.cached("Sins", "Phernos").is_none());
    }

    #[test]
    fn cache_round_trips_through_disk() {
        let dir = scratch_dir();
        let mut service = ScoreService::new(&dir);
        service.cache.insert(
            ScoreService::key("Phernos", "Sins"),
            Cached { score: score(1611), at: now_secs() },
        );
        service.flush();

        let reloaded = ScoreService::new(&dir);
        assert_eq!(reloaded.cached("Sins", "Phernos").unwrap().item_level, Some(1611));
    }

    #[test]
    fn unknown_server_short_circuits_before_any_request() {
        // No client call can happen here: server_id returns None first.
        let mut service = ScoreService::new(&scratch_dir());
        let result = tauri::async_runtime::block_on(service.lookup("Sins", "Nowhere", None));
        assert!(result.is_none());
    }
}
