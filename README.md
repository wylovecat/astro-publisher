# astro-paper publisher

一个面向 **Astro 博客** 的桌面发布工具：把「新增文章 / 新增书籍章节 / 关联题解与视频 / 校验 / 发布上线」从手写 Markdown + 命令行，变成表单 + 按钮。

技术栈是 **Rust + [GPUI](https://gpui.rs)**（UI 用 [gpui-component](https://gpui-kit.com) 0.7.x），产出单一原生二进制，Windows / Linux / macOS 共用一套代码。

> 本项目最初为一个中文算法竞赛教学博客（Astro + AstroPaper）而写，因此默认约定偏该场景：课程大纲即目录、章节用 `_0xNNNN` 顺序 slug、一键推送到支持 Git 集成的静态托管平台。这些约定都集中在少数几个模块里，改成别的 Astro 博客形态并不困难。

## 能力

- **总览（仪表盘）**：文章数 / 草稿数 / 未关联题解数 / 校验错误与警告，各书大纲进度条，以及按 frontmatter 时间排序的「最近修改」——点任意一篇直接进编辑器。
- **文章管理**：列表、搜索、frontmatter 表单（标题 / 标签 / 草稿 / 特色 / 时间），点开即编辑。
- **章节创建（核心）**：一次操作同时生成章节文件并更新课程大纲，自动分配全站递增的 `_0xNNNN` slug；任一步失败整体回滚。题号可一键从洛谷抓回题目名 / 难度 / 标签。
- **大纲维护**：解析 Part / 章 / 小节 / 题目四层结构，支持勾选、关联题解、关联视频、插入新行与新标题。
- **校验**：frontmatter 结构检查 + 大纲里 `/posts/...` 链接的坏链检查；错误阻塞发布，警告不阻塞。
- **预览**：编辑器右侧实时预览，由 gpui-component 的 Markdown 引擎排版（代码块保持等宽与缩进、行内代码带底色），公式交给 **KaTeX 本体** 排版（与站点 rehype-katex 同源），不是近似实现。旁边的「真实预览」按钮会起站点自己的 `pnpm dev`（端口 4321）并在系统浏览器里打开当前文章——草稿在 dev 服务器里同样不可见（站点无条件过滤 draft），所以该按钮只对已发布文章可用。
- **发布流水线**：校验 → 本地构建（权威校验）→ `git commit` → `git push` → 托管平台自动构建。推送失败不强推、不自动 rebase。
- **MCP server**：同一套内容操作以 MCP 工具形式暴露给 AI 助手（stdio），只读工具与写操作分开，发布默认演练。
- **无头 CLI**：以上全部能力都有命令行入口，便于脚本化或在没有图形环境的机器上使用。

## 目录结构

```
├── Cargo.toml                    # workspace（Cargo.lock 入库以锁定 GPUI 生态版本）
├── PLAN.md                       # 最初的实施计划（里程碑 / 风险表 / 验收标准；代码注释里的 plan §x.y 指此）
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
│   │   ├── src/preview.rs        #   pulldown-cmark + KaTeX → HTML（CLI 的 `preview`）
│   │   ├── src/math_dom.rs       #   KaTeX 的 HTML 读回成可布局的公式树（分式 / 上下标 / 根号 / 矩阵）
│   │   ├── src/config.rs         #   配置读写与仓库根目录探测（CLI / GUI / MCP 共用）
│   │   ├── src/publish.rs        #   校验 → 构建 → 提交 → 推送（CLI 与 GUI 共用，事件流式上报）
│   │   ├── src/process.rs        #   子进程（pnpm / git），UTF-8 流式输出
│   │   └── src/bin/cli.rs        #   无头 CLI
│   ├── app/                      # GPUI 桌面应用（侧边栏 + 列表 + 编辑器 + 章节向导 + 发布 + 设置）
│   │   └── src/views/
│   └── mcp/                      # MCP server（stdio），把 core 的能力暴露给 AI 助手
│       └── src/main.rs
├── packaging/
│   ├── build-installer.ps1        # 打包脚本：版本从 Cargo.toml 读，可选签名
│   └── installer.nsi              # NSIS 脚本（必须是带 BOM 的 UTF-8）
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
- **编辑文章**：源码 + 实时预览并排。预览走 gpui-component 的 Markdown 引擎：代码块按代码块画（等宽、缩进保留、背景与右上角语言标签），行内代码带底色。编辑器里 **Ctrl+V 粘贴截图**会写入仓库 `public/images/`，并在光标处插入 `![](/images/<文章名>-<时间戳>.png)`；写入失败会报原因而不是把二进制当文本贴进去。非图片的剪贴板内容照旧走普通文本粘贴。
- **预览里的公式**：`$…$` 与 `$$…$$` 由 KaTeX 本体排版（与站点 rehype-katex 同一套引擎），再由 `core/src/math_dom.rs` 把 KaTeX 的排版结构读回成公式树、`app/src/views/preview_math.rs` 用 GPUI 原生画出——GPUI 没有 CSS 引擎，直接喂 HTML 只会显示 MathML 源码。分式、上下标、根号、`\sum` 的上下限、`\boxed`、`array`/`cases` 都按 KaTeX 的结构还原；KaTeX 自带 SVG 的字形（`\vec` 的箭头、可伸缩括号）退回 Unicode 近似。KaTeX 自己排不出的公式（例如 duktape 后端下的 `pmatrix`）退回显示源码，预览不会少内容。
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

## MCP server（给 AI 助手用）

`crates/mcp` 是一个 stdio MCP server，把 `core` 的能力以工具形式暴露出去。它不是另写一份逻辑：每个工具都是对 core 的直接调用，所以助手建的章节与桌面应用建的走同一条 slug 分配、同一套原子写与回滚、同一个校验门。

```bash
cargo build -p astro-publisher-mcp
# 或者直接用安装包：装好后它是
#   %LOCALAPPDATA%\Programs\astro-publisher\astro-publisher-mcp.exe
# 在 MCP 客户端的配置里指向该路径（从源码构建则指向 target/debug/astro-publisher-mcp(.exe)）
# 仓库路径与桌面应用共用 config.toml；也可用 ASTRO_PUBLISHER_REPO 覆盖
```

工具分三类，**读写是分开的**：

| 类型 | 工具 |
| --- | --- |
| 只读 | `list_posts`、`read_post`、`list_books`、`list_sections`、`lookup_problem`、`validate`、`preview_chapter` |
| 写 | `create_chapter`（建章节文件 + 改大纲，失败整体回滚） |
| 有副作用 | `publish`（**默认演练**：不显式设 `dry_run: false` 就不会 commit / push） |

两条刻意的设计：

- **`preview_chapter` 不碰磁盘**，只算出将要分配的 slug 与改写后的大纲行。工具描述里要求助手先调它、给用户看过再写。
- **发布默认演练**。工具调用本身不等于用户确认，所以真要推送必须显式传 `dry_run: false`；描述里也写明了这一点，避免助手把「帮你发布」理解成可以直接推线上。

`publish` 的返回里带流水线自己的日志行，助手能报告实际发生了什么，而不是只回一个成败。

## 与博客仓库的集成

只需一处改动：书籍数据放在 `src/data/books.json`，由 `src/data/books.ts` 薄包装导出类型，站点其余代码无需改动。工具通过 JSON 读写书籍登记，避免解析 TypeScript 源码。

若把工具放在博客仓库的子目录里，建议在博客的 lint / format 配置中排除该目录（例如 Prettier 白名单机制、ESLint ignore），并忽略 Rust 构建产物 `target/`。

## 已知坑（改 UI 前请读）

- **`Editor` 必须显式给高度**：用 `Editor::new(&state).h(relative(1.0))`。gpui-component 的多行输入内部是 `height: relative(1.0)`，只靠外层 `flex_1` 会让百分比对上不确定的盒子，编辑器塌缩到只显示两三行。
- **按钮用 `Button` 组件**，不要手写 `div().bg(theme.primary)`：需要同时导入 `button::{Button, ButtonVariants}` 与 `Sizable` 才能用 `.primary()` / `.small()`。手写 div 会渲染成一块没有文字的色块。
- 主题里的 `primary` 本身是近黑色（shadcn 的 `neutral-900`），这是设计如此，不是 bug。
- **`TextView::html` 不能用来显示 Markdown**：它的 HTML 路径是散文渲染器——`<pre>` 没有分支会被折成段落，而 `<pre><code>` 里的空白因为父节点是 `code`（只对 `pre`/`textarea` 保留空白）被压成空格，缩进全丢。显示 Markdown 要用 `TextView::markdown`，代码块、行内代码、表格、任务列表都由它原生画。公式不在它的能力范围内，用 `MarkdownPlugin` 自己接。
- **KaTeX 的 HTML 不能直接喂给 GPUI**：KaTeX 输出的是带 CSS class 与 `top`/`vertical-align` 内联样式的 HTML（还有一份 MathML 副本和若干 SVG 字形），`TextView::html` 会把它们当普通文字合并，于是公式显示成 MathML 源码。正确做法见 `core/src/math_dom.rs`。
- **KaTeX 的竖直列表（vlist）条目顺序不固定**：`\frac` 的 DOM 顺序是「下 → 上」，`array` 的行是「上 → 下」。唯一两边一致的是每个条目的 `top` 偏移（越小越靠上），要按它排序。另外每条 entry 的内容包在子 span 里，判断「这条是不是 frac-line」只能往下看一层——再多看就会把嵌套公式内部的 `frac-line` 认成外层的。
- **`katex` crate 的默认后端不支持 MSVC**：默认 `quick-js` 只支持 unix 或 windows-gnu，Windows MSVC 需要 `duktape` 后端（见 `Cargo.toml` 注释）。`duktape` 没有 ES6 的 `Array.prototype.fill`，KaTeX 靠它画 SVG，所以带可伸缩定界符的公式（`pmatrix`/`bmatrix`）在这个后端下会直接报错，调用方要能退回显示源码。
- **滚动要用 `overflow_scrollbar()`**：`Styled` 只生成 `overflow_hidden` 系列，滚动能力在 gpui-component 的 `ScrollableElement` 上，需要 `use gpui_component::scroll::ScrollableElement`。
- **要滚动的 flex 子容器还得加 `.min_h(px(0.0))`**：flex 子项默认 `min-height: auto`，下限由内容撑开，光换 `overflow_scrollbar()` 不会滚——列表会把父容器顶出去，底部依然够不着。
- **`overflow_y_scrollbar()` 会让函数返回类型变成 `Scrollable<Div>`**：辅助渲染函数若有多条 return 分支，`impl IntoElement` 会因各分支类型不一致而编译失败（`expected Div, found Scrollable<Div>`）。收成 `gpui::AnyElement` 并给每条分支加 `.into_any_element()`。
- **`Context::subscribe` 的 `Subscription` 不能随手丢**：gpui 的 `Subscription` 在 `Drop` 时会退订，直接忽略返回值等于没订阅。父实体要被子实体变化驱动重渲染时，用 `.detach()`（`Context::subscribe` 内部持有的是 weak 引用，随父实体一起消失）。
- **`InputState` 有多个 `EventEmitter` 实现**：`cx.subscribe(&input, |_, _, _, cx| ...)` 报 `E0283 type annotations needed`，因为 `InputEvent` 和 `NumberInputEvent` 都满足。要写全类型：`|_, _, _: &gpui_component::input::InputEvent, cx|`。
- **建辅助 UI 函数时返回 `gpui::AnyElement`**：`impl IntoElement` 的返回类型会把 `&mut Context` 的借用带进元素类型，导致后面再拿同一个 `cx` 借用时报 `E0502`；`AnyElement` 不带生命周期。`Render::render` 本身仍按 trait 声明写 `impl IntoElement`。
- **gpui 依赖要对齐版本**：`gpui-component` 依赖的是包名为 `gpui-pre` 的 gpui，而 crates.io 上另有一个无关的 `gpui`；直接用后者会导致 `Render` trait 不匹配。`Cargo.toml` 里已用包别名处理。
- **图标资源必须显式注册**：`gpui_component::init(cx)` **不注册**图标资源（最小化/最大化/关闭等窗口按钮、按钮图标全部静默渲染成空白，不报错）。图标在独立 crate `gpui-kit-assets`，启动时须 `application().with_assets(gpui_kit_assets::Assets).run(...)`。忘记这一步时窗口右上角三个按钮完全消失，症状是「按钮不显示」而不是崩溃。
- **桌面程序不要留控制台**：`crates/app/src/main.rs` 顶部用 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`，所以 **release 版进程没有控制台**（启动时不再出现黑色窗口），代价是 release 下的 `println!`/panic 都看不见——排查问题用 debug 版（`cargo run`，仍带控制台）。
- **连带规则：spawn 子进程必须过 `process::hide_console`**。一个没有控制台的父进程去启动控制台程序时，Windows 会**给子进程新开一个控制台窗口**：发布流水线的 `git`（每次都闪一下）与 `pnpm build`（`pnpm.cmd` 实为 `cmd.exe` → `node.exe`，黑窗整段构建都挂着）。`core/src/process.rs` 里三处 `Command`（`spawn_streaming`、`run_collect`、`kill_tree` 的 `taskkill`）都加了 `CREATE_NO_WINDOW`；实测该标志连**孙进程**也覆盖（GUI 父进程 + 无标志 → 出现新控制台窗口；+ 标志 → 直接子进程与嵌套子进程都不出现）。新增 spawn 时别漏。
- **`astro dev` 预览服务器必须显式收掉，且钩子只有 `on_window_closed` 可用**：「真实预览」起的 `pnpm dev` 是一个 `node` 进程，占着端口 4321，不清理会一直留着。它挂的钩子是 `Window`/`App` 的 `on_window_closed`（关窗时用 `taskkill /T` 收整棵树）；**不要指望 `on_app_quit`**——本版 gpui 只由 `App::shutdown()` 运行那些回调，而关窗路径走不到它（实测：探针写在 `on_app_quit` 里，进程退出了、探针从未生成）。已知缺口：**强杀进程（任务管理器 / `Stop-Process -Force`）仍会留下这个 node**，只有 Windows job object（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`）能覆盖那种情况，目前没做。
- **服务器就绪不要靠 sleep，要读它自己报的地址**：`pnpm dev` 冷启动实测 ~6.4 秒，所以按钮点了不能立刻开浏览器。就绪信号是那行 `┃ Local    http://localhost:4321/`（**带 ANSI 颜色码**，得先 `process::strip_ansi` 再匹配）；从日志里读端口而不是写死 4321，是因为端口被占时 Astro 会静默换一个，写死就会打开错地址。
- **长时间运行的子进程不要用 `spawn_streaming`**：它会把每一行输出都留在内存里直到进程退出（对 `pnpm build` 是对的，对跑几小时的 dev server 就是无界增长）。长任务用 `spawn_background`，它只转发不累积。

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

用打包脚本产出当前用户级安装包（不需要管理员权限）：

```powershell
pwsh packaging\build-installer.ps1              # 编译 + 打包
pwsh packaging\build-installer.ps1 -SkipBuild   # 复用 target\release，只打包
pwsh packaging\build-installer.ps1 -Sign        # 打包后签名（需已配置证书）
```

**版本号只有一个来源：`Cargo.toml`。** 脚本用 `cargo metadata` 读出解析后的版本（正确处理 workspace 继承），再传给 `makensis`，所以安装包文件名、exe 内嵌的 `FileVersion`、注册表卸载项里显示的版本永远是同一个。以前是手工写 `makensis /DAPP_VERSION=0.1.0 …`，和 `Cargo.toml` 各写各的，容易悄悄漂移——现在改 `Cargo.toml` 里的版本再跑脚本即可。

脚本同时把 `dist/SHA256SUMS.txt` 写成 `<hash>  <文件名>`，供分发的下载方核对。

**`-SkipBuild` 会拒绝打包过期二进制**：如果 `target\release\astro-publisher.exe` 比源码树旧（说明源码改过但没重新编译），脚本直接报错并列出「二进制时间 vs 最新源文件」，除非显式加 `-Force`。加这道门是因为真出过事：一次 `-SkipBuild` 把补丁前的旧 `target\release` 打进了安装包，安装包看起来构建成功，实际装的还是没有该修复的程序 —— 用户报的「窗口按钮不显示」就是这么来的。

> `packaging/build-installer.ps1` **刻意只用 ASCII**。含非 ASCII 的 `.ps1` 会按机器的 ANSI 代码页读取，字符串里一个被曲解的字节就会破坏解析（报 `'C' is not recognized as a cmdlet` 之类），与编码环境绑定。同理，`packaging/installer.nsi` 必须是**带 BOM 的 UTF-8**。

### 代码签名（可选）

安装包默认**不签名**，首次运行会有 SmartScreen 提示（「更多信息」→「仍要运行」）。

签名需要**你自己的代码签名证书**——由 CA 在身份核验后签发（OV 或 EV），本地生成不出来，也不是本工具能替你获取的。拿到证书后：

```powershell
# 证书在用户证书存储里（用其指纹）
pwsh packaging\build-installer.ps1 -Sign -CertificateThumbprint <SHA1指纹>

# 或者用 .pfx 文件
pwsh packaging\build-installer.ps1 -Sign -CertificatePath cert.pfx -CertificatePassword <密码>
```

脚本会 fail-fast：**在编译之前**就检查 `signtool` 与证书参数是否齐备，缺什么直接说明并退出，不会让你等完两分钟的构建才发现签名没配。签名后会自动 `signtool verify /pa` 复核，而不是假定签上了。

EV 证书（或已积累信誉的 OV 证书）才能立刻消除 SmartScreen 警告；新签发的 OV 证书仍需要一段时间的下载量积累，这是证书类型决定的，不是脚本的问题。

### 安装内容与行为

- 装到 `%LOCALAPPDATA%\Programs\astro-publisher`（per-user，全程无 UAC）
- 三个可执行文件都装在该目录：桌面应用 `astro-publisher.exe`、无头 CLI `astro-publisher-core.exe`、MCP server `astro-publisher-mcp.exe`（后两个是独立二进制；AI 助手的配置里直接指向安装目录下这一份即可，不必另外构建）。三者都是**无条件打包**：缺任何一个 makensis 会直接报错，不会悄悄出一个少了工具的安装包
- 只给桌面应用建快捷方式：CLI 与 MCP server 是命令行 / stdio 进程，双击启动没有意义
- 开始菜单创建「Astro Publisher」与「卸载 Astro Publisher」
- 在「应用和功能」登记卸载项（HKCU，含版本、发布者、占用大小、项目主页）
- 静默安装：`astro-publisher-<版本>-setup.exe /S`；静默卸载：`uninstall.exe /S`
- 卸载会删程序与快捷方式，但**保留** `%APPDATA%` 下的配置，重装后你的仓库路径还在

需要本机装有 [NSIS](https://nsis.sourceforge.io/)（`winget install NSIS.NSIS`）。脚本会自动在 PATH 及 NSIS 的标准安装位置找 `makensis`，不必自己加 PATH。脚本里 `Unicode true` 之外，**`.nsi` 文件本身必须是带 BOM 的 UTF-8**，否则含中文的字符串会报 `Bad text encoding`。

## 测试

```bash
cargo test
```

测试全部基于 `tests/fixtures/` 下的**合成内容**，覆盖：大纲解析与无损往返、单行编辑不影响相邻行、`_0xNNNN` 分配、章节创建的原子性与回滚、frontmatter 写回（含 CRLF 与代码块里的 `---`）、KaTeX 渲染与降级、仓库根目录自动探测，以及 `math_dom` 把 KaTeX 输出读回公式树（分式 / 上下标 / 根号 / 上下限 / 表格 / 框）。

预览的排版效果只能看，不能断言。`cargo run -p astro-publisher --example previewshot` 会开一个只渲染预览的窗口（固定示例文档，含代码块与各类公式），改动排版后用它截图核对；主程序另有 `--open <n>` / `--view <name>` 两个验证用参数。

## 状态

已经可用：**总览页**（书籍进度 / 未关联题解 / 校验计数 / 最近修改）、文章编辑与预览（含**截图粘贴上传**与**真实预览**：起 `pnpm dev` 用浏览器看站点本体）、**新增章节向导**（含洛谷题目信息抓取）、大纲解析与校验、**一键发布与日志面板**（含演练与停止）、**MCP server**（供 AI 助手读写博客）、**一键打包脚本**（版本号自动取自 `Cargo.toml`，可选签名）。

尚在完善：

- 代码签名证书：脚本已接好签名步骤，但证书需要自行向 CA 申请（见「安装包」小节）
- 本章节暂未纳入 `cargo test`：打包脚本本身没有自动化测试，靠手工跑一次验证

最初的实施计划（里程碑 Spike / M0 / M1 / M2、风险表、验收标准）整理在 [`PLAN.md`](PLAN.md)——代码注释里的 `plan §x.y` 引用都指向它。

## 许可

MIT，见 [LICENSE](LICENSE)。
