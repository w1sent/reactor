//! The notification history behind the bell (SPEC.md §6.0c).
//!
//! The popups themselves are gpui-kit's notification component (`ReactorApp::flush_toasts` pushes
//! them); what this keeps is what that does not: a list to look back through, newest first, and a
//! count of what has not been looked at yet. Model only — no windowing — so the rules are testable.

/// How many notices the history keeps.
pub const HISTORY_CAP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    pub fn from_kind(kind: &str) -> Level {
        match kind {
            "error" => Level::Error,
            "warning" => Level::Warning,
            _ => Level::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: u64,
    pub level: Level,
    pub message: String,
    /// Local wall-clock time, `HH:MM:SS`, as the history shows it.
    pub time: String,
    /// Local date, `YYYY-MM-DD`; the history adds it for notices from another day.
    pub date: String,
}

#[derive(Debug, Default)]
pub struct Notifier {
    history: Vec<Notice>,
    next_id: u64,
    unread: usize,
}

impl Notifier {
    /// Add a notice to the history. Returns its id.
    pub fn push(&mut self, level: Level, message: impl Into<String>, date: String, time: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.history.push(Notice { id, level, message: message.into(), time, date });
        if self.history.len() > HISTORY_CAP {
            self.history.remove(0);
        }
        self.unread += 1;
        id
    }

    /// Everything kept, newest first.
    pub fn history(&self) -> impl Iterator<Item = &Notice> {
        self.history.iter().rev()
    }

    /// How the history shows a notice's time: the time alone for today's, with the date for any
    /// other day.
    pub fn stamp(notice: &Notice, today: &str) -> String {
        if notice.date == today { notice.time.clone() } else { format!("{} {}", notice.date, notice.time) }
    }

    pub fn unread(&self) -> usize {
        self.unread
    }

    pub fn mark_read(&mut self) {
        self.unread = 0;
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.unread = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(n: &mut Notifier, msg: &str) -> u64 {
        n.push(Level::Info, msg, "2026-01-02".into(), "12:00:00".into())
    }

    #[test]
    fn the_history_is_newest_first() {
        let mut n = Notifier::default();
        push(&mut n, "first");
        push(&mut n, "second");
        let order: Vec<&str> = n.history().map(|x| x.message.as_str()).collect();
        assert_eq!(order, ["second", "first"]);
    }

    #[test]
    fn unread_counts_until_the_history_is_opened() {
        let mut n = Notifier::default();
        push(&mut n, "a");
        push(&mut n, "b");
        assert_eq!(n.unread(), 2);
        n.mark_read();
        assert_eq!(n.unread(), 0);
        push(&mut n, "c");
        assert_eq!(n.unread(), 1);
    }

    #[test]
    fn the_history_is_capped_dropping_the_oldest() {
        let mut n = Notifier::default();
        for i in 0..HISTORY_CAP + 5 {
            push(&mut n, &format!("n{i}"));
        }
        assert_eq!(n.history().count(), HISTORY_CAP);
        assert_eq!(n.history().last().unwrap().message, "n5");
    }

    #[test]
    fn a_notice_from_another_day_shows_its_date() {
        let mut n = Notifier::default();
        push(&mut n, "a");
        let notice = n.history().next().unwrap();
        assert_eq!(Notifier::stamp(notice, "2026-01-02"), "12:00:00");
        assert_eq!(Notifier::stamp(notice, "2026-01-03"), "2026-01-02 12:00:00");
    }

    #[test]
    fn clear_empties_it() {
        let mut n = Notifier::default();
        push(&mut n, "a");
        n.clear();
        assert!(n.is_empty() && n.unread() == 0);
    }
}
