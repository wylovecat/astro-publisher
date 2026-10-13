//! Post list view.

use crate::PublisherApp;
use crate::state::PostRow;
use gpui::prelude::*;
use gpui::{Context, IntoElement, Window, div, px};
use gpui_component::{ActiveTheme, label::Label};

/// Render the post table.
pub fn render(
    posts: &[PostRow],
    window: &mut Window,
    cx: &mut Context<PublisherApp>,
) -> gpui::AnyElement {
    let theme = cx.theme();

    if posts.is_empty() {
        return crate::views::placeholder(
            "还没有文章",
            "内容目录里没有找到 Markdown 文件。",
            cx,
        )
        .into_any_element();
    }

    let rows = posts.iter().enumerate().map(|(index, row)| {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let row_for_click = row.clone();
        let is_draft = row.draft;

        div()
            .id(("post-row", index))
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_3()
            .py_2()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|el| el.bg(theme.muted))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(Label::new(row.title.clone()).text_sm())
                    .child(
                        Label::new(format!("{}  →  {}", row.relative, row.url))
                            .text_xs()
                            .text_color(muted),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .children(
                        row.tags
                            .iter()
                            .take(2)
                            .map(|tag| {
                                let tag = tag.clone();
                                div()
                                    .px_2()
                                    .rounded_md()
                                    .bg(theme.muted)
                                    .child(Label::new(tag).text_xs())
                            })
                            .collect::<Vec<_>>(),
                    ),
            )
            .when(is_draft, |el| {
                el.child(
                    div()
                        .px_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .child(Label::new("草稿").text_xs().text_color(muted)),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_post(&row_for_click, window, cx);
            }))
    });

    let header = div()
        .flex()
        .flex_row()
        .w_full()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(theme.border)
        .child(
            Label::new(format!("共 {} 篇（点击任意一行开始编辑）", posts.len()))
                .text_xs()
                .text_color(theme.muted_foreground),
        );

    let _ = window;

    div()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .child(header)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .children(rows),
        )
        .into_any_element()
}
