use std::time::{SystemTime, UNIX_EPOCH};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
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
        .unwrap_or_default()
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
        "client_id": CLIENT_ID,
    });

    let mut resp = match ureq::post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .send_json(&body)
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Token refresh request failed: {e}");
            return None;
        }
    };

    let json: serde_json::Value = match resp.body_mut().read_json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Token refresh JSON parse error: {e}");
            return None;
        }
    };
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

/// Keys carrying a `utilization` field that are not usage windows, so `parse_usage`
/// doesn't log them as unknown.
const NON_WINDOW_KEYS: &[&str] = &["extra_usage"];

pub fn parse_usage(data: &serde_json::Value) -> Vec<UsageWindow> {
    // Warn about unknown top-level keys that look like usage windows
    if let Some(obj) = data.as_object() {
        let known_keys: std::collections::HashSet<&str> = USAGE_WINDOWS
            .iter()
            .map(|(k, _)| *k)
            .chain(NON_WINDOW_KEYS.iter().copied())
            .collect();
        for key in obj.keys() {
            if !known_keys.contains(key.as_str()) && obj[key].get("utilization").is_some() {
                eprintln!("Unknown usage window in API response: {key}");
            }
        }
    }

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

/// Usage credits ("extra usage") — the pay-as-you-go spend that covers you once a plan
/// limit is hit. Amounts are in minor currency units (pence, cents).
#[derive(Debug, Clone, PartialEq)]
pub struct Credits {
    pub enabled: bool,
    /// Fraction of the monthly cap spent, 0.0–1.0.
    pub utilization: f64,
    pub used_minor: i64,
    pub limit_minor: i64,
    pub currency: String,
    pub exponent: u32,
}

impl Credits {
    pub fn remaining_minor(&self) -> i64 {
        (self.limit_minor - self.used_minor).max(0)
    }
}

/// Parse the usage-credits balance. The API currently returns this twice — `extra_usage`
/// and the newer `spend` — so fall back to whichever is present.
pub fn parse_credits(data: &serde_json::Value) -> Option<Credits> {
    parse_credits_extra_usage(data).or_else(|| parse_credits_spend(data))
}

fn parse_credits_extra_usage(data: &serde_json::Value) -> Option<Credits> {
    let e = data.get("extra_usage")?;
    let used_minor = e.get("used_credits")?.as_f64()? as i64;
    let limit_minor = e.get("monthly_limit")?.as_i64()?;
    Some(Credits {
        enabled: e
            .get("is_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        utilization: e
            .get("utilization")
            .and_then(|v| v.as_f64())
            .map(|u| u / 100.0)
            .unwrap_or_else(|| ratio(used_minor, limit_minor)),
        used_minor,
        limit_minor,
        currency: e
            .get("currency")
            .and_then(|v| v.as_str())
            .unwrap_or("USD")
            .to_string(),
        exponent: e
            .get("decimal_places")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as u32,
    })
}

fn parse_credits_spend(data: &serde_json::Value) -> Option<Credits> {
    let s = data.get("spend")?;
    let used = s.get("used")?;
    let limit = s.get("limit")?;
    let used_minor = used.get("amount_minor")?.as_i64()?;
    let limit_minor = limit.get("amount_minor")?.as_i64()?;
    Some(Credits {
        enabled: s.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        utilization: ratio(used_minor, limit_minor),
        used_minor,
        limit_minor,
        currency: used
            .get("currency")
            .and_then(|v| v.as_str())
            .unwrap_or("USD")
            .to_string(),
        exponent: used.get("exponent").and_then(|v| v.as_u64()).unwrap_or(2) as u32,
    })
}

fn ratio(used: i64, limit: i64) -> f64 {
    if limit > 0 {
        used as f64 / limit as f64
    } else {
        0.0
    }
}

/// Format a minor-unit amount with its currency symbol, e.g. `10117` GBP -> `£101.17`.
pub fn format_money(minor: i64, currency: &str, exponent: u32) -> String {
    let symbol = match currency {
        "GBP" => "£",
        "USD" => "$",
        "EUR" => "€",
        "JPY" => "¥",
        other => {
            return format!(
                "{} {:.*}",
                other,
                exponent as usize,
                scaled(minor, exponent)
            );
        }
    };
    format!(
        "{}{:.*}",
        symbol,
        exponent as usize,
        scaled(minor, exponent)
    )
}

fn scaled(minor: i64, exponent: u32) -> f64 {
    minor as f64 / 10f64.powi(exponent as i32)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_usage_all_windows() {
        let data = json!({
            "five_hour": { "utilization": 74.0, "resets_at": "2026-03-24T18:00:00Z" },
            "seven_day": { "utilization": 50.0, "resets_at": "2026-03-31T00:00:00Z" },
        });
        let windows = parse_usage(&data);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5h");
        assert!((windows[0].utilization - 0.74).abs() < 1e-9);
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-03-24T18:00:00Z")
        );
        assert_eq!(windows[1].label, "7d");
        assert!((windows[1].utilization - 0.50).abs() < 1e-9);
    }

    #[test]
    fn test_parse_usage_missing_windows() {
        let data = json!({});
        let windows = parse_usage(&data);
        assert!(windows.is_empty());
    }

    #[test]
    fn test_parse_usage_missing_utilization() {
        let data = json!({
            "five_hour": { "resets_at": "2026-03-24T18:00:00Z" },
        });
        let windows = parse_usage(&data);
        assert!(windows.is_empty());
    }

    #[test]
    fn test_parse_usage_no_resets_at() {
        let data = json!({
            "five_hour": { "utilization": 30.0 },
        });
        let windows = parse_usage(&data);
        assert_eq!(windows.len(), 1);
        assert!(windows[0].resets_at.is_none());
    }

    #[test]
    fn test_parse_credits_from_extra_usage() {
        let data = json!({
            "extra_usage": {
                "is_enabled": true,
                "monthly_limit": 30000,
                "used_credits": 10117.0,
                "utilization": 33.723333333333336,
                "currency": "GBP",
                "decimal_places": 2,
            },
        });
        let c = parse_credits(&data).unwrap();
        assert!(c.enabled);
        assert_eq!(c.used_minor, 10117);
        assert_eq!(c.limit_minor, 30000);
        assert_eq!(c.remaining_minor(), 19883);
        assert_eq!(c.currency, "GBP");
        assert!((c.utilization - 0.3372333333).abs() < 1e-6);
    }

    #[test]
    fn test_parse_credits_falls_back_to_spend() {
        let data = json!({
            "spend": {
                "used": { "amount_minor": 500, "currency": "USD", "exponent": 2 },
                "limit": { "amount_minor": 2000, "currency": "USD", "exponent": 2 },
                "enabled": true,
            },
        });
        let c = parse_credits(&data).unwrap();
        assert_eq!(c.used_minor, 500);
        assert!((c.utilization - 0.25).abs() < 1e-9);
        assert_eq!(c.currency, "USD");
    }

    #[test]
    fn test_parse_credits_absent() {
        assert!(parse_credits(&json!({})).is_none());
    }

    #[test]
    fn test_parse_credits_zero_limit_is_not_nan() {
        let data = json!({
            "spend": {
                "used": { "amount_minor": 0, "currency": "USD", "exponent": 2 },
                "limit": { "amount_minor": 0, "currency": "USD", "exponent": 2 },
            },
        });
        let c = parse_credits(&data).unwrap();
        assert_eq!(c.utilization, 0.0);
        assert_eq!(c.remaining_minor(), 0);
    }

    #[test]
    fn test_parse_usage_ignores_extra_usage_key() {
        let data = json!({
            "five_hour": { "utilization": 10.0 },
            "extra_usage": { "utilization": 50.0 },
        });
        let windows = parse_usage(&data);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "5h");
    }

    #[test]
    fn test_format_money() {
        assert_eq!(format_money(10117, "GBP", 2), "£101.17");
        assert_eq!(format_money(500, "USD", 2), "$5.00");
        assert_eq!(format_money(1234, "SEK", 2), "SEK 12.34");
        assert_eq!(format_money(1234, "JPY", 0), "¥1234");
    }

    #[test]
    fn test_format_reset_time_none() {
        assert_eq!(format_reset_time(None), "unknown");
    }

    #[test]
    fn test_format_reset_time_invalid() {
        assert_eq!(format_reset_time(Some("not-a-date")), "unknown");
    }

    #[test]
    fn test_format_reset_time_past() {
        assert_eq!(format_reset_time(Some("2020-01-01T00:00:00+00:00")), "now");
    }

    #[test]
    fn test_format_reset_time_future() {
        let future =
            chrono::Utc::now() + chrono::Duration::hours(2) + chrono::Duration::minutes(30);
        let s = future.to_rfc3339();
        let result = format_reset_time(Some(&s));
        assert!(result.contains("h"), "Expected hours in: {result}");
        assert!(result.contains("m"), "Expected minutes in: {result}");
    }

    #[test]
    fn test_format_reset_time_z_suffix() {
        // Z suffix should be handled
        assert_eq!(format_reset_time(Some("2020-01-01T00:00:00Z")), "now");
    }

    #[test]
    fn test_format_reset_time_minutes_only() {
        let future = chrono::Utc::now() + chrono::Duration::minutes(15);
        let s = future.to_rfc3339();
        let result = format_reset_time(Some(&s));
        assert!(!result.contains("h"), "Expected no hours in: {result}");
        assert!(result.contains("m"), "Expected minutes in: {result}");
    }

    #[test]
    fn test_get_access_token_valid_not_expired() {
        let future_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 3_600_000; // 1 hour from now
        let creds = json!({
            "accessToken": "test-token-123",
            "expiresAt": future_ms,
        });
        let result = get_access_token(&creds).unwrap();
        assert_eq!(result.access_token, "test-token-123");
        assert!(result.expires_in_secs.unwrap() > 3500);
    }

    #[test]
    fn test_get_access_token_expired_no_refresh() {
        let creds = json!({
            "accessToken": "fallback-token",
            "expiresAt": 1000,
        });
        // No refreshToken, so falls through to using accessToken directly
        let result = get_access_token(&creds).unwrap();
        assert_eq!(result.access_token, "fallback-token");
        assert!(result.expires_in_secs.is_none());
    }

    #[test]
    fn test_get_access_token_missing_token() {
        let creds = json!({
            "expiresAt": 1000,
        });
        assert!(get_access_token(&creds).is_none());
    }

    #[test]
    fn test_get_access_token_no_expiry() {
        let creds = json!({
            "accessToken": "no-expiry-token",
        });
        // expiresAt is 0 (missing), so falls through to direct use
        let result = get_access_token(&creds).unwrap();
        assert_eq!(result.access_token, "no-expiry-token");
        assert!(result.expires_in_secs.is_none());
    }
}
