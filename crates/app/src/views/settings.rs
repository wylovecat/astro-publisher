//! Settings view: repository path, author, git target, and publish options.
//!
//! The repository path is *editable* here on purpose: an installed copy lives
//! under the user's program directory, where walking up the filesystem can never
//! find the blog. Without this field the installed app would be unusable.
//!
//! The settings file itself lives in the platform config directory, never in the
//! repository.

use crate::PublisherApp;
use gpui::prelude::*;
use gpui::{Context, IntoElement, Window, div, px};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    input::Input,
    label::Label,
};

/// Render the settings panel.
pub fn render(
    app: &mut PublisherApp,
    window: &mut Window,
    cx: &mut Context<PublisherApp>,
) -> impl IntoElement {
    // Copy theme colours out before touching `app` mutably: creating the input
    // entity needs `&mut`, which would otherwise alias the theme borrow.
    let border_color = cx.theme().border;
    let danger_color = cx.theme().danger;
    let muted = cx.theme().muted_foreground;

    let config = app.config().clone();
    let config_path = crate::state::Config::path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(未知)".to_owned());

    let repo_ok = crate::state::looks_like_repo(&config.repo());

    // The editable repository path, created on first render of this page.
    let repo_input = app.repo_input(window, cx);

    let details: [(&str, String); 5] = [
        ("内容目录", config.blog().display().to_string()),
        ("默认作者", config.author.clone()),
        (
            "Git 远端 / 分支",
            format!("{} / {}", config.git_remote, config.git_branch),
        ),
        (
            "发布前本地构建",
            if config.run_local_build {
                "开启".to_owned()
            } else {
                "关闭".to_owned()
            },
        ),
        ("EdgeOne 控制台", config.eo_console_url.clone()),
    ];

    div()
        .flex()
        .flex_col()
        .size_full()
        .p_4()
        .gap_3()
        .overflow_hidden()
        .child(Label::new("设置").text_lg())
        // --- Repository path -------------------------------------------------
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap_2()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(if repo_ok { border_color } else { danger_color })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(Label::new("博客仓库根目录").text_sm())
                        .child(
                            Label::new(if repo_ok {
                                "✓ 有效"
                            } else {
                                "✕ 无效：需要含 package.json 与 src/data/blog"
                            })
                            .text_xs()
                            .text_color(if repo_ok { muted } else { danger_color }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w_full()
                        .child(div().flex_1().child(Input::new(&repo_input)))
                        .child(
                            Button::new("apply-repo")
                                .label("应用并保存")
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_repo_path(window, cx);
                                })),
                        ),
                )
                .child(
                    Label::new(format!("配置文件：{config_path}"))
                        .text_xs()
                        .text_color(muted),
                )
                .child(
                    Label::new(
                        "也可以设环境变量 ASTRO_PUBLISHER_REPO 覆盖（多机同步时用）。\
                         改完点「应用并保存」会写入配置并重新加载文章列表。",
                    )
                    .text_xs()
                    .text_color(muted),
                ),
        )
        // --- Other settings (read-only for now) ------------------------------
        .children(details.into_iter().map(move |(name, value)| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(border_color)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(140.0))
                        .child(Label::new(name.to_owned()).text_sm()),
                )
                .child(Label::new(value).text_xs().text_color(muted))
        }))
}
