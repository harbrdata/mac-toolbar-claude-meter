use std::time::{SystemTime, UNIX_EPOCH};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_URL: &str = "https://api.anthropic.com/api/oauth/token";
const USER_AGENT: &str = "claude-o-meter/1.0";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";

pub enum FetchResult {
    Ok(serde_json::Value),
    RateLimited(u64), // retry-after seconds
    AuthError,
    Error(String),
}

/// Token with expiry info for caching.
pub struct TokenResult {
    pub access_token: String,
    /// How many seconds until this token expires (None = unknown).
    pub expires_in_secs: Option<u64>,
}

/// Extract access token from credentials, refreshing if expired.
pub fn get_access_token(creds: &serde_json::Value) -> Option<TokenResult> {
    let expires_at = creds.get("expiresAt").and_then(|v| v.as_u64()).unwrap_or(0);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    if expires_at > 0 && now_ms < expires_at {
        let token = creds
            .get("accessToken")
            .and_then(|v| v.as_str())
            .map(String::from)?;
        let remaining_secs = (expires_at - now_ms) / 1000;
        return Some(TokenResult {
            access_token: token,
            expires_in_secs: Some(remaining_secs),
        });
    }

    // Try refresh
    if let Some(refresh_token) = creds.get("refreshToken").and_then(|v| v.as_str())
        && let Some(tr) = refresh_access_token(refresh_token)
    {
        return Some(tr);
    }

    let token = creds
        .get("accessToken")
        .and_then(|v| v.as_str())
        .map(String::from)?;
    Some(TokenResult {
        access_token: token,
        expires_in_secs: None,
    })
}

fn refresh_access_token(refresh_token: &str) -> Option<TokenResult> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });

    let mut resp = ureq::post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.body_mut().read_json().ok()?;
    let token = json
        .get("access_token")
        .or_else(|| json.get("accessToken"))
        .and_then(|v| v.as_str())
        .map(String::from)?;
    let expires_in = json.get("expires_in").and_then(|v| v.as_u64());
    Some(TokenResult {
        access_token: token,
        expires_in_secs: expires_in,
    })
}

/// Fetch usage data from the Anthropic API.
pub fn fetch_usage(access_token: &str) -> FetchResult {
    let result = ureq::get(USAGE_URL)
        .header("Authorization", &format!("Bearer {access_token}"))
        .header("User-Agent", USER_AGENT)
        .header("anthropic-beta", ANTHROPIC_BETA)
        .call();

    match result {
        Ok(mut resp) => match resp.body_mut().read_json::<serde_json::Value>() {
            Ok(json) => FetchResult::Ok(json),
            Err(e) => FetchResult::Error(format!("JSON parse error: {e}")),
        },
        Err(ureq::Error::StatusCode(401)) => {
            eprintln!("API returned 401 — re-login with `claude login`");
            FetchResult::AuthError
        }
        Err(ureq::Error::StatusCode(429)) => {
            // ureq doesn't expose headers on error responses easily, use default
            eprintln!("API rate-limited (429)");
            FetchResult::RateLimited(0)
        }
        Err(e) => FetchResult::Error(format!("Request failed: {e}")),
    }
}

#[derive(Debug, Clone)]
pub struct UsageWindow {
    pub label: &'static str,
    pub utilization: f64,
    pub resets_at: Option<String>,
}

const USAGE_WINDOWS: &[(&str, &str)] = &[
    ("five_hour", "5h"),
    ("seven_day", "7d"),
    ("seven_day_opus", "Opus"),
    ("seven_day_sonnet", "Sonnet"),
    ("seven_day_cowork", "Cowork"),
    ("seven_day_oauth_apps", "OAuth"),
];

pub fn parse_usage(data: &serde_json::Value) -> Vec<UsageWindow> {
    USAGE_WINDOWS
        .iter()
        .filter_map(|(key, label)| {
            let window = data.get(*key)?;
            let raw_util = window.get("utilization")?.as_f64().unwrap_or(0.0);
            let resets_at = window
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(String::from);
            Some(UsageWindow {
                label,
                utilization: raw_util / 100.0,
                resets_at,
            })
        })
        .collect()
}

/// Format reset time as human-readable countdown.
pub fn format_reset_time(resets_at: Option<&str>) -> String {
    let Some(s) = resets_at else {
        return "unknown".into();
    };

    let Ok(reset_dt) = chrono::DateTime::parse_from_rfc3339(s) else {
        // Try with Z suffix
        let fixed = s.replace("Z", "+00:00");
        let Ok(reset_dt) = chrono::DateTime::parse_from_rfc3339(&fixed) else {
            return "unknown".into();
        };
        return format_delta(reset_dt);
    };
    format_delta(reset_dt)
}

fn format_delta(reset_dt: chrono::DateTime<chrono::FixedOffset>) -> String {
    let now = chrono::Utc::now();
    let delta = reset_dt.signed_duration_since(now);
    let total_seconds = delta.num_seconds();
    if total_seconds <= 0 {
        return "now".into();
    }
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}
