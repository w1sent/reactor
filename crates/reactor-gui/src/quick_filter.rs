//! The rule behind the search boxes in the Tools and Toolsets panels: every word typed must
//! be in the entry, in any of its fields, whatever the case. Pure, so it is tested alone.

/// Whether an entry with these `fields` (its id, its description, its tags …) matches what was
/// typed: every word of `query` is somewhere in them. An empty query matches everything.
pub fn matches(query: &str, fields: &[&str]) -> bool {
    let haystack: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .all(|word| haystack.iter().any(|f| f.contains(&word)))
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn every_word_must_be_somewhere_in_the_entry_whatever_the_case() {
        let ghidra = [
            "ghidra",
            "Ghidra: a software reverse engineering suite",
            "re disasm decompile",
        ];
        assert!(matches("", &ghidra), "an empty query is every entry");
        assert!(matches("   ", &ghidra));
        assert!(matches("GHI", &ghidra), "a part of a word");
        assert!(
            matches("reverse suite", &ghidra),
            "words from the same field"
        );
        assert!(
            matches("disasm engineering", &ghidra),
            "words from different fields"
        );
        assert!(
            !matches("reverse nothing", &ghidra),
            "one word missing is no match"
        );
        assert!(!matches("x", &[]), "no fields, no match");
    }
}
