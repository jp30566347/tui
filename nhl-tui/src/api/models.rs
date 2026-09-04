use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct ScoreResponse {
    pub games: Vec<Game>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: u64,
    /// The API spells this `startTimeUTC`, which `rename_all = "camelCase"`
    /// would otherwise map to `startTimeUtc` and silently leave as `None`.
    #[serde(rename = "startTimeUTC")]
    pub start_time_utc: Option<String>,
    pub game_state: String,
    pub away_team: TeamScore,
    pub home_team: TeamScore,
    pub game_outcome: Option<GameOutcome>,
    pub period: Option<u32>,
    pub period_descriptor: Option<PeriodDescriptor>,
    pub clock: Option<GameClock>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TeamScore {
    pub abbrev: String,
    pub score: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TeamName {
    pub default: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GameOutcome {
    pub last_period_type: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GameClock {
    pub time_remaining: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PeriodDescriptor {
    pub number: u32,
    pub period_type: Option<String>,
}

impl PeriodDescriptor {
    /// "1st", "2nd", "3rd", "OT", "2OT", "SO".
    pub fn label(&self) -> String {
        match self.period_type.as_deref() {
            Some("SO") => "SO".to_string(),
            _ => match self.number {
                1 => "1st".to_string(),
                2 => "2nd".to_string(),
                3 => "3rd".to_string(),
                4 => "OT".to_string(),
                // saturating: the period number is server-supplied, and 0
                // would underflow.
                n => format!("{}OT", n.saturating_sub(3)),
            },
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct StandingsResponse {
    pub standings: Vec<Standing>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Standing {
    pub team_name: TeamName,
    pub team_abbrev: TeamAbbrev,
    pub conference_name: String,
    pub division_name: String,
    pub games_played: u32,
    pub wins: u32,
    pub losses: u32,
    pub ot_losses: u32,
    pub points: u32,
    pub goal_for: u32,
    pub goal_against: u32,
    pub goal_differential: i32,
    pub streak_code: Option<String>,
    pub streak_count: Option<u32>,
    pub division_sequence: Option<u32>,
    pub conference_sequence: Option<u32>,
    pub league_sequence: Option<u32>,
    pub point_pctg: Option<f64>,
    pub l10_wins: Option<u32>,
    pub l10_losses: Option<u32>,
    pub l10_ot_losses: Option<u32>,
    /// 0 for a team holding a top-three spot in its division; 1 and 2 are the
    /// two wild card berths; 3 and up are outside the playoff picture.
    pub wildcard_sequence: Option<u32>,
}

impl Standing {
    /// Whether the team currently holds a playoff berth: a divisional top
    /// three, or one of the conference's two wild cards.
    pub fn in_playoff_spot(&self) -> bool {
        matches!(self.wildcard_sequence, Some(0..=2))
    }

    /// Record over the last ten games, as "W-L-OTL".
    pub fn last_ten(&self) -> Option<String> {
        Some(format!(
            "{}-{}-{}",
            self.l10_wins?, self.l10_losses?, self.l10_ot_losses?
        ))
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct TeamAbbrev {
    pub default: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleResponse {
    pub game_week: Vec<GameDay>,
    /// Nearest dates on either side that actually have games, and the season
    /// boundaries. These are what make an empty week actionable.
    pub next_start_date: Option<String>,
    pub previous_start_date: Option<String>,
    pub regular_season_start_date: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GameDay {
    pub date: String,
    pub day_abbrev: String,
    pub games: Vec<ScheduleGame>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleGame {
    #[serde(rename = "startTimeUTC")]
    pub start_time_utc: String,
    pub away_team: ScheduleTeam,
    pub home_team: ScheduleTeam,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleTeam {
    pub abbrev: String,
    pub place_name: Option<TeamName>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatLeader {
    pub first_name: Option<NameField>,
    pub last_name: Option<NameField>,
    pub position: Option<String>,
    pub team_abbrev: Option<String>,
    /// Integer for counting stats, fractional for faceoff % and time on ice.
    pub value: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NameField {
    pub default: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BoxscoreResponse {
    pub away_team: BoxscoreTeam,
    pub home_team: BoxscoreTeam,
    pub summary: Option<Summary>,
}

/// The `/right-rail` endpoint: goals and shots broken out by period, plus the
/// team stat comparison. The `/landing` endpoint above carries the scoring and
/// penalty summaries but none of this.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GameStats {
    pub linescore: Option<Linescore>,
    #[serde(default)]
    pub shots_by_period: Vec<PeriodCount>,
    #[serde(default)]
    pub team_game_stats: Vec<TeamStat>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Linescore {
    #[serde(default)]
    pub by_period: Vec<PeriodCount>,
}

/// One period's worth of a per-side count, used for both goals and shots.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PeriodCount {
    pub period_descriptor: PeriodDescriptor,
    pub away: u32,
    pub home: u32,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TeamStat {
    pub category: String,
    /// Mixed: a plain number for shots, a string like "22/49" for faceoffs.
    pub away_value: serde_json::Value,
    pub home_value: serde_json::Value,
}

impl TeamStat {
    /// The label to show, or `None` for categories deliberately not rendered
    /// (the percentage variants duplicate the raw counts beside them).
    pub fn label(&self) -> Option<&'static str> {
        Some(match self.category.as_str() {
            "sog" => "Shots",
            "faceoffWins" => "Faceoffs",
            "powerPlay" => "Power play",
            "pim" => "PIM",
            "hits" => "Hits",
            "blockedShots" => "Blocks",
            "giveaways" => "Giveaways",
            "takeaways" => "Takeaways",
            _ => return None,
        })
    }
}

/// Renders a mixed number-or-string stat value.
pub fn stat_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => match n.as_f64() {
            // Whole numbers are counts; anything fractional is a rate.
            Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
            Some(f) => format!("{:.1}%", f * 100.0),
            None => n.to_string(),
        },
        _ => String::new(),
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub scoring: Option<Vec<ScoringPeriod>>,
    pub penalties: Option<Vec<PenaltyPeriod>>,
    pub three_stars: Option<Vec<ThreeStar>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PenaltyPeriod {
    pub period_descriptor: PeriodDescriptor,
    pub penalties: Vec<Penalty>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Penalty {
    pub time_in_period: String,
    pub duration: Option<u32>,
    pub committed_by_player: Option<PlayerName>,
    pub team_abbrev: Option<NameField>,
    /// A slug such as "interference"; rendered with the underscores removed.
    pub desc_key: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlayerName {
    pub first_name: Option<NameField>,
    pub last_name: Option<NameField>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThreeStar {
    pub star: u32,
    pub name: Option<NameField>,
    pub team_abbrev: Option<String>,
    pub position: Option<String>,
    pub goals: Option<u32>,
    pub assists: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BoxscoreTeam {
    pub abbrev: String,
    pub name: Option<TeamName>,
    pub score: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScoringPeriod {
    pub period_descriptor: PeriodDescriptor,
    pub goals: Vec<Goal>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub time_in_period: String,
    pub team_abbrev: NameField,
    pub first_name: Option<NameField>,
    pub last_name: Option<NameField>,
    pub strength: Option<String>,
    pub assists: Vec<Assist>,
    pub goals_to_date: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Assist {
    pub first_name: Option<NameField>,
    pub last_name: Option<NameField>,
    pub assists_to_date: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live endpoint spells this field `startTimeUTC`. Under a plain
    /// `rename_all = "camelCase"` it silently deserialized as `None` and every
    /// upcoming game rendered as "TBD".
    #[test]
    fn game_parses_the_uppercase_start_time_field() {
        let json = r#"{
            "id": 2025021000,
            "startTimeUTC": "2026-03-10T23:00:00Z",
            "gameState": "FUT",
            "awayTeam": { "abbrev": "TOR", "score": 0 },
            "homeTeam": { "abbrev": "MTL", "score": 0 },
            "periodDescriptor": { "number": 4, "periodType": "OT" },
            "gameOutcome": { "lastPeriodType": "OT" }
        }"#;
        let game: Game = serde_json::from_str(json).expect("game should parse");
        assert_eq!(game.start_time_utc.as_deref(), Some("2026-03-10T23:00:00Z"));
        assert_eq!(
            game.period_descriptor.map(|p| p.label()).as_deref(),
            Some("OT")
        );
    }

    /// Optional fields the score endpoint omits for scheduled games must not
    /// fail the whole response.
    #[test]
    fn game_tolerates_missing_optional_fields() {
        let json = r#"{
            "id": 1,
            "gameState": "FUT",
            "awayTeam": { "abbrev": "TOR" },
            "homeTeam": { "abbrev": "MTL" }
        }"#;
        let game: Game = serde_json::from_str(json).expect("sparse game should parse");
        assert!(game.start_time_utc.is_none());
        assert!(game.clock.is_none());
        assert!(game.away_team.score.is_none());
    }

    /// Counting stats come back as integers and faceoff/TOI as floats; both
    /// have to land in the same field.
    #[test]
    fn stat_leader_value_accepts_integers_and_floats() {
        let int: StatLeader =
            serde_json::from_str(r#"{ "firstName": {"default": "Connor"}, "value": 138 }"#)
                .expect("integer value should parse");
        assert_eq!(int.value, Some(138.0));

        let float: StatLeader =
            serde_json::from_str(r#"{ "lastName": {"default": "Giroux"}, "value": 0.630788 }"#)
                .expect("float value should parse");
        assert_eq!(float.value, Some(0.630788));
    }
}

#[cfg(test)]
mod right_rail_tests {
    use super::*;

    /// Trimmed from a real `/right-rail` response.
    const SAMPLE: &str = r#"{
        "linescore": {
            "byPeriod": [
                {"periodDescriptor": {"number": 1, "periodType": "REG"}, "away": 0, "home": 0},
                {"periodDescriptor": {"number": 4, "periodType": "OT"}, "away": 0, "home": 1}
            ],
            "totals": {"away": 1, "home": 2}
        },
        "shotsByPeriod": [
            {"periodDescriptor": {"number": 1, "periodType": "REG"}, "away": 5, "home": 3},
            {"periodDescriptor": {"number": 4, "periodType": "OT"}, "away": 0, "home": 1}
        ],
        "teamGameStats": [
            {"category": "sog", "awayValue": 16, "homeValue": 23},
            {"category": "faceoffWins", "awayValue": "22/49", "homeValue": "27/49"},
            {"category": "powerPlayPctg", "awayValue": 0.0, "homeValue": 0.0}
        ],
        "gameInfo": {"referees": []},
        "seasonSeries": []
    }"#;

    #[test]
    fn right_rail_parses() {
        let stats: GameStats = serde_json::from_str(SAMPLE).expect("right-rail should parse");
        assert_eq!(stats.shots_by_period.len(), 2);
        assert_eq!(stats.shots_by_period[1].home, 1);
        assert_eq!(stats.linescore.as_ref().map(|l| l.by_period.len()), Some(2));
        assert_eq!(stats.team_game_stats.len(), 3);
    }

    #[test]
    fn stat_values_render_counts_strings_and_rates() {
        let stats: GameStats = serde_json::from_str(SAMPLE).unwrap();
        let by = |name: &str| {
            let s = stats
                .team_game_stats
                .iter()
                .find(|s| s.category == name)
                .unwrap();
            stat_value(&s.away_value)
        };
        assert_eq!(by("sog"), "16");
        assert_eq!(by("faceoffWins"), "22/49");
        // Percentage categories are not rendered, but must not panic.
        assert_eq!(by("powerPlayPctg"), "0");
    }
}
