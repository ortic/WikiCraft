//! The language model behind *Ask the wiki*: Claude through the Anthropic API, or any server with
//! an OpenAI-compatible chat API, also a model running in your own network (Ollama, vLLM,
//! llama.cpp's server, LM Studio, LocalAI). Set up with `WIKICRAFT_AI_PROVIDER`,
//! `WIKICRAFT_AI_BASE_URL`, `WIKICRAFT_AI_MODEL` and `WIKICRAFT_AI_API_KEY`; nothing is sent
//! anywhere until it is.

use std::time::Duration;

use openidconnect::reqwest;
use serde_json::{Value, json};

use crate::config::Config;
use crate::error::{ApiError, ApiResult};

const ANTHROPIC_VERSION: &str = "2026-10-21";
const ANTHROPIC_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_MODEL: &str = "claude-opus-5-5";
/// Claude models that take the server-side fallback when they decline a request.
const FALLBACK_MODELS: [&str; 6] = ["claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5", "claude-fable-5-2", "claude-fable-5-1", "claude-fable-5"];
/// tries per question while the service is busy or can't be reached
const ATTEMPTS: u64 = 3;
const REFUSED: &str = "I can't help with this question.";
const CUT_OFF: &str = "(The answer was cut off.)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Claude through the Anthropic Messages API
    Anthropic,
    /// `POST {base}/chat/completions`, as OpenAI-compatible servers take it
    OpenAiCompatible,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAiCompatible => "openai-compatible",
        }
    }
}

/// How the server reaches its model.
#[derive(Debug, Clone)]
pub struct Setup {
    pub provider: Provider,
    pub base_url: String,
    pub key: Option<String>,
    pub model: String,
}

/// The model the server is set up with (`Ok(None)`: none), or what is missing.
fn resolve(c: &Config) -> Result<Option<Setup>, String> {
    let set = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let (key, base, model) = (set(&c.ai_api_key), set(&c.ai_base_url), set(&c.ai_model));
    let provider = match set(&c.ai_provider).map(|p| p.to_ascii_lowercase()).as_deref() {
        Some("anthropic") => Provider::Anthropic,
        Some("openai-compatible" | "openai") => Provider::OpenAiCompatible,
        Some(other) => return Err(format!("WIKICRAFT_AI_PROVIDER is `anthropic` or `openai-compatible`, not `{other}`")),
        // a key alone (also with a gateway's address) has always meant Claude
        None if key.is_some() => Provider::Anthropic,
        // an address without a key: a model server, usually in your own network
        None if base.is_some() => Provider::OpenAiCompatible,
        None => return Ok(None),
    };
    match provider {
        Provider::Anthropic => {
            let Some(key) = key else {
                return Err("Claude needs an Anthropic API key (WIKICRAFT_AI_API_KEY)".into());
            };
            Ok(Some(Setup {
                provider,
                base_url: base.unwrap_or_else(|| ANTHROPIC_URL.into()),
                key: Some(key),
                model: model.unwrap_or_else(|| ANTHROPIC_MODEL.into()),
            }))
        }
        Provider::OpenAiCompatible => match (base, model) {
            (Some(base_url), Some(model)) => Ok(Some(Setup { provider, base_url, key, model })),
            _ => Err("an OpenAI-compatible AI server needs WIKICRAFT_AI_BASE_URL and WIKICRAFT_AI_MODEL".into()),
        },
    }
}

/// The model the server is set up with, if any.
pub fn setup(c: &Config) -> Option<Setup> {
    resolve(c).ok().flatten()
}

/// Says at start-up which model answers questions, or why none does.
pub fn log_setup(c: &Config) {
    match resolve(c) {
        Ok(Some(s)) => tracing::info!(provider = s.provider.name(), model = %s.model, local = local(&s.base_url), "Ask the wiki is set up"),
        Ok(None) => {}
        Err(e) => tracing::warn!("Ask the wiki is off: {e}"),
    }
}

/// Whether an address is in the server's own network (nothing leaves it then).
pub fn local(base: &str) -> bool {
    let Ok(u) = openidconnect::url::Url::parse(base) else { return false };
    let Some(host) = u.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => host == "localhost" || !host.contains('.') || [".local", ".internal", ".lan", ".home.arpa"].iter().any(|s| host.ends_with(s)),
    }
}

/// What administrators see about the setup (`null` when there is none).
pub fn setup_json(c: &Config) -> Value {
    match setup(c) {
        Some(s) => json!({ "provider": s.provider.name(), "model": s.model, "local": local(&s.base_url) }),
        None => Value::Null,
    }
}

/// A model's answer.
pub struct Answer {
    pub text: String,
    /// the model that wrote it (as the service reports it)
    pub model: String,
}

/// Asks the model one question (non-streaming), retrying while the service is busy. `effort`
/// tells Claude how hard to think; other models decide that themselves. A refusal comes back as
/// a polite non-answer, an answer that ran out of room with a note that it was cut off.
pub async fn answer(c: &Config, system: &str, prompt: &str, effort: &str) -> ApiResult<Answer> {
    let s = setup(c).ok_or_else(|| ApiError::bad("Ask the wiki isn't enabled on this server"))?;
    let base = s.base_url.trim_end_matches('/');
    let fallback = s.provider == Provider::Anthropic && FALLBACK_MODELS.contains(&s.model.as_str());
    let (url, body) = match s.provider {
        Provider::Anthropic => {
            let mut body = json!({
                "model": s.model,
                "max_tokens": 16000,
                "system": system,
                "messages": [{ "role": "user", "content": prompt }],
                "output_config": { "effort": effort },
            });
            if fallback {
                // a declined request is re-run on Anthropic's recommended fallback model
                body["fallbacks"] = json!("default");
            }
            (format!("{base}/v1/messages"), body)
        }
        // no max_tokens or temperature: servers and models disagree on both (some reject any
        // temperature but the default), and their defaults let the answer finish
        Provider::OpenAiCompatible => (
            format!("{base}/chat/completions"),
            json!({
                "model": s.model,
                "messages": [{ "role": "system", "content": system }, { "role": "user", "content": prompt }],
                "stream": false,
            }),
        ),
    };
    let body = serde_json::to_vec(&body).map_err(|e| ApiError::Internal(e.into()))?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(c.ai_timeout_secs.max(10))).build().map_err(|e| ApiError::Internal(e.into()))?;
    let mut attempt = 0;
    let resp: Value = loop {
        attempt += 1;
        let mut req = client.post(&url).header("content-type", "application/json").body(body.clone());
        req = match (s.provider, &s.key) {
            (Provider::Anthropic, key) => {
                let req = req.header("x-api-key", key.as_deref().unwrap_or_default()).header("anthropic-version", ANTHROPIC_VERSION);
                if fallback { req.header("anthropic-beta", "server-side-fallback-2026-07-01") } else { req }
            }
            (Provider::OpenAiCompatible, Some(key)) => req.bearer_auth(key),
            (Provider::OpenAiCompatible, None) => req,
        };
        let r = match req.send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                tracing::warn!("ask: no answer from the AI service within {}s", c.ai_timeout_secs.max(10));
                return Err(ApiError::bad("the AI took too long to answer; try again in a moment"));
            }
            Err(e) if attempt < ATTEMPTS => {
                tracing::warn!("ask: request failed ({e}), retrying");
                tokio::time::sleep(Duration::from_millis(800 * attempt)).await;
                continue;
            }
            Err(e) => return Err(ApiError::Internal(anyhow::anyhow!("AI service unreachable at {url}: {e}"))),
        };
        let status = r.status().as_u16();
        // 429 (rate limit), 5xx and 529 (overloaded) are worth retrying
        if (status == 429 || status >= 500) && attempt < ATTEMPTS {
            let wait = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(attempt).min(10);
            tracing::warn!(status, "ask: AI service busy, retrying in {wait}s");
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }
        let bytes = r.bytes().await.unwrap_or_default();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let msg = v["error"]["message"]
                .as_str()
                .or(v["error"].as_str())
                .or(v["detail"].as_str())
                .map(String::from)
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).chars().take(300).collect());
            let hint = match status {
                401 | 403 => " (is WIKICRAFT_AI_API_KEY right?)",
                404 => " (check WIKICRAFT_AI_MODEL and WIKICRAFT_AI_BASE_URL)",
                _ => "",
            };
            tracing::error!(status, "ask: AI service error: {msg}{hint}");
            return Err(match status {
                429 | 500..=599 => ApiError::bad("the AI service is busy; try again in a moment"),
                _ => ApiError::Internal(anyhow::anyhow!("AI service rejected the request (HTTP {status}): {msg}{hint}")),
            });
        }
        break v;
    };
    let (text, model, refused, cut) = match s.provider {
        Provider::Anthropic => {
            let text = resp["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            (text, resp["model"].as_str(), resp["stop_reason"] == "refusal", resp["stop_reason"] == "max_tokens")
        }
        Provider::OpenAiCompatible => {
            let choice = &resp["choices"][0];
            let message = &choice["message"];
            let text = match &message["content"] {
                Value::String(t) => t.clone(),
                // a list of parts, as some servers send it
                Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""),
                _ => String::new(),
            };
            let refused = message["refusal"].as_str().is_some_and(|r| !r.is_empty()) || choice["finish_reason"] == "content_filter";
            (text, resp["model"].as_str(), refused, choice["finish_reason"] == "length")
        }
    };
    let model = model.filter(|m| !m.is_empty()).unwrap_or(&s.model).to_string();
    if refused {
        return Ok(Answer { text: REFUSED.into(), model });
    }
    let text = strip_thinking(&text);
    if text.is_empty() {
        if cut {
            return Err(ApiError::bad("the AI ran out of room before it could answer; try a narrower question"));
        }
        return Err(ApiError::Internal(anyhow::anyhow!("AI service returned no text")));
    }
    Ok(Answer { text: if cut { format!("{text}\n\n{CUT_OFF}") } else { text }, model })
}

/// The answer without the reasoning some models write before it (`<think>…</think>`; also when
/// the opening tag was part of the prompt template, or the answer was cut off while thinking).
fn strip_thinking(text: &str) -> String {
    let mut out = text.to_string();
    if let Some(end) = out.find("</think>")
        && !out[..end].contains("<think>")
    {
        out.replace_range(..end + "</think>".len(), "");
    }
    while let Some(a) = out.find("<think>") {
        match out[a..].find("</think>") {
            Some(b) => out.replace_range(a..a + b + "</think>".len(), ""),
            None => out.truncate(a),
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_is_left_out() {
        assert_eq!(strip_thinking("<think>hmm</think>\n\nThe answer [1]."), "The answer [1].");
        assert_eq!(strip_thinking("hmm, let me see</think>The answer."), "The answer.");
        assert_eq!(strip_thinking("A <think>x</think>B <think>still thinking"), "A B");
        assert_eq!(strip_thinking("<think>only thinking"), "");
        assert_eq!(strip_thinking("  plain  "), "plain");
    }

    #[test]
    fn own_network() {
        for base in [
            "http://localhost:11434/v1",
            "http://ollama:11434/v1",
            "http://10.0.0.5:8000/v1",
            "http://[::1]:8080/v1",
            "http://llm.internal/v1",
            "http://192.168.1.20/v1",
        ] {
            assert!(local(base), "{base}");
        }
        for base in ["https://api.openai.com/v1", "https://api.anthropic.com", "http://8.8.8.8/v1", "not a url"] {
            assert!(!local(base), "{base}");
        }
    }
}
