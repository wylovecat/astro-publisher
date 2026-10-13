//! Outline view: shows each book's progress and its parsed syllabus.

use crate::state::Config;
use astro_publisher_core::{books, outline::Outline};
use gpui::prelude::*;
use gpui::{Context, IntoElement, div};
use gpui_component::{ActiveTheme, label::Label};

/// A book plus its parsed outline, or the error that stopped us reading it.
struct BookProgress {
    id: String,
    title: String,
    index_path: String,
    sections: usize,
    done: usize,
    total: usize,
    unwritten: Vec<(String, String)>,
    error: Option<String>,
}

fn collect(config: &Config) -> Vec<BookProgress> {
    let blog = config.blog();
    let Ok(list) = books::list_books(&config.repo()) else {
        return Vec::new();
    };

    list.into_iter()
        .map(|book| {
            let path = blog.join(book.index_path());
            match Outline::read(&path) {
                Ok(outline) => {
                    let (done, total) = outline.progress();
                    let unwritten = outline
                        .entries
                        .iter()
                        .filter(|e| !e.solution.is_linked())
                        .take(12)
                        .map(|e| {
                            (
                                e.problem_id.clone().unwrap_or_else(|| "?".to_owned()),
                                e.title.clone(),
                            )
                        })
                        .collect();

                    BookProgress {
                        id: book.id.clone(),
                        title: book.title.clone(),
                        index_path: book.index_path(),
                        sections: outline.sections.len(),
                        done,
                        total,
                        unwritten,
                        error: None,
                    }
                }
                Err(err) => BookProgress {
                    id: book.id.clone(),
                    title: book.title.clone(),
                    index_path: book.index_path(),
                    sections: 0,
                    done: 0,
                    total: 0,
                    unwritten: Vec::new(),
                    error: Some(err.to_string()),
                },
            }
        })
        .collect()
}

/// Render the outline overview.
pub fn render(config: &Config, cx: &Context<crate::PublisherApp>) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let progress = collect(config);

    if progress.is_empty() {
        return div()
            .flex()
            .flex_col()
            .size_full()
            .p_4()
            .child(Label::new("没有读到书籍登记表（src/data/books.json）").text_sm());
    }

    div()
        .flex()
        .flex_col()
        .size_full()
        .p_4()
        .gap_4()
        .overflow_hidden()
        .child(Label::new("课程进度").text_lg())
        .children(progress.into_iter().map(move |book| {
            let percent = if book.total == 0 {
                0
            } else {
                book.done * 100 / book.total
            };

            div()
                .flex()
                .flex_col()
                .w_full()
                .p_3()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(Label::new(book.title.clone()).text_base())
                        .child(
                            Label::new(format!("{} / {} ({}%)", book.done, book.total, percent))
                                .text_sm()
                                .text_color(muted),
                        ),
                )
                .child(
                    Label::new(format!(
                        "id: {}  ·  大纲: {}  ·  {} 个标题",
                        book.id, book.index_path, book.sections
                    ))
                    .text_xs()
                    .text_color(muted),
                )
                .children(book.error.map(|err| {
                    Label::new(format!("读取失败：{err}"))
                        .text_xs()
                        .text_color(theme.danger)
                }))
                .when(!book.unwritten.is_empty(), |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                Label::new("尚未写题解（前 12 题）")
                                    .text_xs()
                                    .text_color(muted),
                            )
                            .children(book.unwritten.into_iter().map(|(id, title)| {
                                Label::new(format!("{id}  {title}")).text_xs()
                            })),
                    )
                })
        }))
}
