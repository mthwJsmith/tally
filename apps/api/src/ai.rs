//! OpenRouter-backed transaction categorisation.
//!
//! Reads `openrouter_api_key` + `openrouter_model` from the `settings` table.
//! API key is encrypted at rest using the same Crypto helper as OAuth tokens
//! (we store ciphertext in `settings.value`, prefixed with `enc:<nonce_b64>:<ct_b64>`).

use crate::db::Db;
use crate::models::{Category, Transaction};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "meta-llama/llama-3.1-8b-instruct:free";

#[derive(Clone)]
pub struct AiClient {
    db: Db,
    http: Client,
}

impl AiClient {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            http: Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("tally/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client"),
        }
    }

    /// Encrypt + store the user's OpenRouter API key.
    pub async fn set_api_key(&self, key: &str) -> Result<()> {
        let (nonce, ct) = self.db.crypto.encrypt(key)?;
        let stored = format!("enc:{}:{}", B64.encode(nonce), B64.encode(ct));
        self.db.set_setting("openrouter_api_key", &stored).await
    }

    pub async fn clear_api_key(&self) -> Result<()> {
        self.db.set_setting("openrouter_api_key", "").await
    }

    pub async fn get_api_key(&self) -> Result<Option<String>> {
        let Some(raw) = self.db.get_setting("openrouter_api_key").await? else {
            return Ok(None);
        };
        if raw.is_empty() {
            return Ok(None);
        }
        let parts: Vec<&str> = raw.splitn(3, ':').collect();
        if parts.len() != 3 || parts[0] != "enc" {
            // Treat as plain (legacy).
            return Ok(Some(raw));
        }
        let nonce = B64.decode(parts[1]).context("decode nonce")?;
        let ct = B64.decode(parts[2]).context("decode ciphertext")?;
        Ok(Some(self.db.crypto.decrypt(&nonce, &ct)?))
    }

    pub async fn get_model(&self) -> Result<String> {
        Ok(self
            .db
            .get_setting("openrouter_model")
            .await?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()))
    }

    pub async fn set_model(&self, model: &str) -> Result<()> {
        self.db.set_setting("openrouter_model", model).await
    }

    pub async fn is_configured(&self) -> bool {
        self.get_api_key().await.ok().flatten().is_some()
    }

    /// Categorise up to `limit` currently-uncategorised transactions in place.
    /// No-op (returns 0) if the AI isn't configured. Returns how many were applied.
    pub async fn categorise_uncategorised(&self, limit: i64) -> Result<usize> {
        if !self.is_configured().await {
            return Ok(0);
        }
        let categories = self.db.list_categories().await?;
        let mut txns = self
            .db
            .list_transactions(None, None, None, None, None, None, None, None, limit * 4, 0)
            .await?;
        txns.retain(|t| t.category_id.is_none());
        txns.truncate(limit.max(0) as usize);
        let mut applied = 0;
        for t in &txns {
            if let Ok(Some(cat_id)) = self.suggest_category(t, &categories).await {
                if self.db.update_transaction_category(t.id, Some(cat_id)).await.is_ok() {
                    applied += 1;
                }
            }
        }
        Ok(applied)
    }

    /// Ask the model to pick the best category for a transaction.
    /// Returns the chosen `category_id` (one from the supplied list) or `None` if
    /// no good match.
    pub async fn suggest_category(
        &self,
        txn: &Transaction,
        categories: &[Category],
    ) -> Result<Option<i64>> {
        let Some(api_key) = self.get_api_key().await? else {
            return Err(anyhow!("OpenRouter API key not configured"));
        };
        let model = self.get_model().await?;

        let category_list: String = categories
            .iter()
            .map(|c| format!("{} = {}", c.id, c.name))
            .collect::<Vec<_>>()
            .join(", ");

        let amount = txn.amount_cents as f64 / 100.0;
        let direction = if txn.is_credit == 1 { "in" } else { "out" };
        let mut detail = String::new();
        if let Some(ref m) = txn.merchant_name {
            detail.push_str(&format!("merchant: {}; ", m));
        }
        if let Some(ref c) = txn.counterparty_name {
            detail.push_str(&format!("counterparty: {}; ", c));
        }

        let user_msg = format!(
            "Bank transaction to categorise.\n\nDescription: {}\n{}Amount: £{:.2} {}\n\nAvailable categories (id = name):\n{}\n\nPick the single best category id. If none fits, return null. Briefly explain why in `reason`.",
            txn.description, detail, amount.abs(), direction, category_list
        );

        // Build the allowed-id list as integers + null. The JSON Schema below pins this.
        let mut enum_values: Vec<serde_json::Value> = categories
            .iter()
            .map(|c| serde_json::json!(c.id))
            .collect();
        enum_values.push(serde_json::Value::Null);

        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["category_id", "reason"],
            "properties": {
                "category_id": {
                    "description": "The chosen category id, or null if no good match.",
                    "type": ["integer", "null"],
                    "enum": enum_values
                },
                "reason": {
                    "description": "Short explanation (1 sentence).",
                    "type": "string",
                    "maxLength": 200
                }
            }
        });

        let body = OpenRouterRequest {
            model,
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content:
                        "You are a precise UK personal-finance assistant. You categorise bank \
                         transactions by selecting the best-matching category id from the user's \
                         own list. Stay strictly within the provided ids; do not invent names. \
                         Reply only with JSON matching the supplied schema."
                            .into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user_msg,
                },
            ],
            temperature: 0.0,
            max_tokens: 120,
            response_format: Some(ResponseFormat {
                kind: "json_schema".into(),
                json_schema: Some(JsonSchemaWrapper {
                    name: "transaction_category".into(),
                    strict: true,
                    schema,
                }),
            }),
        };

        let resp = self
            .http
            .post(OPENROUTER_URL)
            .bearer_auth(api_key)
            .header("HTTP-Referer", "https://tally.local")
            .header("X-Title", "tally")
            .json(&body)
            .send()
            .await
            .context("post to OpenRouter")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenRouter {status}: {text}"));
        }

        let parsed: OpenRouterResponse = resp.json().await.context("decode OpenRouter response")?;
        let content = parsed
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        // Strict JSON schema response. Parse and validate.
        // Some models still wrap with markdown fences — strip them.
        let cleaned = strip_code_fence(&content);
        let parsed_json: serde_json::Value = match serde_json::from_str(cleaned) {
            Ok(v) => v,
            Err(_) => {
                // Soft fallback: try the first integer in the raw text (for models that ignored
                // the response_format despite our request).
                let digits: String = content
                    .chars()
                    .skip_while(|c| !c.is_ascii_digit())
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                let id: i64 = digits.parse().unwrap_or(0);
                if id > 0 && categories.iter().any(|c| c.id == id) {
                    return Ok(Some(id));
                }
                return Ok(None);
            }
        };

        let id_opt = parsed_json
            .get("category_id")
            .and_then(|v| if v.is_null() { None } else { v.as_i64() });
        let Some(id) = id_opt else { return Ok(None) };
        if !categories.iter().any(|c| c.id == id) {
            return Ok(None);
        }
        Ok(Some(id))
    }
}

fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        return rest.trim_end_matches("```").trim();
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        return rest.trim_end_matches("```").trim();
    }
    trimmed
}

#[derive(Debug, Serialize)]
struct OpenRouterRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f64,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_schema: Option<JsonSchemaWrapper>,
}

#[derive(Debug, Serialize)]
struct JsonSchemaWrapper {
    name: String,
    strict: bool,
    schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OpenRouterResponse {
    choices: Vec<OpenRouterChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterChoice {
    message: OpenRouterChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct OpenRouterChoiceMessage {
    content: String,
}
