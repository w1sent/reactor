//! Fetching a page. The one place this crate touches the network, and only when the user
//! names a URL.

use std::io::Read;
use std::time::Duration;

use crate::convert::{Error, Result};

/// A page larger than this is not a page.
const MAX_BYTES: u64 = 64 << 20;

#[derive(Debug)]
pub struct Fetched {
    /// Where it ended up after redirects, without a fragment.
    pub url: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

/// Fetch an `http` or `https` URL. A URL with credentials in it is refused: they would be
/// stored with the document.
pub fn fetch(url: &str) -> Result<Fetched> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|e| Error::Io(format!("{url}: not a URL ({e})")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Error::Unsupported(format!(
            "{url}: only http and https pages can be fetched"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(Error::Unsupported(format!(
            "{url}: a URL with credentials in it would be stored with the document"
        )));
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("REactor/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Io(format!("{url}: {e}")))?;
    let response = client
        .get(parsed)
        .send()
        .map_err(|e| Error::Io(format!("{url}: {e}")))?;
    if !response.status().is_success() {
        return Err(Error::Io(format!("{url}: {}", response.status())));
    }
    let mut final_url = response.url().clone();
    final_url.set_fragment(None);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut bytes = Vec::new();
    response
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Io(format!("{url}: {e}")))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::Unsupported(format!(
            "{url}: over {} MiB",
            MAX_BYTES >> 20
        )));
    }
    Ok(Fetched {
        url: final_url.to_string(),
        bytes,
        content_type,
    })
}
