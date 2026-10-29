//! Official OpenAI reporting APIs. Admin credentials are used only for these
//! fixed HTTPS endpoints and are never persisted or included in error messages.
use chrono::{Datelike, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiApiUsage {
    pub start_time: i64,
    pub end_time: i64,
    pub usage: Option<UsageTotals>,
    pub cost_usd: Option<f64>,
    pub usage_error: Option<String>,
    pub cost_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Page {
    data: Vec<Bucket>,
    has_more: bool,
    next_page: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Bucket {
    results: Vec<serde_json::Value>,
}

fn time_range(range: &str, now: chrono::DateTime<Utc>) -> Result<(i64, i64), String> {
    let today = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let start = match range {
        "today" => today,
        "month" => Utc
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .unwrap(),
        "30d" => today - chrono::Duration::days(29),
        _ => return Err("OPENAI_USAGE_INVALID_RANGE".into()),
    };
    // end_time is exclusive. Keep even a query at exactly midnight nonempty.
    Ok((start.timestamp(), now.timestamp().max(start.timestamp() + 1)))
}

fn normalized_filter(value: Option<&str>) -> Result<Option<&str>, String> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_some_and(|value| value.starts_with("sk-")) {
        // API reports filter by a key's ID, never its secret value.
        return Err("OPENAI_USAGE_KEY_ID_REQUIRED".into());
    }
    Ok(value)
}

fn reporting_url(
    endpoint: &str,
    start: i64,
    end: i64,
    api_key_id: Option<&str>,
    project_id: Option<&str>,
) -> reqwest::Url {
    let mut url = reqwest::Url::parse(&format!(
        "https://api.openai.com/v1/organization/{endpoint}"
    ))
    .unwrap();
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("start_time", &start.to_string());
        query.append_pair("end_time", &end.to_string());
        query.append_pair("bucket_width", "1d");
        query.append_pair("limit", "31");
        if let Some(id) = api_key_id {
            query.append_pair("api_key_ids[]", id);
        }
        if let Some(id) = project_id {
            query.append_pair("project_ids[]", id);
        }
    }
    url
}

fn http_error(status: u16) -> String {
    match status {
        401 | 403 => "OPENAI_USAGE_ADMIN_KEY_REQUIRED".into(),
        429 => "OPENAI_USAGE_RATE_LIMITED".into(),
        _ => format!("OPENAI_USAGE_HTTP_{status}"),
    }
}

fn next_cursor(page: &Page, seen: &mut HashSet<String>) -> Result<Option<String>, String> {
    if !page.has_more {
        return Ok(None);
    }
    let cursor = page
        .next_page
        .as_deref()
        .filter(|cursor| !cursor.trim().is_empty())
        .ok_or("OPENAI_USAGE_INVALID_PAGE")?;
    if !seen.insert(cursor.to_string()) {
        return Err("OPENAI_USAGE_INVALID_PAGE".into());
    }
    Ok(Some(cursor.to_string()))
}

async fn fetch_results(
    client: &reqwest::Client,
    url: reqwest::Url,
    key: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let mut results = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    for _ in 0..32 {
        let mut page_url = url.clone();
        if let Some(cursor) = &cursor {
            page_url.query_pairs_mut().append_pair("page", cursor);
        }
        let response = client
            .get(page_url)
            .bearer_auth(key)
            .send()
            .await
            .map_err(|_| "OPENAI_USAGE_NETWORK_FAILED".to_string())?;
        if !response.status().is_success() {
            return Err(http_error(response.status().as_u16()));
        }
        let page: Page = response
            .json()
            .await
            .map_err(|_| "OPENAI_USAGE_INVALID_RESPONSE".to_string())?;
        cursor = next_cursor(&page, &mut seen)?;
        results.extend(page.data.into_iter().flat_map(|bucket| bucket.results));
        if cursor.is_none() {
            return Ok(results);
        }
    }
    Err("OPENAI_USAGE_TOO_MANY_PAGES".into())
}

fn summarize_usage(results: &[serde_json::Value]) -> Result<UsageTotals, String> {
    let mut totals = UsageTotals::default();
    for result in results {
        for (field, total) in [
            ("num_model_requests", &mut totals.requests),
            ("input_tokens", &mut totals.input_tokens),
            ("output_tokens", &mut totals.output_tokens),
            ("input_cached_tokens", &mut totals.cached_input_tokens),
        ] {
            let amount = match result.get(field) {
                None | Some(serde_json::Value::Null) if field == "input_cached_tokens" => 0,
                Some(value) => value.as_u64().ok_or("OPENAI_USAGE_INVALID_RESPONSE")?,
                None => return Err("OPENAI_USAGE_INVALID_RESPONSE".into()),
            };
            *total = total
                .checked_add(amount)
                .ok_or("OPENAI_USAGE_INVALID_RESPONSE")?;
        }
    }
    Ok(totals)
}

fn summarize_cost(results: &[serde_json::Value]) -> Result<f64, String> {
    let mut total = 0.0;
    for result in results {
        let amount = &result["amount"];
        if !amount["currency"]
            .as_str()
            .is_some_and(|unit| unit.eq_ignore_ascii_case("usd"))
        {
            return Err("OPENAI_USAGE_UNSUPPORTED_CURRENCY".into());
        }
        total += amount["value"].as_f64().ok_or("OPENAI_USAGE_INVALID_RESPONSE")?;
        if !total.is_finite() {
            return Err("OPENAI_USAGE_INVALID_RESPONSE".into());
        }
    }
    Ok(total)
}

pub async fn query_usage(
    admin_key: &str,
    api_key_id: Option<&str>,
    project_id: Option<&str>,
    range: &str,
) -> Result<OpenAiApiUsage, String> {
    let key = admin_key.trim();
    if key.is_empty() {
        return Err("OPENAI_USAGE_ADMIN_KEY_REQUIRED".into());
    }
    let api_key_id = normalized_filter(api_key_id)?;
    let project_id = normalized_filter(project_id)?;
    let (start_time, end_time) = time_range(range, Utc::now())?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "OPENAI_USAGE_NETWORK_FAILED".to_string())?;
    let usage_url = reporting_url(
        "usage/completions", start_time, end_time, api_key_id, project_id,
    );
    let costs_url = reporting_url("costs", start_time, end_time, api_key_id, project_id);
    let (usage, cost) = tokio::join!(
        fetch_results(&client, usage_url, key),
        fetch_results(&client, costs_url, key),
    );
    let usage = usage.and_then(|results| summarize_usage(&results));
    let cost = cost.and_then(|results| summarize_cost(&results));
    // A reporting permission or endpoint failure must not hide the other report.
    Ok(OpenAiApiUsage {
        start_time,
        end_time,
        usage_error: usage.as_ref().err().cloned(),
        cost_error: cost.as_ref().err().cloned(),
        usage: usage.ok(),
        cost_usd: cost.ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ranges_use_utc_calendar_boundaries() {
        let now = Utc.with_ymd_and_hms(2026, 3, 1, 9, 30, 0).unwrap();
        assert_eq!(time_range("month", now).unwrap().0, Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap().timestamp());
        assert_eq!(time_range("30d", now).unwrap().0, Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap().timestamp());
        assert!(time_range("invalid", now).is_err());
    }

    #[test]
    fn filters_and_pagination_preserve_scope_without_secrets() {
        let url = reporting_url("costs", 10, 20, Some("key_123"), Some("proj_123"));
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(url.host_str(), Some("api.openai.com"));
        assert_eq!(query.get("api_key_ids[]").unwrap(), "key_123");
        assert_eq!(query.get("project_ids[]").unwrap(), "proj_123");
        assert_eq!(query.get("end_time").unwrap(), "20");
        assert!(normalized_filter(Some(" sk-proj-secret ")).is_err());
        assert_eq!(normalized_filter(Some("  ")).unwrap(), None);
        let mut seen = HashSet::new();
        let page: Page = serde_json::from_value(json!({"data": [], "has_more": true, "next_page": "p1"})).unwrap();
        assert_eq!(next_cursor(&page, &mut seen).unwrap(), Some("p1".into()));
        assert!(next_cursor(&page, &mut seen).is_err());
        let missing: Page = serde_json::from_value(json!({"data": [], "has_more": true})).unwrap();
        assert!(next_cursor(&missing, &mut seen).is_err());
    }

    #[test]
    fn sums_tokens_without_double_counting_cache() {
        let totals = summarize_usage(&[
            json!({"input_tokens": 1000, "input_cached_tokens": 400, "output_tokens": 50, "num_model_requests": 2}),
            json!({"input_tokens": 500, "output_tokens": 100, "num_model_requests": 1}),
        ]).unwrap();
        assert_eq!(totals, UsageTotals { requests: 3, input_tokens: 1500, output_tokens: 150, cached_input_tokens: 400 });
        assert_eq!(summarize_usage(&[]).unwrap(), UsageTotals::default());
        assert!(summarize_usage(&[json!({})]).is_err());
    }

    #[test]
    fn costs_preserve_fractional_dollars_and_reject_unknown_data() {
        let total = summarize_cost(&[
            json!({"amount": {"value": 0.06, "currency": "usd"}}),
            json!({"amount": {"value": 1.25, "currency": "usd"}}),
        ]).unwrap();
        assert!((total - 1.31).abs() < 1e-10);
        assert_eq!(summarize_cost(&[]).unwrap(), 0.0);
        assert!(summarize_cost(&[json!({"amount": {"value": 2, "currency": "eur"}})]).is_err());
        assert!(summarize_cost(&[json!({})]).is_err());
        assert_eq!(http_error(403), "OPENAI_USAGE_ADMIN_KEY_REQUIRED");
        assert_eq!(http_error(429), "OPENAI_USAGE_RATE_LIMITED");
    }

    #[tokio::test]
    async fn fetches_every_page_with_the_same_filters_and_authorization() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let worker = std::thread::spawn(move || {
            for index in 0..2 {
                let request = server.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
                assert!(request.url().contains("api_key_ids%5B%5D=key_test"));
                assert!(request.url().contains("project_ids%5B%5D=proj_test"));
                assert!(!request.url().contains("sk-admin-test"));
                assert_eq!(request.headers().iter().find(|header| header.field.equiv("Authorization")).unwrap().value.as_str(), "Bearer sk-admin-test");
                assert_eq!(request.url().contains("page=next"), index == 1);
                let body = json!({
                    "data": [{"results": [{"input_tokens": 100, "output_tokens": 25, "num_model_requests": 1}]}],
                    "has_more": index == 0,
                    "next_page": if index == 0 { Some("next") } else { None },
                });
                request.respond(tiny_http::Response::from_string(body.to_string()).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap()
                )).unwrap();
            }
        });
        let mut url = reporting_url("usage/completions", 10, 20, Some("key_test"), Some("proj_test"));
        url.set_scheme("http").unwrap();
        url.set_host(Some("127.0.0.1")).unwrap();
        url.set_port(Some(address.port())).unwrap();
        let results = fetch_results(&reqwest::Client::new(), url, "sk-admin-test").await.unwrap();
        worker.join().unwrap();
        let totals = summarize_usage(&results).unwrap();
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.input_tokens, 200);
    }

    #[tokio::test]
    async fn permission_errors_do_not_expose_the_response_body() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let worker = std::thread::spawn(move || {
            let request = server.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
            request.respond(tiny_http::Response::from_string("sk-admin-secret echoed by upstream")
                .with_status_code(403)).unwrap();
        });
        let url = reqwest::Url::parse(&format!("http://{address}/usage")).unwrap();
        let error = fetch_results(&reqwest::Client::new(), url, "sk-admin-test").await.unwrap_err();
        worker.join().unwrap();
        assert_eq!(error, "OPENAI_USAGE_ADMIN_KEY_REQUIRED");
        assert!(!error.contains("secret"));
    }
}
