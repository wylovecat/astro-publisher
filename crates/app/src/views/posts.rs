//! Post list view.

use crate::PublisherApp;
use crate::state::PostRow;
use gpui::prelude::*;
use gpui::{Context, Entity, IntoElement, Window, div, px};
use gpui_component::scroll::ScrollableElement;
use gpui_component::{ActiveTheme, input::Input, input::InputState, label::Label};

/// Render the post table.
pub fn render(
    posts: &[PostRow],
    search: Entity<InputState>,
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

    // Re-filtered on every render; `PublisherApp` re-renders this view on each
    // keystroke via the input subscription set up in `PublisherApp::new`.
    let query = search.read(cx).value().trim().to_owned();
    let visible: Vec<(usize, &PostRow)> = posts
        .iter()
        .enumerate()
        .filter(|(_, row)| row.matches_query(&query))
        .collect();

    let rows = visible.iter().map(|(index, row)| {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let row_for_click = (*row).clone();
        let is_draft = row.draft;

        div()
            .id(("post-row", *index))
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

    let summary = if query.is_empty() {
        format!("共 {} 篇（点击任意一行开始编辑）", posts.len())
    } else {
        format!("匹配 {} / {} 篇", visible.len(), posts.len())
    };

    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .w_full()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(theme.border)
        .child(
            Label::new(summary)
                .text_xs()
                .text_color(theme.muted_foreground),
        )
        .child(div().w(px(280.0)).flex_shrink_0().child(Input::new(&search)));

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
                // A flex child defaults to a `min-height: auto` floor, which
                // would let the rows push the list past the window instead of
                // scrolling. Zeroing it is what makes the scrollbar appear.
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .when(visible.is_empty(), |el| {
                    // Stays inside the list area on purpose: an early return
                    // covering the whole view would take the search box with
                    // it and leave a mistyped query with no way to clear it.
                    el.items_center()
                        .justify_center()
                        .gap_2()
                        .p_4()
                        .child(Label::new("没有匹配的文章").text_lg())
                        .child(
                            Label::new(format!(
                                "没有标题、路径或标签包含「{query}」的文章。"
                            ))
                            .text_sm()
                            .text_color(theme.muted_foreground),
                        )
                })
                .children(rows),
        )
        .into_any_element()
}