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
fn diagnostic_summary_counts_rendered_content_without_exporting_text_or_urls() {
    let doc=Document::parse("<a href='/one'>Repeated wrapped destination with words</a><a href='/one'>Again</a><a href='/two'>Second</a><div style='display:none'><a href='/hidden'>Hidden</a></div><form><input name=q></form>").unwrap();
    let page = doc.layout(160);
    let summary = page.summary();
    assert_eq!(summary.links, 2);
    assert_eq!(summary.controls, 1);
    assert!(summary.items > 2);
    assert_eq!(summary.images, 0);
    assert!(!summary.truncated);
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
        Document::parse(&"x".repeat(rustos_browser::MAX_HTML + 1)).unwrap_err(),
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

#[test]
fn proportional_metrics_drive_wrap_geometry_and_link_hits() {
    struct Proportional;
    impl rustos_text::Metrics for Proportional {
        fn advance(&self, ch: char, _: rustos_text::Style) -> u32 {
            if ch == 'W' {
                9
            } else {
                3
            }
        }
        fn line_height(&self, _: rustos_text::Style) -> u32 {
            14
        }
        fn default_size(&self) -> u16 {
            16
        }
    }
    let doc =
        Document::parse("<p><a href='/wide'>WWW</a></p><p><a href='/narrow'>iii</a></p>").unwrap();
    let page = doc.layout_with(200, &Proportional);
    let wide = page.items.iter().find(|item| item.text == "WWW").unwrap();
    let narrow = page.items.iter().find(|item| item.text == "iii").unwrap();
    assert_eq!(wide.width, 27);
    assert_eq!(narrow.width, 9);
    assert_eq!(wide.height, 14);
    assert_eq!(page.hit(wide.x + 26, wide.y), Some("/wide"));
    assert_eq!(page.hit(wide.x + 27, wide.y), None);
}

#[test]
fn decoded_images_have_aspect_geometry_link_hits_and_a_visible_page() {
    use std::sync::Arc;
    let mut doc =
        Document::parse("<a href='/next'><img src='/logo.png' width='120' alt='fallback'></a>")
            .unwrap();
    let image = rustos_browser::Image::decode(include_bytes!(
        "../../web-image/tests/fixtures/two-pixels.png"
    ))
    .unwrap();
    let node = doc.nodes.iter().position(|node| node.tag == "img").unwrap();
    doc.set_image(node, Arc::new(image))
        .expect("associate decoded image");
    let page = doc.layout(640);
    assert!(!page.is_empty());
    assert_eq!(page.hit(20, 20), Some("/next"));
    assert!(page
        .items
        .iter()
        .any(|item| item.width == 120 && item.height == 60));
}

#[test]
fn linked_and_inline_author_sheets_follow_document_order_and_importance() {
    let mut doc = Document::parse(
        "<link rel='stylesheet' href='/first.css'><style>p {color:blue}</style><p id=x>Order</p>",
    )
    .unwrap();
    assert_eq!(doc.apply_stylesheets(|_| Some("p {color:red}".into())), 0);
    assert_eq!(
        doc.layout(200)
            .items
            .iter()
            .find(|item| item.text == "Order")
            .unwrap()
            .color,
        0x0000ff
    );
    let doc=Document::parse("<style>p {color:blue !important} #x{color:green}</style><p id=x style='color:red'>Priority</p><p style='color:red!important'>Inline important</p>").unwrap();
    let page = doc.layout(200);
    assert_eq!(
        page.items
            .iter()
            .find(|item| item.text == "Priority")
            .unwrap()
            .color,
        0x0000ff
    );
    assert!(page
        .items
        .iter()
        .any(|item| item.text.contains("Inline") && item.color == 0xff0000));
}

#[test]
fn text_after_image_keeps_its_own_metrics_and_image_geometry() {
    use std::sync::Arc;
    let mut doc =
        Document::parse("<body style='color:black'><img src='/x' width=120>After</body>").unwrap();
    let node = doc.nodes.iter().position(|node| node.tag == "img").unwrap();
    let image = rustos_browser::Image::decode(include_bytes!(
        "../../web-image/tests/fixtures/two-pixels.png"
    ))
    .unwrap();
    doc.set_image(node, Arc::new(image)).unwrap();
    let page = doc.layout(640);
    let image = page.items.iter().find(|i| i.image.is_some()).unwrap();
    assert_eq!(
        image.width, 120,
        "text must not stretch the preceding image"
    );
    assert!(image.text.is_empty());
    assert!(page
        .items
        .iter()
        .any(|i| i.image.is_none() && i.text == "After" && i.x == image.x + 120));
}

#[test]
fn image_limits_reject_invalid_buffers_and_keep_previous_resource_on_failure() {
    use std::sync::Arc;
    let mut doc = Document::parse(&"<img>".repeat(17)).unwrap();
    let nodes: Vec<_> = doc
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.tag == "img")
        .map(|(id, _)| id)
        .collect();
    let valid = Arc::new(rustos_browser::Image {
        width: 1,
        height: 1,
        rgba: vec![255; 4],
    });
    assert!(doc.set_image(usize::MAX, valid.clone()).is_err());
    assert!(doc
        .set_image(
            nodes[0],
            Arc::new(rustos_browser::Image {
                width: 2,
                height: 1,
                rgba: vec![0; 4]
            })
        )
        .is_err());
    for &id in &nodes[..16] {
        doc.set_image(id, valid.clone()).unwrap();
    }
    assert!(doc.set_image(nodes[16], valid.clone()).is_err());
    doc.set_image(nodes[0], valid).unwrap(); // Replacement does not consume another slot.
    let page = doc.layout(640);
    assert_eq!(page.images.len(), 16);
    assert_eq!(page.images[0].image.rgba, [255; 4]);

    let mut doc = Document::parse("<img><img><img><img><img>").unwrap();
    let nodes: Vec<_> = doc
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.tag == "img")
        .map(|(id, _)| id)
        .collect();
    let large = Arc::new(rustos_browser::Image {
        width: 1024,
        height: 1024,
        rgba: vec![0; 4 * 1024 * 1024],
    });
    for &id in &nodes[..4] {
        doc.set_image(id, large.clone()).unwrap();
    }
    assert!(doc.set_image(nodes[4], large).is_err());
    assert_eq!(doc.layout(640).images.len(), 4);
}

#[test]
fn normal_document_budget_accepts_large_bootstrap_data_but_rejects_the_full_boundary() {
    let source=format!("<title>Large document</title><script type=application/json>{}</script><p>Still readable</p>","x".repeat(1300*1024));
    let doc =
        Document::parse(&source).expect("ordinary modern bootstrap data fits the document budget");
    assert_eq!(doc.title, "Large document");
    assert!(doc
        .layout(640)
        .items
        .iter()
        .any(|item| item.text.contains("Still readable")));
    assert_eq!(
        Document::parse(&"x".repeat(rustos_browser::MAX_HTML + 1)).unwrap_err(),
        Error::Limit
    );
}
#[test]
fn raw_text_end_tags_require_a_complete_case_insensitive_tag_name() {
    let doc =
        Document::parse("<script>const text='</scripture>'; const n=42;</ScRiPt ><p>After</p>")
            .unwrap();
    let script = doc.nodes.iter().find(|node| node.tag == "script").unwrap();
    assert!(script.text.contains("const n=42"));
    assert!(script.text.contains("</scripture>"));
    let page = doc.layout(640);
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<String>(),
        "After"
    );
}

#[test]
fn script_classification_distinguishes_data_from_executable_sources() {
    use rustos_browser::ScriptKind;
    let doc=Document::parse("<script type=application/json>{\"value\":42}</script><script type=' Text/JavaScript ; charset=utf-8'>42</script><script type=MODULE src='/app.js'></script><script>42</script>").unwrap();
    assert_eq!(
        doc.nodes
            .iter()
            .filter_map(|node| node.script_kind())
            .collect::<Vec<_>>(),
        [
            ScriptKind::Data,
            ScriptKind::Classic,
            ScriptKind::Module,
            ScriptKind::Classic
        ]
    );
}

#[test]
fn ordinary_pages_can_load_six_sheets_in_order_and_keep_a_finite_link_budget() {
    let source = (0..6)
        .map(|n| format!("<link rel=stylesheet href='/sheet-{n}.css'>"))
        .collect::<String>()
        + "<p>Six sheets</p>";
    let mut doc =
        Document::parse(&source).expect("six ordinary stylesheets fit the document profile");
    let mut fetched = Vec::new();
    assert_eq!(
        doc.apply_stylesheets(|url| {
            fetched.push(url.to_string());
            Some(format!("p{{color:#{:06x}}}", fetched.len()))
        }),
        0
    );
    assert_eq!(
        fetched,
        (0..6)
            .map(|n| format!("/sheet-{n}.css"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        doc.layout(640)
            .items
            .iter()
            .find(|item| item.text == "Six sheets")
            .unwrap()
            .color,
        6
    );
    assert!(Document::parse(&"<link rel=stylesheet href='/x.css'>".repeat(16)).is_ok());
    assert_eq!(
        Document::parse(&"<link rel=stylesheet href='/x.css'>".repeat(17)).unwrap_err(),
        Error::Limit
    );
}
