//! Identity: the one hard rule among the graph's conventions
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! The same thing from two pulls must land on the same node, or nothing correlates. So
//! an id is derived, never assigned: from a type's normalized key fields, else from
//! where the element came from, else — for something a person typed with no key — a
//! random id that is stored and never made again.
//!
//! Types and their normalizers are rows (`known_type`, `type_alias`); a normalizer is
//! one of the functions here, named by a row. An unknown type matches on the exact
//! string.

use std::net::IpAddr;

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::Kind;
use crate::error::{Error, Result};

/// A stable id: the kind, then 32 hex digits of a SHA-256 over the parts.
pub fn derived_id(kind: Kind, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for part in parts {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part.as_bytes());
    }
    let digest = h.finalize();
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{}--{hex}", kind.as_str())
}

/// An id for something with no key and no source: random, time-ordered.
pub fn random_id(kind: Kind) -> String {
    format!("{}--{}", kind.as_str(), uuid::Uuid::now_v7().simple())
}

/// A type name as it is stored: trimmed, lowercase, and an alias replaced by what it
/// stands for (`ipv4` → `net.ipv4`).
pub fn canonical_type(conn: &Connection, raw: &str) -> Result<String> {
    let name = raw.trim().to_lowercase();
    if name.is_empty() {
        return Err(Error::Invalid("an element needs a type".into()));
    }
    let alias: Option<String> = conn
        .query_row(
            "SELECT name FROM type_alias WHERE alias = ?1",
            [&name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(alias.unwrap_or(name))
}

/// What a known type says about identity.
pub struct TypeRule {
    pub normalizer: Option<String>,
    /// The fields whose values make the identity. `value` is the element's value; any
    /// other name is a property.
    pub key_fields: Vec<String>,
}

pub fn type_rule(conn: &Connection, type_: &str) -> Result<TypeRule> {
    let row: Option<(Option<String>, String)> = conn
        .query_row(
            "SELECT normalizer, key_fields FROM known_type WHERE name = ?1",
            [type_],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((normalizer, keys)) => TypeRule {
            normalizer,
            key_fields: serde_json::from_str(&keys).unwrap_or_else(|_| vec!["value".into()]),
        },
        None => TypeRule {
            normalizer: None,
            key_fields: vec!["value".into()],
        },
    })
}

/// The canonical form of a value. Returns the type too: an IPv4-mapped IPv6 address
/// *is* an IPv4 address, and must land on that node.
pub fn normalize(normalizer: Option<&str>, type_: &str, value: &str) -> (String, String) {
    let trimmed = value.trim();
    match normalizer {
        Some("lowercase") => (type_.to_string(), trimmed.to_lowercase()),
        Some("uppercase") => (type_.to_string(), trimmed.to_uppercase()),
        Some("domain") => (
            type_.to_string(),
            trimmed.trim_end_matches('.').to_lowercase(),
        ),
        Some("ip") => match trimmed.parse::<IpAddr>() {
            Ok(IpAddr::V4(v4)) => ("net.ipv4".into(), v4.to_string()),
            Ok(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
                Some(v4) => ("net.ipv4".into(), v4.to_string()),
                None => ("net.ipv6".into(), v6.to_string()),
            },
            // Not an address: kept as written, so nothing is lost and nothing is guessed.
            Err(_) => (type_.to_string(), trimmed.to_string()),
        },
        _ => (type_.to_string(), trimmed.to_string()),
    }
}

/// An external's URI as it is stored, or why it cannot be one
/// ([ADR-0066](../../../../docs/adr/0066-the-case-is-addressed-by-references.md)). A thing
/// of the case is a reference (`artifact:data/raw/vpn.csv#row=12`, `session:3#entry=7`), and a
/// bare relative path is a file of the case. A network URI keeps its `scheme://`, and so does
/// a scheme a program registered ([`crate::reference::register_scheme`]).
pub fn canonical_uri(raw: &str) -> Result<String> {
    let uri = raw.trim().replace('\\', "/");
    if uri.is_empty() {
        return Err(Error::Invalid("an external needs a URI".into()));
    }
    // A reference that names a case: kept as it is. Whether it is this case is for whoever
    // knows the case to say (`reference::localize`); another case's stays another case's.
    if uri.starts_with("reactor://") {
        return Ok(crate::reference::parse(&uri)?.to_string());
    }
    if let Some((scheme, rest)) = uri.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return Err(Error::Invalid(format!(
                "`{raw}`: no file: URIs -- name a file in the case by its relative path"
            )));
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return Err(Error::Invalid(format!(
                "`{raw}`: a URI must not carry credentials"
            )));
        }
        return Ok(uri);
    }
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("file:") {
        return Err(Error::Invalid(format!(
            "`{raw}`: no file: URIs -- name a file in the case by its relative path"
        )));
    }
    let scheme = uri.split_once(':').map(|(s, _)| s).filter(|s| {
        s.len() > 1
            && s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    if let Some(scheme) = scheme {
        if crate::reference::Kind::from_scheme(scheme).is_some() {
            return Ok(crate::reference::parse(&uri)?.short());
        }
        if crate::reference::is_registered(scheme) {
            return Ok(uri);
        }
        let hint = match scheme.to_ascii_lowercase().as_str() {
            "csv" | "tsv" | "jsonl" | "json" | "sqlite" => {
                " -- a file is `artifact:<path>`, with `#table=<t>`, `#/pointer` or `#row=<n>` for a place in it"
            }
            _ => "",
        };
        return Err(Error::Invalid(format!(
            "`{raw}`: `{scheme}:` is not a kind of reference or a registered scheme{hint}"
        )));
    }
    if uri.starts_with('/') || uri.split('/').any(|part| part == "..") {
        return Err(Error::Invalid(format!(
            "`{raw}`: a local resource is named relative to the case and stays inside it"
        )));
    }
    let (path, frag) = match uri.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (uri.as_str(), None),
    };
    let mut r = crate::reference::Reference::artifact(path);
    if let crate::reference::Target::Artifact { locator, .. } = &mut r.target {
        *locator = frag.filter(|f| !f.is_empty()).map(str::to_string);
    }
    Ok(r.short())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_normalized_by_their_types_rule() {
        let n = |f, t, v| normalize(Some(f), t, v);
        assert_eq!(
            n("domain", "net.domain", " CDN-Sync.Example. "),
            ("net.domain".into(), "cdn-sync.example".into())
        );
        assert_eq!(
            n("lowercase", "file.hash.md5", "ABCDEF"),
            ("file.hash.md5".into(), "abcdef".into())
        );
        assert_eq!(
            n("ip", "net.ipv4", "198.51.100.7"),
            ("net.ipv4".into(), "198.51.100.7".into())
        );
        assert_eq!(
            n("ip", "net.ipv6", "2001:DB8:0:0::1"),
            ("net.ipv6".into(), "2001:db8::1".into())
        );
        // A mapped address is the IPv4 address, whatever type it was given.
        assert_eq!(
            n("ip", "net.ipv6", "::ffff:198.51.100.7"),
            ("net.ipv4".into(), "198.51.100.7".into())
        );
        assert_eq!(
            n("ip", "net.ipv4", "not an ip"),
            ("net.ipv4".into(), "not an ip".into())
        );
        assert_eq!(
            normalize(None, "x.custom", " Kept As Is "),
            ("x.custom".into(), "Kept As Is".into())
        );
    }

    #[test]
    fn ids_are_stable_and_do_not_collide_on_concatenation() {
        let a = derived_id(Kind::Entity, &["net.domain", "a.example"]);
        assert_eq!(a, derived_id(Kind::Entity, &["net.domain", "a.example"]));
        assert!(a.starts_with("entity--") && a.len() == "entity--".len() + 32);
        assert_ne!(
            derived_id(Kind::Entity, &["ab", "c"]),
            derived_id(Kind::Entity, &["a", "bc"])
        );
        assert_ne!(random_id(Kind::Annotation), random_id(Kind::Annotation));
    }

    #[test]
    fn a_uri_stays_inside_the_case_or_is_a_network_uri() {
        for ok in [
            "artifact:samples/dropper.bin",
            "artifact:reactor_data.sqlite#table=fw_conn",
            "artifact:.reactor/analysis.db#table=artifact",
            "artifact:data/raw/vpn.csv#row=12",
            "session:1759-1#entry=42",
            "reactor://other/artifact/data/vpn.csv",
            "https://timesketch.internal/sketch/12",
            "smb://fileserver/cases/2026-17/day1.pcap",
        ] {
            assert_eq!(canonical_uri(ok).unwrap(), ok);
        }
        // A bare path is a file of the case.
        assert_eq!(
            canonical_uri(r"data\raw\x.csv").unwrap(),
            "artifact:data/raw/x.csv"
        );
        assert_eq!(
            canonical_uri("data/a b.csv").unwrap(),
            "artifact:data/a%20b.csv"
        );
        for bad in [
            "",
            "file:///etc/passwd",
            "file:notes.txt",
            "/etc/passwd",
            "../other-case/x",
            "artifact:../../x.csv",
            "https://user:pw@host/x",
            "csv:data/raw/vpn.csv#row=12",
            "sqlite:reactor_data.sqlite?table=t",
            "reactor://bad name/artifact/x",
            "nokind:thing",
        ] {
            assert!(canonical_uri(bad).is_err(), "{bad}");
        }
    }
}
