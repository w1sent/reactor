//! The case's sessions as the Session Tree panel lists them, so that an old one can be
//! continued after the case is opened again.
//!
//! Opening a case starts a session, so most of what is stored is sessions in which nothing
//! was ever said: they are left out, except the one that is open.

use reactor_db::{Case, sessions};

/// One session of the case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    /// The first thing said in it, in a few words; empty if nothing was.
    pub prompt: String,
    /// When it was started, in local time: `2026-10-04 14:30`.
    pub started: String,
    pub entries: usize,
    /// The window's active session: the one whose transcript was last in focus.
    pub current: bool,
    /// Open in this window, in a transcript tab (or with its tab closed).
    pub open_here: bool,
    /// Another window has it open: it cannot be opened here until that one lets go.
    pub open_elsewhere: bool,
}

/// How a session is named inside its case, for a title or a status line: `#3` for the third
/// session. A session made before numbering has an id of another kind (a time and a process
/// id), shown by what tells it from the others: the part after its last `-`.
pub fn short_id(id: &str) -> String {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        return format!("#{id}");
    }
    match id.rsplit_once('-') {
        Some((_, tail)) if !tail.is_empty() => tail.to_string(),
        _ => {
            let n = id.chars().count();
            id.chars().skip(n.saturating_sub(8)).collect()
        }
    }
}

/// A session named across cases: `<case id>#<index>`. Inside its case `#<index>` is enough.
pub fn reference(case_id: &str, session: &str) -> String {
    format!("{case_id}#{session}")
}

/// The session id in a reference, whichever form it is given in: `3`, `#3` or
/// `<case id>#3`.
pub fn parse_reference(text: &str) -> String {
    text.trim()
        .rsplit_once('#')
        .map_or(text.trim(), |(_, id)| id)
        .to_string()
}

/// An RFC 3339 time as local `YYYY-MM-DD HH:MM`; what cannot be read is shown as it is.
pub fn local_stamp(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

/// The case's sessions, newest first, without the empty ones (but with those open in this
/// window, whatever they hold; `current` is the active one among them). Whether another
/// window has one is asked of the lock it would hold.
pub fn session_rows(case: &Case, current: &str, open_here: &[String]) -> Vec<SessionRow> {
    sessions::list(case.db())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| open_here.contains(&s.id) || s.first_prompt.is_some())
        .map(|s| {
            let is_current = s.id == current;
            let here = open_here.contains(&s.id);
            SessionRow {
                // The lock is let go as soon as it is known to be free.
                open_elsewhere: !here && case.lock_session(&s.id).is_err(),
                open_here: here,
                current: is_current,
                prompt: s
                    .first_prompt
                    .as_deref()
                    .map(|p| crate::backend::first_words(p, 90))
                    .unwrap_or_default(),
                started: local_stamp(&s.created_at),
                entries: s.entries,
                id: s.id,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactor_db::sessions::Row;

    fn say(case: &Case, id: &str, text: Option<&str>) {
        let first = match text {
            Some(t) => Row {
                id: 0,
                parent: None,
                ts: 1,
                kind: "user".into(),
                body: serde_json::json!({ "text": t }).to_string(),
                blob_sha256: None,
            },
            None => Row {
                id: 0,
                parent: None,
                ts: 1,
                kind: "custom".into(),
                body: "{}".into(),
                blob_sha256: None,
            },
        };
        sessions::create(case.db(), id, 1, first).unwrap();
    }

    #[test]
    fn the_list_has_what_was_said_and_the_open_session_and_marks_the_one_in_another_window() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), None).unwrap();
        say(&case, "a", Some("Look at the dropper"));
        say(&case, "b", None);
        say(&case, "c", Some("What does the config say?\nmore"));
        say(&case, "d", None);

        // Another window has `c`.
        let held = case.lock_session("c").unwrap();
        let rows = session_rows(&case, "d", &["d".to_string()]);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        // Empty `b` is left out; empty `d` is the open one and stays.
        assert_eq!(rows.len(), 3, "{ids:?}");
        assert!(!ids.contains(&"b") && ids.contains(&"a") && ids.contains(&"d"));
        let by = |id: &str| rows.iter().find(|r| r.id == id).unwrap();
        assert!(by("d").current && by("d").open_here && !by("d").open_elsewhere);
        assert!(by("c").open_elsewhere && !by("c").current);
        assert_eq!(by("c").prompt, "What does the config say?");
        assert!(!by("a").open_elsewhere);
        assert_eq!(by("a").entries, 1);
        // Asking did not take the sessions: `a` can still be claimed, `c` is still held.
        assert!(case.lock_session("a").is_ok());
        drop(held);
        assert!(case.lock_session("c").is_ok());
    }

    #[test]
    fn a_session_is_named_by_its_number_and_across_cases_by_the_case_and_the_number() {
        assert_eq!(short_id("3"), "#3");
        assert_eq!(short_id("12"), "#12");
        assert_eq!(reference("case-1", "3"), "case-1#3");
        for form in ["3", "#3", " #3 ", "01a1-7fee#3"] {
            assert_eq!(parse_reference(form), "3", "{form}");
        }
        assert_eq!(parse_reference("1759581234-4242"), "1759581234-4242");
        assert_eq!(short_id("1759581234-a1b2c3"), "a1b2c3");
        assert_eq!(short_id("1759581234-4242"), "4242");
        assert_eq!(short_id("abcdefghijkl"), "efghijkl");
        assert_eq!(short_id("ab"), "ab");
        assert_eq!(short_id("x-"), "x-");
    }

    #[test]
    fn a_time_is_shown_in_local_form_and_an_unreadable_one_as_it_is() {
        assert_eq!(local_stamp("not a time"), "not a time");
        let s = local_stamp("2026-10-04T12:30:00Z");
        assert_eq!(s.len(), 16);
        assert!(s.starts_with("2026-10-0"), "{s}");
    }
}
