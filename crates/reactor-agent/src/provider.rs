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

/// Any of the wired-up providers.
#[derive(Clone)]
pub enum AnyLlm {
    Anthropic(RigLlm<AnthropicModel>),
    OpenAi(RigLlm<OpenAiModel>),
    Gemini(RigLlm<GeminiModel>),
    OpenRouter(RigLlm<OpenRouterModel>),
    Ollama(RigLlm<OllamaModel>),
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

impl AnyLlm {
    /// `anthropic/claude-…`, `openai/gpt-…`, `ollama/llama3`, …
    pub fn from_spec(spec: &str) -> Result<(AnyLlm, String)> {
        let (provider, model) = spec
            .split_once('/')
            .ok_or_else(|| Error::Model(format!("model must be provider/name, e.g. anthropic/claude-sonnet-5-5 (providers: {})", PROVIDERS.join(", "))))?;
        let fail = |e: rig_core::client::ProviderClientError| Error::Model(format!("{provider}: {e}"));
        let llm = match provider {
            "anthropic" => AnyLlm::Anthropic(RigLlm::new(anthropic::Client::from_env().map_err(fail)?.completion_model(model), spec)),
            "openai" => AnyLlm::OpenAi(RigLlm::new(openai::Client::from_env().map_err(fail)?.completion_model(model), spec)),
            "gemini" => AnyLlm::Gemini(RigLlm::new(gemini::Client::from_env().map_err(fail)?.completion_model(model), spec)),
            "openrouter" => AnyLlm::OpenRouter(RigLlm::new(openrouter::Client::from_env().map_err(fail)?.completion_model(model), spec)),
            "ollama" => AnyLlm::Ollama(RigLlm::new(ollama::Client::from_env().map_err(fail)?.completion_model(model), spec)),
            other => return Err(Error::Model(format!("unknown provider `{other}` (providers: {})", PROVIDERS.join(", ")))),
        };
        Ok((llm, provider.to_string()))
    }
}

impl Llm for AnyLlm {
    async fn complete(&self, req: LlmRequest, on_delta: &mut (dyn FnMut(Delta) + Send)) -> Result<Reply> {
        match self {
            AnyLlm::Anthropic(m) => m.complete(req, on_delta).await,
            AnyLlm::OpenAi(m) => m.complete(req, on_delta).await,
            AnyLlm::Gemini(m) => m.complete(req, on_delta).await,
            AnyLlm::OpenRouter(m) => m.complete(req, on_delta).await,
            AnyLlm::Ollama(m) => m.complete(req, on_delta).await,
        }
    }

    fn name(&self) -> String {
        match self {
            AnyLlm::Anthropic(m) => m.name(),
            AnyLlm::OpenAi(m) => m.name(),
            AnyLlm::Gemini(m) => m.name(),
            AnyLlm::OpenRouter(m) => m.name(),
            AnyLlm::Ollama(m) => m.name(),
        }
    }
}
