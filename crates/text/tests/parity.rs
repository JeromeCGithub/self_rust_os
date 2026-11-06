#![cfg(feature = "truetype")]
use rustos_text::{Family, FontSet, Metrics, Style};
#[test]
fn on_demand_faces_preserve_existing_unicode_metrics_and_exact_coverage() {
    let bytes: [&[u8]; 6] = [
        include_bytes!("../../../third_party/fonts/DejaVuSans.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSans-Bold.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSerif.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSansMono.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSerif-Bold.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSansMono-Bold.ttf"),
    ];
    let expected: Vec<_> = bytes
        .iter()
        .map(|b| {
            fontdue::Font::from_bytes(
                *b,
                fontdue::FontSettings {
                    load_substitutions: false,
                    ..Default::default()
                },
            )
            .unwrap()
        })
        .collect();
    let actual = FontSet::load(bytes[0], bytes[1], bytes[2], bytes[3])
        .unwrap()
        .with_bold_faces(bytes[4], bytes[5])
        .unwrap();
    for (family, bold, index) in [
        (Family::Sans, false, 0),
        (Family::Sans, true, 1),
        (Family::Serif, false, 2),
        (Family::Monospace, false, 3),
        (Family::Serif, true, 4),
        (Family::Monospace, true, 5),
    ] {
        for size in [8, 16, 24, 32, 96] {
            for ch in ['A', 'g', ' ', 'é', 'Ω', 'Ж', '中', '😀', '\u{301}'] {
                let style = Style { family, bold, size };
                let face = if expected[index].lookup_glyph_index(ch) == 0 {
                    &expected[usize::from(bold)]
                } else {
                    &expected[index]
                };
                let (metrics, coverage) = face.rasterize(ch, size as f32);
                let glyph = actual.glyph(ch, style).unwrap();
                assert_eq!(glyph.coverage, coverage, "{family:?} {bold} {size} {ch}");
                assert_eq!(
                    (glyph.width, glyph.height, glyph.left),
                    (metrics.width as u32, metrics.height as u32, metrics.xmin)
                );
                assert_eq!(
                    actual.advance(ch, style),
                    metrics.advance_width.ceil() as u32
                );
                assert_eq!(
                    actual.line_height(style),
                    expected[index].horizontal_line_metrics(size as f32)
                        .unwrap()
                        .new_line_size
                        .ceil() as u32
                );
            }
        }
    }
}
