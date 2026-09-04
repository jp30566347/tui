pub mod models;

use std::collections::HashMap;
use std::time::Duration;

use color_eyre::eyre::{Context, Result};
use models::*;
use tui_common::http::preview;

const BASE_URL: &str = "https://api-web.nhle.com/v1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on a response body. The largest endpoint here is ~110 KB, so this is
/// a wide margin; it exists so a hostile or malfunctioning endpoint (or a
/// captive portal) cannot exhaust memory.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Cheap to clone: `reqwest::Client` is internally reference counted.
#[derive(Clone)]
pub struct NhlClient {
    client: reqwest::Client,
}

impl Default for NhlClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NhlClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("nhl-tui/", env!("CARGO_PKG_VERSION")))
                .timeout(REQUEST_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str, what: &str) -> Result<T> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("{what}: request failed"))?
            .error_for_status()
            .with_context(|| format!("{what}: bad status"))?;
        let body = tui_common::http::read_body(resp, what, MAX_BODY_BYTES).await?;
        serde_json::from_str(&body)
            .with_context(|| format!("{what}: could not parse response ({})", preview(&body)))
    }

    pub async fn get_scores(&self, date: &str) -> Result<ScoreResponse> {
        self.get_json(&format!("{BASE_URL}/score/{date}"), "scores")
            .await
    }

    pub async fn get_standings(&self) -> Result<StandingsResponse> {
        self.get_json(&format!("{BASE_URL}/standings/now"), "standings")
            .await
    }

    pub async fn get_schedule(&self, date: &str) -> Result<ScheduleResponse> {
        self.get_json(&format!("{BASE_URL}/schedule/{date}"), "schedule")
            .await
    }

    /// Omitting `categories` returns every category the endpoint supports in a
    /// single response, keyed by category name.
    pub async fn get_skater_leaders(&self, limit: u32) -> Result<HashMap<String, Vec<StatLeader>>> {
        self.get_json(
            &format!("{BASE_URL}/skater-stats-leaders/current?limit={limit}"),
            "skater leaders",
        )
        .await
    }

    /// Same shape as the skater endpoint, over wins, GAA, save % and shutouts.
    pub async fn get_goalie_leaders(&self, limit: u32) -> Result<HashMap<String, Vec<StatLeader>>> {
        self.get_json(
            &format!("{BASE_URL}/goalie-stats-leaders/current?limit={limit}"),
            "goalie leaders",
        )
        .await
    }

    /// Per-period goals and shots plus the team stat comparison, none of
    /// which the landing endpoint carries. Smaller than `/boxscore`, which
    /// only offers a shots-on-goal total.
    pub async fn get_game_stats(&self, game_id: u64) -> Result<GameStats> {
        self.get_json(
            &format!("{BASE_URL}/gamecenter/{game_id}/right-rail"),
            "game stats",
        )
        .await
    }

    pub async fn get_boxscore(&self, game_id: u64) -> Result<BoxscoreResponse> {
        self.get_json(
            &format!("{BASE_URL}/gamecenter/{game_id}/landing"),
            "boxscore",
        )
        .await
    }
}
