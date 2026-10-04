//! Converting documents and chunking them. Every fixture is built here, from the format's
//! own structure, so a test says what the format looks like.

use std::io::Write;

use reactor_ingest::{CHUNKER, Error, chunk, convert, convert_file};

fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    out
}

fn only(name: &str, bytes: &[u8]) -> reactor_ingest::Converted {
    let mut docs = convert(name, bytes).unwrap();
    assert_eq!(docs.len(), 1);
    docs.remove(0)
}

#[test]
fn markdown_keeps_its_structure_and_names_itself_by_its_first_heading() {
    let d = only("notes.md", b"\xef\xbb\xbf# Unpacking the dropper\n\nIt is UPX.\n\n\n\n## Stage 2\n\nDecrypts with RC4.  \n");
    assert_eq!(d.title.as_deref(), Some("Unpacking the dropper"));
    assert_eq!(d.mime, "text/markdown");
    assert_eq!(
        d.markdown,
        "# Unpacking the dropper\n\nIt is UPX.\n\n## Stage 2\n\nDecrypts with RC4."
    );
}

#[test]
fn plain_text_takes_its_title_from_the_file_name_and_a_bad_encoding_is_not_refused() {
    let d = only("sandbox-report.txt", b"line one\nline two\n");
    assert_eq!(
        (d.title.as_deref(), d.mime.as_str()),
        (Some("sandbox-report"), "text/plain")
    );
    let latin1 = only("old.txt", b"caf\xe9 au lait");
    assert_eq!(latin1.markdown, "café au lait");
}

#[test]
fn a_web_page_loses_its_furniture_and_keeps_its_content() {
    let page = br#"<!doctype html><html><head><title>CVE-2026-1 advisory</title>
        <style>body{}</style><script>track()</script></head>
        <body><nav><a href="/">Home</a><a href="/x">Products</a></nav>
        <header>Site banner</header>
        <main><h1>Advisory</h1><p>A <b>heap overflow</b> in <code>parse()</code>.</p>
        <table><tr><th>Version</th><th>Fixed</th></tr><tr><td>1.2</td><td>1.3</td></tr></table>
        <pre>strcpy(buf, in);</pre></main>
        <footer>Copyright</footer></body></html>"#;
    // By content, whatever it is called: a download without an extension is still a page.
    let d = only("download", page);
    assert_eq!(d.title.as_deref(), Some("CVE-2026-1 advisory"));
    assert_eq!(d.mime, "text/html");
    for kept in [
        "# Advisory",
        "heap overflow",
        "`parse()`",
        "strcpy(buf, in);",
        "Version",
    ] {
        assert!(
            d.markdown.contains(kept),
            "{kept} missing from:\n{}",
            d.markdown
        );
    }
    for gone in ["Home", "Site banner", "track()", "Copyright", "body{}"] {
        assert!(
            !d.markdown.contains(gone),
            "{gone} still in:\n{}",
            d.markdown
        );
    }
}

#[test]
fn a_word_document_keeps_headings_lists_and_tables() {
    let w = r#"<w:document xmlns:w="x"><w:body>
      <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Findings</w:t></w:r></w:p>
      <w:p><w:r><w:t>The sample </w:t></w:r><w:r><w:t>beacons &amp; sleeps.</w:t></w:r></w:p>
      <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Network</w:t></w:r></w:p>
      <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>c2.example.net</w:t></w:r></w:p>
      <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>198.51.100.7</w:t></w:r></w:p>
      <w:p><w:r><w:t>Done.</w:t></w:r></w:p>
      <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Host</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Port</w:t></w:r></w:p></w:tc></w:tr>
             <w:tr><w:tc><w:p><w:r><w:t>ws-042</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>443</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
    </w:body></w:document>"#;
    let d = only("report.docx", &zip_of(&[("word/document.xml", w)]));
    assert_eq!(d.title.as_deref(), Some("Findings"));
    assert_eq!(
        d.markdown,
        "# Findings\n\nThe sample beacons & sleeps.\n\n## Network\n\n- c2.example.net\n- 198.51.100.7\n\nDone.\n\n| Host | Port |\n| ws-042 | 443 |"
    );
    // The same file without its extension is still recognised by what is in it.
    assert_eq!(
        only("report", &zip_of(&[("word/document.xml", w)])).markdown,
        d.markdown
    );
}

#[test]
fn an_opendocument_text_is_read_the_same_way() {
    let c = r#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:table="b"><office:body><office:text>
      <text:h text:outline-level="1">Timeline</text:h>
      <text:p>First <text:s/>seen on Tuesday.</text:p>
      <text:list><text:list-item><text:p>dropper</text:p></text:list-item><text:list-item><text:p>loader</text:p></text:list-item></text:list>
      <table:table><table:table-row><table:table-cell><text:p>when</text:p></table:table-cell><table:table-cell><text:p>what</text:p></table:table-cell></table:table-row></table:table>
    </office:text></office:body></office:document-content>"#;
    let d = only("timeline.odt", &zip_of(&[("content.xml", c)]));
    assert_eq!(
        d.markdown,
        "# Timeline\n\nFirst seen on Tuesday.\n\n- dropper\n- loader\n\n| when | what |"
    );
}

#[test]
fn an_epub_is_its_pages_in_order_with_the_packages_title() {
    let opf =
        r#"<package><metadata><dc:title>Practical Malware Notes</dc:title></metadata></package>"#;
    let page = |n: &str| format!("<html><body><h1>Chapter {n}</h1><p>text {n}</p></body></html>");
    let bytes = zip_of(&[
        ("OEBPS/content.opf", opf),
        ("OEBPS/ch2.xhtml", &page("2")),
        ("OEBPS/ch1.xhtml", &page("1")),
    ]);
    let d = only("book.epub", &bytes);
    assert_eq!(d.title.as_deref(), Some("Practical Malware Notes"));
    assert!(d.markdown.find("Chapter 1").unwrap() < d.markdown.find("Chapter 2").unwrap());
}

/// A one-page PDF with a line of text, with its cross-reference table computed.
fn pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 18 Tf 72 700 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    out
}

#[test]
fn a_pdfs_text_is_extracted_and_a_damaged_one_is_an_error_not_a_crash() {
    let d = only("whitepaper.pdf", &pdf("Process hollowing explained"));
    assert!(
        d.markdown.contains("Process hollowing explained"),
        "{}",
        d.markdown
    );
    assert_eq!(
        (d.title.as_deref(), d.mime.as_str()),
        (Some("whitepaper"), "application/pdf")
    );
    let e = convert("broken.pdf", b"%PDF-1.4\nnot really a pdf").unwrap_err();
    assert!(matches!(e, Error::Unreadable(_)), "{e}");
    // A PDF with a page and no text in it is an empty document, and says what that means.
    let blank = convert("scan.pdf", &pdf("")).unwrap_err();
    assert!(
        matches!(blank, Error::Empty(_)) && blank.to_string().contains("OCR"),
        "{blank}"
    );
}

const DUMP: &str = r#"<mediawiki xmlns="http://www.mediawiki.org/xml/export-0.11/">
  <siteinfo><sitename>Malware Wiki</sitename></siteinfo>
  <page><title>Emotet</title><ns>0</ns><revision><text>{{Infobox malware|name=Emotet}}
'''Emotet''' is a [[banking trojan|trojan]] first seen in 2014.&lt;ref&gt;cite&lt;/ref&gt;

== Delivery ==
* Spam with [[Microsoft Word|Word]] attachments
* [http://example.org/report the report]
=== Persistence ===
# Run key
# Scheduled task

{| class="wikitable"
|-
! Year !! Event
|-
| 2014 || First seen
|}
[[Category:Botnets]]</text></revision></page>
  <page><title>Emo</title><ns>0</ns><redirect title="Emotet" /><revision><text>#REDIRECT [[Emotet]]</text></revision></page>
  <page><title>Talk:Emotet</title><ns>1</ns><revision><text>discussion</text></revision></page>
  <page><title>Help:Editing</title><ns>12</ns><revision><text>Use the &amp; sign.</text></revision></page>
</mediawiki>"#;

#[test]
fn a_wiki_dump_is_one_document_per_page_and_wikitext_becomes_readable() {
    let docs = convert("dump.xml", DUMP.as_bytes()).unwrap();
    let titles: Vec<_> = docs
        .iter()
        .map(|d| d.fragment.as_deref().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Emotet", "Help:Editing"],
        "no redirect, no discussion page"
    );
    let m = &docs[0].markdown;
    assert!(m.starts_with("# Emotet\n\n"), "{m}");
    assert!(m.contains("Emotet is a trojan first seen in 2014."), "{m}");
    assert!(
        m.contains("## Delivery") && m.contains("### Persistence"),
        "{m}"
    );
    assert!(
        m.contains("- Spam with Word attachments") && m.contains("- the report"),
        "{m}"
    );
    assert!(
        m.contains("1. Run key") && m.contains("1. Scheduled task"),
        "{m}"
    );
    assert!(
        m.contains("| Year | Event |") && m.contains("| 2014 | First seen |"),
        "{m}"
    );
    for gone in ["Infobox", "[[", "'''", "cite", "Botnets", "wikitable"] {
        assert!(!m.contains(gone), "{gone} still in:\n{m}");
    }
    assert_eq!(docs[1].markdown, "# Help:Editing\n\nUse the & sign.");
}

#[test]
fn a_file_is_converted_and_a_dump_is_handed_over_page_by_page() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dump.xml"), DUMP).unwrap();
    std::fs::write(dir.path().join("a.md"), "# A\n\ntext").unwrap();
    std::fs::write(dir.path().join("empty.txt"), "  \n\n").unwrap();
    let collect = |name: &str| {
        let mut got = Vec::new();
        let r = convert_file(&dir.path().join(name), &mut |c, _| {
            got.push(c.title.unwrap());
            Ok(())
        });
        (r, got)
    };
    assert_eq!(collect("dump.xml").1, ["Emotet", "Help:Editing"]);
    assert_eq!(collect("a.md").1, ["A"]);
    let (r, got) = collect("empty.txt");
    assert!(matches!(r, Err(Error::Empty(_))) && got.is_empty());
    assert!(matches!(collect("nope.md").0, Err(Error::Io(_))));
}

#[test]
fn chunks_follow_the_structure_and_know_which_headings_they_sit_under() {
    let para = |w: &str| format!("{} ", w).repeat(60); // about 240 characters
    let doc = format!(
        "# Guide\n\nIntro. {}\n\n## Install\n\n{}\n\n### Linux\n\n{}\n\n## Usage\n\n{}",
        para("intro"),
        para("install").repeat(3),
        para("linux").repeat(4),
        para("usage")
    );
    let chunks = chunk(&doc);
    assert!(chunks.len() >= 4, "{}", chunks.len());
    assert_eq!(
        chunks.iter().map(|c| c.ord).collect::<Vec<_>>(),
        (0..chunks.len()).collect::<Vec<_>>()
    );
    for c in &chunks {
        assert!(
            c.text.chars().count() <= 2000,
            "{} characters",
            c.text.chars().count()
        );
        assert!(!c.text.trim().is_empty());
    }
    let path_of = |word: &str| {
        chunks
            .iter()
            .find(|c| c.text.contains(word))
            .unwrap()
            .heading_path
            .clone()
    };
    assert_eq!(path_of("intro intro"), "Guide");
    assert_eq!(path_of("install install"), "Guide > Install");
    assert_eq!(path_of("linux linux"), "Guide > Install > Linux");
    assert_eq!(
        path_of("usage usage"),
        "Guide > Usage",
        "a new section closes the deeper ones"
    );
    // Cutting is deterministic, and what was cut is what was written.
    assert_eq!(chunk(&doc), chunks);
    let words = |s: &str| s.split_whitespace().count();
    assert_eq!(
        chunks.iter().map(|c| words(&c.text)).sum::<usize>(),
        words(&doc)
    );
    assert!(CHUNKER.contains("v1"));
}

#[test]
fn a_short_document_is_one_chunk_and_nothing_is_none() {
    let c = chunk("# Note\n\nRemember the mutex name.");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].heading_path, "Note");
    assert!(chunk("").is_empty());
    assert!(chunk("   \n\n  ").is_empty());
}

#[test]
fn a_binary_file_is_not_read_as_text() {
    let e = convert("picture.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0").unwrap_err();
    assert!(matches!(e, Error::Unsupported(_)), "{e}");
    assert!(e.to_string().contains("binary"), "{e}");
}

/// A server that answers one request per entry with a canned response.
fn serve(responses: Vec<String>) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{addr}")
}

fn ok(content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn a_page_is_fetched_and_converted_and_what_cannot_be_is_refused() {
    use reactor_ingest::fetch;
    let base = serve(vec![
        ok(
            "text/html",
            "<html><head><title>Advisory</title></head><body><main><h1>CVE</h1><p>fixed in 2.0</p></main></body></html>",
        ),
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
    ]);
    let page = fetch(&format!("{base}/advisory#section")).unwrap();
    assert_eq!(page.url, format!("{base}/advisory"), "no fragment");
    assert_eq!(page.content_type.as_deref(), Some("text/html"));
    let docs = convert(&page.url, &page.bytes).unwrap();
    assert_eq!(docs[0].title.as_deref(), Some("Advisory"));
    assert!(docs[0].markdown.contains("fixed in 2.0"));

    let e = fetch(&format!("{base}/missing")).unwrap_err();
    assert!(e.to_string().contains("404"), "{e}");
    for bad in [
        "ftp://example.org/x",
        "file:///etc/passwd",
        "https://user:pw@example.org/",
        "not a url",
    ] {
        assert!(fetch(bad).is_err(), "{bad}");
    }
}
