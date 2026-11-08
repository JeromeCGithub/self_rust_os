//! Bounded intrinsic widths and independent inline/flex formatting contexts.
use super::*;

/// A formatting context retains ranges into the same paint/control arenas.
/// Translation moves hit targets and pixels together; no second DOM or surface.
pub(super) struct Fragment {
    items: core::ops::Range<usize>,
    controls: core::ops::Range<usize>,
    pub(super) width: u32,
    pub(super) height: u32,
}
impl Fragment {
    pub(super) fn translate(&self, page: &mut Page, x: i32, y: i32) {
        for item in &mut page.items[self.items.clone()] {
            item.x += x;
            item.y += y;
        }
        for control in &mut page.controls[self.controls.clone()] {
            control.x += x;
            control.y += y;
        }
    }
}
fn preferred_width(
    doc: &Document,
    id: usize,
    style: &Style,
    available: u32,
    metrics: &impl rustos_text::Metrics,
) -> u32 {
    if style.hidden {
        return 0;
    }
    let extra = style.padding.horizontal() + style.border * 2;
    let content = if let Some(width) = style.width {
        if style.border_box {
            width.saturating_sub(extra)
        } else {
            width
        }
    } else if doc.nodes[id].tag.is_empty() {
        doc.nodes[id]
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .fold(0u32, |n, c| {
                n.saturating_add(metrics.advance(c, style.font))
            })
    } else if doc.nodes[id].tag == "img" {
        doc.nodes[id]
            .attr("width")
            .and_then(|v| v.parse().ok())
            .or_else(|| {
                doc.images
                    .iter()
                    .find(|image| image.node == id)
                    .map(|image| image.image.width)
            })
            .unwrap_or(0)
    } else if let Some(field) = doc
        .forms
        .iter()
        .flat_map(|form| &form.fields)
        .find(|field| field.node == id)
    {
        if field.kind == FieldKind::Hidden {
            return 0;
        }
        if field.kind.is_text() {
            400u32.saturating_sub(extra)
        } else {
            (field.value.len().max(6) as u32 * 8 + 20).saturating_sub(extra)
        }
    } else {
        let mut line = 0u32;
        let mut widest = 0u32;
        let mut count = 0u32;
        for &child in &doc.nodes[id].children {
            let child_style = css::style(doc, child, style);
            if child_style.hidden {
                continue;
            }
            let width = preferred_width(doc, child, &child_style, available, metrics);
            if style.flex {
                line = line.saturating_add(width);
                count += 1;
            } else if child_style.block && !child_style.atomic && !child_style.float_right {
                widest = widest.max(line).max(width);
                line = 0;
            } else {
                line = line.saturating_add(width);
            }
        }
        widest.max(line.saturating_add(style.gap.saturating_mul(count.saturating_sub(1))))
    };
    content
        .max(style.min_content_width())
        .saturating_add(extra)
        .saturating_add(style.margin.horizontal())
        .min(available)
}
pub(super) fn fragment(
    doc: &Document,
    id: usize,
    style: &Style,
    link: Option<&str>,
    available: u32,
    page: &mut Page,
    metrics: &impl rustos_text::Metrics,
) -> Fragment {
    let width = preferred_width(doc, id, style, available, metrics);
    let (items, controls) = (page.items.len(), page.controls.len());
    let mut local = Flow::new(0, width as i32, 0, page);
    let mut style = style.clone();
    style.atomic = false;
    style.float_right = false;
    style.block = true;
    node_styled(doc, id, &style, link, &mut local, page, metrics);
    local.flush(page);
    Fragment {
        items: items..page.items.len(),
        controls: controls..page.controls.len(),
        width,
        height: local.y.max(0) as u32,
    }
}
pub(super) fn row(
    doc: &Document,
    id: usize,
    style: &Style,
    link: Option<&str>,
    flow: &mut Flow,
    page: &mut Page,
    metrics: &impl rustos_text::Metrics,
) {
    let available = (flow.right - flow.left).max(0) as u32;
    let mut children = Vec::new();
    for &child in &doc.nodes[id].children {
        let child_style = css::style(doc, child, style);
        if child_style.hidden
            || (doc.nodes[child].tag.is_empty() && doc.nodes[child].text.trim().is_empty())
        {
            continue;
        }
        let width = preferred_width(doc, child, &child_style, available, metrics);
        children.push((child, child_style, width));
    }
    let gap = style.gap.min(available / children.len().max(1) as u32);
    let gaps = gap.saturating_mul(children.len().saturating_sub(1) as u32);
    let total = children
        .iter()
        .fold(0u32, |n, (_, _, w)| n.saturating_add(*w));
    let space = available.saturating_sub(gaps);
    let mut fragments = Vec::new();
    for (child, child_style, width) in children {
        let width = if total > space {
            (u64::from(width) * u64::from(space) / u64::from(total)) as u32
        } else {
            width
        };
        fragments.push(fragment(
            doc,
            child,
            &child_style,
            link,
            width,
            page,
            metrics,
        ));
    }
    let content = fragments.iter().map(|f| f.width).sum::<u32>() + gaps;
    let height = style
        .height
        .map(|height| {
            if style.border_box {
                height.saturating_sub(style.padding.vertical() + style.border * 2)
            } else {
                height
            }
        })
        .unwrap_or_else(|| fragments.iter().map(|f| f.height).max().unwrap_or(0))
        .max(style.min_content_height());
    let spare = available.saturating_sub(content);
    let mut x = flow.left
        + match style.justify {
            css::Align::Start => 0,
            css::Align::Center => spare as i32 / 2,
            css::Align::End => spare as i32,
        };
    for fragment in fragments {
        let spare = height as i32 - fragment.height as i32;
        let dy = match style.cross_align {
            css::Align::Start => 0,
            css::Align::Center => spare / 2,
            css::Align::End => spare,
        };
        fragment.translate(page, x, flow.y + dy);
        x += fragment.width as i32 + gap as i32;
    }
    flow.y += height as i32;
    flow.item_start = page.items.len();
    flow.control_start = page.controls.len();
}
