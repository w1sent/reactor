//! The little XML reading the formats share.

use quick_xml::events::BytesRef;

/// Add the character an entity or character reference stands for. An entity this does not
/// know (a DTD's own) is kept as written, so the text is not silently shortened.
pub(crate) fn push_ref(out: &mut String, r: &BytesRef) {
    match r.resolve_char_ref() {
        Ok(Some(c)) => out.push(c),
        _ => match &**r {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "apos" => out.push('\''),
            "quot" => out.push('"'),
            other => {
                out.push('&');
                out.push_str(other);
                out.push(';');
            }
        },
    }
}

/// A tag's local name (`w:p` → `p`) as text.
pub(crate) fn local(name: quick_xml::name::LocalName<'_>) -> String {
    name.as_ref().to_string()
}
