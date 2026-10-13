# astro-paper publisher

一个面向 **Astro 博客** 的桌面发布工具：把「新增文章 / 新增书籍章节 / 关联题解与视频 / 校验 / 发布上线」从手写 Markdown + 命令行，变成表单 + 按钮。

技术栈是 **Rust + [GPUI](https://gpui.rs)**（UI 用 [gpui-component](https://gpui-kit.com) 0.7.x），产出单一原生二进制，Windows / Linux / macOS 共用一套代码。

> 本项目最初为一个中文算法竞赛教学博客（Astro + AstroPaper）而写，因此默认约定偏该场景：课程大纲即目录、章节用 `_0xNNNN` 顺序 slug、一键推送到支持 Git 集成的静态托管平台。这些约定都集中在少数几个模块里，改成别的 Astro 博客形态并不困难。

## 能力

- **文章管理**：列表、搜索、frontmatter 表单（标题 / 标签 / 草稿 / 特色 / 时间），点开即编辑。
- **章节创建（核心）**：一次操作同时生成章节文件并更新课程大纲，自动分配全站递增的 `_0xNNNN` slug；任一步失败整体回滚。题号可一键从洛谷抓回题目名 / 难度 / 标签。
- **大纲维护**：解析 Part / 章 / 小节 / 题目四层结构，支持勾选、关联题解、关联视频、插入新行与新标题。
- **校验**：frontmatter 结构检查 + 大纲里 `/posts/...` 链接的坏链检查；错误阻塞发布，警告不阻塞。
- **预览**：`pulldown-cmark` 解析 Markdown，公式交给 **KaTeX 本体** 渲染（与站点 rehype-katex 同源），不是近似实现。
- **发布流水线**：校验 → 本地构建（权威校验）→ `git commit` → `git push` → 托管平台自动构建。推送失败不强推、不自动 rebase。
- **无头 CLI**：以上全部能力都有命令行入口，便于脚本化或在没有图形环境的机器上使用。

## 目录结构

```
├── Cargo.toml                    # workspace（Cargo.lock 入库以锁定 GPUI 生态版本）
├── crates/
│   ├── core/                     # 内容操作库：纯 Rust、无 UI 依赖、可 cargo test
│   │   ├── src/frontmatter.rs    #   frontmatter 解析 / 只重写围栏区的写回
│   │   ├── src/outline.rs        #   课程大纲行解析 + 外科手术式写回
│   │   ├── src/chapters.rs       #   preview_chapter（只读预演）/ create_chapter（原子多文件 + 回滚）
│   │   ├── src/slug.rs           #   _0xNNNN 序列扫描与分配
│   │   ├── src/posts.rs          #   文章读取 / 列表 / tags
│   │   ├── src/books.rs          #   books.json 读写
│   │   ├── src/problem.rs        #   洛谷题目信息抓取（题号 → 题目名 / 难度 / 标签，带磁盘缓存）
│   │   ├── src/images.rs         #   粘贴图片的落盘位置与 Markdown 引用
│   │   ├── src/validate.rs       #   全量校验（frontmatter 镜像 + 大纲坏链）
│   │   ├── src/preview.rs        #   pulldown-cmark + KaTeX → HTML
│   │   ├── src/publish.rs        #   校验 → 构建 → 提交 → 推送（CLI 与 GUI 共用，事件流式上报）
│   │   ├── src/process.rs        #   子进程（pnpm / git），UTF-8 流式输出
│   │   └── src/bin/cli.rs        #   无头 CLI
│   └── app/                      # GPUI 桌面应用（侧边栏 + 列表 + 编辑器 + 章节向导 + 发布 + 设置）
│       └── src/views/
└── tests/fixtures/               # 合成测试内容（不含任何真实博客数据）
```

## 构建与运行

```bash
cargo build --release        # 首次编译较久（GPUI + tree-sitter），之后增量很快
cargo test                   # 40 个测试，全部基于合成 fixture

# 桌面应用
cargo run -p astro-publisher
cargo run -p astro-publisher -- --open 3       # 直接打开列表里第 4 篇文章

# CLI（无需图形环境）
cargo run -p astro-publisher-core -- check   --repo /path/to/blog
cargo run -p astro-publisher-core -- books   --repo /path/to/blog
cargo run -p astro-publisher-core -- posts   --repo /path/to/blog
cargo run -p astro-publisher-core -- preview --file /path/to/blog/src/data/blog/some-post.md
```

`--repo` 指向博客仓库根目录；工具从 `<repo>/src/data/blog` 与 `<repo>/src/data/books.json` 读取内容。

**应用会自动找仓库根目录**，不需要先配置：先看环境变量 `ASTRO_PUBLISHER_REPO`，再从当前工作目录逐级向上找，最后从可执行文件所在目录逐级向上找（判定标准是同时存在 `package.json` 与 `src/data/blog`）。所以 `cargo run`、双击可执行文件、从任意子目录启动都能定位到仓库；配置里存的路径若失效会自动重新探测。

### 平台依赖

- **Windows**：MSVC 工具链（Visual Studio Build Tools 即可）。
- **Linux**：X11/Wayland 与 Vulkan/Mesa 相关开发包（`libxcb`、`libwayland`、Mesa 驱动等）。
- **Node 侧**：使用发布流水线时需要仓库自身的 `pnpm` 与 Node（构建 Astro 站点用），否则工具本身不依赖 Node。

## CLI 命令

| 命令 | 说明 |
| --- | --- |
| `check` | 校验全部文章的 frontmatter 与大纲链接（错误阻塞发布，警告不阻塞） |
| `books` | 列出 `books.json` 中登记的书籍 |
| `posts` | 列出文章（`--drafts` 含草稿） |
| `new-post` | 新建普通文章：`--title --group <dir> --file-name <name> [--tag t]... [--draft]` |
| `new-chapter` | **新建课程章节**：`--book <id> --section "<小节>" --problem <P1001> --title <题目名> [--video <url>]` |
| `preview` | 渲染 Markdown 预览（`--file` 或 `--text`），公式走真 KaTeX |
| `publish` | 校验 → `pnpm build` → `git commit` → `git push`（`--dry-run` 演练，`--no-build` 跳过构建） |

`new-chapter` 是核心操作：它分配全站 `_0xNNNN` 序列 slug、生成章节文件、并把大纲对应行更新成 `📖[题解](/posts/<group>/<slug>)`。整个过程是原子的——任一步失败都会删掉已建文件、还原大纲。

## 应用内操作

- **新增章节**（侧边栏）：选书 → 选小节 → 填题号 → 点「抓取」自动取回题目名（可手动改）→ 填视频链接 → 「将执行的操作」实时显示确切的 slug、章节文件路径与改写后的大纲行 → 点「创建章节」。勾选框控制 `[x]` 是否跟随题解（基础篇用勾选表示进度，进阶篇用视频标记）。
- **编辑文章**：源码 + 实时预览并排。编辑器里 **Ctrl+V 粘贴截图**会写入仓库 `public/images/`，并在光标处插入 `![](/images/<文章名>-<时间戳>.png)`；写入失败会报原因而不是把二进制当文本贴进去。非图片的剪贴板内容照旧走普通文本粘贴。
- **发布**：填提交信息 → 「发布」跑完整流水线，「演练」只校验与构建不提交，「停止」会在下一个安全点中止（正在构建时会结束构建进程）。左侧是五个步骤的实时状态，右侧是校验、构建与 git 的逐行输出。推送失败只提示手动处理，不做自动 rebase 或强推。

发布流水线实现在 `core/src/publish.rs`，CLI 的 `publish` 与应用里的按钮共用同一份，行为一致。

「抓取」走 `core/src/problem.rs`：一次 GET 题目页，解析页面内嵌的 `lentille-context` JSON 得到题目名、难度与标签。结果是纯函数解析，网络调用可注入，所以测试不需要联网（唯一联网的测试标了 `#[ignore]`，用 `cargo test -p astro-publisher-core -- --ignored live_lookup` 手动跑）。抓取结果按题号缓存在平台缓存目录，同一题第二次查不再联网；抓取失败只提示原因，题目名照旧手填，不影响创建。

## 发布链路

```
校验 → pnpm build（权威校验）→ git commit → git push origin main
                                                ↓
                       静态托管平台检测到 main 推送 → 自动构建部署
```

约定：**推送即发布**。本地构建用来在推送前拦下 frontmatter 错误（`astro check`）；推送失败时不做自动 rebase、不强推，只提示手动处理。

> 注意：本工具不会替你决定「提交哪些文件」。CLI 的 `publish` 使用 `git add -A`，请先用 `--dry-run` 确认待提交内容。

## 内容约定

工具对博客仓库有若干约定，改动内容结构时请对照：

- **frontmatter 只重写 `---` 围栏区**，正文逐字节保留（含代码块里的 `---`）；行尾风格（LF/CRLF）与 BOM 会原样保留。
- **大纲行**：列表符 `-` / `*` 混用都支持，写回时保留该行原有风格。
  - 题解标记：`📖题解`（未写）↔ `📖[题解](/posts/<group>/<slug>)`（已写）
  - 视频标记：`📺视频`（占位）↔ `📺[视频](<url>)`（已发布）
  - 题号从题目链接的 URL 里提取，不做括号配对（标题可能含全角括号）。
  - `[x]` 勾选的含义可按书配置：演示用的基础篇中「勾选 ⇔ 题解已写」，由工具同步维护；进阶篇用视频标记进度，工具不改其勾选语义。
- **`_0xNNNN` slug 是全站统一序列**，下一个 = 全站最大 + 1。
- 同一题号在大纲中可出现多次，写操作按「行号 + 小节」定位，不做全局替换。
- **粘贴的图片落在 `public/images/`**，文件名是 `<文章名>-<YYYYMMDD-HHMMSS>.<ext>`，正文里用绝对路径 `/images/<文件名>` 引用（`public/` 下的东西站点按根路径提供）。只接受站点能直接服务的扩展名（png/jpg/jpeg/webp/gif/svg/bmp/tiff/ico）。
- 书籍登记在 `src/data/books.json`（见下），每条至少要有 `id`、`title`、`description`、`indexId`（指向大纲文章）。

## 与博客仓库的集成

只需一处改动：书籍数据放在 `src/data/books.json`，由 `src/data/books.ts` 薄包装导出类型，站点其余代码无需改动。工具通过 JSON 读写书籍登记，避免解析 TypeScript 源码。

若把工具放在博客仓库的子目录里，建议在博客的 lint / format 配置中排除该目录（例如 Prettier 白名单机制、ESLint ignore），并忽略 Rust 构建产物 `target/`。

## 已知坑（改 UI 前请读）

- **`Editor` 必须显式给高度**：用 `Editor::new(&state).h(relative(1.0))`。gpui-component 的多行输入内部是 `height: relative(1.0)`，只靠外层 `flex_1` 会让百分比对上不确定的盒子，编辑器塌缩到只显示两三行。
- **按钮用 `Button` 组件**，不要手写 `div().bg(theme.primary)`：需要同时导入 `button::{Button, ButtonVariants}` 与 `Sizable` 才能用 `.primary()` / `.small()`。手写 div 会渲染成一块没有文字的色块。
- 主题里的 `primary` 本身是近黑色（shadcn 的 `neutral-900`），这是设计如此，不是 bug。
- **`katex` crate 的默认后端不支持 MSVC**：默认 `quick-js` 只支持 unix 或 windows-gnu，Windows MSVC 需要 `duktape` 后端（见 `Cargo.toml` 注释）。
- **滚动要用 `overflow_scrollbar()`**：`Styled` 只生成 `overflow_hidden` 系列，滚动能力在 gpui-component 的 `ScrollableElement` 上，需要 `use gpui_component::scroll::ScrollableElement`。
- **建辅助 UI 函数时返回 `gpui::AnyElement`**：`impl IntoElement` 的返回类型会把 `&mut Context` 的借用带进元素类型，导致后面再拿同一个 `cx` 借用时报 `E0502`；`AnyElement` 不带生命周期。`Render::render` 本身仍按 trait 声明写 `impl IntoElement`。
- **gpui 依赖要对齐版本**：`gpui-component` 依赖的是包名为 `gpui-pre` 的 gpui，而 crates.io 上另有一个无关的 `gpui`；直接用后者会导致 `Render` trait 不匹配。`Cargo.toml` 里已用包别名处理。
- **图标资源必须显式注册**：`gpui_component::init(cx)` **不注册**图标资源（最小化/最大化/关闭等窗口按钮、按钮图标全部静默渲染成空白，不报错）。图标在独立 crate `gpui-kit-assets`，启动时须 `application().with_assets(gpui_kit_assets::Assets).run(...)`。忘记这一步时窗口右上角三个按钮完全消失，症状是「按钮不显示」而不是崩溃。

## 配置

配置文件位于系统配置目录，**不进入仓库**：

| 平台 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\astro-publisher\astro-publisher\config\config.toml` |
| Linux | `~/.config/astro-publisher/astro-publisher/config.toml` |
| macOS | `~/Library/Application Support/dev.astro-publisher.astro-publisher/config.toml` |

（具体位置以应用「设置」页显示为准——那里直接打印当前生效的路径。）

在「设置」页可以直接填仓库根目录并保存；也可用环境变量 `ASTRO_PUBLISHER_REPO` 覆盖（多机同步时更方便，优先级高于配置文件）。

## 安装包（Windows）

`packaging/installer.nsi` 是 NSIS 脚本，产出当前用户级安装包（不需要管理员权限）：

```powershell
cargo build --release -p astro-publisher -p astro-publisher-core
makensis /DAPP_VERSION=0.1.0 /DSOURCE_DIR="$PWD\target\release" `
         /DOUT_FILE="$PWD\dist\astro-publisher-0.1.0-setup.exe" packaging\installer.nsi
```

安装内容与行为：

- 装到 `%LOCALAPPDATA%\Programs\astro-publisher`（per-user，全程无 UAC）
- 开始菜单创建「Astro Publisher」与「卸载 Astro Publisher」
- 在「应用和功能」登记卸载项（HKCU，含版本、发布者、占用大小、项目主页）
- 静默安装：`astro-publisher-0.1.0-setup.exe /S`；静默卸载：`uninstall.exe /S`
- 卸载会删程序与快捷方式，但**保留** `%APPDATA%` 下的配置，重装后你的仓库路径还在

需要本机装有 [NSIS](https://nsis.sourceforge.io/)（`winget install NSIS.NSIS`）。脚本里 `Unicode true` 之外，**`.nsi` 文件本身必须是带 BOM 的 UTF-8**，否则含中文的字符串会报 `Bad text encoding`。

> 安装包未做代码签名，首次运行会有 SmartScreen 提示（「更多信息」→「仍要运行」）。

## 测试

```bash
cargo test
```

测试全部基于 `tests/fixtures/` 下的**合成内容**，覆盖：大纲解析与无损往返、单行编辑不影响相邻行、`_0xNNNN` 分配、章节创建的原子性与回滚、frontmatter 写回（含 CRLF 与代码块里的 `---`）、KaTeX 渲染与降级、仓库根目录自动探测。

## 状态

已经可用：文章编辑与预览（含**截图粘贴上传**）、**新增章节向导**（含洛谷题目信息抓取）、大纲解析与校验、**一键发布与日志面板**（含演练与停止）。

尚在完善：

- MCP server（供 AI 助手直接操作博客）

## 许可

MIT，见 [LICENSE](LICENSE)。
