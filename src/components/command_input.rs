//! The launcher's search field: what people type into to find anything.

use gpui_kit::assets::IconName;
use gpui_kit::base::h_flex;
use gpui_kit::base::input::{Input, InputAccessibility, InputEditorStyle, InputState};
use gpui_kit::*;

use crate::theme::ActiveTheme;

/// The search field at the top of the launcher.
///
/// The owner keeps the query's `Entity<InputState>`, so it can read and react
/// to the text; this element only presents it, with a leading search icon.
#[derive(IntoElement)]
pub struct CommandInput {
    state: Entity<InputState>,
}

impl CommandInput {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl RenderOnce for CommandInput {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let style = InputEditorStyle {
            foreground: theme.foreground,
            muted_foreground: theme.muted_foreground,
            selection: theme.selection,
            caret: theme.foreground,
            ..InputEditorStyle::default()
        };
        let muted_foreground = theme.muted_foreground;

        // Applied every render, so the field follows the current theme. Neither
        // setter notifies, so this cannot cause another render.
        self.state.update(cx, |state, _| {
            state.set_editor_style(style);
            state.set_accessibility(
                InputAccessibility::new(Some(Role::TextInput)).with_label(Some("Search".into())),
            );
        });

        h_flex()
            .w_full()
            .gap_3()
            .px_4()
            .py_3()
            .text_lg()
            .child(div().text_color(muted_foreground).child(IconName::Search))
            .child(div().flex_1().child(Input::new(&self.state)))
    }
}
