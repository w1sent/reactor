//! Web pages: the title, and the part that is the page rather than its furniture.

use scraper::{Html, Selector};

pub struct Page {
    pub title: Option<String>,
    pub markdown: String,
}

/// What is never content, whatever the page is.
const FURNITURE: [&str; 9] = [
    "script", "style", "noscript", "template", "svg", "nav", "header", "footer", "aside",
];

fn text_of(html: &Html, selector: &str) -> Option<String> {
    let sel = Selector::parse(selector).ok()?;
    let t: String = html.select(&sel).next()?.text().collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(t).filter(|t| !t.is_empty())
}

/// Convert a page: `<main>` or `<article>` if it has one, else the body, without
/// navigation, scripts and boilerplate.
pub fn to_markdown(source: &str) -> Page {
    let html = Html::parse_document(source);
    let title = text_of(&html, "title").or_else(|| text_of(&html, "h1"));
    let main = ["main", "article", "[role=main]", "body"]
        .iter()
        .find_map(|s| {
            let sel = Selector::parse(s).ok()?;
            html.select(&sel).next().map(|e| e.html())
        })
        .unwrap_or_else(|| source.to_string());
    let skip: Vec<&str> = FURNITURE.to_vec();
    let converter = htmd::HtmlToMarkdown::builder().skip_tags(skip).build();
    let markdown = converter
        .convert(&main)
        .unwrap_or_else(|_| html.root_element().text().collect::<Vec<_>>().join(" "));
    Page { title, markdown }
}
