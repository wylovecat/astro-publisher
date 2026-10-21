//! Outline view: each book's progress plus one-click chapter creation.
//!
//! The outline is where the remaining work is visible, so it is also where the
//! work should start: every entry whose 题解 marker is still a placeholder gets a
//! 新建 button. The entry already carries the problem id, title and section, so
//! the button needs no form — it calls `chapters::create_chapter_from_entry` and
//! reports what was written.
//!
//! Creating walks the corpus to allocate the site-wide `_0xNNNN` sequence, so it
//! runs on a background task. The report lives on the book card rather than on
//! the row, because a created entry leaves the "unwritten" list immediately and
//! could not report its own row afterwards.

use crate::state::Config;
use astro_publisher_core::books::{self, Book};
use astro_publisher_core::chapters::{self, ChapterOutcome};
use astro_publisher_core::outline::{Outline, UnwrittenEntry};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, IntoElement, Render, SharedString, Window, div, px};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    label::Label,
    scroll::ScrollableElement,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

/// How many unwritten entries a book shows before it is collapsed.
const PREVIEW_LIMIT: usize = 12;

/// Identifies one entry across the whole view: line indexes are unique per file,
/// so book id plus line stays a valid key across reloads.
type EntryKey = (String, usize);

/// A book plus its parsed outline, or the error that stopped us reading it.
#[derive(Clone)]
struct BookProgress {
    book: Book,
    sections: usize,
    done: usize,
    total: usize,
    unwritten: Vec<UnwrittenEntry>,
    error: Option<String>,
}

/// What one row is currently doing.
#[derive(Clone)]
enum RowState {
    Idle,
    Creating,
    Failed(String),
}

/// The last thing that happened to one book, shown under its title.
#[derive(Clone)]
struct Notice {
    message: String,
    failed: bool,
}

/// Announced after a successful creation so the app can reload its post list:
/// a chapter written here is a new file on disk, and the 文章 page should show it
/// without the user reaching for 重新加载.
pub enum OutlineEvent {
    Created { title_line: String },
}

impl EventEmitter<OutlineEvent> for OutlineView {}

pub struct OutlineView {
    config: Config,
    /// Files this session wrote, so a publish commits the chapter that was just
    /// created rather than whatever else the checkout contains.
    session: crate::state::SessionFiles,
    books: Vec<BookProgress>,
    /// Book id to its most recent outcome.
    notices: HashMap<String, Notice>,
    /// Rows with a creation in flight, so the spinner survives a reload.
    busy: HashSet<EntryKey>,
    /// Rows whose creation failed, so the row offers a retry.
    failed: HashMap<EntryKey, String>,
    /// `[x]` tracks the solution link (lgj semantics); `lgs` tracks videos.
    check_implies_solution: bool,
    /// Books whose unwritten list is expanded past `PREVIEW_LIMIT`.
    expanded: HashSet<String>,
}

impl OutlineView {
    pub fn new(
        config: Config,
        session: crate::state::SessionFiles,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            config,
            session,
            books: Vec::new(),
            notices: HashMap::new(),
            busy: HashSet::new(),
            failed: HashMap::new(),
            check_implies_solution: true,
            expanded: HashSet::new(),
        };
        view.reload();
        view
    }

    /// Re-read every book outline from disk.
    ///
    /// Cheap enough (a handful of Markdown files) to do on the UI thread, which is
    /// also what makes the progress numbers update the moment a creation finishes.
    fn reload(&mut self) {
        self.books = collect(&self.config);
        // A failed key whose row is no longer listed is dead weight.
        let live: HashSet<EntryKey> = self
            .books
            .iter()
            .flat_map(|book| {
                book.unwritten
                    .iter()
                    .map(move |entry| (book.book.id.clone(), entry.line))
            })
            .collect();
        self.failed.retain(|key, _| live.contains(key));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload();
        cx.notify();
    }

    /// Re-read the book registry and outlines. Called when Settings adds a
    /// book; creations in flight are keyed by (book id, line), so they simply
    /// drop out of the rows that no longer exist.
    pub fn reload_books(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    fn row_state(&self, book_id: &str, line: usize) -> RowState {
        let key = (book_id.to_owned(), line);
        if self.busy.contains(&key) {
            RowState::Creating
        } else if let Some(message) = self.failed.get(&key) {
            RowState::Failed(message.clone())
        } else {
            RowState::Idle
        }
    }

    /// Create the chapter for one unwritten entry.
    ///
    /// Validates the entry before touching the disk: a row with no problem id has
    /// no file name to write, and that is worth saying without a background round
    /// trip.
    fn create(&mut self, book_index: usize, line: usize, cx: &mut Context<Self>) {
        let Some(progress) = self.books.get(book_index) else {
            return;
        };
        let book_id = progress.book.id.clone();
        let Some(entry) = progress.unwritten.iter().find(|e| e.line == line) else {
            return;
        };
        let entry = entry.clone();
        let book_ref = progress.book.as_book_ref();
        let key = (book_id.clone(), line);

        // `Button` has no `disabled` state in this version, and `loading` only
        // swaps the icon, so a second click during a run has to be refused here —
        // it would otherwise allocate a second slug for the same problem.
        if self.busy.contains(&key) {
            return;
        }

        if let Err(err) = chapters::input_from_entry(
            &entry,
            &self.config.author,
            self.check_implies_solution,
        ) {
            self.failed.insert((book_id, line), err.to_string());
            cx.notify();
            return;
        }

        let blog = self.config.blog();
        let author = self.config.author.clone();
        let check = self.check_implies_solution;
        let repo = self.config.repo();

        // Read the chapter template before anything is written: an unreadable
        // configured template is reported instead of silently falling back.
        let template = match self.config.chapter_template_text(&repo) {
            Ok(template) => template,
            Err(message) => {
                self.failed.insert(key.clone(), message);
                cx.notify();
                return;
            }
        };

        self.busy.insert(key.clone());
        self.failed.remove(&key);
        cx.notify();

        let (tx, rx) = mpsc::channel::<Result<ChapterOutcome, String>>();
        let work = cx.background_executor().spawn(async move {
            let outcome = chapters::create_chapter_from_entry(
                &blog,
                &book_ref,
                &entry,
                &author,
                check,
                template.as_deref(),
            )
            .map_err(|err| err.to_string());
            let _ = tx.send(outcome);
        });

        cx.spawn(async move |this, cx| {
            work.await;
            let outcome = rx.recv().ok();
            let key = key.clone();
            this.update(cx, |this, cx| {
                this.busy.remove(&key);
                match outcome {
                    Some(Ok(outcome)) => {
                        // Two files changed: the chapter and the outline line.
                        this.session.record(&repo, &outcome.chapter_path);
                        this.session.record(&repo, &outcome.outline_path);
                        this.notices.insert(
                            key.0.clone(),
                            Notice {
                                message: format!(
                                    "已创建 {}（{}）· {}",
                                    outcome.title_line,
                                    outcome.slug,
                                    outcome.chapter_path.display()
                                ),
                                failed: false,
                            },
                        );
                        cx.emit(OutlineEvent::Created {
                            title_line: outcome.title_line.clone(),
                        });
                    }
                    Some(Err(message)) => {
                        this.failed.insert(key.clone(), message.clone());
                        this.notices.insert(
                            key.0.clone(),
                            Notice {
                                message,
                                failed: true,
                            },
                        );
                    }
                    None => {
                        this.notices.insert(
                            key.0.clone(),
                            Notice {
                                message: "创建过程没有返回结果".to_owned(),
                                failed: true,
                            },
                        );
                    }
                }
                this.reload();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The entries of one book to show, plus how many the list is hiding.
    fn visible_unwritten<'a>(
        &self,
        progress: &'a BookProgress,
    ) -> (Vec<&'a UnwrittenEntry>, usize, bool) {
        let total = progress.unwritten.len();
        let expanded = self.expanded.contains(&progress.book.id);
        let take = if expanded { total } else { PREVIEW_LIMIT };
        (
            progress.unwritten.iter().take(take).collect(),
            total,
            expanded,
        )
    }

    /// One book card: title, progress, notice, and the creatable entries.
    fn book_card(&self, book_index: usize, progress: &BookProgress, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let book_id = progress.book.id.clone();
        let percent = progress.done.saturating_mul(100) / progress.total.max(1);
        let (visible, total, expanded) = self.visible_unwritten(progress);
        let hidden = total.saturating_sub(visible.len());

        // Rows are built here rather than in a method taking `&self`, so the
        // listener closures below can take `&mut cx` without aliasing.
        let rows = visible.into_iter().map(|entry| {
            let state = self.row_state(&book_id, entry.line);
            // A plain-text entry has no problem id, so there is no file to name
            // the chapter after; a button that could only fail would be a lie.
            let creatable = entry
                .problem_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty());

            // Captured by the click listener, so copy the index out rather than
            // dragging a borrow of `progress` into the closure.
            let line = entry.line;
            let action: gpui::AnyElement = if creatable {
                Button::new(SharedString::from(format!(
                    "outline-create-{book_index}-{}",
                    entry.line
                )))
                    .label(match &state {
                        RowState::Creating => "新建中…",
                        RowState::Failed(_) => "重试",
                        RowState::Idle => "新建",
                    })
                    .small()
                    .primary()
                    .loading(matches!(state, RowState::Creating))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.create(book_index, line, cx);
                    }))
                    .into_any_element()
            } else {
                Label::new("这行没有题号，无法自动命名")
                    .text_xs()
                    .text_color(muted)
                    .into_any_element()
            };

            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .child(
                    div().flex_1().min_w(px(0.0)).child(
                        Label::new(format!(
                            "{}  {}",
                            entry
                                .problem_id
                                .clone()
                                .unwrap_or_else(|| "(无题号)".to_owned()),
                            entry.title
                        ))
                        .text_xs(),
                    ),
                )
                .child(Label::new(entry.section.clone()).text_xs().text_color(muted))
                .when_some(match state {
                    RowState::Failed(message) => Some(message),
                    _ => None,
                }, |el, message| {
                    el.child(Label::new(message).text_xs().text_color(theme.danger))
                })
                .child(action)
                .into_any_element()
        });

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
                    .child(Label::new(progress.book.title.clone()).text_base())
                    .child(
                        Label::new(format!(
                            "{} / {} ({}%)",
                            progress.done, progress.total, percent
                        ))
                        .text_sm()
                        .text_color(muted),
                    ),
            )
            .child(
                Label::new(format!(
                    "id: {}  ·  大纲: {}  ·  {} 个标题",
                    book_id,
                    progress.book.index_path(),
                    progress.sections
                ))
                .text_xs()
                .text_color(muted),
            )
            .when_some(progress.error.clone(), |el, err| {
                el.child(
                    Label::new(format!("读取失败：{err}"))
                        .text_xs()
                        .text_color(theme.danger),
                )
            })
            .when_some(self.notices.get(&book_id).cloned(), |el, notice| {
                el.child(
                    Label::new(format!(
                        "{} {}",
                        if notice.failed { "✗" } else { "✓" },
                        notice.message
                    ))
                    .text_xs()
                    .text_color(if notice.failed {
                        theme.danger
                    } else {
                        theme.success
                    }),
                )
            })
            .when(!progress.unwritten.is_empty(), |el| {
                let expand_id = book_id.clone();
                let is_expanded = expanded;
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(
                                    Label::new(if expanded {
                                        format!("尚未写题解（全部 {total} 题）")
                                    } else {
                                        format!("尚未写题解（前 {PREVIEW_LIMIT} 题，共 {total} 题）")
                                    })
                                    .text_xs()
                                    .text_color(muted),
                                )
                                .when(hidden > 0 || expanded, |el| {
                                    el.child(
                                        Button::new(SharedString::from(format!(
                                            "outline-expand-{book_index}"
                                        )))
                                            .label(if is_expanded {
                                                "只看前 12 题"
                                            } else {
                                                "显示全部"
                                            })
                                            .small()
                                            .ghost()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if is_expanded {
                                                    this.expanded.remove(&expand_id);
                                                } else {
                                                    this.expanded.insert(expand_id.clone());
                                                }
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                        .children(rows),
                )
            })
            .when(
                progress.error.is_none() && progress.unwritten.is_empty(),
                |el| el.child(Label::new("这一本的题目都写完了。").text_xs().text_color(muted)),
            )
            .into_any_element()
    }
}

impl Render for OutlineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        if self.books.is_empty() {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .p_4()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Label::new("没有读到书籍登记表（src/data/books.json）").text_sm())
                .child(
                    Label::new(self.config.blog().display().to_string())
                        .text_xs()
                        .text_color(muted),
                )
                .into_any_element();
        }

        let cards: Vec<gpui::AnyElement> = (0..self.books.len())
            .filter_map(|index| self.books.get(index).map(|b| self.book_card(index, b, cx)))
            .collect();

        div()
            .flex()
            .flex_col()
            .size_full()
            .p_4()
            .gap_3()
            // One card per book listing up to a dozen entries each, so this list
            // routinely outgrows the window.
            .overflow_y_scrollbar()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .gap_3()
                    .child(
                        Checkbox::new("outline-check-implies-solution")
                            .label("题解写完后勾选 [x]（进阶篇用视频标记进度，可关掉）")
                            .checked(self.check_implies_solution)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.check_implies_solution = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("outline-reload")
                            .label("重新读取大纲")
                            .small()
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                Label::new("点「新建」会按大纲里已有的题号、题目名和小节写好章节文件，并把题解链接回填到那一行。")
                    .text_xs()
                    .text_color(muted),
            )
            .children(cards)
            .into_any_element()
    }
}

/// Read every book outline and summarise its progress.
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
                    BookProgress {
                        book,
                        sections: outline.sections.len(),
                        done,
                        total,
                        unwritten: outline.unwritten(),
                        error: None,
                    }
                }
                Err(err) => BookProgress {
                    book,
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

/// The view as an entity, so the app can keep it across page switches.
pub type OutlineEntity = Entity<OutlineView>;
