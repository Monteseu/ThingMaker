//! Chat mode: a local model with nothing around it.
//!
//! ThingMaker talks to every agent over ACP, so a chat is an ACP agent too:
//! this executable started with [`CHAT_ARG`], speaking JSON-RPC on stdin and
//! stdout, passing each prompt to Ollama and streaming the reply back as
//! `agent_message_chunk` updates (and `agent_thought_chunk` for a model's
//! thinking). The session actor, the transcript, storage and the screens
//! then work as they do for any agent.
//!
//! It has no tools: it cannot read, change or run anything, and its only
//! connection is to Ollama on `127.0.0.1`. Each conversation is a JSONL file
//! of messages in its own folder, so a chat can be resumed and forked.

use std::{
    collections::HashMap,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use serde_json::{json, Value};

use super::ollama::{ChatChunk, ChatEnd, ChatMessage, Ollama};

/// The argument that starts this executable as the chat agent.
pub const CHAT_ARG: &str = "--thingmaker-local-chat";
/// The model a new chat starts on.
pub const MODEL_ENV: &str = "THINGMAKER_LOCAL_MODEL";
/// Where conversations are kept.
pub const DIR_ENV: &str = "THINGMAKER_LOCAL_CHAT_DIR";
/// The context window, in tokens.
pub const WINDOW_ENV: &str = "THINGMAKER_LOCAL_WINDOW";

/// The most text one attached file adds to a message.
const MAX_FILE_CHARS: usize = 200_000;

/// What the agent needs from Ollama; tests stand in for it.
pub trait ChatBackend: Send + Sync {
    fn models(&self) -> Vec<String>;
    /// Whether the model can think before answering (and so can be told not to).
    fn thinks(&self, model: &str) -> bool;
    fn chat(&self, model: &str, messages: &[ChatMessage], num_ctx: Option<u64>, think: Option<bool>, cancel: &AtomicBool, on_chunk: &mut dyn FnMut(ChatChunk)) -> Result<ChatEnd, String>;
}

impl ChatBackend for Ollama {
    fn models(&self) -> Vec<String> {
        Ollama::models(self).map(|models| models.into_iter().map(|model| model.name).collect()).unwrap_or_default()
    }

    fn thinks(&self, model: &str) -> bool {
        Ollama::model(self, model).map(|model| model.capabilities.iter().any(|capability| capability == "thinking")).unwrap_or(false)
    }

    fn chat(&self, model: &str, messages: &[ChatMessage], num_ctx: Option<u64>, think: Option<bool>, cancel: &AtomicBool, on_chunk: &mut dyn FnMut(ChatChunk)) -> Result<ChatEnd, String> {
        Ollama::chat(self, model, messages, num_ctx, think, cancel, on_chunk)
    }
}

#[derive(Debug, Clone)]
pub struct ChatConfig {
    pub dir: PathBuf,
    pub model: String,
    pub window: Option<u64>,
}

impl ChatConfig {
    pub fn from_env() -> Self {
        let dir = std::env::var_os(DIR_ENV).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("thingmaker-local-chat"));
        let model = std::env::var(MODEL_ENV).unwrap_or_default();
        let window = std::env::var(WINDOW_ENV).ok().and_then(|value| value.parse().ok());
        Self { dir, model, window }
    }
}

struct Session {
    model: String,
    /// Whether a model that thinks may do so. Off by default: on a laptop,
    /// thinking can take most of a reply's time.
    think: bool,
    messages: Vec<ChatMessage>,
    cancel: Option<Arc<AtomicBool>>,
}

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

struct Agent {
    config: ChatConfig,
    backend: Arc<dyn ChatBackend>,
    sessions: Mutex<HashMap<String, Session>>,
    out: Out,
}

/// Runs the agent over stdin and stdout until stdin closes.
pub fn run() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    serve(stdin.lock(), Box::new(std::io::stdout()), Arc::new(Ollama::local()), ChatConfig::from_env());
    Ok(())
}

/// Serves ACP on `input`/`output` until `input` ends. Turns run on their own
/// threads so a `session/cancel` can arrive while one streams.
pub fn serve(input: impl BufRead, output: Box<dyn Write + Send>, backend: Arc<dyn ChatBackend>, config: ChatConfig) {
    let _ = std::fs::create_dir_all(&config.dir);
    let agent = Arc::new(Agent { config, backend, sessions: Mutex::new(HashMap::new()), out: Arc::new(Mutex::new(output)) });
    let mut turns = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(method) = message.get("method").and_then(Value::as_str).map(str::to_string) else { continue };
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if method == "session/prompt" {
            let agent = Arc::clone(&agent);
            turns.push(std::thread::spawn(move || agent.prompt(id, &params)));
            continue;
        }
        agent.handle(&method, id, &params);
    }
    for turn in turns {
        let _ = turn.join();
    }
}

impl Agent {
    fn send(&self, message: Value) {
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{message}");
            let _ = out.flush();
        }
    }

    fn reply(&self, id: Option<Value>, result: Value) {
        if let Some(id) = id {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    }

    fn fail(&self, id: Option<Value>, code: i64, message: &str) {
        if let Some(id) = id {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
        }
    }

    fn update(&self, session: &str, update: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": update } }));
    }

    fn handle(&self, method: &str, id: Option<Value>, params: &Value) {
        let session_id = params.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
        match method {
            "initialize" => self.reply(
                id,
                json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "promptCapabilities": { "image": true, "embeddedContext": true },
                        "loadSession": true,
                        "sessionCapabilities": { "close": {}, "fork": {} }
                    },
                    "agentInfo": { "name": "thingmaker-local-chat", "title": "Local chat", "version": env!("CARGO_PKG_VERSION") },
                    "authMethods": []
                }),
            ),
            "session/new" => {
                let new_id = uuid::Uuid::new_v4().to_string();
                let model = self.config.model.clone();
                self.store(&new_id, &model, &[]);
                self.sessions.lock().unwrap().insert(new_id.clone(), Session { model: model.clone(), think: false, messages: Vec::new(), cancel: None });
                self.reply(id, json!({ "sessionId": new_id, "configOptions": self.options(&model, false) }));
            }
            "session/load" | "session/resume" => match self.restore(&session_id) {
                Some((model, messages)) => {
                    if method == "session/load" {
                        for message in &messages {
                            let kind = if message.role == "user" { "user_message_chunk" } else { "agent_message_chunk" };
                            self.update(&session_id, json!({ "sessionUpdate": kind, "content": { "type": "text", "text": message.content } }));
                        }
                    }
                    self.sessions.lock().unwrap().insert(session_id.clone(), Session { model: model.clone(), think: false, messages, cancel: None });
                    self.reply(id, json!({ "sessionId": session_id, "configOptions": self.options(&model, false) }));
                }
                None => self.fail(id, -32602, &format!("no conversation {session_id}")),
            },
            "session/fork" => {
                let source = self.sessions.lock().unwrap().get(&session_id).map(|session| (session.model.clone(), session.messages.clone())).or_else(|| self.restore(&session_id));
                match source {
                    Some((model, messages)) => {
                        let copy = uuid::Uuid::new_v4().to_string();
                        self.store(&copy, &model, &messages);
                        self.sessions.lock().unwrap().insert(copy.clone(), Session { model: model.clone(), think: false, messages, cancel: None });
                        self.reply(id, json!({ "sessionId": copy, "configOptions": self.options(&model, false) }));
                    }
                    None => self.fail(id, -32602, &format!("no conversation {session_id}")),
                }
            }
            "session/set_config_option" => {
                // The desktop also sets mode on every agent; a chat has
                // none, so it is accepted and ignored. Effort is Think.
                let option = params.get("configId").and_then(Value::as_str).unwrap_or_default();
                let value = params.get("value").and_then(Value::as_str).unwrap_or_default().to_string();
                let model = {
                    let mut sessions = self.sessions.lock().unwrap();
                    match sessions.get_mut(&session_id) {
                        Some(session) => {
                            if option == "effort" {
                                session.think = value == "on";
                            }
                            if option == "model" && !value.is_empty() {
                                session.model = value;
                            }
                            Some((session.model.clone(), session.think, session.messages.clone()))
                        }
                        None => None,
                    }
                };
                match model {
                    Some((model, think, messages)) => {
                        if option == "model" {
                            self.store(&session_id, &model, &messages);
                        }
                        self.reply(id, json!({ "configOptions": self.options(&model, think) }));
                    }
                    None => self.fail(id, -32602, &format!("no conversation {session_id}")),
                }
            }
            "session/cancel" => {
                if let Some(cancel) = self.sessions.lock().unwrap().get(&session_id).and_then(|session| session.cancel.clone()) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            "session/close" => {
                self.sessions.lock().unwrap().remove(&session_id);
                self.reply(id, json!({}));
            }
            _ => self.fail(id, -32601, &format!("a local chat has no {method}")),
        }
    }

    fn prompt(&self, id: Option<Value>, params: &Value) {
        let session_id = params.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
        let message = user_message(params.get("prompt"));
        let cancel = Arc::new(AtomicBool::new(false));
        let (model, think, history) = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(session) = sessions.get_mut(&session_id) else {
                drop(sessions);
                return self.fail(id, -32602, &format!("no conversation {session_id}"));
            };
            if session.cancel.is_some() {
                drop(sessions);
                return self.fail(id, -32603, "this chat is already answering");
            }
            session.messages.push(message);
            session.cancel = Some(Arc::clone(&cancel));
            (session.model.clone(), session.think, session.messages.clone())
        };
        if model.is_empty() {
            self.finish(&session_id, None);
            return self.fail(id, -32603, "no local model is chosen for this chat");
        }
        self.store(&session_id, &model, &history);
        let mut text = String::new();
        let think = self.backend.thinks(&model).then_some(think);
        let ended = self.backend.chat(&model, &history, self.config.window, think, &cancel, &mut |chunk| match chunk {
            ChatChunk::Thinking(thought) => self.update(&session_id, json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": thought } })),
            ChatChunk::Text(piece) => {
                text.push_str(&piece);
                self.update(&session_id, json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": piece } }));
            }
        });
        let reply = (!text.is_empty()).then(|| ChatMessage { role: "assistant".into(), content: text, images: Vec::new() });
        self.finish(&session_id, reply);
        match ended {
            Ok(ChatEnd::Done(usage)) => self.reply(id, json!({ "stopReason": "end_turn", "usage": { "inputTokens": usage.input_tokens, "outputTokens": usage.output_tokens, "totalTokens": usage.input_tokens + usage.output_tokens } })),
            Ok(ChatEnd::Cancelled) => self.reply(id, json!({ "stopReason": "cancelled" })),
            Err(error) => self.fail(id, -32603, &error),
        }
    }

    /// The turn is over: the reply (or what came of it) joins the history.
    fn finish(&self, session_id: &str, reply: Option<ChatMessage>) {
        let saved = {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.get_mut(session_id).map(|session| {
                session.cancel = None;
                if let Some(reply) = reply {
                    session.messages.push(reply);
                }
                (session.model.clone(), session.messages.clone())
            })
        };
        if let Some((model, messages)) = saved {
            self.store(session_id, &model, &messages);
        }
    }

    fn options(&self, current: &str, think: bool) -> Value {
        let mut models = self.backend.models();
        if !current.is_empty() && !models.iter().any(|model| model == current) {
            models.insert(0, current.to_string());
        }
        let mut options = vec![json!({
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": current,
            "options": models.iter().map(|model| json!({ "value": model, "name": model })).collect::<Vec<_>>()
        })];
        // Thinking shows as the effort control, so the composer's own switch
        // turns it on and off.
        if !current.is_empty() && self.backend.thinks(current) {
            options.push(json!({
                "id": "effort",
                "name": "Think",
                "category": "thought_level",
                "type": "select",
                "currentValue": if think { "on" } else { "off" },
                "options": [{ "value": "off", "name": "Think off" }, { "value": "on", "name": "Think on" }]
            }));
        }
        Value::Array(options)
    }

    fn file(&self, session_id: &str) -> Option<PathBuf> {
        // Ids are ours (uuids); anything else never names a file.
        uuid::Uuid::parse_str(session_id).ok().map(|_| self.config.dir.join(format!("{session_id}.jsonl")))
    }

    /// Writes the conversation: a first line with the model, then one line
    /// per message. Rewritten whole, so a crash leaves the previous copy.
    fn store(&self, session_id: &str, model: &str, messages: &[ChatMessage]) {
        let Some(path) = self.file(session_id) else { return };
        let mut text = json!({ "model": model }).to_string();
        text.push('\n');
        for message in messages {
            text.push_str(&serde_json::to_string(message).unwrap_or_default());
            text.push('\n');
        }
        let temp = path.with_extension("jsonl.tmp");
        if std::fs::write(&temp, text).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    }

    fn restore(&self, session_id: &str) -> Option<(String, Vec<ChatMessage>)> {
        read_conversation(&self.file(session_id)?)
    }
}

/// A stored conversation: its model and messages.
pub fn read_conversation(path: &Path) -> Option<(String, Vec<ChatMessage>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let model = serde_json::from_str::<Value>(lines.next()?).ok()?.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
    let messages = lines.filter_map(|line| serde_json::from_str::<ChatMessage>(line).ok()).collect();
    Some((model, messages))
}

/// A prompt's blocks as one user message: text as it is, images for models
/// that see, attached files pasted in with a size cap.
fn user_message(prompt: Option<&Value>) -> ChatMessage {
    let mut parts: Vec<String> = Vec::new();
    let mut images = Vec::new();
    for block in prompt.and_then(Value::as_array).into_iter().flatten() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => parts.push(block.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
            Some("image") => {
                if let Some(data) = block.get("data").and_then(Value::as_str) {
                    images.push(data.to_string());
                }
            }
            Some("resource") => {
                let resource = block.get("resource");
                let uri = resource.and_then(|r| r.get("uri")).and_then(Value::as_str).unwrap_or("file");
                if let Some(text) = resource.and_then(|r| r.get("text")).and_then(Value::as_str) {
                    let cut: String = text.chars().take(MAX_FILE_CHARS).collect();
                    let more = if cut.len() < text.len() { "\n[…cut]" } else { "" };
                    parts.push(format!("<file {uri}>\n{cut}{more}\n</file>"));
                }
            }
            Some("resource_link") => {
                if let Some(uri) = block.get("uri").and_then(Value::as_str) {
                    parts.push(format!("(attached: {uri})"));
                }
            }
            _ => {}
        }
    }
    ChatMessage { role: "user".into(), content: parts.join("\n\n"), images }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Echoes the last message back in two pieces, with a thought first.
    struct Echo;

    impl ChatBackend for Echo {
        fn models(&self) -> Vec<String> {
            vec!["qwen3:8b".into(), "llama3.1:8b".into()]
        }

        fn thinks(&self, model: &str) -> bool {
            model == "qwen3:8b"
        }

        fn chat(&self, model: &str, messages: &[ChatMessage], _: Option<u64>, think: Option<bool>, cancel: &AtomicBool, on_chunk: &mut dyn FnMut(ChatChunk)) -> Result<ChatEnd, String> {
            if think == Some(false) {
                on_chunk(ChatChunk::Text(String::new()));
            }
            if model == "down" {
                return Err("Ollama is not running".into());
            }
            on_chunk(ChatChunk::Thinking("hmm".into()));
            if cancel.load(Ordering::Relaxed) {
                return Ok(ChatEnd::Cancelled);
            }
            let last = messages.last().map(|message| message.content.clone()).unwrap_or_default();
            on_chunk(ChatChunk::Text(format!("{} ", messages.len())));
            on_chunk(ChatChunk::Text(format!("you said {last}")));
            Ok(ChatEnd::Done(Default::default()))
        }
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn run(dir: &Path, model: &str, lines: &[Value]) -> Vec<Value> {
        let input = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
        let sink = Sink::default();
        serve(Cursor::new(input), Box::new(sink.clone()), Arc::new(Echo), ChatConfig { dir: dir.to_path_buf(), model: model.into(), window: Some(16384) });
        let bytes = sink.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    fn result(out: &[Value], id: u64) -> &Value {
        out.iter().find(|message| message.get("id") == Some(&json!(id))).unwrap()
    }

    fn session_of(out: &[Value]) -> String {
        result(out, 1)["result"]["sessionId"].as_str().unwrap().to_string()
    }

    #[test]
    fn a_chat_streams_the_models_reply_and_keeps_the_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let start = run(dir.path(), "qwen3:8b", &[json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}), json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/w","mcpServers":[]}})]);
        assert_eq!(result(&start, 0)["result"]["agentCapabilities"]["loadSession"], json!(true));
        let id = session_of(&start);
        assert_eq!(result(&start, 1)["result"]["configOptions"][0]["currentValue"], json!("qwen3:8b"));

        // A new process, as when ThingMaker restarts: load replays, then a turn.
        let out = run(dir.path(), "qwen3:8b", &[
            json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":id,"cwd":"/w","mcpServers":[]}}),
            json!({"jsonrpc":"2.0","id":2,"method":"session/set_config_option","params":{"sessionId":id,"configId":"mode","value":"acceptEdits"}}),
            json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":id,"prompt":[{"type":"text","text":"hello"}]}}),
        ]);
        assert!(result(&out, 2).get("result").is_some(), "a mode the chat does not have is accepted, not refused");
        let think = &result(&out, 1)["result"]["configOptions"][1];
        assert_eq!((think["id"].as_str(), think["currentValue"].as_str()), (Some("effort"), Some("off")), "a model that thinks gets Think, off");
        let texts: Vec<&str> = out.iter().filter(|m| m["params"]["update"]["sessionUpdate"] == json!("agent_message_chunk")).map(|m| m["params"]["update"]["content"]["text"].as_str().unwrap()).collect();
        assert_eq!(texts.concat(), "1 you said hello");
        assert!(out.iter().any(|m| m["params"]["update"]["sessionUpdate"] == json!("agent_thought_chunk")));
        assert_eq!(result(&out, 3)["result"]["stopReason"], json!("end_turn"));

        let again = run(dir.path(), "qwen3:8b", &[json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":id}}), json!({"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":id,"prompt":[{"type":"text","text":"again"}]}})]);
        let replayed: Vec<&str> = again.iter().filter_map(|m| m["params"]["update"]["content"]["text"].as_str()).collect();
        assert_eq!(&replayed[..2], ["hello", "1 you said hello"], "the earlier turn is replayed in order");
        assert!(again.iter().any(|m| m["params"]["update"]["content"]["text"] == json!("3 ")), "the model saw the whole conversation");
    }

    #[test]
    fn a_fork_copies_the_conversation_and_the_model_can_change() {
        let dir = tempfile::tempdir().unwrap();
        let start = run(dir.path(), "qwen3:8b", &[json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{}})]);
        let id = session_of(&start);
        let out = run(dir.path(), "qwen3:8b", &[
            json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":id}}),
            json!({"jsonrpc":"2.0","id":2,"method":"session/set_config_option","params":{"sessionId":id,"configId":"model","value":"llama3.1:8b"}}),
            json!({"jsonrpc":"2.0","id":3,"method":"session/fork","params":{"sessionId":id}}),
        ]);
        assert_eq!(result(&out, 2)["result"]["configOptions"][0]["currentValue"], json!("llama3.1:8b"));
        let copy = result(&out, 3)["result"]["sessionId"].as_str().unwrap().to_string();
        assert_ne!(copy, id);
        assert_eq!(read_conversation(&dir.path().join(format!("{copy}.jsonl"))).unwrap().0, "llama3.1:8b");
    }

    #[test]
    fn errors_are_said_and_ids_never_name_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let start = run(dir.path(), "down", &[json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{}})]);
        let id = session_of(&start);
        let out = run(dir.path(), "down", &[
            json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":id}}),
            json!({"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":id,"prompt":[{"type":"text","text":"hi"}]}}),
            json!({"jsonrpc":"2.0","id":3,"method":"session/load","params":{"sessionId":"../../etc/passwd"}}),
            json!({"jsonrpc":"2.0","id":4,"method":"terminal/create","params":{}}),
        ]);
        assert_eq!(result(&out, 2)["error"]["message"], json!("Ollama is not running"));
        assert!(result(&out, 3).get("error").is_some());
        assert!(result(&out, 4).get("error").is_some(), "a chat has no tools");
    }

    #[test]
    fn attachments_become_part_of_the_message() {
        let message = user_message(Some(&json!([
            {"type":"text","text":"look"},
            {"type":"image","data":"QUJD","mimeType":"image/png"},
            {"type":"resource","resource":{"uri":"file:///a.txt","text":"hello"}}
        ])));
        assert_eq!(message.images, vec!["QUJD".to_string()]);
        assert!(message.content.starts_with("look\n\n<file file:///a.txt>\nhello"));
    }
}

/// Against the real Ollama on this computer, when `THINGMAKER_REAL_LOCAL`
/// names an installed model.
#[cfg(test)]
mod real {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_real_chat_turn() {
        let Ok(model) = std::env::var("THINGMAKER_REAL_LOCAL") else { return };
        let dir = tempfile::tempdir().unwrap();
        let sink = Arc::new(Mutex::new(Vec::<u8>::new()));
        struct Out(Arc<Mutex<Vec<u8>>>);
        impl Write for Out {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let new = json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{}}).to_string();
        serve(Cursor::new(new), Box::new(Out(Arc::clone(&sink))), Arc::new(Ollama::local()), ChatConfig { dir: dir.path().into(), model: model.clone(), window: Some(8192) });
        let first: Value = serde_json::from_str(String::from_utf8(sink.lock().unwrap().clone()).unwrap().lines().next().unwrap()).unwrap();
        let id = first["result"]["sessionId"].as_str().unwrap().to_string();
        sink.lock().unwrap().clear();
        let input = [
            json!({"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":id}}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":id,"prompt":[{"type":"text","text":"Name three deep-sea creatures, one word each."}]}}).to_string(),
        ]
        .join("\n");
        let started = std::time::Instant::now();
        serve(Cursor::new(input), Box::new(Out(Arc::clone(&sink))), Arc::new(Ollama::local()), ChatConfig { dir: dir.path().into(), model, window: Some(8192) });
        let text = String::from_utf8(sink.lock().unwrap().clone()).unwrap();
        let reply: String = text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).filter(|m| m["params"]["update"]["sessionUpdate"] == json!("agent_message_chunk")).filter_map(|m| m["params"]["update"]["content"]["text"].as_str().map(str::to_string)).collect();
        let done = text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).find(|m| m["id"] == json!(2)).unwrap();
        eprintln!("REPLY in {:?}: {reply:?}\nDONE: {done}", started.elapsed());
        assert_eq!(done["result"]["stopReason"], json!("end_turn"));
        assert!(!reply.trim().is_empty());
    }
}
