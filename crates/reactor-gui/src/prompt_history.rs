//! Up and down in the prompt: the messages sent before, and the text that was being typed.
//!
//! Walking back saves what was in the prompt (the *draft*) the first time, so walking forward past
//! the newest message puts it back. This is only the bookkeeping; whether an arrow key means
//! "move the cursor" or "walk the history" is decided by where the cursor is (`app.rs`).

#[derive(Debug, Default)]
pub struct PromptHistory {
    items: Vec<String>,
    /// Which item the prompt shows; `None` is the person's own text.
    pos: Option<usize>,
    draft: String,
}

impl PromptHistory {
    pub fn from_messages(items: impl IntoIterator<Item = String>) -> Self {
        let mut h = PromptHistory::default();
        for m in items {
            h.remember(&m);
        }
        h
    }

    /// Keep a message that was sent, and go back to editing fresh text. The same message twice
    /// in a row is kept once.
    pub fn remember(&mut self, text: &str) {
        if self.items.last().map(String::as_str) != Some(text) {
            self.items.push(text.to_string());
        }
        self.pos = None;
        self.draft.clear();
    }

    /// Step back from `current` (what the prompt holds now). `None` when there is nothing older.
    pub fn prev(&mut self, current: &str) -> Option<&str> {
        let next = match self.pos {
            None => {
                if self.items.is_empty() {
                    return None;
                }
                self.draft = current.to_string();
                self.items.len() - 1
            }
            Some(0) => return None,
            Some(i) => i - 1,
        };
        self.pos = Some(next);
        Some(&self.items[next])
    }

    /// Step forward: the next message, then the draft. `None` when not walking.
    pub fn next(&mut self) -> Option<String> {
        let at = self.pos?;
        if at + 1 < self.items.len() {
            self.pos = Some(at + 1);
            Some(self.items[at + 1].clone())
        } else {
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    pub fn is_walking(&self) -> bool {
        self.pos.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(items: &[&str]) -> PromptHistory {
        PromptHistory::from_messages(items.iter().map(|s| s.to_string()))
    }

    #[test]
    fn up_walks_back_through_what_was_sent_and_stops_at_the_oldest() {
        let mut h = h(&["one", "two", "three"]);
        assert_eq!(h.prev(""), Some("three"));
        assert_eq!(h.prev("three"), Some("two"));
        assert_eq!(h.prev("two"), Some("one"));
        assert_eq!(h.prev("one"), None, "nothing older");
        assert!(h.is_walking());
    }

    #[test]
    fn down_walks_forward_and_ends_on_what_was_being_typed() {
        let mut h = h(&["one", "two"]);
        h.prev("half a thought");
        h.prev("two");
        assert_eq!(h.next().as_deref(), Some("two"));
        assert_eq!(h.next().as_deref(), Some("half a thought"), "the draft comes back");
        assert!(!h.is_walking());
        assert_eq!(h.next(), None, "down does nothing when not walking");
    }

    #[test]
    fn the_draft_is_the_text_at_the_moment_of_the_first_step_back() {
        let mut h = h(&["one"]);
        h.prev("first draft");
        h.next();
        h.prev("second draft");
        assert_eq!(h.next().as_deref(), Some("second draft"));
    }

    #[test]
    fn sending_ends_the_walk_and_a_repeat_is_kept_once() {
        let mut h = h(&["one", "two"]);
        h.prev("");
        h.remember("two");
        assert!(!h.is_walking());
        assert_eq!(h.prev(""), Some("two"));
        assert_eq!(h.prev("two"), Some("one"), "sending `two` again did not add a second `two`");
    }

    #[test]
    fn an_empty_history_has_nothing_to_show() {
        let mut h = h(&[]);
        assert_eq!(h.prev("x"), None);
        assert!(h.is_empty() && !h.is_walking());
    }
}
