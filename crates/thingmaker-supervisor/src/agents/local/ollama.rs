//! Ollama on this computer, through its HTTP API: what is installed, what is
//! loaded, downloads, and chat.
//!
//! Only `127.0.0.1` is ever addressed. Models that run on Ollama's servers
//! (`:cloud`) are left out everywhere, because they are neither free nor
//! private, and the copies ThingMaker makes to set a model's context window
//! (`…-thingmaker-32k`) are hidden from what the person sees.

use std::{
    io::{BufRead, BufReader},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const ADDRESS: &str = "http://127.0.0.1:11434";

/// Where Ollama listens: [`ADDRESS`], or `THINGMAKER_OLLAMA_ADDRESS` when it
/// names this computer.
pub fn address() -> String {
    std::env::var(ADDRESS_ENV).ok().filter(|value| is_this_computer(value)).unwrap_or_else(|| ADDRESS.to_string())
}

pub const ADDRESS_ENV: &str = "THINGMAKER_OLLAMA_ADDRESS";

fn is_this_computer(address: &str) -> bool {
    let rest = address.strip_prefix("http://").unwrap_or("");
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost")
}

/// What a derived copy's name carries, so it can be hidden and traced back.
const DERIVED_MARK: &str = "thingmaker-";

/// An installed model, as the Providers card and the pickers show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    pub name: String,
    pub family: String,
    pub parameter_size: String,
    pub quantization: String,
    pub size_bytes: u64,
    /// The longest window the model supports, when Ollama says.
    pub context_length: Option<u64>,
    /// Ollama's capability names: `completion`, `tools`, `vision`, `thinking`, …
    pub capabilities: Vec<String>,
    /// Memory each token of context window takes (the KV cache), from the
    /// model's shape; typical 8B values where Ollama does not say.
    pub kv_bytes_per_token: u64,
}

impl LocalModel {
    /// Whether it can run as an agent: Claude Code needs tool calls.
    pub fn uses_tools(&self) -> bool {
        self.capabilities.iter().any(|capability| capability == "tools")
    }

    pub fn sees_images(&self) -> bool {
        self.capabilities.iter().any(|capability| capability == "vision")
    }
}

/// A model in memory right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedModel {
    pub name: String,
    pub size_bytes: u64,
    pub context_length: Option<u64>,
}

/// How far a download is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

/// One message of a chat, as Ollama takes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Base64 images, for models that see.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

/// A piece of a streamed reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatChunk {
    Thinking(String),
    Text(String),
}

/// Totals Ollama reports when a reply is done.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
}

/// How a chat ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEnd {
    Done(ChatUsage),
    Cancelled,
}

/// Whether a name is one of Ollama's cloud models.
pub fn is_cloud(name: &str) -> bool {
    let tag = name.rsplit_once(':').map_or("", |(_, tag)| tag);
    tag == "cloud" || tag.ends_with("-cloud")
}

/// Whether a name is a copy ThingMaker made to set a window.
pub fn is_derived(name: &str) -> bool {
    name.contains(DERIVED_MARK)
}

/// The name of the copy of `model` with a `num_ctx` window:
/// `qwen3:8b` → `qwen3:8b-thingmaker-32k`.
pub fn derived_name(model: &str, num_ctx: u64) -> String {
    let size = if num_ctx.is_multiple_of(1024) { format!("{}k", num_ctx / 1024) } else { num_ctx.to_string() };
    if model.contains(':') {
        format!("{model}-{DERIVED_MARK}{size}")
    } else {
        format!("{model}:{DERIVED_MARK}{size}")
    }
}

/// The model a derived copy was made from.
pub fn base_name(name: &str) -> &str {
    match name.find(DERIVED_MARK) {
        Some(at) => name[..at].trim_end_matches(['-', ':']),
        None => name,
    }
}

pub struct Ollama {
    base: String,
    client: reqwest::blocking::Client,
    /// For streams, which can run as long as a model takes to answer.
    streaming: reqwest::blocking::Client,
}

impl Ollama {
    pub fn new(base: &str) -> Self {
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(20)).no_proxy().build().unwrap_or_default();
        let streaming = reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(5)).no_proxy().build().unwrap_or_default();
        Self { base: base.trim_end_matches('/').to_string(), client, streaming }
    }

    /// Ollama on this computer. `THINGMAKER_OLLAMA_ADDRESS` can move it to
    /// another port (tests do), but never off this computer.
    pub fn local() -> Self {
        Self::new(&address())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let response = self.client.get(self.url(path)).send().map_err(unreachable)?;
        read_json(response)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let response = self.client.post(self.url(path)).json(body).send().map_err(unreachable)?;
        read_json(response)
    }

    /// Ollama's version; an error means it is not running.
    pub fn version(&self) -> Result<String, String> {
        Ok(self.get("/api/version")?.get("version").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    /// The installed models, without cloud models and without ThingMaker's
    /// own copies.
    pub fn models(&self) -> Result<Vec<LocalModel>, String> {
        let tags = self.get("/api/tags")?;
        let mut models = Vec::new();
        for entry in tags.get("models").and_then(Value::as_array).into_iter().flatten() {
            let name = entry.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() || is_cloud(name) || is_derived(name) || entry.get("remote_host").is_some() {
                continue;
            }
            let details = entry.get("details").cloned().unwrap_or(Value::Null);
            // One more call per model for what it can do; a model whose
            // details cannot be read is listed without capabilities.
            let show = self.post("/api/show", &json!({ "model": name })).unwrap_or(Value::Null);
            models.push(model_from(name, entry, &details, &show));
        }
        models.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(models)
    }

    /// One model's details.
    pub fn model(&self, name: &str) -> Result<LocalModel, String> {
        let show = self.post("/api/show", &json!({ "model": name }))?;
        let details = show.get("details").cloned().unwrap_or(Value::Null);
        Ok(model_from(name, &Value::Null, &details, &show))
    }

    /// What is in memory now.
    pub fn loaded(&self) -> Result<Vec<LoadedModel>, String> {
        let ps = self.get("/api/ps")?;
        Ok(ps
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|entry| LoadedModel {
                name: base_name(entry.get("name").and_then(Value::as_str).unwrap_or_default()).to_string(),
                size_bytes: entry.get("size").and_then(Value::as_u64).unwrap_or(0),
                context_length: entry.get("context_length").and_then(Value::as_u64),
            })
            .collect())
    }

    /// Downloads a model, reporting progress, until done or `cancel` is set.
    pub fn pull(&self, name: &str, cancel: &AtomicBool, mut on_progress: impl FnMut(PullProgress)) -> Result<bool, String> {
        if is_cloud(name) {
            return Err(format!("{name} runs on Ollama's servers, not on this computer"));
        }
        let response = self.streaming.post(self.url("/api/pull")).json(&json!({ "model": name, "stream": true })).send().map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(error_text(response));
        }
        for line in BufReader::new(response).lines() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let line = line.map_err(|error| error.to_string())?;
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                return Err(error.to_string());
            }
            on_progress(PullProgress {
                status: value.get("status").and_then(Value::as_str).unwrap_or_default().to_string(),
                completed: value.get("completed").and_then(Value::as_u64),
                total: value.get("total").and_then(Value::as_u64),
            });
        }
        Ok(true)
    }

    /// Removes a model, and the copies ThingMaker made of it.
    pub fn delete(&self, name: &str) -> Result<(), String> {
        let derived: Vec<String> = self
            .get("/api/tags")
            .ok()
            .and_then(|tags| tags.get("models").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
            .filter(|other| is_derived(other) && base_name(other) == name)
            .map(str::to_string)
            .collect();
        for copy in derived {
            let _ = self.client.delete(self.url("/api/delete")).json(&json!({ "model": copy })).send();
        }
        let response = self.client.delete(self.url("/api/delete")).json(&json!({ "model": name })).send().map_err(unreachable)?;
        if response.status().is_success() { Ok(()) } else { Err(error_text(response)) }
    }

    /// The copy of `model` with a `num_ctx` window, made if it is missing.
    /// Ollama loads a plain model with a small window (4,096 tokens), too
    /// small for an agent's instructions; the copy carries the size with it.
    pub fn with_window(&self, model: &str, num_ctx: u64) -> Result<String, String> {
        let name = derived_name(model, num_ctx);
        if self.post("/api/show", &json!({ "model": name })).is_ok() {
            return Ok(name);
        }
        self.post("/api/create", &json!({ "model": name, "from": model, "parameters": { "num_ctx": num_ctx }, "stream": false }))?;
        Ok(name)
    }

    /// Streams a chat reply into `on_chunk` until it is done or `cancel` is
    /// set. `num_ctx` sets the window for this request; `think` turns a
    /// thinking model's thinking on or off.
    pub fn chat(&self, model: &str, messages: &[ChatMessage], num_ctx: Option<u64>, think: Option<bool>, cancel: &AtomicBool, mut on_chunk: impl FnMut(ChatChunk)) -> Result<ChatEnd, String> {
        let mut body = json!({ "model": model, "messages": messages, "stream": true });
        if let Some(think) = think {
            body["think"] = json!(think);
        }
        if let Some(num_ctx) = num_ctx {
            body["options"] = json!({ "num_ctx": num_ctx });
        }
        let response = self.streaming.post(self.url("/api/chat")).json(&body).send().map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(error_text(response));
        }
        for line in BufReader::new(response).lines() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(ChatEnd::Cancelled);
            }
            let line = line.map_err(|error| error.to_string())?;
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                return Err(error.to_string());
            }
            let message = value.get("message");
            if let Some(thinking) = message.and_then(|m| m.get("thinking")).and_then(Value::as_str).filter(|text| !text.is_empty()) {
                on_chunk(ChatChunk::Thinking(thinking.to_string()));
            }
            if let Some(text) = message.and_then(|m| m.get("content")).and_then(Value::as_str).filter(|text| !text.is_empty()) {
                on_chunk(ChatChunk::Text(text.to_string()));
            }
            if value.get("done").and_then(Value::as_bool) == Some(true) {
                return Ok(ChatEnd::Done(ChatUsage {
                    input_tokens: value.get("prompt_eval_count").and_then(Value::as_u64).unwrap_or(0),
                    output_tokens: value.get("eval_count").and_then(Value::as_u64).unwrap_or(0),
                    duration_ms: value.get("total_duration").and_then(Value::as_u64).unwrap_or(0) / 1_000_000,
                }));
            }
        }
        if cancel.load(Ordering::Relaxed) { Ok(ChatEnd::Cancelled) } else { Err("Ollama stopped answering before the reply was done".into()) }
    }
}

fn model_from(name: &str, entry: &Value, details: &Value, show: &Value) -> LocalModel {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or_default().to_string();
    let context_length = show
        .get("model_info")
        .and_then(Value::as_object)
        .and_then(|info| info.iter().find(|(key, _)| key.ends_with(".context_length")).and_then(|(_, value)| value.as_u64()));
    let info = show.get("model_info").and_then(Value::as_object);
    let number = |suffix: &str| info.and_then(|info| info.iter().find(|(key, _)| key.ends_with(suffix)).and_then(|(_, value)| value.as_u64()));
    let blocks = number(".block_count").unwrap_or(32);
    let heads = number(".attention.head_count").unwrap_or(32).max(1);
    let kv_heads = number(".attention.head_count_kv").unwrap_or(8);
    let head_dim = number(".embedding_length").map(|width| width / heads).unwrap_or(128);
    let key = number(".attention.key_length").unwrap_or(head_dim);
    let value = number(".attention.value_length").unwrap_or(head_dim);
    // Keys and values for every layer, at two bytes each (f16).
    let kv_bytes_per_token = blocks * kv_heads * (key + value) * 2;
    LocalModel {
        kv_bytes_per_token,
        name: name.to_string(),
        family: text(details.get("family")),
        parameter_size: text(details.get("parameter_size")),
        quantization: text(details.get("quantization_level")),
        size_bytes: entry.get("size").and_then(Value::as_u64).unwrap_or(0),
        context_length,
        capabilities: show.get("capabilities").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
    }
}

fn unreachable(error: reqwest::Error) -> String {
    if error.is_connect() || error.is_timeout() { "Ollama is not running".into() } else { error.to_string() }
}

fn read_json(response: reqwest::blocking::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(error_text(response));
    }
    response.json::<Value>().map_err(|error| format!("Ollama answered something unexpected: {error}"))
}

fn error_text(response: reqwest::blocking::Response) -> String {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    serde_json::from_str::<Value>(&body).ok().and_then(|value| value.get("error").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| format!("Ollama answered {status}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_models_and_thingmakers_own_copies_are_told_apart() {
        assert!(is_cloud("gpt-oss:120b-cloud"));
        assert!(is_cloud("deepseek-v3.1:671b-cloud"));
        assert!(is_cloud("kimi:cloud"));
        assert!(!is_cloud("qwen3:8b"));
        assert_eq!(derived_name("qwen3:8b", 32768), "qwen3:8b-thingmaker-32k");
        assert_eq!(derived_name("mistral", 16384), "mistral:thingmaker-16k");
        assert!(is_derived("qwen3:8b-thingmaker-32k"));
        assert_eq!(base_name("qwen3:8b-thingmaker-32k"), "qwen3:8b");
        assert_eq!(base_name("mistral:thingmaker-16k"), "mistral");
        assert_eq!(base_name("qwen3:8b"), "qwen3:8b");
    }

    #[test]
    fn the_address_can_move_ports_but_never_leaves_the_computer() {
        assert!(is_this_computer("http://127.0.0.1:5555"));
        assert!(is_this_computer("http://localhost:11434/"));
        assert!(!is_this_computer("http://192.168.1.20:11434"));
        assert!(!is_this_computer("https://ollama.example.com"));
        assert!(!is_this_computer("http://127.0.0.1.evil.com:80"));
    }

    #[test]
    fn a_model_reads_its_window_and_what_it_can_do() {
        let entry = json!({ "size": 4_900_000_000u64 });
        let details = json!({ "family": "llama", "parameter_size": "8.0B", "quantization_level": "Q4_K_M" });
        let show = json!({ "capabilities": ["completion", "tools"], "model_info": { "llama.context_length": 131072 } });
        let model = model_from("llama3.1:8b", &entry, &details, &show);
        assert_eq!(model.context_length, Some(131072));
        assert!(model.uses_tools());
        assert!(!model.sees_images());
        assert_eq!(model.parameter_size, "8.0B");
        let llama = json!({ "model_info": { "llama.block_count": 32, "llama.attention.head_count": 32, "llama.attention.head_count_kv": 8, "llama.embedding_length": 4096 } });
        assert_eq!(model_from("llama3.1:8b", &entry, &details, &llama).kv_bytes_per_token, 131_072, "Phase 0 measured about 4 GB more at 32k");
    }
}
