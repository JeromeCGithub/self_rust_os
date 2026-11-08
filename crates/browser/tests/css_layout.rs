//! Box coordinates captured from Chromium for the checked-in HTML fixture.
use rustos_browser::{Document, Item};

fn document() -> Document {
    Document::parse(include_str!("fixtures/css-layout.html")).unwrap()
}
fn background(page: &rustos_browser::Page, color: u32) -> &Item {
    page.items
        .iter()
        .find(|item| item.background == Some(color))
        .unwrap()
}
fn bounds(item: &Item) -> (i32, i32, u32, u32) {
    (item.x, item.y, item.width, item.height)
}
#[test]
fn css_four_sided_boxes_have_browser_content_and_border_geometry() {
    let page = document().layout(640);
    assert_eq!(bounds(background(&page, 0xffccaa)), (8, 4, 120, 32));
    let text = page.items.iter().find(|item| item.text == "Panel").unwrap();
    assert_eq!((text.x, text.y), (18, 10));
}
#[test]
fn css_auto_margins_center_a_fixed_width_box() {
    let page = document().layout(640);
    assert_eq!(bounds(background(&page, 0xbbddff)), (280, 40, 80, 30));
}
#[test]
fn inline_blocks_share_a_centered_line_instead_of_full_width_backgrounds() {
    let page = document().layout(640);
    assert_eq!(bounds(background(&page, 0xaaddbb)), (230, 70, 80, 24));
    assert_eq!(bounds(background(&page, 0xddbbff)), (310, 70, 100, 24));
}
#[test]
fn flex_rows_respect_gap_and_cross_axis_centering() {
    let page = document().layout(640);
    assert_eq!(bounds(background(&page, 0xffaaaa)), (530, 114, 40, 20));
    assert_eq!(bounds(background(&page, 0xaaffaa)), (580, 109, 60, 30));
}
#[test]
fn authored_control_dimensions_and_fonts_reach_the_renderer() {
    let page = document().layout(640);
    let control = &page.controls[0];
    assert_eq!(
        (control.x, control.y, control.width, control.height),
        (0, 154, 514, 34)
    );
    assert_eq!(control.font.size, 18);
    assert_eq!(control.font.family, rustos_text::Family::Sans);
    assert_eq!(control.color, 0x000000); // Chromium's UA FieldText overrides body color.
}

#[test]
fn border_box_minimums_include_padding_and_color_decorations_follow_the_cascade() {
    let doc=Document::parse("<style>body{margin:0} .chip{display:inline-block;box-sizing:border-box;min-width:80px;min-height:40px;padding:10px 12px;background:blue;font:14px/18px monospace}a{color:inherit;text-decoration:none}</style><div style='color:#123456'><a href='/next' class='chip'>Go</a></div>").unwrap();
    let page = doc.layout(640);
    assert_eq!(bounds(background(&page, 0x0000ff)), (0, 0, 80, 40));
    let text = page.items.iter().find(|item| item.text == "Go").unwrap();
    assert_eq!(text.color, 0x123456);
    assert!(!text.underline);
    assert_eq!(page.hit(text.x, text.y), Some("/next"));
}
#[test]
fn malformed_sides_are_ignored_atomically_and_display_can_override_display_none() {
    let doc=Document::parse("<style>body{margin:0} #box{display:none;margin:4px 8px;padding:6px 10px}#box{display:block;margin:1px bad;padding:2px -1px;width:100px;height:20px;background:red}</style><div id=box>Visible</div>").unwrap();
    let page = doc.layout(640);
    assert_eq!(bounds(background(&page, 0xff0000)), (8, 4, 120, 32));
}

#[test]
fn background_radius_is_retained_for_the_native_painter() {
    let doc = Document::parse(
        "<div style='background:blue;border-radius:100px;width:80px;height:40px'>Sign in</div>",
    )
    .unwrap();
    let page = doc.layout(640);
    assert_eq!(background(&page, 0x0000ff).radius, 100);
}

#[test]
fn fixed_height_flex_centers_an_item_whose_margin_box_overflows() {
    let doc=Document::parse("<style>body{margin:0}</style><div style='display:flex;align-items:center;height:48px'><div style='margin:12px 0;width:80px;height:40px;background:blue'>Go</div></div>").unwrap();
    let page = doc.layout(640);
    assert_eq!(bounds(background(&page, 0x0000ff)), (0, 4, 80, 40));
}

#[test]
fn inline_box_padding_remains_a_link_hit_target() {
    let doc=Document::parse("<a href='/next' style='display:inline-block;background:blue;padding:10px;width:80px;height:20px'>Go</a>").unwrap();
    let page = doc.layout(640);
    let area = background(&page, 0x0000ff);
    assert_eq!(page.hit(area.x + 1, area.y + 1), Some("/next"));
}
