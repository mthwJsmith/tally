//! Deal price sources for the watchlist: free RSS feeds (HotUKDeals / CamelCamelCamel) and an
//! optional self-hosted changedetection.io instance, polled via its REST API. tally does no HTML
//! scraping itself — changedetection.io (paired with a real browser) handles arbitrary product
//! URLs and bot detection.

use anyhow::{anyhow, Result};
use regex::Regex;
use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct DealItem {
    pub title: String,
    pub url: Option<String>,
    pub guid: String,
    pub price_cents: Option<i64>,
}

#[derive(Clone)]
pub struct DealsClient {
    http: Client,
    pub cd_url: Option<String>,
    pub cd_api_key: Option<String>,
}

impl DealsClient {
    pub fn from_env() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("tally/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client build");
        Self {
            http,
            cd_url: std::env::var("TALLY_CHANGEDETECTION_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            cd_api_key: std::env::var("TALLY_CHANGEDETECTION_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }

    /// Fetch and parse an RSS feed into deal items, extracting a price from title/description.
    pub async fn fetch_rss(&self, url: &str) -> Result<Vec<DealItem>> {
        let bytes = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let channel = rss::Channel::read_from(&bytes[..])?;
        Ok(channel
            .items()
            .iter()
            .map(|item| {
                let title = item.title().unwrap_or("(untitled)").to_string();
                let desc = item.description().unwrap_or("");
                let url = item.link().map(|s| s.to_string());
                let guid = item
                    .guid()
                    .map(|g| g.value().to_string())
                    .or_else(|| url.clone())
                    .unwrap_or_else(|| title.clone());
                let price_cents = extract_price_cents(&format!("{title} {desc}"));
                DealItem {
                    title,
                    url,
                    guid,
                    price_cents,
                }
            })
            .collect())
    }

    /// Pull the latest price for a changedetection.io watch via its REST API. Returns `Ok(None)`
    /// when changedetection isn't configured.
    pub async fn fetch_changedetection(&self, uuid: &str) -> Result<Option<DealItem>> {
        let (Some(base), Some(key)) = (&self.cd_url, &self.cd_api_key) else {
            return Ok(None);
        };
        let url = format!("{}/api/v1/watch/{}", base.trim_end_matches('/'), uuid);
        let resp = self.http.get(&url).header("x-api-key", key).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("changedetection {uuid} -> {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await?;
        let page_url = v.get("url").and_then(|x| x.as_str()).map(|s| s.to_string());
        let title = v
            .get("title")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| page_url.clone())
            .unwrap_or_else(|| format!("watch {uuid}"));
        let last_changed = v.get("last_changed").and_then(|x| x.as_i64()).unwrap_or(0);
        // cd.io exposes a price for price/restock watches under a couple of keys by version.
        let price = v
            .get("price")
            .and_then(json_num)
            .or_else(|| v.pointer("/restock/price").and_then(json_num));
        Ok(Some(DealItem {
            title,
            url: page_url,
            guid: format!("{uuid}:{last_changed}"),
            price_cents: price.map(|p| (p * 100.0).round() as i64),
        }))
    }
}

fn json_num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Pull the first `£NN.NN` out of free text and return it in pence.
pub fn extract_price_cents(text: &str) -> Option<i64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"£\s?(\d+(?:[.,]\d{1,2})?)").unwrap());
    let raw = re.captures(text)?.get(1)?.as_str().replace(',', ".");
    let pounds: f64 = raw.parse().ok()?;
    Some((pounds * 100.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_extraction() {
        assert_eq!(extract_price_cents("Festool TS 55 now £499.99 at FFX"), Some(49999));
        assert_eq!(extract_price_cents("£1200 off today"), Some(120000));
        assert_eq!(extract_price_cents("no price here"), None);
    }

    #[test]
    fn rss_parse_and_price() {
        let xml = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>t</title>
          <item><title>Widget £19.99</title><link>http://x/1</link><guid>g1</guid>
            <description>great deal</description></item>
          <item><title>Gadget</title><link>http://x/2</link><guid>g2</guid></item>
        </channel></rss>"#;
        let ch = rss::Channel::read_from(xml.as_bytes()).unwrap();
        assert_eq!(ch.items().len(), 2);
        assert_eq!(extract_price_cents(ch.items()[0].title().unwrap()), Some(1999));
    }
}
