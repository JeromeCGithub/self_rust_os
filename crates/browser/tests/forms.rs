use rustos_browser::{decode_html, Document, Encoding, Error, FieldKind};
#[test]
fn get_forms_encode_successful_controls_and_only_activated_submitter() {
    let doc=Document::parse("<form action='/search?old=x#top'><input type='hidden' name='ie' value='UTF-8'><input name='q' value='rust os'><input name='skip' value='no' disabled><input type='submit' name='btnG' value='Google Search'><input type='submit' name='btnI' value='Lucky'></form>").unwrap();
    let mut form = doc.forms.into_iter().next().expect("parsed GET form");
    assert_eq!(form.fields[1].kind, FieldKind::Text);
    form.fields[1].value = "rust & café?".into();
    assert_eq!(
        form.submit("https://www.google.com/", Some(3)).unwrap(),
        "https://www.google.com/search?ie=UTF-8&q=rust+%26+caf%C3%A9%3F&btnG=Google+Search#top"
    );
    assert_eq!(
        form.submit("https://www.google.com/", None).unwrap(),
        "https://www.google.com/search?ie=UTF-8&q=rust+%26+caf%C3%A9%3F#top"
    );
}
#[test]
fn form_controls_are_visible_editable_clipped_and_hidden_controls_do_not_paint() {
    let doc=Document::parse("<form><input type=hidden name=secret value=private><input name=q title='Google Search'><input type=submit value=Search></form>").unwrap();
    let page = doc.layout(320);
    assert_eq!(page.controls.len(), 2);
    for c in &page.controls {
        assert!(c.x >= 0 && c.x as u32 + c.width <= 320);
    }
    assert!(!page.items.iter().any(|i| i.text.contains("private")));
}
#[test]
fn form_submission_rejects_unsupported_methods_active_urls_and_oversized_queries() {
    let doc=Document::parse("<form method=post><input name=q></form><form action='javascript:alert(1)'><input name=q></form><form><input name=q></form>").unwrap();
    assert_eq!(
        doc.forms[0].submit("https://www.google.com/", None),
        Err(Error::UnsupportedForm)
    );
    assert_eq!(
        doc.forms[1].submit("https://www.google.com/", None),
        Err(Error::InvalidUrl)
    );
    let mut form = doc.forms[2].clone();
    form.fields[0].value = "x".repeat(2048);
    assert_eq!(
        form.submit("https://www.google.com/", None),
        Err(Error::Limit)
    );
    assert_eq!(
        Document::parse(&format!("<form>{}</form>", "<input name=q>".repeat(65))).unwrap_err(),
        Error::Limit
    );
}
#[test]
fn html_decoding_handles_google_latin1_utf8_and_invalid_declared_utf8() {
    assert_eq!(
        decode_html(b"Fran\xe7ais", Some(Encoding::Windows1252)).unwrap(),
        "Français"
    );
    assert_eq!(
        decode_html(b"\x80 price", Some(Encoding::Windows1252)).unwrap(),
        "€ price"
    );
    assert_eq!(decode_html("café".as_bytes(), None).unwrap(), "café");
    assert_eq!(
        decode_html(b"bad\xff", Some(Encoding::Utf8)),
        Err(Error::Encoding)
    );
}

#[test]
fn google_short_hex_colors_and_script_only_pages_have_correct_visual_state() {
    let doc =
        Document::parse("<style>body {background:#fff;color:#abc}</style><p>Google</p>").unwrap();
    let page = doc.layout(640);
    assert_eq!(page.background, 0xffffff);
    assert_eq!(
        page.items
            .iter()
            .find(|i| i.text == "Google")
            .unwrap()
            .color,
        0xaabbcc
    );
    assert!(!page.is_empty());
    assert!(Document::parse(
        "<style>p{display:none}</style><p>Hidden</p><script>unsupported()</script>"
    )
    .unwrap()
    .layout(640)
    .is_empty());
    assert!(!Document::parse("<form><input name=q></form>")
        .unwrap()
        .layout(640)
        .is_empty());
}
