use rustos_browser::{address, resolve, Document, Error};

#[test]
fn html_entities_raw_text_and_external_sheets() {
    let doc = Document::parse("<!doctype html><TITLE>A &amp; B</TITLE><link rel='stylesheet' href='/site.css'><style>p {color:red}</style><h1>Welcome</h1><p>Hello &lt;web&gt; &#x1f600; <a href='next.html'>Next</a></p><script>if (x < y) evil()</script>").unwrap();
    assert_eq!(doc.title, "A & B");
    assert_eq!(doc.stylesheets, ["/site.css"]);
    let page = doc.layout(320);
    let text = page
        .items
        .iter()
        .map(|i| i.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(text.contains("Hello <web> 😀"), "{text}");
    assert!(!text.contains("evil"));
    assert!(!text.contains("color:red"));
    let link = page.items.iter().find(|i| i.text == "Next").unwrap();
    assert_eq!(page.hit(link.x, link.y), Some("next.html"));
    assert_eq!(page.hit(-1, -1), None);
}
#[test]
fn css_cascade_inheritance_background_and_hidden_content() {
    let mut doc = Document::parse("<style>p {color:red} .note {color:#2468ac; padding:10px; background-color:#ffeecc} #main {color:blue} .hidden {display:none}</style><div class='note'><p id='main' style='color: #123456'>Visible</p><p>Inherited?</p><p class='hidden'>Secret</p></div>").unwrap();
    doc.add_css("p { color: green; }").unwrap();
    let page = doc.layout(300);
    let visible = page.items.iter().find(|i| i.text == "Visible").unwrap();
    assert_eq!(visible.color, 0x123456);
    assert!(visible.x >= 18);
    assert!(page.items.iter().any(|i| i.background == Some(0xffeecc)));
    assert!(!page.items.iter().any(|i| i.text.contains("Secret")));
    assert_eq!(
        page.items
            .iter()
            .find(|i| i.text == "Inherited?")
            .unwrap()
            .color,
        0x008000
    );
}
#[test]
fn flow_wraps_long_words_preserves_pre_and_bounds_geometry() {
    let doc = Document::parse("<h1>Heading</h1><p>abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz</p><pre>a  b\nc</pre>").unwrap();
    let page = doc.layout(96);
    assert!(page.height > 60);
    for item in &page.items {
        assert!(item.x >= 0 && item.x as u32 + item.width <= 96, "{item:?}");
    }
    let text = page
        .items
        .iter()
        .map(|i| i.text.as_str())
        .collect::<Vec<_>>()
        .join("");
    assert!(text.contains("a  b"));
    assert_eq!(doc.layout(0).items.len(), 0);
}
#[test]
fn malformed_html_recovers_and_limits_are_explicit() {
    assert!(Document::parse("<p>one<p>two<!-- unfinished").is_ok());
    assert_eq!(
        Document::parse(&"x".repeat(262145)).unwrap_err(),
        Error::Limit
    );
    assert_eq!(
        Document::parse(&"<div>".repeat(65)).unwrap_err(),
        Error::Limit
    );
    assert_eq!(
        Document::parse(&"<br>".repeat(4097)).unwrap_err(),
        Error::Limit
    );
}
#[test]
fn address_and_link_resolution_reject_privileged_schemes() {
    assert_eq!(
        address(" info.cern.ch "),
        Ok("https://info.cern.ch/".into())
    );
    assert_eq!(
        address("rust os"),
        Ok("https://www.google.com/search?q=rust%20os".into())
    );
    assert_eq!(
        resolve("https://host.test/a/b?q=1", "../c#title"),
        Ok("https://host.test/c#title".into())
    );
    assert_eq!(
        resolve("https://host.test/a/b", "?q=two"),
        Ok("https://host.test/a/b?q=two".into())
    );
    assert_eq!(
        resolve("https://host.test/a/b", "//other.test/x"),
        Ok("https://other.test/x".into())
    );
    for bad in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,hi",
        "https://a/b\r\nx",
    ] {
        assert_eq!(address(bad), Err(Error::InvalidUrl), "{bad}");
        assert_eq!(
            resolve("https://host.test/", bad),
            Err(Error::InvalidUrl),
            "{bad}"
        );
    }
}

#[test]
fn omitted_body_still_receives_document_css() {
    let doc =
        Document::parse("<style>body {background-color:#f5f0df;color:navy}</style><p>Fragment</p>")
            .unwrap();
    let page = doc.layout(640);
    assert_eq!(page.background, 0xf5f0df);
    assert_eq!(
        page.items
            .iter()
            .find(|i| i.text == "Fragment")
            .unwrap()
            .color,
        0x000080
    );
}

#[test]
fn optional_list_end_tags_do_not_accumulate_indentation() {
    let doc =
        Document::parse("<dl><dt>First<dd>one<dt>Second<dd>two</dl><ul><li>A<li>B</ul>").unwrap();
    let page = doc.layout(320);
    let x = |text| page.items.iter().find(|item| item.text == text).unwrap().x;
    assert_eq!(x("First"), x("Second"));
}
