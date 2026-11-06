#![cfg(feature = "truetype")]
use rustos_text::{Error, Family, FontSet, Metrics, Style};
fn fonts() -> FontSet {
    FontSet::load(
        include_bytes!("../../../third_party/fonts/DejaVuSans.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSans-Bold.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSerif.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSansMono.ttf"),
    )
    .expect("native font family")
}
#[test]
fn proportional_unicode_glyphs_have_real_coverage_and_matching_metrics() {
    let fonts = fonts();
    let style = Style::default();
    assert!(fonts.advance('W', style) > fonts.advance('i', style) * 2);
    for ch in ['A', 'g', 'é', 'Ω', 'Ж'] {
        let glyph = fonts.glyph(ch, style).unwrap();
        assert_eq!(glyph.coverage.len(), (glyph.width * glyph.height) as usize);
        assert!(
            glyph.coverage.iter().any(|&a| a > 0 && a < 255),
            "antialias coverage"
        );
        assert_eq!(glyph.advance, fonts.advance(ch, style));
        assert!(glyph.height <= fonts.line_height(style) + 8);
    }
    let mono = Style {
        family: Family::Monospace,
        ..style
    };
    assert_eq!(fonts.advance('W', mono), fonts.advance('i', mono));
    let large = Style { size: 32, ..style };
    assert!(fonts.advance('W', large) >= fonts.advance('W', style) * 2 - 1);
}
#[test]
fn invalid_font_data_and_unbounded_sizes_are_rejected() {
    assert!(matches!(
        FontSet::load(b"broken", b"", b"", b""),
        Err(Error::Font)
    ));
    let fonts = fonts();
    for size in [0, 97, u16::MAX] {
        assert!(matches!(
            fonts.glyph(
                'W',
                Style {
                    size,
                    ..Style::default()
                }
            ),
            Err(Error::Limit)
        ));
    }
}

#[test]
fn bold_serif_and_monospace_use_their_own_real_outlines() {
    let fonts = fonts()
        .with_bold_faces(
            include_bytes!("../../../third_party/fonts/DejaVuSerif-Bold.ttf"),
            include_bytes!("../../../third_party/fonts/DejaVuSansMono-Bold.ttf"),
        )
        .unwrap();
    for family in [Family::Serif, Family::Monospace] {
        let regular = fonts
            .glyph(
                'W',
                Style {
                    family,
                    ..Style::default()
                },
            )
            .unwrap();
        let bold = fonts
            .glyph(
                'W',
                Style {
                    family,
                    bold: true,
                    ..Style::default()
                },
            )
            .unwrap();
        assert_ne!(
            regular.coverage, bold.coverage,
            "real bold face for {:?}",
            family
        );
    }
}
