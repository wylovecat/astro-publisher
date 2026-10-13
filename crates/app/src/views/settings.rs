//! Settings view: repository path, author, git target, and publish options.
//!
//! The settings file lives in the platform config directory, never in the
//! repository (plan §2.2).

use crate::state::Config;
use gpui::prelude::*;
use gpui::{Context, IntoElement, div, px};
use gpui_component::{ActiveTheme, Sizable, button::{Button, ButtonVariants}, label::Label};

/// Render the settings panel.
pub fn render(config: &Config, cx: &mut Context<crate::PublisherApp>) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;

    let config_path = Config::path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(未知)".to_owned());

    let repo_ok = crate::state::looks_like_repo(&config.repo());

    let rows: [(&str, String); 6] = [
        ("仓库根目录", config.repo_path.clone()),
        ("内容目录", config.blog().display().to_string()),
        ("默认作者", config.author.clone()),
        ("Git 远端 / 分支", format!("{} / {}", config.git_remote, config.git_branch)),
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
        .child(
            Label::new(if repo_ok {
                "✓ 仓库路径有效"
            } else {
                "✕ 当前路径不像博客仓库（缺少 src/data/blog 或 package.json）"
            })
            .text_xs()
            .text_color(if repo_ok { muted } else { theme.danger }),
        )
        .children(rows.into_iter().map(|(name, value)| {
            let muted = theme.muted_foreground;
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(140.0))
                        .child(Label::new(name.to_owned()).text_sm()),
                )
                .child(Label::new(value).text_xs().text_color(muted))
        }))
        .child(
            Button::new("save-config")
                .label("写回配置文件")
                .small()
                .secondary()
                .on_click(cx.listener(|this, _, _, cx| match this.config.clone().save() {
                    Ok(()) => this.set_notice("配置已保存".to_owned(), cx),
                    Err(err) => this.set_notice(format!("保存失败：{err}"), cx),
                })),
        )
        .child(
            Label::new(format!("配置文件：{config_path}"))
                .text_xs()
                .text_color(muted),
        )
        .child(
            Label::new("仓库路径可用环境变量 LLGORITHMNOTE_REPO 覆盖（多机同步时用）。")
                .text_xs()
                .text_color(muted),
        )
}
