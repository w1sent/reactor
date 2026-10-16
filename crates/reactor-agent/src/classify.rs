//! What a failed or cut-off model call was, so the loop can answer it
//! ([ADR-0057](../../../docs/adr/0057-a-turn-recovers-from-a-rejected-or-cut-off-request.md)).
//!
//! Providers say these things in free text. The phrases below are the ones the providers
//! REactor talks to are known to use; the list is meant to grow one line at a time when
//! a provider is found that says it differently. Matching is on the lowercased message
//! with underscores read as spaces, so `context_length_exceeded` and
//! `context length exceeded` are one phrase.

use crate::error::Error;

/// Phrases of a provider rejecting a request for its size.
const OVERFLOW: &[&str] = &[
    "prompt is too long",
    "prompt too long",
    "prompt exceeds max length",
    "request too large",
    "request exceeds the maximum size",
    "exceeds the context window",
    "exceeds the model's maximum context length",
    "exceeds model's maximum context length",
    "maximum context length is",
    "context length exceeded",
    "context window exceeds",
    "model context window exceeded",
    "exceeds the available context size",
    "greater than the context length",
    "exceeded max context length",
    "exceeded model token limit",
    "maximum prompt length",
    "reduce the length of the messages",
    "input is too long",
    "input token count",
    "is longer than the model's context length",
    "too large for model",
    "range of input length should be",
    "token limit exceeded",
    "too many tokens",
    "prompt token count",
    "configured context size",
];

/// Phrases that look like [`OVERFLOW`] but are about rate, not size.
const NOT_OVERFLOW: &[&str] = &["rate limit", "too many requests", "throttl"];

/// Phrases of a provider's policy stopping a request or its reply.
const BLOCKED: &[&str] = &[
    "content filter",
    "content policy",
    "cyber policy",
    "policy violation",
    "usage polic",
    "safety system",
    "was flagged",
    "flagged for",
];

/// Phrases of a failure worth another try after a pause.
const TRANSIENT: &[&str] = &[
    "rate limit",
    "throttl",
    "too many requests",
    "overloaded",
    "temporarily unavailable",
    "service unavailable",
    "internal server error",
    "bad gateway",
    "gateway timeout",
    "server error",
    "timed out",
    "timeout",
    "connection reset",
    "connection closed",
    "broken pipe",
    "unexpected eof",
    "error decoding response body",
    "stream closed",
    "no output from the model",
    "try again",
    "at capacity",
];

/// HTTP statuses that mean "try again".
const TRANSIENT_STATUS: &[&str] = &["429", "500", "502", "503", "504", "529"];

fn normalized(message: &str) -> String {
    message.to_lowercase().replace('_', " ")
}

fn has_any(text: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| text.contains(p))
}

/// Whether `text` holds one of `codes` as a number of its own, in a message that talks
/// about a status at all (so "maximum 500 tokens" is not a server error).
fn has_status(text: &str, codes: &[&str]) -> bool {
    if !["status", "http", "code", "error"]
        .iter()
        .any(|w| text.contains(w))
    {
        return false;
    }
    let bytes = text.as_bytes();
    codes.iter().any(|code| {
        text.match_indices(code).any(|(i, _)| {
            let before = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after = bytes
                .get(i + code.len())
                .is_none_or(|b| !b.is_ascii_digit());
            before && after
        })
    })
}

/// Whether a provider's message says the request was too long for the model.
pub fn is_overflow(message: &str) -> bool {
    let text = normalized(message);
    has_any(&text, OVERFLOW) && !has_any(&text, NOT_OVERFLOW)
}

/// Whether a provider's message says its policy stopped the request.
pub fn is_blocked(message: &str) -> bool {
    has_any(&normalized(message), BLOCKED)
}

/// Whether a failure is worth another try after a pause.
pub fn is_transient(message: &str) -> bool {
    let text = normalized(message);
    has_any(&text, TRANSIENT) || has_status(&text, TRANSIENT_STATUS)
}

/// The error a provider's message stands for.
pub fn classify(message: String) -> Error {
    if is_overflow(&message) {
        Error::Overflow(message)
    } else if is_blocked(&message) {
        Error::Blocked(message)
    } else if is_transient(&message) {
        Error::Transient(message)
    } else {
        Error::Model(message)
    }
}

/// Why a reply ended, as far as the loop cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopKind {
    /// The model finished.
    Stop,
    /// The model stopped to have tools run.
    ToolUse,
    /// The reply hit the output limit: whatever it was saying is unfinished.
    Length,
    /// The provider filtered the reply.
    Filtered,
    /// Anything else, or nothing at all.
    Other,
}

impl StopKind {
    /// Read the label a reply carries (`Reply::stop`): rig's finish reason lowercased, or a
    /// provider's own spelling.
    pub fn from_label(label: Option<&str>) -> StopKind {
        let Some(label) = label else {
            return StopKind::Other;
        };
        let label = normalized(label);
        let label = label.trim();
        match label {
            "length" | "max tokens" | "maxtokens" | "max output tokens" => StopKind::Length,
            "contentfilter" | "content filter" | "refusal" | "safety" | "filtered" => {
                StopKind::Filtered
            }
            "toolcalls" | "tool calls" | "tool use" | "tooluse" => StopKind::ToolUse,
            "stop" | "end turn" | "endturn" => StopKind::Stop,
            _ if label.starts_with("other") => StopKind::Other,
            _ => StopKind::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_that_say_the_request_was_too_long_are_overflows() {
        for m in [
            "prompt is too long: 213462 tokens > 200000 maximum",
            "Your input exceeds the context window of this model",
            "This endpoint's maximum context length is 32768 tokens. However, you requested about 40000",
            "the request exceeds the available context size, try increasing it",
            "invalid_request_error: context_length_exceeded",
            "Prompt has 5000 tokens, but the configured context size is 4096 tokens",
            "prompt too long; exceeded max context length by 1204 tokens",
        ] {
            assert!(matches!(classify(m.into()), Error::Overflow(_)), "{m}");
        }
    }

    #[test]
    fn a_rate_limit_that_mentions_tokens_is_not_an_overflow() {
        let e = classify(
            "ThrottlingException: Too many tokens, please wait before trying again".into(),
        );
        assert!(matches!(e, Error::Transient(_)), "{e:?}");
        let e = classify("429 rate limit: too many tokens per minute".into());
        assert!(matches!(e, Error::Transient(_)), "{e:?}");
    }

    #[test]
    fn a_policy_block_is_never_retried_unchanged() {
        for m in [
            "This request was flagged for cyber policy.",
            "response blocked by content_filter",
        ] {
            assert!(matches!(classify(m.into()), Error::Blocked(_)), "{m}");
        }
    }

    #[test]
    fn servers_that_are_busy_or_unreachable_are_transient() {
        for m in [
            "HTTP status 503 Service Unavailable",
            "error: 529 overloaded_error",
            "request timed out",
            "connection reset by peer",
            "no output from the model for 300s",
        ] {
            assert!(matches!(classify(m.into()), Error::Transient(_)), "{m}");
        }
    }

    #[test]
    fn a_number_in_a_message_is_not_a_status_unless_the_message_is_about_one() {
        assert!(matches!(
            classify("the model gpt-5002 does not exist".into()),
            Error::Model(_)
        ));
        assert!(matches!(
            classify("invalid api key".into()),
            Error::Model(_)
        ));
        assert!(matches!(
            classify("you may set at most 500 tools".into()),
            Error::Model(_)
        ));
    }

    #[test]
    fn stop_labels_read_the_way_rig_and_providers_spell_them() {
        let k = |s| StopKind::from_label(Some(s));
        assert_eq!(k("length"), StopKind::Length);
        assert_eq!(k("max_tokens"), StopKind::Length);
        assert_eq!(k("toolcalls"), StopKind::ToolUse);
        assert_eq!(k("tool_use"), StopKind::ToolUse);
        assert_eq!(k("contentfilter"), StopKind::Filtered);
        assert_eq!(k("stop"), StopKind::Stop);
        assert_eq!(k("end_turn"), StopKind::Stop);
        assert_eq!(k("other(\"weird\")"), StopKind::Other);
        assert_eq!(StopKind::from_label(None), StopKind::Other);
    }
}
