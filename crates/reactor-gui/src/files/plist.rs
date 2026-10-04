//! Property lists: Apple's `bplist00` binary format, and the XML form, read into one tree the
//! data viewer shows.
//!
//! The binary format is a table of objects, each a marker byte (a type, and a size or a length)
//! and its bytes, with arrays and dictionaries made of *references* to other objects by number;
//! a trailer at the end says how wide offsets and references are, how many objects there are
//! and which is the top one. Because it is references, a hostile file can refer to itself or
//! share an object a million ways, so reading is bounded: by depth, by the number of nodes
//! produced, and by checking every offset against the file.
//!
//! Pure functions of bytes, tested against files Python's `plistlib` wrote.

use std::fmt::Write as _;

/// Deeper than this is a file that refers to itself, or is not meant to be read.
const MAX_DEPTH: usize = 128;
/// Most nodes a tree may have, counting shared objects each time they are used.
const MAX_NODES: usize = 2_000_000;
/// Seconds from the Unix epoch to 2001-01-01, the epoch of a plist date.
const APPLE_EPOCH: i64 = 978_307_200;

/// A property list value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i128),
    Real(f64),
    /// Seconds since 2001-01-01 00:00:00 UTC.
    Date(f64),
    Data(Vec<u8>),
    /// Data that is itself a property list: the bytes, and what they hold, read.
    Embedded {
        bytes: Vec<u8>,
        inner: Box<Value>,
    },
    String(String),
    /// A keyed-archive object reference (binary only).
    Uid(u64),
    Array(Vec<Value>),
    Set(Vec<Value>),
    /// Keys in file order. A key that is not a string is shown as it would read.
    Dict(Vec<(String, Value)>),
}

impl Value {
    /// What it is, in a word.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "integer",
            Value::Real(_) => "real",
            Value::Date(_) => "date",
            Value::Data(_) | Value::Embedded { .. } => "data",
            Value::String(_) => "string",
            Value::Uid(_) => "uid",
            Value::Array(_) => "array",
            Value::Set(_) => "set",
            Value::Dict(_) => "dict",
        }
    }

    /// The children of an array, set or dictionary, with the key where there is one.
    pub fn children(&self) -> Vec<(Option<&str>, &Value)> {
        match self {
            Value::Array(v) | Value::Set(v) => v.iter().map(|c| (None, c)).collect(),
            Value::Dict(v) => v.iter().map(|(k, c)| (Some(k.as_str()), c)).collect(),
            Value::Embedded { inner, .. } => vec![(Some("↳ property list"), inner.as_ref())],
            _ => Vec::new(),
        }
    }

    pub fn is_container(&self) -> bool {
        matches!(
            self,
            Value::Array(_) | Value::Set(_) | Value::Dict(_) | Value::Embedded { .. }
        )
    }

    /// The value at a path of child positions from this one.
    pub fn at(&self, path: &[usize]) -> Option<&Value> {
        let mut here = self;
        for &i in path {
            here = match here {
                Value::Array(v) | Value::Set(v) => v.get(i)?,
                Value::Dict(v) => &v.get(i)?.1,
                Value::Embedded { inner, .. } if i == 0 => inner,
                _ => return None,
            };
        }
        Some(here)
    }

    /// One line for a row of the tree: the value for a leaf, how many for a container.
    pub fn summary(&self) -> String {
        match self {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Real(r) => r.to_string(),
            Value::Date(d) => date_string(*d),
            Value::Data(d) => format!("{} byte{}: {}", d.len(), plural(d.len()), hex_preview(d)),
            Value::Embedded { bytes, inner } => format!(
                "{} byte{}, a property list: {}",
                bytes.len(),
                plural(bytes.len()),
                inner.summary()
            ),
            Value::String(s) => format!("\"{}\"", one_line(s, 200)),
            Value::Uid(u) => format!("UID({u})"),
            Value::Array(v) | Value::Set(v) => format!("{} item{}", v.len(), plural(v.len())),
            Value::Dict(v) => format!("{} key{}", v.len(), plural(v.len())),
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

fn hex_preview(data: &[u8]) -> String {
    let mut s: String = data
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if data.len() > 16 {
        s.push_str(" …");
    }
    s
}

/// A plist date as an RFC 3339 time in UTC; what cannot be a time is shown as seconds.
pub fn date_string(secs_since_2001: f64) -> String {
    if !secs_since_2001.is_finite() {
        return format!("{secs_since_2001}");
    }
    let whole = secs_since_2001.floor();
    let nanos = ((secs_since_2001 - whole) * 1e9).round() as u32;
    // Far beyond any calendar, or past what the arithmetic holds, is not a time.
    (whole as i64)
        .checked_add(APPLE_EPOCH)
        .and_then(|unix| chrono::DateTime::from_timestamp(unix, nanos.min(999_999_999)))
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("{secs_since_2001} s since 2001"))
}

// -- reading ------------------------------------------------------------------------------------

/// Read a property list, whichever form it is in. Data in it that is a property list too is
/// read, and found as a child of the data (see [`Value::Embedded`]).
pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    parse_one(bytes).map(|v| unwrap_embedded(v, 0))
}

fn parse_one(bytes: &[u8]) -> Result<Value, String> {
    if bytes.starts_with(b"bplist") {
        parse_binary(bytes)
    } else if looks_like_xml_plist(bytes) {
        parse_xml(std::str::from_utf8(bytes).map_err(|_| "the XML is not UTF-8".to_string())?)
    } else {
        Err("not a property list".into())
    }
}

/// How deep property lists inside property lists are followed.
const MAX_NESTING: usize = 6;

/// Replace data that is a property list by the list it holds. A keyed archive inside is
/// resolved too: what is wanted of it is the objects, not the table they are kept in.
fn unwrap_embedded(value: Value, nesting: usize) -> Value {
    match value {
        Value::Data(bytes) if nesting < MAX_NESTING && bytes.len() >= 8 => {
            let inner = if bytes.starts_with(b"bplist") || looks_like_xml_plist(&bytes) {
                parse_one(&bytes).ok()
            } else {
                None
            };
            match inner {
                Some(inner) => {
                    let inner = unwrap_embedded(inner, nesting + 1);
                    let inner = super::archive::resolve(&inner).unwrap_or(inner);
                    Value::Embedded {
                        bytes,
                        inner: Box::new(inner),
                    }
                }
                None => Value::Data(bytes),
            }
        }
        Value::Array(v) => {
            Value::Array(v.into_iter().map(|c| unwrap_embedded(c, nesting)).collect())
        }
        Value::Set(v) => Value::Set(v.into_iter().map(|c| unwrap_embedded(c, nesting)).collect()),
        Value::Dict(v) => Value::Dict(
            v.into_iter()
                .map(|(k, c)| (k, unwrap_embedded(c, nesting)))
                .collect(),
        ),
        other => other,
    }
}

/// A property list as the viewer holds it: as written, and — if it is a keyed archive — what
/// the archive says, with the objects found and the references followed.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub raw: Value,
    pub archive: Option<Value>,
}

impl Document {
    pub fn read(bytes: &[u8]) -> Result<Document, String> {
        let raw = parse(bytes)?;
        let archive = super::archive::resolve(&raw);
        Ok(Document { raw, archive })
    }

    /// What is shown: the archive's objects if it is one and `raw` is not asked for.
    pub fn shown(&self, raw: bool) -> &Value {
        match (&self.archive, raw) {
            (Some(a), false) => a,
            _ => &self.raw,
        }
    }
}

/// Whether text is an XML property list: an XML document whose root is `<plist>`.
pub fn looks_like_xml_plist(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("<?xml") && head.contains("<plist")
        || head.starts_with("<!DOCTYPE plist")
        || head.starts_with("<plist")
}

struct Binary<'a> {
    bytes: &'a [u8],
    offsets: Vec<usize>,
    ref_size: usize,
    nodes: usize,
}

fn be(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |n, b| (n << 8) | u64::from(*b))
}

/// Read the binary form (`bplist00`, and the compatible `bplist0x`).
pub fn parse_binary(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() < 8 + 32 || !bytes.starts_with(b"bplist") {
        return Err("not a binary property list".into());
    }
    let trailer = &bytes[bytes.len() - 32..];
    let offset_size = trailer[6] as usize;
    let ref_size = trailer[7] as usize;
    let count = be(&trailer[8..16]) as usize;
    let top = be(&trailer[16..24]) as usize;
    let table = be(&trailer[24..32]) as usize;
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) {
        return Err("the trailer has an impossible size".into());
    }
    if count == 0 || count > bytes.len() || top >= count {
        return Err("the trailer's object count is not possible for this file".into());
    }
    let table_end = count
        .checked_mul(offset_size)
        .and_then(|n| n.checked_add(table))
        .ok_or("the offset table overflows")?;
    if table < 8 || table_end > bytes.len() - 32 {
        return Err("the offset table is outside the file".into());
    }
    let offsets: Vec<usize> = (0..count)
        .map(|i| be(&bytes[table + i * offset_size..table + (i + 1) * offset_size]) as usize)
        .collect();
    if offsets.iter().any(|o| *o < 8 || *o >= table) {
        return Err("an object starts outside the file's objects".into());
    }
    let mut reader = Binary {
        bytes,
        offsets,
        ref_size,
        nodes: 0,
    };
    reader.object(top, 0, &mut Vec::new())
}

impl Binary<'_> {
    /// `len` bytes at `at`, or an error that says the file stops short.
    fn take(&self, at: usize, len: usize) -> Result<&[u8], String> {
        at.checked_add(len)
            .and_then(|end| self.bytes.get(at..end))
            .ok_or_else(|| "an object runs past the end of the file".to_string())
    }

    /// The length that follows a marker: in its low nibble, or — when that is 15 — in an
    /// integer object that comes next. Returns it with where the object's bytes begin.
    fn length(&self, at: usize, info: u8) -> Result<(usize, usize), String> {
        if info != 0xF {
            return Ok((info as usize, at + 1));
        }
        let marker = *self.take(at + 1, 1)?.first().unwrap();
        if marker >> 4 != 0x1 {
            return Err("a length is not an integer".into());
        }
        let width = 1usize << (marker & 0xF);
        if width > 8 {
            return Err("a length is too large".into());
        }
        let n = be(self.take(at + 2, width)?) as usize;
        Ok((n, at + 2 + width))
    }

    fn reference(&self, at: usize) -> Result<usize, String> {
        let r = be(self.take(at, self.ref_size)?) as usize;
        if r >= self.offsets.len() {
            return Err("a reference is to an object that is not there".into());
        }
        Ok(r)
    }

    fn object(
        &mut self,
        index: usize,
        depth: usize,
        walking: &mut Vec<usize>,
    ) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        if walking.contains(&index) {
            return Err("an object contains itself".into());
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err("too many objects".into());
        }
        let at = self.offsets[index];
        let marker = *self.take(at, 1)?.first().unwrap();
        let (kind, info) = (marker >> 4, marker & 0xF);
        Ok(match kind {
            0x0 => match info {
                0x0 | 0xF => Value::Null,
                0x8 => Value::Bool(false),
                0x9 => Value::Bool(true),
                _ => return Err(format!("an unknown simple value (0x{marker:02x})")),
            },
            0x1 => {
                let width = 1usize << info;
                if width > 16 {
                    return Err("an integer is wider than 128 bits".into());
                }
                let raw = self.take(at + 1, width)?;
                Value::Int(if width == 8 {
                    // Eight bytes are signed; narrower ones are not.
                    i128::from(be(raw) as i64)
                } else if width == 16 {
                    // Sixteen: signed, from two 64-bit halves.
                    (i128::from(be(&raw[..8]) as i64) << 64) | i128::from(be(&raw[8..]))
                } else {
                    i128::from(be(raw))
                })
            }
            0x2 => match info {
                2 => Value::Real(f64::from(f32::from_bits(be(self.take(at + 1, 4)?) as u32))),
                3 => Value::Real(f64::from_bits(be(self.take(at + 1, 8)?))),
                _ => return Err("a real is four or eight bytes".into()),
            },
            0x3 if info == 3 => Value::Date(f64::from_bits(be(self.take(at + 1, 8)?))),
            0x4 => {
                let (len, start) = self.length(at, info)?;
                Value::Data(self.take(start, len)?.to_vec())
            }
            0x5 => {
                let (len, start) = self.length(at, info)?;
                // ASCII, but anything above 0x7f is read as Latin-1 rather than refused.
                Value::String(self.take(start, len)?.iter().map(|&b| b as char).collect())
            }
            0x6 => {
                let (len, start) = self.length(at, info)?;
                let raw = self.take(start, len.checked_mul(2).ok_or("a string is too long")?)?;
                let units: Vec<u16> = raw
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                Value::String(String::from_utf16_lossy(&units))
            }
            0x7 => {
                let (len, start) = self.length(at, info)?;
                Value::String(String::from_utf8_lossy(self.take(start, len)?).into_owned())
            }
            0x8 => Value::Uid(be(self.take(at + 1, info as usize + 1)?)),
            0xA | 0xC => {
                let (len, start) = self.length(at, info)?;
                self.take(
                    start,
                    len.checked_mul(self.ref_size).ok_or("too many items")?,
                )?;
                walking.push(index);
                let mut items = Vec::with_capacity(len.min(1024));
                for i in 0..len {
                    let r = self.reference(start + i * self.ref_size)?;
                    items.push(self.object(r, depth + 1, walking)?);
                }
                walking.pop();
                if kind == 0xA {
                    Value::Array(items)
                } else {
                    Value::Set(items)
                }
            }
            0xD => {
                let (len, start) = self.length(at, info)?;
                let span = len.checked_mul(self.ref_size).ok_or("too many entries")?;
                self.take(start, span.checked_mul(2).ok_or("too many entries")?)?;
                walking.push(index);
                let mut entries = Vec::with_capacity(len.min(1024));
                for i in 0..len {
                    let k = self.reference(start + i * self.ref_size)?;
                    let v = self.reference(start + span + i * self.ref_size)?;
                    let key = match self.object(k, depth + 1, walking)? {
                        Value::String(s) => s,
                        other => other.summary(),
                    };
                    entries.push((key, self.object(v, depth + 1, walking)?));
                }
                walking.pop();
                Value::Dict(entries)
            }
            _ => return Err(format!("an unknown object type (0x{marker:02x})")),
        })
    }
}

/// Read the XML form.
pub fn parse_xml(text: &str) -> Result<Value, String> {
    let options = roxmltree::ParsingOptions {
        // A plist names its DTD; it is not read.
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text.trim_start_matches('\u{feff}'), options)
        .map_err(|e| format!("not valid XML: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "plist" {
        return Err("the XML is not a <plist>".into());
    }
    let top = root
        .children()
        .find(|n| n.is_element())
        .ok_or("the <plist> is empty")?;
    let mut nodes = 0;
    xml_value(top, 0, &mut nodes)
}

fn xml_value(node: roxmltree::Node, depth: usize, nodes: &mut usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err("nested too deeply".into());
    }
    *nodes += 1;
    if *nodes > MAX_NODES {
        return Err("too many objects".into());
    }
    let text = || node.children().filter_map(|c| c.text()).collect::<String>();
    let elements = || node.children().filter(roxmltree::Node::is_element);
    Ok(match node.tag_name().name() {
        "dict" => {
            let mut entries = Vec::new();
            let mut items = elements();
            while let Some(k) = items.next() {
                if k.tag_name().name() != "key" {
                    return Err(
                        "a <dict> has something other than a <key> where one belongs".into(),
                    );
                }
                let v = items.next().ok_or("a <key> has no value")?;
                entries.push((
                    k.children().filter_map(|c| c.text()).collect::<String>(),
                    xml_value(v, depth + 1, nodes)?,
                ));
            }
            Value::Dict(entries)
        }
        "array" => Value::Array(
            elements()
                .map(|c| xml_value(c, depth + 1, nodes))
                .collect::<Result<_, _>>()?,
        ),
        "string" => Value::String(text()),
        "integer" => {
            let t = text();
            let t = t.trim();
            Value::Int(
                if let Some(hex) = t.strip_prefix("0x") {
                    i128::from_str_radix(hex, 16)
                } else {
                    t.parse()
                }
                .map_err(|_| format!("`{t}` is not an integer"))?,
            )
        }
        "real" => {
            let t = text();
            let t = t.trim();
            Value::Real(match t {
                "nan" | "NaN" => f64::NAN,
                "inf" | "infinity" => f64::INFINITY,
                "-inf" | "-infinity" => f64::NEG_INFINITY,
                _ => t.parse().map_err(|_| format!("`{t}` is not a real"))?,
            })
        }
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "date" => {
            let t = text();
            let when = chrono::DateTime::parse_from_rfc3339(t.trim())
                .map_err(|_| format!("`{}` is not a date", t.trim()))?;
            Value::Date(
                when.timestamp() as f64 - APPLE_EPOCH as f64
                    + f64::from(when.timestamp_subsec_nanos()) / 1e9,
            )
        }
        "data" => Value::Data(base64_decode(&text())?),
        other => return Err(format!("`<{other}>` is not a property list value")),
    })
}

// -- writing ------------------------------------------------------------------------------------

/// The XML form of a value, as `plutil -convert xml1` would write it: for the viewer's text
/// view and for copying. A UID, which has no XML form, is written as a one-key dictionary.
pub fn to_xml(value: &Value) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n",
    );
    write_xml(value, 0, &mut out);
    out.push_str("</plist>\n");
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn write_xml(value: &Value, depth: usize, out: &mut String) {
    let pad = "\t".repeat(depth);
    let _ = match value {
        Value::Null => writeln!(out, "{pad}<string></string>"),
        Value::Bool(true) => writeln!(out, "{pad}<true/>"),
        Value::Bool(false) => writeln!(out, "{pad}<false/>"),
        Value::Int(i) => writeln!(out, "{pad}<integer>{i}</integer>"),
        Value::Real(r) => writeln!(out, "{pad}<real>{r}</real>"),
        Value::Date(d) => writeln!(out, "{pad}<date>{}</date>", date_string(*d)),
        Value::Data(d) | Value::Embedded { bytes: d, .. } => {
            let _ = writeln!(out, "{pad}<data>");
            for line in base64_encode(d).as_bytes().chunks(68) {
                let _ = writeln!(out, "{pad}{}", String::from_utf8_lossy(line));
            }
            writeln!(out, "{pad}</data>")
        }
        Value::String(s) => writeln!(out, "{pad}<string>{}</string>", escape(s)),
        Value::Uid(u) => {
            let _ = writeln!(out, "{pad}<dict>");
            let _ = writeln!(out, "{pad}\t<key>CF$UID</key>");
            let _ = writeln!(out, "{pad}\t<integer>{u}</integer>");
            writeln!(out, "{pad}</dict>")
        }
        Value::Array(v) | Value::Set(v) => {
            if v.is_empty() {
                return {
                    let _ = writeln!(out, "{pad}<array/>");
                };
            }
            let _ = writeln!(out, "{pad}<array>");
            for c in v {
                write_xml(c, depth + 1, out);
            }
            writeln!(out, "{pad}</array>")
        }
        Value::Dict(v) => {
            if v.is_empty() {
                return {
                    let _ = writeln!(out, "{pad}<dict/>");
                };
            }
            let _ = writeln!(out, "{pad}<dict>");
            for (k, c) in v {
                let _ = writeln!(out, "{pad}\t<key>{}</key>", escape(k));
                write_xml(c, depth + 1, out);
            }
            writeln!(out, "{pad}</dict>")
        }
    };
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let v = B64
            .iter()
            .position(|b| *b == c)
            .ok_or_else(|| format!("`{}` is not base64", c as char))? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

// -- taking values out --------------------------------------------------------------------------

/// The bytes of a data value, or of a list that was found inside one.
pub fn data_bytes(v: &Value) -> Option<&[u8]> {
    match v {
        Value::Data(d) | Value::Embedded { bytes: d, .. } => Some(d),
        _ => None,
    }
}

/// Bytes as lowercase hex, no spaces: what is pasted into another tool.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bytes as base64.
pub fn base64(bytes: &[u8]) -> String {
    base64_encode(bytes)
}

/// A value as JSON, for copying a whole structure. JSON has no dates, data or UIDs, so a date
/// is its time as text, data is `{"$data": base64}` (with the list inside, as `$plist`, if it is
/// one), a UID is `{"$uid": n}`, and a number JSON cannot hold (an integer past 64 bits, NaN,
/// infinity) is text.
pub fn to_json(v: &Value) -> String {
    serde_json::to_string_pretty(&json_of(v)).unwrap_or_default()
}

fn json_of(v: &Value) -> serde_json::Value {
    use serde_json::{Map, Number, Value as J};
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => i64::try_from(*i)
            .map(|n| J::Number(n.into()))
            .or_else(|_| u64::try_from(*i).map(|n| J::Number(n.into())))
            .unwrap_or_else(|_| J::String(i.to_string())),
        Value::Real(r) => Number::from_f64(*r)
            .map(J::Number)
            .unwrap_or_else(|| J::String(r.to_string())),
        Value::Date(d) => J::String(date_string(*d)),
        Value::Data(d) => {
            let mut m = Map::new();
            m.insert("$data".into(), J::String(base64_encode(d)));
            J::Object(m)
        }
        Value::Embedded { bytes, inner } => {
            let mut m = Map::new();
            m.insert("$data".into(), J::String(base64_encode(bytes)));
            m.insert("$plist".into(), json_of(inner));
            J::Object(m)
        }
        Value::String(s) => J::String(s.clone()),
        Value::Uid(u) => {
            let mut m = Map::new();
            m.insert("$uid".into(), J::Number((*u).into()));
            J::Object(m)
        }
        Value::Array(items) | Value::Set(items) => J::Array(items.iter().map(json_of).collect()),
        Value::Dict(entries) => {
            let mut m = Map::new();
            for (k, c) in entries {
                m.insert(k.clone(), json_of(c));
            }
            J::Object(m)
        }
    }
}

/// What "copy value" puts on the clipboard: the text of a string as it is (no quotes), a number
/// or a date as written, data as hex, and a list or dictionary as JSON.
pub fn copy_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Real(r) => r.to_string(),
        Value::Date(d) => date_string(*d),
        Value::Uid(u) => u.to_string(),
        Value::Data(d) | Value::Embedded { bytes: d, .. } => hex(d),
        Value::Array(_) | Value::Set(_) | Value::Dict(_) => to_json(v),
    }
}

/// Where a path leads, as a person would say it: `root › items › 2 › name`. Dictionaries are
/// followed by key, arrays by position (from 0).
pub fn path_label(root: &Value, path: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut here = root;
    for &i in path {
        let step = match here {
            Value::Dict(d) => d.get(i).map(|(k, c)| (k.clone(), c)),
            Value::Array(v) | Value::Set(v) => v.get(i).map(|c| (i.to_string(), c)),
            Value::Embedded { inner, .. } if i == 0 => Some(("↳".to_string(), inner.as_ref())),
            _ => None,
        };
        match step {
            Some((label, next)) => {
                parts.push(label);
                here = next;
            }
            None => break,
        }
    }
    if parts.is_empty() {
        "(the list)".into()
    } else {
        parts.join(" › ")
    }
}

// -- the tree the viewer draws ------------------------------------------------------------------

/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Where it is: the position of each ancestor among its siblings, then its own.
    pub path: Vec<usize>,
    pub depth: usize,
    pub key: Option<String>,
    pub kind: &'static str,
    pub summary: String,
    pub container: bool,
    pub open: bool,
}

/// The rows that are visible: the root, and the children of every container that is open.
/// At most `limit` rows, so that opening something huge does not stall the window.
pub fn rows(root: &Value, open: &std::collections::HashSet<Vec<usize>>, limit: usize) -> Vec<Row> {
    let mut out = Vec::new();
    let mut stack: Vec<(Vec<usize>, Option<String>, &Value)> = vec![(Vec::new(), None, root)];
    while let Some((path, key, value)) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        let is_open = value.is_container() && open.contains(&path);
        out.push(Row {
            depth: path.len(),
            key,
            kind: value.kind(),
            summary: value.summary(),
            container: value.is_container(),
            open: is_open,
            path: path.clone(),
        });
        if is_open {
            // Pushed backwards so they come out in order.
            for (i, (k, child)) in value.children().into_iter().enumerate().rev() {
                let mut p = path.clone();
                p.push(i);
                stack.push((p, k.map(str::to_string), child));
            }
        }
    }
    out
}

/// The paths of every container down to `depth` levels (for opening the first levels), or all
/// of them (`usize::MAX`), at most `limit`.
pub fn containers(
    root: &Value,
    depth: usize,
    limit: usize,
) -> std::collections::HashSet<Vec<usize>> {
    let mut found = std::collections::HashSet::new();
    let mut stack: Vec<(Vec<usize>, &Value)> = vec![(Vec::new(), root)];
    while let Some((path, value)) = stack.pop() {
        if !value.is_container() || found.len() >= limit {
            continue;
        }
        if path.len() < depth {
            found.insert(path.clone());
            for (i, (_, child)) in value.children().into_iter().enumerate() {
                let mut p = path.clone();
                p.push(i);
                stack.push((p, child));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINARY: &[u8] = include_bytes!("../../tests/fixtures/sample.bplist");
    const WITH_UID: &[u8] = include_bytes!("../../tests/fixtures/sample-uid.bplist");
    const XML: &[u8] = include_bytes!("../../tests/fixtures/sample.plist");

    fn get<'a>(v: &'a Value, key: &str) -> &'a Value {
        match v {
            Value::Dict(d) => {
                &d.iter()
                    .find(|(k, _)| k == key)
                    .unwrap_or_else(|| panic!("no {key}"))
                    .1
            }
            _ => panic!("not a dict"),
        }
    }

    #[test]
    fn a_binary_plist_written_by_python_is_read_value_for_value() {
        let v = parse(BINARY).unwrap();
        assert_eq!(get(&v, "name"), &Value::String("REactor".into()));
        assert_eq!(
            get(&v, "unicode"),
            &Value::String("héllo ✓ 日本".into()),
            "UTF-16 strings"
        );
        assert_eq!(get(&v, "int_small"), &Value::Int(7));
        assert_eq!(get(&v, "int_big"), &Value::Int(4_294_967_296));
        assert_eq!(
            get(&v, "int_negative"),
            &Value::Int(-42),
            "eight bytes are signed"
        );
        assert_eq!(get(&v, "real"), &Value::Real(3.5));
        assert_eq!(get(&v, "yes"), &Value::Bool(true));
        assert_eq!(get(&v, "no"), &Value::Bool(false));
        assert_eq!(
            get(&v, "data"),
            &Value::Data(b"\x00\x01\x02binary\xff".to_vec())
        );
        assert_eq!(
            date_string(match get(&v, "date") {
                Value::Date(d) => *d,
                _ => panic!(),
            }),
            "2020-01-02T03:04:05Z"
        );
        match get(&v, "list") {
            Value::Array(items) => {
                assert_eq!(items.len(), 4);
                assert_eq!(items[0], Value::String("a".into()));
                assert_eq!(
                    items[2],
                    Value::Array(vec![Value::Real(2.5), Value::Bool(false)])
                );
                assert_eq!(
                    items[3],
                    Value::Dict(vec![("k".into(), Value::String("v".into()))])
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(get(&v, "empty_dict"), &Value::Dict(Vec::new()));
        assert_eq!(get(&v, "empty_list"), &Value::Array(Vec::new()));
        // A UID is its own kind (binary only).
        assert_eq!(get(&parse(WITH_UID).unwrap(), "uid"), &Value::Uid(3));
    }

    #[test]
    fn the_xml_form_of_the_same_list_reads_as_the_same_tree() {
        let binary = parse(BINARY).unwrap();
        let xml = parse(XML).unwrap();
        assert_eq!(binary, xml, "one list, two forms, one tree");
        assert!(looks_like_xml_plist(XML) && !looks_like_xml_plist(BINARY));
    }

    #[test]
    fn writing_xml_and_reading_it_back_changes_nothing() {
        let v = parse(BINARY).unwrap();
        let text = to_xml(&v);
        assert!(
            text.starts_with("<?xml") && text.contains("<plist version=\"1.0\">"),
            "{text}"
        );
        assert!(
            text.contains("<key>unicode</key>") && text.contains("<string>héllo ✓ 日本</string>")
        );
        assert_eq!(parse(text.as_bytes()).unwrap(), v);
        // Markup in a string is escaped, and survives.
        let tricky = Value::Dict(vec![("a<b".into(), Value::String("x & <y>".into()))]);
        assert_eq!(parse(to_xml(&tricky).as_bytes()).unwrap(), tricky);
    }

    #[test]
    fn a_damaged_or_hostile_file_is_an_error_and_never_a_crash() {
        assert!(parse(b"plain text").is_err());
        assert!(parse(b"bplist00").is_err(), "no trailer");
        // Truncated anywhere, never a panic.
        for cut in 0..BINARY.len() {
            let _ = parse(&BINARY[..cut]);
        }
        // Every byte changed in turn: an error or a value, never a panic.
        for i in 0..BINARY.len() {
            let mut b = BINARY.to_vec();
            b[i] = b[i].wrapping_add(0x55);
            let _ = parse(&b);
        }
        // An offset table that points past the file.
        let mut b = BINARY.to_vec();
        let n = b.len();
        b[n - 8..].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(parse(&b).is_err());
        // An array that contains itself: object 0 is an array of one reference, to object 0.
        let mut cyc = b"bplist00".to_vec();
        cyc.extend([0xA1, 0x00]); // at offset 8: an array of 1 item, reference 0
        let table = cyc.len();
        cyc.push(8); // object 0 is at offset 8
        cyc.extend([0, 0, 0, 0, 0, 0]); // unused, sort version
        cyc.push(1); // offset size
        cyc.push(1); // reference size
        cyc.extend(1u64.to_be_bytes()); // one object
        cyc.extend(0u64.to_be_bytes()); // the top object
        cyc.extend((table as u64).to_be_bytes());
        let err = parse(&cyc).unwrap_err();
        assert!(err.contains("contains itself"), "{err}");
    }

    #[test]
    fn the_tree_shows_what_is_open_and_the_paths_find_values() {
        let v = parse(BINARY).unwrap();
        let shut = rows(&v, &Default::default(), 1000);
        assert_eq!(shut.len(), 1, "only the root, shut");
        assert_eq!(shut[0].summary, "13 keys");
        // One level: the root is open, what is in it is shut.
        let first = containers(&v, 1, 1000);
        let open = rows(&v, &first, 1000);
        // The root and its thirteen entries; the list and the two empty containers are shut.
        assert_eq!(
            open.len(),
            14,
            "{:?}",
            open.iter().map(|r| &r.summary).collect::<Vec<_>>()
        );
        let list = open
            .iter()
            .find(|r| r.key.as_deref() == Some("list"))
            .unwrap();
        assert!(list.container && !list.open && list.summary == "4 items");
        assert_eq!(v.at(&list.path).unwrap().kind(), "array");
        // Two levels opens the list as well: its four items come into view.
        assert_eq!(rows(&v, &containers(&v, 2, 1000), 1000).len(), 18);
        // Everything open reaches the nested values, in order.
        let all = rows(&v, &containers(&v, usize::MAX, 1000), 1000);
        assert!(
            all.iter()
                .any(|r| r.key.as_deref() == Some("k") && r.depth == 3)
        );
        // A limit cuts a huge tree off.
        assert_eq!(rows(&v, &containers(&v, usize::MAX, 1000), 5).len(), 5);
        // Data is summarised, with its size.
        assert!(
            all.iter()
                .any(|r| r.kind == "data" && r.summary.starts_with("10 bytes: 00 01 02"))
        );
    }

    #[test]
    fn dates_are_read_from_2001_and_what_is_not_a_time_is_said_so() {
        assert_eq!(date_string(0.0), "2001-01-01T00:00:00Z");
        assert_eq!(date_string(86_400.0), "2001-01-02T00:00:00Z");
        assert_eq!(date_string(f64::NAN), "NaN");
        assert!(date_string(1e30).contains("since 2001"));
    }

    #[test]
    fn what_is_copied_out_of_a_field_is_the_field_as_it_reads() {
        assert_eq!(
            copy_text(&Value::String("héllo \"x\"".into())),
            "héllo \"x\"",
            "no quotes"
        );
        assert_eq!(copy_text(&Value::Int(-42)), "-42");
        assert_eq!(copy_text(&Value::Real(3.5)), "3.5");
        assert_eq!(copy_text(&Value::Bool(true)), "true");
        assert_eq!(copy_text(&Value::Date(0.0)), "2001-01-01T00:00:00Z");
        assert_eq!(copy_text(&Value::Uid(3)), "3");
        assert_eq!(copy_text(&Value::Data(vec![0, 1, 0xab, 0xff])), "0001abff");
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(hex(b""), "");
        assert_eq!(data_bytes(&Value::Data(vec![1])), Some(&[1u8][..]));
        assert_eq!(data_bytes(&Value::String("x".into())), None);
    }

    #[test]
    fn a_structure_is_copied_as_json_that_can_hold_what_json_cannot() {
        let v = parse(BINARY).unwrap();
        let json: serde_json::Value = serde_json::from_str(&to_json(&v)).unwrap();
        assert_eq!(json["name"], "REactor");
        assert_eq!(json["int_negative"], -42);
        assert_eq!(json["yes"], true);
        assert_eq!(json["date"], "2020-01-02T03:04:05Z");
        assert_eq!(json["data"]["$data"], base64(b"\x00\x01\x02binary\xff"));
        assert_eq!(json["list"][3]["k"], "v");
        // What a number type cannot hold becomes text rather than a wrong number.
        let odd = Value::Array(vec![
            Value::Int(i128::from(u64::MAX) + 1),
            Value::Real(f64::NAN),
            Value::Uid(7),
        ]);
        let j: serde_json::Value = serde_json::from_str(&to_json(&odd)).unwrap();
        assert_eq!(j[0], "18446744073709551616");
        assert_eq!(j[1], "NaN");
        assert_eq!(j[2]["$uid"], 7);
        // `copy value` of a structure is that JSON.
        assert_eq!(copy_text(&odd), to_json(&odd));
    }

    #[test]
    fn a_path_reads_as_the_keys_and_places_that_lead_to_it() {
        let v = parse(BINARY).unwrap();
        let rows = rows(&v, &containers(&v, usize::MAX, 1000), 1000);
        let k = rows.iter().find(|r| r.key.as_deref() == Some("k")).unwrap();
        assert_eq!(path_label(&v, &k.path), "list › 3 › k");
        assert_eq!(path_label(&v, &[]), "(the list)");
        assert_eq!(
            path_label(&v, &[99]),
            "(the list)",
            "a path that leads nowhere says so"
        );
    }
}
