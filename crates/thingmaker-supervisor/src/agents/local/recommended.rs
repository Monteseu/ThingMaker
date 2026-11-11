//! Models worth getting, and whether one fits this computer.
//!
//! The list is part of ThingMaker, never fetched: looking it up online would
//! say what the person runs. Sizes are Ollama's downloads, rounded.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommended {
    pub name: &'static str,
    /// What it is good at, in a few words.
    pub purpose: &'static str,
    pub size_bytes: u64,
    /// Whether it can be an agent (it makes tool calls Claude Code accepts).
    pub agent: bool,
}

const GB: u64 = 1_000_000_000;

/// In the order offered. Phase 0: `qwen3:8b` made real tool calls through
/// Claude Code where `llama3.1:8b` only described them.
pub const RECOMMENDED: &[Recommended] = &[
    Recommended { name: "qwen3:8b", purpose: "Everyday chat and reasoning; can be an agent", size_bytes: 5_200_000_000, agent: true },
    Recommended { name: "qwen3:4b", purpose: "Quick chat on smaller computers", size_bytes: 2_500_000_000, agent: true },
    Recommended { name: "qwen2.5-coder:7b", purpose: "Code", size_bytes: 4_700_000_000, agent: true },
    Recommended { name: "gemma3:12b", purpose: "Chat about images; chat only", size_bytes: 8_100_000_000, agent: false },
    Recommended { name: "qwen3:14b", purpose: "Stronger chat and reasoning", size_bytes: 9_300_000_000, agent: true },
    Recommended { name: "gpt-oss:20b", purpose: "Stronger reasoning; can be an agent", size_bytes: 14 * GB, agent: true },
    Recommended { name: "qwen3-coder:30b", purpose: "Stronger code agent", size_bytes: 19 * GB, agent: true },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    Fits,
    Tight,
    TooBig,
}

/// Whether a model of `size_bytes` runs well here. A loaded model needs its
/// file plus about 2.5 GB for a 16k window and overhead (Phase 0 measured an
/// 8B model at 5.3 GB with a 4k window and 9.1 GB with 32k), and the rest of
/// the computer needs room too.
pub fn fit(size_bytes: u64, memory_bytes: u64) -> Fit {
    let need = size_bytes as f64 + 2.5 * GB as f64;
    let memory = memory_bytes as f64;
    if need <= memory * 0.7 {
        Fit::Fits
    } else if need <= memory * 0.9 {
        Fit::Tight
    } else {
        Fit::TooBig
    }
}

/// One context window a model can be run with, and whether it fits here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowOption {
    pub tokens: u64,
    /// The model's own maximum, offered even when it cannot run here.
    pub max: bool,
    pub fit: Option<Fit>,
}

/// The windows offered for a model: 16k, 32k and 64k where they are below
/// its maximum, then the maximum itself.
pub fn window_options(context_length: Option<u64>, size_bytes: u64, kv_bytes_per_token: u64, memory_bytes: Option<u64>) -> Vec<WindowOption> {
    let max = context_length.unwrap_or(65_536).max(4_096);
    let fit = |tokens: u64| memory_bytes.map(|memory| window_fit(size_bytes, kv_bytes_per_token, tokens, memory));
    let mut options: Vec<WindowOption> = [16_384u64, 32_768, 65_536].into_iter().filter(|tokens| *tokens < max).map(|tokens| WindowOption { tokens, max: false, fit: fit(tokens) }).collect();
    options.push(WindowOption { tokens: max, max: true, fit: fit(max) });
    options
}

/// The largest offered window that fits; the smallest when none does.
pub fn default_window(options: &[WindowOption]) -> u64 {
    options.iter().rev().find(|option| option.fit == Some(Fit::Fits)).or_else(|| options.first()).map(|option| option.tokens).unwrap_or(16_384)
}

/// Whether a model fits with a `tokens` window: its file, the KV cache for
/// the window, and about 1 GB more, against the same shares of memory as [`fit`].
pub fn window_fit(size_bytes: u64, kv_bytes_per_token: u64, tokens: u64, memory_bytes: u64) -> Fit {
    let need = size_bytes as f64 + (kv_bytes_per_token * tokens) as f64 + GB as f64;
    let memory = memory_bytes as f64;
    if need <= memory * 0.7 {
        Fit::Fits
    } else if need <= memory * 0.9 {
        Fit::Tight
    } else {
        Fit::TooBig
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_sixteen_gigabytes_an_eight_b_model_fits_and_twenty_b_does_not() {
        let memory = 16 * 1024 * 1024 * 1024;
        assert_eq!(fit(5_200_000_000, memory), Fit::Fits);
        assert_eq!(fit(11 * GB, memory), Fit::Tight);
        assert_eq!(fit(14 * GB, memory), Fit::TooBig);
        assert_eq!(fit(14 * GB, 64 * 1024 * 1024 * 1024), Fit::Fits);
    }

    #[test]
    fn windows_go_up_to_the_models_maximum_and_default_to_the_largest_that_fits() {
        let memory = 16 * 1024 * 1024 * 1024;
        // qwen3:8b: 5.2 GB, 36 layers × 8 KV heads × 256 × 2 bytes per token, 40k maximum.
        let qwen = window_options(Some(40_960), 5_200_000_000, 147_456, Some(memory));
        assert_eq!(qwen.iter().map(|o| o.tokens).collect::<Vec<_>>(), [16_384, 32_768, 40_960]);
        assert!(qwen.last().unwrap().max);
        assert_eq!(default_window(&qwen), 32_768);
        // llama3.1:8b goes to 128k, which does not fit here but is still offered.
        let llama = window_options(Some(131_072), 4_900_000_000, 131_072, Some(memory));
        assert_eq!(llama.iter().map(|o| o.tokens).collect::<Vec<_>>(), [16_384, 32_768, 65_536, 131_072]);
        assert_eq!(llama.last().unwrap().fit, Some(Fit::TooBig));
        assert_eq!(default_window(&llama), 32_768);
    }
}
