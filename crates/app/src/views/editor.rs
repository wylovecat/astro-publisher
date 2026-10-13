//! Editor view: Markdown source on one side, live core-rendered preview on the
//! other, with the frontmatter form above.
//!
//! The preview uses `astro_publisher_core::preview` (pulldown-cmark + the KaTeX
//! binding), so mathematics typesets the same way the published site does.
//! gpui-component's own Markdown renderer is deliberately not used for the
//! preview: it does not run KaTeX (verified during the spike).

use astro_publisher_core::{images, preview, text_stats};
use gpui::prelude::*;
use gpui::{
    App, ClipboardEntry, Context, Entity, IntoElement, Render, WeakEntity, Window, div, px,
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
    /// Repository root, for resolving where pasted images go.
    repo: std::path::PathBuf,
    /// Handle used by the paste handler, which only has `&mut App`.
    handle: WeakEntity<EditorView>,
    /// Rendered HTML, refreshed when the document changes.
    preview_html: String,
    stats: preview::TextStats,
    dirty: bool,
    status: String,
}

impl EditorView {
    pub fn new(
        row: crate::state::PostRow,
        repo: std::path::PathBuf,
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
            repo,
            handle: cx.weak_entity(),
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
                                    .appearance(true)
                                    // Images pasted from the clipboard land in
                                    // `public/images/` and become a Markdown
                                    // reference; everything else falls through
                                    // to the editor's normal text paste.
                                    // Captured by value: `on_paste` hands the
                                    // handler `&mut App`, not this view's
                                    // context.
                                    .on_paste({
                                        let repo = self.repo.clone();
                                        let stem = post_stem(&self.row.path);
                                        let source = self.source.clone();
                                        let status = self.handle.clone();
                                        move |item, window, cx| {
                                            paste_image(
                                                &repo, &stem, &source, &status, item, window, cx,
                                            )
                                        }
                                    }),
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

/// The post's file stem, used to name images pasted into it (`lg-P1001`).
fn post_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Store a clipboard image and insert a Markdown reference for it.
///
/// Returns whether the paste was consumed. A clipboard payload with no image is
/// left alone so the editor's normal text paste still works.
///
/// A free function rather than a method because `Editor::on_paste` hands the
/// handler `&mut App`; borrowing the view through its weak handle and passing
/// that same `App` in would alias.
fn paste_image(
    repo: &std::path::Path,
    post_stem: &str,
    source: &Entity<EditorState>,
    status: &WeakEntity<EditorView>,
    item: &gpui::ClipboardItem,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(image) = item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image),
        _ => None,
    }) else {
        return false;
    };

    let outcome = images::store_image(
        repo,
        post_stem,
        image_extension(image.format),
        image.bytes(),
        &stamp(),
    )
    .map(|stored| {
        let markdown = images::markdown_reference(&stored.url_path, "");
        // Insert at the caret so the reference lands where the user pasted.
        source.update(cx, |state, cx| state.insert(markdown, window, cx));
        stored.url_path
    });

    let message = match outcome {
        Ok(url) => format!("已插入图片 {url}"),
        Err(err) => format!("图片粘贴失败：{err}"),
    };
    let _ = status.update(cx, |this, cx| {
        this.status = message;
        cx.notify();
    });

    // Consumed either way: falling through would paste raw image bytes as text,
    // which is worse than reporting the failure.
    true
}

/// The extension `core::images` knows for a clipboard image format.
fn image_extension(format: gpui::ImageFormat) -> &'static str {
    match format {
        gpui::ImageFormat::Png => "png",
        gpui::ImageFormat::Jpeg => "jpg",
        gpui::ImageFormat::Webp => "webp",
        gpui::ImageFormat::Gif => "gif",
        gpui::ImageFormat::Svg => "svg",
        gpui::ImageFormat::Bmp => "bmp",
        gpui::ImageFormat::Tiff => "tiff",
        gpui::ImageFormat::Ico => "ico",
        gpui::ImageFormat::Pnm => "pnm",
    }
}

/// Timestamp for pasted-image file names: `20261013-183000`.
///
/// Built from the core timestamp (`2026-10-13T18:30:00.000+08:00`) so the app
/// does not need its own clock, and so the blog's +08:00 convention holds.
fn stamp() -> String {
    let iso = astro_publisher_core::now_local();
    let digits: String = iso.chars().filter(char::is_ascii_digit).collect();
    // `YYYYMMDDhhmmss…`; keep the first 14 digits and split date from time.
    if digits.len() >= 14 {
        format!("{}-{}", &digits[0..8], &digits[8..14])
    } else {
        digits
    }
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
