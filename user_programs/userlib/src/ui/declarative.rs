//! Allocation-free component values and typed styles for the native UI.
use super::raster::{Color, Rect};

/// Container spacing and inherited foreground color, in content pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Style {
    /// Inset on all four sides of a column, in pixels.
    pub padding: u16,
    /// Additional vertical spacing between children, in pixels.
    pub gap: u16,
    /// Inherited text color; None uses the parent or widget default.
    pub color: Option<Color>,
}
impl Style {
    /// No additional spacing or foreground override.
    pub const fn new() -> Self {
        Self {
            padding: 0,
            gap: 0,
            color: None,
        }
    }
    /// Sets the column's content inset.
    pub const fn padding(mut self, value: u16) -> Self {
        self.padding = value;
        self
    }
    /// Sets additional spacing between children.
    pub const fn gap(mut self, value: u16) -> Self {
        self.gap = value;
        self
    }
    /// Sets the inherited foreground color.
    pub const fn color(mut self, value: Color) -> Self {
        self.color = Some(value);
        self
    }
}
/// Invalid view structure. Nesting is bounded to protect the 16 KiB user stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The tree exceeds sixteen levels, counting containers and leaves.
    TooDeep,
}
/// Native render operations; components do not access display mappings.
pub trait Renderer<M> {
    /// Paints text within a content-local layout slot; scale is 1 or 2.
    fn text(&mut self, bounds: Rect, text: &str, color: Color, scale: u32);
    /// Paints a button and captures its message on activation, without dispatching.
    fn button(&mut self, bounds: Rect, key: &str, label: &str, action: &M, color: Option<Color>);
    /// Paints a card; None preserves the default title and detail colors.
    fn card(&mut self, bounds: Rect, title: &str, detail: &str, color: Option<Color>);
}
/// Intrinsic vertical layout, independent of application messages.
pub trait Layout {
    /// Vertical slot height, including the widget's built-in spacing.
    fn height(&self) -> u32;
    /// Number of siblings represented by a child chain; ordinary views count as one.
    fn count(&self) -> u32 {
        1
    }
    /// Maximum nesting depth, saturating rather than wrapping.
    fn depth(&self) -> u8 {
        1
    }
}
/// A component function returns an owned value implementing this trait.
pub trait View<M>: Layout {
    /// Emits primitives with inherited styling. Call the free `render` function
    /// for root rendering so structural validation happens before any painting.
    fn render(
        &self,
        renderer: &mut impl Renderer<M>,
        bounds: Rect,
        color: Option<Color>,
        gap: u16,
        depth: u8,
    ) -> Result<(), RenderError>;
}
/// Draws a root component inside a content-local viewport.
pub fn render<M>(
    view: &impl View<M>,
    renderer: &mut impl Renderer<M>,
    width: u32,
    height: u32,
) -> Result<(), RenderError> {
    if view.depth() > 16 {
        return Err(RenderError::TooDeep);
    }
    if width == 0 || height == 0 {
        return Ok(());
    }
    view.render(renderer, Rect::new(0, 0, width, height), None, 0, 0)
}
/// Empty child list emitted by `rsx!`.
pub struct Empty;
/// A typed child list, stored inline without allocation.
pub struct Children<H, T>(pub H, pub T);
/// Vertical component container, normally constructed by `rsx!`.
pub struct Column<C> {
    /// Container spacing and inherited color.
    pub style: Style,
    /// Child components in declaration order.
    pub children: C,
}
/// Borrowed text component.
pub struct Text<'a> {
    /// Text to draw; the value must live as long as the view.
    pub text: &'a str,
    /// Uses the heading size and vertical slot when true.
    pub heading: bool,
    /// Overrides inherited foreground styling.
    pub color: Option<Color>,
}
/// Button component with stable identity and an application-owned message.
pub struct Button<'a, M> {
    /// Unique stable key within the mounted view.
    pub key: &'a str,
    /// Visible text; changing it does not change identity.
    pub label: &'a str,
    /// Cloned on activation and dispatched after painting.
    pub action: M,
}
/// Rounded information card component.
pub struct Card<'a> {
    /// Primary text.
    pub title: &'a str,
    /// Supporting text.
    pub detail: &'a str,
}
/// Additional vertical space, capped at 400 pixels.
pub struct Space(pub u32);

impl Layout for Empty {
    fn height(&self) -> u32 {
        0
    }
    fn count(&self) -> u32 {
        0
    }
    fn depth(&self) -> u8 {
        0
    }
}
impl<H: Layout, T: Layout> Layout for Children<H, T> {
    fn height(&self) -> u32 {
        self.0.height().saturating_add(self.1.height())
    }
    fn count(&self) -> u32 {
        1u32.saturating_add(self.1.count())
    }
    fn depth(&self) -> u8 {
        self.0.depth().max(self.1.depth())
    }
}
impl<C: Layout> Layout for Column<C> {
    fn height(&self) -> u32 {
        self.children
            .height()
            .saturating_add(u32::from(self.style.padding) * 2)
            .saturating_add(
                self.children
                    .count()
                    .saturating_sub(1)
                    .saturating_mul(u32::from(self.style.gap)),
            )
    }
    fn depth(&self) -> u8 {
        self.children.depth().saturating_add(1)
    }
}
impl Layout for Text<'_> {
    fn height(&self) -> u32 {
        if self.heading {
            36
        } else {
            22
        }
    }
}
impl<M> Layout for Button<'_, M> {
    fn height(&self) -> u32 {
        50
    }
}
impl Layout for Card<'_> {
    fn height(&self) -> u32 {
        88
    }
}
impl Layout for Space {
    fn height(&self) -> u32 {
        self.0.min(400)
    }
}
impl<M> View<M> for Empty {
    fn render(
        &self,
        _: &mut impl Renderer<M>,
        _: Rect,
        _: Option<Color>,
        _: u16,
        _: u8,
    ) -> Result<(), RenderError> {
        Ok(())
    }
}
impl<M, H: View<M>, T: View<M>> View<M> for Children<H, T> {
    fn render(
        &self,
        r: &mut impl Renderer<M>,
        b: Rect,
        c: Option<Color>,
        gap: u16,
        depth: u8,
    ) -> Result<(), RenderError> {
        if b.width == 0 || b.height == 0 {
            return Ok(());
        }
        self.0.render(
            r,
            Rect::new(b.x, b.y, b.width, self.0.height()),
            c,
            gap,
            depth,
        )?;
        self.1.render(
            r,
            Rect::new(
                b.x,
                b.y.saturating_add(
                    self.0
                        .height()
                        .saturating_add(u32::from(gap))
                        .min(i32::MAX as u32) as i32,
                ),
                b.width,
                b.height,
            ),
            c,
            gap,
            depth,
        )
    }
}
impl<M, C: View<M>> View<M> for Column<C> {
    fn render(
        &self,
        r: &mut impl Renderer<M>,
        b: Rect,
        c: Option<Color>,
        _: u16,
        depth: u8,
    ) -> Result<(), RenderError> {
        if depth >= 16 {
            return Err(RenderError::TooDeep);
        }
        let pad = u32::from(self.style.padding);
        self.children.render(
            r,
            Rect::new(
                b.x.saturating_add(pad as i32),
                b.y.saturating_add(pad as i32),
                b.width.saturating_sub(pad * 2),
                b.height.saturating_sub(pad * 2),
            ),
            self.style.color.or(c),
            self.style.gap,
            depth + 1,
        )
    }
}
impl<M> View<M> for Text<'_> {
    fn render(
        &self,
        r: &mut impl Renderer<M>,
        b: Rect,
        c: Option<Color>,
        _: u16,
        _: u8,
    ) -> Result<(), RenderError> {
        r.text(
            b,
            self.text,
            self.color
                .or(c)
                .unwrap_or(Color::rgb(if self.heading { 0xf6f9ff } else { 0xc8d8e9 })),
            if self.heading { 2 } else { 1 },
        );
        Ok(())
    }
}
impl<M> View<M> for Button<'_, M> {
    fn render(
        &self,
        r: &mut impl Renderer<M>,
        b: Rect,
        c: Option<Color>,
        _: u16,
        _: u8,
    ) -> Result<(), RenderError> {
        r.button(
            Rect::new(b.x, b.y, b.width, 38),
            self.key,
            self.label,
            &self.action,
            c,
        );
        Ok(())
    }
}
impl<M> View<M> for Card<'_> {
    fn render(
        &self,
        r: &mut impl Renderer<M>,
        b: Rect,
        c: Option<Color>,
        _: u16,
        _: u8,
    ) -> Result<(), RenderError> {
        r.card(Rect::new(b.x, b.y, b.width, 72), self.title, self.detail, c);
        Ok(())
    }
}
impl<M> View<M> for Space {
    fn render(
        &self,
        _: &mut impl Renderer<M>,
        _: Rect,
        _: Option<Color>,
        _: u16,
        _: u8,
    ) -> Result<(), RenderError> {
        Ok(())
    }
}

/// Native RSX: typed columns, text, cards, buttons and Rust component expressions.
#[macro_export]
macro_rules! rsx {
    (column { style: $style:expr, $($children:tt)* }) => { $crate::ui::declarative::Column { style: $style, children: $crate::rsx!(@children $($children)*) } };
    (column { $($children:tt)* }) => { $crate::rsx!(column { style: $crate::ui::declarative::Style::new(), $($children)* }) };
    (heading { $text:expr }) => { $crate::ui::declarative::Text { text: $text, heading: true, color: None } };
    (label { color: $color:expr, $text:expr }) => { $crate::ui::declarative::Text { text: $text, heading: false, color: Some($color) } };
    (label { $text:expr }) => { $crate::ui::declarative::Text { text: $text, heading: false, color: None } };
    (button { key: $key:expr, onclick: $action:expr, $label:expr }) => { $crate::ui::declarative::Button { key: $key, label: $label, action: $action } };
    (card { title: $title:expr, $detail:expr }) => { $crate::ui::declarative::Card { title: $title, detail: $detail } };
    (space { $height:expr }) => { $crate::ui::declarative::Space($height) };
    (@children) => { $crate::ui::declarative::Empty };
    (@children { $component:expr } $($rest:tt)*) => { $crate::ui::declarative::Children($component, $crate::rsx!(@children $($rest)*)) };
    (@children $kind:ident { $($body:tt)* } $($rest:tt)*) => { $crate::ui::declarative::Children($crate::rsx!($kind { $($body)* }), $crate::rsx!(@children $($rest)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excessive_nesting_is_rejected_before_any_drawing() {
        fn wrap<C>(child: C) -> Column<Children<C, Empty>> {
            Column {
                style: Style::new(),
                children: Children(child, Empty),
            }
        }
        let tree = crate::rsx!(label { "deep" });
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let tree = wrap(tree);
        let mut out = Recording::default();
        assert_eq!(render(&tree, &mut out, 100, 100), Err(RenderError::TooDeep));
        assert!(out.text.is_empty());
    }
    #[test]
    fn exhausted_width_and_empty_viewports_emit_nothing() {
        let tree = crate::rsx!(column { style: Style::new().padding(60), label { "hidden" } });
        let mut out = Recording::default();
        render(&tree, &mut out, 100, 160).unwrap();
        assert!(out.text.is_empty());
        render(&crate::rsx!(label { "hidden" }), &mut out, 0, 100).unwrap();
        render(&crate::rsx!(label { "hidden" }), &mut out, 100, 0).unwrap();
        assert!(out.text.is_empty());
    }
    #[derive(Clone, Debug, PartialEq)]
    enum Action {
        Toggle,
    }
    #[derive(Default)]
    struct Recording {
        text: std::vec::Vec<(Rect, std::string::String, Color)>,
        buttons: std::vec::Vec<(Rect, std::string::String, std::string::String)>,
    }
    impl Renderer<Action> for Recording {
        fn text(&mut self, b: Rect, t: &str, c: Color, _: u32) {
            self.text.push((b, t.into(), c));
        }
        fn button(&mut self, b: Rect, k: &str, l: &str, _: &Action, _: Option<Color>) {
            self.buttons.push((b, k.into(), l.into()));
        }
        fn card(&mut self, _: Rect, _: &str, _: &str, _: Option<Color>) {}
    }
    fn child(active: bool) -> impl View<Action> {
        crate::rsx!(label { if active { "On" } else { "Off" } })
    }
    fn app(active: bool) -> impl View<Action> {
        crate::rsx!(column { style: Style::new().padding(12).gap(4).color(Color::rgb(0x123456)),
            heading { "Settings" }
            { child(active) }
            button { key: "toggle", onclick: Action::Toggle, if active { "Disable" } else { "Enable" } }
        })
    }
    #[test]
    fn functions_compose_styled_views_and_keep_stable_button_keys() {
        for active in [false, true] {
            let mut output = Recording::default();
            render(&app(active), &mut output, 200, 160).unwrap();
            assert_eq!(output.text.len(), 2);
            assert_eq!(
                output.text[0],
                (
                    Rect::new(12, 12, 176, 36),
                    "Settings".into(),
                    Color::rgb(0x123456)
                )
            );
            assert_eq!(output.text[1].0, Rect::new(12, 52, 176, 22));
            assert_eq!(output.text[1].1, if active { "On" } else { "Off" });
            assert_eq!(output.buttons[0].0, Rect::new(12, 78, 176, 38));
            assert_eq!(output.buttons[0].1, "toggle");
            assert_eq!(
                output.buttons[0].2,
                if active { "Disable" } else { "Enable" }
            );
        }
    }
    #[test]
    fn nested_columns_reserve_padding_and_gap_before_siblings() {
        let tree = crate::rsx!(column { style: Style::new().padding(2),
            column { style: Style::new().padding(3).gap(5), label { "A" } label { "B" } }
            label { "C" }
        });
        let mut out = Recording::default();
        render(&tree, &mut out, 100, 160).unwrap();
        assert_eq!(
            out.text
                .iter()
                .map(|(b, _, _)| (b.x, b.y, b.width))
                .collect::<std::vec::Vec<_>>(),
            [(5, 5, 90), (5, 32, 90), (2, 57, 96)]
        );
    }
}
