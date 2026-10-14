//! Dashboard: the whole corpus at a glance (plan §4.1).
//!
//! Everything here is derived, never stored — the three sources are the same ones
//! the other pages use, so the numbers cannot drift from them:
//!
//! * book progress comes from parsing each book's outline (as the outline page
//!   does, `[x]` counts included);
//! * the validation counts come from `validate::validate_all`, the same call the
//!   publish panel makes for its preflight;
//! * "recently modified" is ordered by the frontmatter timestamps the site
//!   publishes, not by file mtimes: mtimes are per-machine and change on a clone.
//!
//! It reads and writes nothing, so it is safe to leave rendering on every notify.

use crate::PublisherApp;
use crate::state::{Config, PostRow};
use astro_publisher_core::{books, outline::Outline, validate};
use gpui::prelude::*;
use gpui::{Context, IntoElement, Window, div, px};
use gpui_component::{ActiveTheme, label::Label, progress::Progress, scroll::ScrollableElement};

/// How many recently-modified posts to list.
const RECENT: usize = 8;

/// One book's syllabus progress.
struct BookRow {
    title: String,
    index_path: String,
    done: usize,
    total: usize,
    sections: usize,
    unwritten: usize,
    error: Option<String>,
}

struct Dashboard {
    posts: usize,
    drafts: usize,
    /// Entries across all books whose 题解 is not linked yet.
    unwritten: usize,
    errors: usize,
    warnings: usize,
    books: Vec<BookRow>,
    /// Indices into the post list, newest first.
    recent: Vec<usize>,
}

fn collect(config: &Config, posts: &[PostRow]) -> Dashboard {
    let blog = config.blog();

    let mut rows = Vec::new();
    let mut unwritten = 0;
    if let Ok(list) = books::list_books(&config.repo()) {
        for book in list {
            // Read the derived path out before the struct literal: taking
            // `book.title` first would leave `book` partially moved.
            let index_path = book.index_path();
            match Outline::read(&blog.join(&index_path)) {
                Ok(outline) => {
                    let (done, total) = outline.progress();
                    let missing = outline
                        .entries
                        .iter()
                        .filter(|entry| !entry.solution.is_linked())
                        .count();
                    unwritten += missing;
                    rows.push(BookRow {
                        title: book.title,
                        index_path,
                        done,
                        total,
                        sections: outline.sections.len(),
                        unwritten: missing,
                        error: None,
                    });
                }
                Err(err) => rows.push(BookRow {
                    title: book.title,
                    index_path,
                    done: 0,
                    total: 0,
                    sections: 0,
                    unwritten: 0,
                    error: Some(err.to_string()),
                }),
            }
        }
    }

    let report = validate::validate_all(&blog);

    let mut stamped: Vec<(String, usize)> = posts
        .iter()
        .enumerate()
        .map(|(index, post)| {
            (
                sortable(post.modified.as_deref().unwrap_or_default()),
                index,
            )
        })
        .collect();
    // Newest first; posts with no timestamp at all sort last on an empty key.
    stamped.sort_by(|a, b| b.0.cmp(&a.0));

    Dashboard {
        posts: posts.len(),
        drafts: posts.iter().filter(|post| post.draft).count(),
        unwritten,
        errors: report.error_count(),
        warnings: report.warning_count(),
        books: rows,
        recent: stamped.into_iter().take(RECENT).map(|(_, i)| i).collect(),
    }
}

/// A sortable form of an ISO-ish timestamp.
///
/// The corpus contains unpadded dates (`2026-1-23T23:00:00.000+08:00` — the
/// site's schema tolerates them), which compare wrongly as plain strings: `1`
/// sorts after `10`. Padding month and day fixes the order without pulling in a
/// date parser for what is only a display sort.
fn sortable(stamp: &str) -> String {
    if stamp.trim().is_empty() {
        // An absent timestamp has to sort before every real one, so that
        // newest-first leaves those posts at the bottom.
        return String::new();
    }
    let (date, rest) = stamp.split_once('T').unwrap_or((stamp, ""));
    let mut parts = date.splitn(3, '-');
    let year = parts.next().unwrap_or_default();
    let pad = |part: Option<&str>| match part.unwrap_or_default() {
        "" => String::new(),
        one if one.len() == 1 => format!("0{one}"),
        other => other.to_owned(),
    };
    format!("{year}-{}-{}T{rest}", pad(parts.next()), pad(parts.next()))
}

/// Render the dashboard.
pub fn render(
    app: &mut PublisherApp,
    _window: &mut Window,
    cx: &mut Context<PublisherApp>,
) -> gpui::AnyElement {
    let data = collect(app.config(), &app.posts);
    // Cloned out before the rows are built: their click handlers need `&mut app`.
    let recent: Vec<PostRow> = data
        .recent
        .iter()
        .map(|index| app.posts[*index].clone())
        .collect();

    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let border = theme.border;
    let danger = theme.danger;
    let success = theme.success;

    let stat = |label: &str, value: String, colour: gpui::Hsla| {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .gap_1()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(border)
            .child(Label::new(value).text_lg().text_color(colour))
            .child(Label::new(label.to_owned()).text_xs().text_color(muted))
    };

    let stats = div()
        .flex()
        .flex_row()
        .w_full()
        .gap_3()
        .child(stat("篇文章", data.posts.to_string(), theme.foreground))
        .child(stat("草稿", data.drafts.to_string(), theme.foreground))
        .child(stat(
            "未关联题解",
            data.unwritten.to_string(),
            theme.warning,
        ))
        .child(stat(
            "校验错误",
            data.errors.to_string(),
            if data.errors == 0 { success } else { danger },
        ))
        .child(stat(
            "校验警告",
            data.warnings.to_string(),
            if data.warnings == 0 {
                success
            } else {
                theme.warning
            },
        ));

    let books_empty = data.books.is_empty();
    let book_cards = data
        .books
        .into_iter()
        .enumerate()
        .map(move |(index, book)| {
            let percent = if book.total == 0 {
                0.0
            } else {
                book.done as f32 * 100.0 / book.total as f32
            };

            div()
                .flex()
                .flex_col()
                .w_full()
                .gap_2()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(Label::new(book.title.clone()).text_sm())
                        .child(
                            Label::new(format!(
                                "{} / {}（{:.0}%）",
                                book.done, book.total, percent
                            ))
                            .text_xs()
                            .text_color(muted),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .child(Progress::new(format!("book-progress-{index}")).value(percent)),
                )
                .child(
                    Label::new(format!(
                        "大纲 {}  ·  {} 个小节  ·  未写题解 {}",
                        book.index_path, book.sections, book.unwritten
                    ))
                    .text_xs()
                    .text_color(muted),
                )
                .children(book.error.map(|err| {
                    Label::new(format!("读取失败：{err}"))
                        .text_xs()
                        .text_color(danger)
                }))
        });

    let recent_rows = recent.into_iter().enumerate().map(|(index, row)| {
        let row_for_click = row.clone();
        div()
            .id(("recent-row", index))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .w_full()
            .px_3()
            .py_2()
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
                    .child(Label::new(row.relative.clone()).text_xs().text_color(muted)),
            )
            .child(
                Label::new(
                    row.modified
                        .clone()
                        .unwrap_or_else(|| "(无时间)".to_owned()),
                )
                .text_xs()
                .text_color(muted),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_post(&row_for_click, window, cx);
            }))
    });

    div()
        .flex()
        .flex_col()
        .size_full()
        .p_4()
        .gap_3()
        // Five stat cards, a card per book and a list of recent posts all add up
        // past the window height.
        .overflow_y_scrollbar()
        .child(Label::new("总览").text_lg())
        .child(stats)
        .child(Label::new("书籍进度").text_sm())
        .when(books_empty, |el| {
            el.child(
                Label::new("没有读到书籍登记表（src/data/books.json）。")
                    .text_xs()
                    .text_color(muted),
            )
        })
        .children(book_cards)
        .child(Label::new(format!("最近修改（前 {RECENT} 篇）")).text_sm())
        .when(recent_rows.len() == 0, |el| {
            el.child(Label::new("还没有文章。").text_xs().text_color(muted))
        })
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(border)
                .children(recent_rows),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_unpadded_dates_so_they_sort_by_time() {
        // The case plain string comparison gets wrong: `2026-1-9` sorts *after*
        // `2026-1-10` because the day is compared character by character. Both
        // shapes occur in the corpus, the unpadded one being the trap.
        let mut stamps = vec![
            "2026-1-9T10:00:00.000+08:00",
            "2026-1-10T10:00:00.000+08:00",
        ];
        stamps.sort_by_key(|stamp| sortable(stamp));
        assert_eq!(stamps[0], "2026-1-9T10:00:00.000+08:00");
        assert_eq!(stamps[1], "2026-1-10T10:00:00.000+08:00");
    }

    #[test]
    fn sortable_keeps_the_time_part_intact() {
        assert_eq!(
            sortable("2026-1-2T09:30:00.000+08:00"),
            "2026-01-02T09:30:00.000+08:00"
        );
        assert_eq!(
            sortable("2026-10-14T22:13:11.000+08:00"),
            "2026-10-14T22:13:11.000+08:00"
        );
    }

    #[test]
    fn a_post_without_a_timestamp_sorts_last() {
        let mut posts = vec!["", "2026-10-14T22:13:11.000+08:00"];
        // Newest first, as the dashboard orders them.
        posts.sort_by(|a, b| sortable(b).cmp(&sortable(a)));
        assert_eq!(posts[0], "2026-10-14T22:13:11.000+08:00");
        assert_eq!(posts[1], "");
    }
}
