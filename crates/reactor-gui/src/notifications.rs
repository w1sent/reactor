//! The window's notifications: a short-lived popup for each new one, and a history to look back
//! through (SPEC.md §6.4).
//!
//! This is the model only — no windowing — so the rules are testable: a notice is shown as a
//! toast until its timeout passes or it is dismissed, stays in the history either way, and counts
//! as unread until the history has been opened. Time is passed in, never read here.

use std::time::{Duration, Instant};

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
    /// The notices still shown as popups, with when each goes.
    toasts: Vec<(u64, Instant)>,
    next_id: u64,
    unread: usize,
}

impl Notifier {
    /// Add a notice and show it for `timeout`.
    pub fn push(&mut self, level: Level, message: impl Into<String>, date: String, time: String, now: Instant, timeout: Duration) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.history.push(Notice { id, level, message: message.into(), time, date });
        if self.history.len() > HISTORY_CAP {
            let gone = self.history.remove(0);
            self.toasts.retain(|(t, _)| *t != gone.id);
        }
        self.toasts.push((id, now + timeout));
        self.unread += 1;
        id
    }

    /// Drop the popups whose time has come. Returns whether anything went.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|(_, deadline)| *deadline > now);
        self.toasts.len() != before
    }

    /// Close one popup early. The notice stays in the history.
    pub fn dismiss(&mut self, id: u64) {
        self.toasts.retain(|(t, _)| *t != id);
    }

    /// The popups now shown, oldest first.
    pub fn toasts(&self) -> Vec<&Notice> {
        self.toasts.iter().filter_map(|(id, _)| self.history.iter().find(|n| n.id == *id)).collect()
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
        self.toasts.clear();
        self.unread = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_secs(5);

    fn push(n: &mut Notifier, msg: &str, now: Instant) -> u64 {
        n.push(Level::Info, msg, "2026-01-02".into(), "12:00:00".into(), now, T)
    }

    #[test]
    fn a_notice_is_a_popup_until_its_timeout_and_stays_in_the_history() {
        let (mut n, t0) = (Notifier::default(), Instant::now());
        push(&mut n, "one", t0);
        assert_eq!(n.toasts().len(), 1);
        assert!(!n.expire(t0 + Duration::from_secs(4)));
        assert_eq!(n.toasts().len(), 1);
        assert!(n.expire(t0 + Duration::from_secs(5)), "it goes exactly at the timeout");
        assert!(n.toasts().is_empty());
        assert_eq!(n.history().count(), 1, "the history keeps it");
    }

    #[test]
    fn each_notice_has_its_own_timeout() {
        let (mut n, t0) = (Notifier::default(), Instant::now());
        push(&mut n, "early", t0);
        push(&mut n, "late", t0 + Duration::from_secs(3));
        n.expire(t0 + Duration::from_secs(6));
        let shown: Vec<&str> = n.toasts().iter().map(|x| x.message.as_str()).collect();
        assert_eq!(shown, ["late"]);
    }

    #[test]
    fn the_history_is_newest_first_and_dismissing_a_popup_does_not_delete_it() {
        let (mut n, t0) = (Notifier::default(), Instant::now());
        let first = push(&mut n, "first", t0);
        push(&mut n, "second", t0);
        n.dismiss(first);
        assert_eq!(n.toasts().len(), 1);
        let order: Vec<&str> = n.history().map(|x| x.message.as_str()).collect();
        assert_eq!(order, ["second", "first"]);
    }

    #[test]
    fn unread_counts_until_the_history_is_opened() {
        let (mut n, t0) = (Notifier::default(), Instant::now());
        push(&mut n, "a", t0);
        push(&mut n, "b", t0);
        assert_eq!(n.unread(), 2);
        n.mark_read();
        assert_eq!(n.unread(), 0);
        push(&mut n, "c", t0);
        assert_eq!(n.unread(), 1);
    }

    #[test]
    fn the_history_is_capped_and_a_popup_of_a_dropped_notice_goes_with_it() {
        let (mut n, t0) = (Notifier::default(), Instant::now());
        for i in 0..HISTORY_CAP + 5 {
            push(&mut n, &format!("n{i}"), t0);
        }
        assert_eq!(n.history().count(), HISTORY_CAP);
        assert_eq!(n.history().last().unwrap().message, "n5");
        assert_eq!(n.toasts().len(), HISTORY_CAP, "no popup refers to a notice that is gone");
    }

    #[test]
    fn a_notice_from_another_day_shows_its_date() {
        let (mut n, t0) = (Notifier::default(), Instant::now());
        push(&mut n, "a", t0);
        let notice = n.history().next().unwrap();
        assert_eq!(Notifier::stamp(notice, "2026-01-02"), "12:00:00");
        assert_eq!(Notifier::stamp(notice, "2026-01-03"), "2026-01-02 12:00:00");
    }

    #[test]
    fn clear_empties_both() {
        let (mut n, t0) = (Notifier::default(), Instant::now());
        push(&mut n, "a", t0);
        n.clear();
        assert!(n.is_empty() && n.toasts().is_empty() && n.unread() == 0);
    }
}
