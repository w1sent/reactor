//! Choosing a model: `provider/model` → an [`Llm`], through rig.
//!
//! rig speaks to twenty-odd providers behind one API, which is the point of building
//! on it (ADR-0033); this module names the ones REactor wires up. Credentials come from
//! the provider's usual environment variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …).
//! Adding a provider is one variant and one match arm.

use rig_core::client::{CompletionClient, ProviderClient};
use rig_core::providers::{anthropic, gemini, ollama, openai, openrouter};

use crate::error::{Error, Result};
use crate::llm::{Delta, Llm, LlmRequest, Reply, RigLlm};

type AnthropicModel = <anthropic::Client as CompletionClient>::CompletionModel;
type OpenAiModel = <openai::Client as CompletionClient>::CompletionModel;
type GeminiModel = <gemini::Client as CompletionClient>::CompletionModel;
type OpenRouterModel = <openrouter::Client as CompletionClient>::CompletionModel;
type OllamaModel = <ollama::Client as CompletionClient>::CompletionModel;

/// What the settings choose about building a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Keep the start of the request (system prompt, tool definitions) in the provider's cache for
    /// an hour, where the provider has such a thing ([ADR-0059](../../../docs/adr/0059-the-cache-is-measured-before-it-is-tuned.md)).
    pub prefix_one_hour: bool,
}

/// Any of the wired-up providers.
#[derive(Clone)]
pub enum AnyLlm {
    Anthropic(RigLlm<AnthropicModel>),
    OpenAi(RigLlm<OpenAiModel>),
    Gemini(RigLlm<GeminiModel>),
    OpenRouter(RigLlm<OpenRouterModel>),
    Ollama(RigLlm<OllamaModel>),
    /// No model chosen yet. Every call says so, rather than the session being unable to
    /// open until a model is configured — a window can open and offer a picker.
    Missing(String),
}

/// The providers by name, for `--help` and error messages.
pub const PROVIDERS: [&str; 5] = ["anthropic", "openai", "gemini", "openrouter", "ollama"];

/// A context window to assume for a provider when the caller does not say. These are
/// floors, not facts about a particular model — a model with a larger window loses
/// nothing but headroom, one with a smaller one should be told (`--window`).
pub fn default_window(provider: &str) -> u64 {
    match provider {
        "anthropic" => 200_000,
        "gemini" => 1_000_000,
        "ollama" => 32_000,
        _ => 128_000,
    }
}

/// The thinking levels a provider's models take, in menu order. `Default` (send nothing and let
/// the model decide) is always there as well and is not listed. These are the provider's own
/// notion; a model that does not think ignores or rejects the parameter.
///
/// Ollama differs by model and is asked of the server instead (`/api/show`, see
/// `connection::ollama_thinking_levels`); the guess here is for when it cannot be: `gpt-oss`
/// takes the named levels, the others on or off.
pub fn thinking_levels(provider: &str, model: &str) -> &'static [&'static str] {
    match provider {
        "openai" => &["low", "medium", "high"],
        "ollama" if model.contains("gpt-oss") => &["low", "medium", "high"],
        "ollama" => &["off", "on"],
        "anthropic" | "gemini" | "openrouter" => &["off", "low", "medium", "high"],
        _ => &[],
    }
}

/// The request parameters that set `level` on `provider`, or `None` for no level (or one the
/// provider does not take).
pub fn thinking_params(
    provider: &str,
    model: &str,
    level: Option<&str>,
) -> Option<serde_json::Value> {
    use serde_json::json;
    // Ollama's levels are what the server says for the model; the caller has checked them.
    let level = level.filter(|l| {
        if provider == "ollama" {
            ["off", "on", "low", "medium", "high"].contains(l)
        } else {
            thinking_levels(provider, model).contains(l)
        }
    })?;
    Some(match (provider, level) {
        ("anthropic", "off") => json!({"thinking": {"type": "disabled"}}),
        // The budget must stay under the response's `max_tokens`.
        ("anthropic", l) => {
            let budget = match l {
                "low" => 1024,
                "medium" => 4096,
                _ => 8000,
            };
            json!({"thinking": {"type": "enabled", "budget_tokens": budget}})
        }
        ("openai", l) => json!({"reasoning": {"effort": l}}),
        ("gemini", l) => {
            let budget = match l {
                "off" => 0,
                "low" => 1024,
                "medium" => 8192,
                _ => 24576,
            };
            json!({"generationConfig": {"thinkingConfig": {"thinkingBudget": budget}}})
        }
        ("openrouter", "off") => json!({"reasoning": {"enabled": false}}),
        ("openrouter", l) => json!({"reasoning": {"effort": l}}),
        ("ollama", "off") => json!({"think": false}),
        ("ollama", "on") => json!({"think": true}),
        ("ollama", l) => json!({"think": l}),
        _ => return None,
    })
}

impl AnyLlm {
    /// `anthropic/claude-…`, `openai/gpt-…`, `ollama/llama3`, …
    pub fn from_spec(spec: &str) -> Result<(AnyLlm, String)> {
        Self::from_spec_thinking(spec, None)
    }

    /// As [`AnyLlm::from_spec`], with a thinking level (`off`, `low`, `medium`, `high`) sent on
    /// every request. `None` sends nothing.
    pub fn from_spec_thinking(spec: &str, thinking: Option<&str>) -> Result<(AnyLlm, String)> {
        Self::from_spec_with(spec, thinking, &Options::default())
    }

    /// As [`AnyLlm::from_spec_thinking`], with the provider options the settings choose.
    pub fn from_spec_with(
        spec: &str,
        thinking: Option<&str>,
        options: &Options,
    ) -> Result<(AnyLlm, String)> {
        let (provider, model) = spec.split_once('/').ok_or_else(|| {
            Error::Model(format!(
                "model must be provider/name, e.g. anthropic/claude-sonnet-5-5 (providers: {})",
                PROVIDERS.join(", ")
            ))
        })?;
        let fail =
            |e: rig_core::client::ProviderClientError| Error::Model(format!("{provider}: {e}"));
        let llm = match provider {
            "anthropic" => AnyLlm::Anthropic(
                RigLlm::new(
                    // Anthropic caches nothing unless asked: every request would otherwise pay
                    // in full for a prompt that is the last one plus a few lines. The break
                    // point moves forward as the conversation grows (five minutes by default).
                    {
                        let m = anthropic::Client::from_env()
                            .map_err(fail)?
                            .completion_model(model)
                            .with_automatic_caching();
                        // The system prompt and tool definitions change least; an hour keeps
                        // them across the pauses of a long case (ADR-0059).
                        if options.prefix_one_hour {
                            m.with_static_prefix_cache_ttl(anthropic::completion::CacheTtl::OneHour)
                        } else {
                            m
                        }
                    },
                    spec,
                )
                // Its input count leaves the cached part out, which the budget needs in.
                .with_cache_counted_apart(true)
                .with_params(thinking_params(provider, model, thinking)),
            ),
            "openai" => AnyLlm::OpenAi(RigLlm::new(
                openai::Client::from_env()
                    .map_err(fail)?
                    .completion_model(model),
                spec,
            )
            .with_params(thinking_params(provider, model, thinking))),
            "gemini" => AnyLlm::Gemini(RigLlm::new(
                gemini::Client::from_env()
                    .map_err(fail)?
                    .completion_model(model),
                spec,
            )
            .with_params(thinking_params(provider, model, thinking))),
            "openrouter" => AnyLlm::OpenRouter(RigLlm::new(
                openrouter::Client::from_env()
                    .map_err(fail)?
                    .completion_model(model),
                spec,
            )
            .with_params(thinking_params(provider, model, thinking))),
            "ollama" => AnyLlm::Ollama(RigLlm::new(
                ollama::Client::from_env()
                    .map_err(fail)?
                    .completion_model(model),
                spec,
            )
            // Without it Ollama serves its own default window and silently cuts the prompt.
            .with_num_ctx(true)
            .with_params(thinking_params(provider, model, thinking))),
            other => {
                return Err(Error::Model(format!(
                    "unknown provider `{other}` (providers: {})",
                    PROVIDERS.join(", ")
                )));
            }
        };
        Ok((llm, provider.to_string()))
    }
}

impl Llm for AnyLlm {
    async fn complete(
        &self,
        req: LlmRequest,
        on_delta: &mut (dyn FnMut(Delta) + Send),
    ) -> Result<Reply> {
        match self {
            AnyLlm::Anthropic(m) => m.complete(req, on_delta).await,
            AnyLlm::OpenAi(m) => m.complete(req, on_delta).await,
            AnyLlm::Gemini(m) => m.complete(req, on_delta).await,
            AnyLlm::OpenRouter(m) => m.complete(req, on_delta).await,
            AnyLlm::Ollama(m) => m.complete(req, on_delta).await,
            AnyLlm::Missing(why) => Err(Error::Model(why.clone())),
        }
    }

    fn name(&self) -> String {
        match self {
            AnyLlm::Anthropic(m) => m.name(),
            AnyLlm::OpenAi(m) => m.name(),
            AnyLlm::Gemini(m) => m.name(),
            AnyLlm::OpenRouter(m) => m.name(),
            AnyLlm::Ollama(m) => m.name(),
            AnyLlm::Missing(_) => "none".to_string(),
        }
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    #[test]
    fn a_level_becomes_the_providers_own_parameter() {
        let p = thinking_params("anthropic", "m", Some("low")).unwrap();
        assert_eq!(p["thinking"]["budget_tokens"], 1024);
        assert_eq!(thinking_params("ollama", "llama3", Some("off")).unwrap()["think"], false);
        assert_eq!(
            thinking_params("openai", "m", Some("high")).unwrap()["reasoning"]["effort"],
            "high"
        );
    }

    #[test]
    fn no_level_or_one_the_provider_lacks_sends_nothing() {
        assert!(thinking_params("anthropic", "m", None).is_none());
        assert!(thinking_params("openai", "m", Some("off")).is_none());
        assert_eq!(
            thinking_params("ollama", "qwen3", Some("on")).unwrap()["think"],
            true
        );
        assert_eq!(
            thinking_params("ollama", "gpt-oss:20b", Some("high")).unwrap()["think"],
            "high"
        );
        assert!(thinking_params("ollama", "x", Some("bogus")).is_none());
        assert!(thinking_params("nowhere", "m", Some("high")).is_none());
    }
}
