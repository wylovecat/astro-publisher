//! Editor view: Markdown source on one side, live core-rendered preview on the
//! other, with the frontmatter form above.
//!
//! The preview uses `astro_publisher_core::preview` (pulldown-cmark + the KaTeX
//! binding), so mathematics typesets the same way the published site does.
//! gpui-component's own Markdown renderer is deliberately not used for the
//! preview: it does not run KaTeX (verified during the spike).

use astro_publisher_core::{preview, text_stats};
use gpui::prelude::*;
use gpui::{
    Context, Entity, IntoElement, Render, Window, div, px,
};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    input::{Editor, EditorState},
    label::Label,
    text::TextView,
};

/// An open post being edited.
pub struct EditorView {
    pub row: crate::state::PostRow,
    pub source: Entity<EditorState>,
    /// Rendered HTML, refreshed when the document changes.
    preview_html: String,
    stats: preview::TextStats,
    dirty: bool,
    status: String,
}

impl EditorView {
    pub fn new(
        row: crate::state::PostRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = read_whole_file(&row.path).unwrap_or_default();

        let source = cx.new(|cx| {
            EditorState::new(window, cx)
                .default_value(text.clone())
                .language("markdown")
        });

        // Re-render the preview whenever the buffer changes.
        cx.subscribe(&source, |this, _state, event, cx| {
            if matches!(event, gpui_component::input::InputEvent::Change) {
                this.dirty = true;
                this.refresh_preview(cx);
            }
        })
        .detach();

        let rendered = preview::render_preview(&body_of(&text));
        let stats = text_stats(&body_of(&text));

        Self {
            row,
            source,
            preview_html: rendered.html,
            stats,
            dirty: false,
            status: "已载入".to_owned(),
        }
    }

    /// Recompute the preview and statistics from the current buffer.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        let text = self.source.read(cx).value().to_string();
        let body = body_of(&text);
        let rendered = preview::render_preview(&body);
        self.stats = text_stats(&body);
        self.preview_html = rendered.html;
        self.status = if rendered.math_failures.is_empty() {
            "预览已更新".to_owned()
        } else {
            format!("{} 个公式未渲染", rendered.math_failures.len())
        };
        cx.notify();
    }

    /// Persist the buffer back to disk.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.source.read(cx).value().to_string();
        match std::fs::write(&self.row.path, text) {
            Ok(()) => {
                self.dirty = false;
                self.status = "已保存".to_owned();
            }
            Err(err) => {
                self.status = format!("保存失败：{err}");
            }
        }
        cx.notify();
    }

    fn frontmatter_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(Label::new(self.row.title.clone()).text_base())
                    .child(
                        Label::new(format!(
                            "{}  ·  {}",
                            self.row.relative,
                            if self.row.draft { "草稿" } else { "已发布" }
                        ))
                        .text_xs()
                        .text_color(muted),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        Label::new(format!(
                            "{} 字 · {} 行 · 约 {} 分钟",
                            self.stats.cjk_chars + self.stats.words,
                            self.stats.lines,
                            self.stats.reading_minutes
                        ))
                        .text_xs()
                        .text_color(muted),
                    )
                    .child(
                        Label::new(if self.dirty {
                            format!("{}（未保存）", self.status)
                        } else {
                            self.status.clone()
                        })
                        .text_xs()
                        .text_color(if self.dirty { theme.warning } else { muted }),
                    )
                    .child(
                        Button::new("save")
                            .label("保存")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out first: `frontmatter_bar` needs `&mut Context`,
        // and holding a theme borrow across that call would alias it.
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let bar = self.frontmatter_bar(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(bar)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    // Source
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .min_w(px(0.0))
                            .border_r_1()
                            .border_color(border)
                            .child(
                                div().px_3().py_1().child(
                                    Label::new("Markdown 源码")
                                        .text_xs()
                                        .text_color(muted),
                                ),
                            )
                            // Give the editor a definite height from the pane's
                            // resolved box. gpui-component's multi-line input
                            // sets `height: relative(1.0)`, which needs a parent
                            // with a known height; a flex fraction alone leaves
                            // it at its minimum-row floor (a couple of lines).
                            .child(
                                Editor::new(&self.source)
                                    .h(gpui::relative(1.0))
                                    .appearance(true),
                            ),
                    )
                    // Preview
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .child(
                                div().px_3().py_1().child(
                                    Label::new("预览（与站点同源：KaTeX）")
                                        .text_xs()
                                        .text_color(muted),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .w_full()
                                    .min_h(px(0.0))
                                    .px_3()
                                    .overflow_hidden()
                                    .child(TextView::html(
                                        "editor-preview",
                                        self.preview_html.clone(),
                                    )),
                            ),
                    ),
            )
    }
}

/// Read a file as UTF-8, tolerating a BOM.
fn read_whole_file(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
}

/// The body of a document, i.e. everything after the frontmatter block.
///
/// The preview intentionally renders the body only: frontmatter is edited
/// through the form, and rendering it as text would be noise.
fn body_of(text: &str) -> String {
    astro_publisher_core::frontmatter::MarkdownFile::parse(text, "<buffer>")
        .map(|file| file.body)
        .unwrap_or_else(|_| text.to_owned())
}
