//! Hermit's look. Hermit paints every surface itself on gpui-kit's unstyled
//! Base layer, so this is the one place colors and shapes are defined.

use gpui_kit::*;

pub struct Theme {
    /// The launcher surface. Translucent, so the blurred desktop shows through.
    pub background: Hsla,
    pub foreground: Hsla,
    /// Secondary text and icons, such as the search field's placeholder.
    pub muted_foreground: Hsla,
    /// Behind selected text; translucent, so the selected glyphs stay legible.
    pub selection: Hsla,
    /// Hairlines between regions.
    pub border: Hsla,
    /// The launcher window's corner radius. In pixels, not rems, because the
    /// compositor shapes its blur with it and knows nothing of rem sizes.
    pub window_radius: Pixels,
}

impl Global for Theme {}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: hsla(0., 0., 0., 0.75),
            foreground: hsla(0., 0., 0.98, 1.),
            muted_foreground: hsla(0., 0., 0.98, 0.55),
            selection: hsla(0., 0., 1., 0.25),
            border: hsla(0., 0., 1., 0.1),
            window_radius: px(16.),
        }
    }
}

pub fn init(cx: &mut App) {
    cx.set_global(Theme::default());
}

/// `cx.theme()` in render code.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}
