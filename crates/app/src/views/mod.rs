//! Views for each section of the app.

pub mod editor;
pub mod outline;
pub mod posts;
pub mod publish;
pub mod settings;

use gpui::prelude::*;
use gpui::{Context, IntoElement, div};
use gpui_component::{ActiveTheme, label::Label};

/// A centred "nothing here yet" panel.
pub fn placeholder(
    title: &str,
    hint: &str,
    cx: &mut Context<crate::PublisherApp>,
) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;

    div()
        .flex()
        .flex_col()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .child(Label::new(title.to_owned()).text_lg())
        .child(Label::new(hint.to_owned()).text_sm().text_color(muted))
}
