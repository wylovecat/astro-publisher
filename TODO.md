# 待办

**优先级(用户 2026-10-19 定):Windows 侧优先 → 跨平台 → Linux 次之 → macOS 不考虑。**

下面每条都是对着仓库核过的,不是从印象里抄的。核实时对应 `main` 的 `bc0e351`。

---

## W 档:Windows 优先

### W1. ~~安装包这条路,在当前代码上从没走通过~~ —— 已完成(2026-10-19)

**做完了,四条都验过:**

- **打包**:`pwsh packaging\build-installer.ps1` 从当前源码编出 release + 安装包,产出 `dist\astro-publisher-0.2.0-setup.exe`。
- **安装**:静默 `/S` exit 0;三个 exe 的 **SHA256 与 `target\release` 逐个一致**(证明装进去的确实是新构建,不是缓存的旧文件);注册表卸载项 `DisplayVersion = 0.2.0.0`;开始菜单两个快捷方式都建好。
- **运行**:从**安装目录**启动 exe(不是 `target\debug`),文章页与**发布面板**都正常渲染,待提交清单与五个步骤都在。
- **单实例**:再启一个安装版,弹出 `#32770` 对话框,内容是「astro-publisher 已经在运行了(进程 28400)」——正是第一个实例的 pid;点确定后第二进程退出,主实例不受影响。
- **卸载**:静默 `/S` exit 0,程序目录删除、注册表卸载项移除、快捷方式移除,**配置按设计保留**。

> `makensis` 不需要装:NSIS 早已装在 `C:\Program Files (x86)\NSIS\makensis.exe`,只是不在 PATH 上,而 `build-installer.ps1` 本来就会回落到那个位置找(脚本第 141 行)。我一度因为 `Get-Command makensis` 返回空就判成「没装」,是错的——判断「装没装」要看程序本身在不在,不能只看 PATH。

顺带查清一件事:**这台机器上其实早就装过 0.1.0**(`%LOCALAPPDATA%\Programs\astro-publisher` 在我安装前就存在,里面是 10-15 那次的旧二进制)。所以「从没走过」这句不准确——准确说法是**装出来的那份从没被运行过**,而且装的是落后三个 PR 的旧构建。

### W2. ~~版本号停在 0.1.0~~ —— 已 bump 到 0.2.0(2026-10-19)

workspace `Cargo.toml` 的 `version` 从 `0.1.0` 改为 `0.2.0`,三个 crate 继承它,`Cargo.lock` 同步。安装包文件名与注册表卸载项都已是 `0.2.0` / `0.2.0.0`。

### W2b. ~~exe 内嵌版本号是空的~~ —— 已修(2026-10-19)

**W1 验证时发现的**:README 写着「安装包文件名、exe 内嵌的 FileVersion、注册表卸载项里显示的版本永远是同一个」,但实测 `VersionInfo.FileVersion` 是空的。

原因在 `packaging/installer.nsi`:那几条 `VIAddVersionKey` 装的是**安装器自己**(setup.exe)的版本资源,不是被安装的 exe;而 Rust 二进制默认不带任何版本资源。

**已修**:三个 crate 各加一个 `build.rs`,用 `winresource` 把版本资源写进 exe,版本取 `CARGO_PKG_VERSION`(仍是 `Cargo.toml` 单一来源)。脚本在 `CARGO_CFG_TARGET_OS != "windows"` 时直接返回,**Linux/macOS 构建不受影响**。

**实测**(装出来的那一份,不是编译产物):

```
astro-publisher.exe        FileVersion='0.2.0' ProductName='Astro Publisher'
astro-publisher-core.exe   FileVersion='0.2.0' ProductName='Astro Publisher'
astro-publisher-mcp.exe    FileVersion='0.2.0' ProductName='Astro Publisher'
```

`FileDescription` 取自各 crate 的 `description`,`CompanyName` = 作者,`LegalCopyright` = MIT。三个 exe 的 SHA256 仍与 `target\release` 逐个一致,程序启动正常(资源是链接期加进去的,所以要真跑一次才算数——已跑)。

> 踩到的小坑:`winresource` 的 `res.set()` 收 `&str`,直接传 `String` 编译不过,得先 `let` 出来再 `&`。

### W3. 代码签名 —— 需要你出手

脚本已接好签名步骤(`-Sign -CertificateThumbprint` / `-CertificatePath`),证书要自行向 CA 申请,本地生不出来。没签之前每次安装都有 SmartScreen 提示。

排在 W 档是因为它**只影响 Windows**,而且是「给别人装」这条路上最明显的粗糙处。

---

## 跨平台档:和平台优先级无关,按需要做

### X1. CLI / MCP 暴露「从第 N 步重跑」

core 已经全部支持(`publish::run_from(options, start_step, …)`、`publish::pull_rebase`),接一个参数的事。

我之前**故意没做**:它是交互式恢复动作,要人看着日志决定从哪一步接;而且让 AI 去触发 rebase 不合适——AI 没有「我确认了要变基」这个概念。要加的话建议 MCP 侧**只暴露 `run_from`,不给 `pull_rebase`**。

### X2. 发布日志自动滚到底

重试后新追加的「—— 从第 N 步重新开始 ——」在可视区下方,得手动滚。**既有行为,不是最近那次改动引入的。**

暂时**无法自动验证**:gpui 不接收我用 `mouse_event(MOUSEEVENTF_WHEEL)` 合成的滚轮事件(连发 30 次面板纹丝不动),所以「滚到底」这一步只能靠代码判断内容确实追加在末尾。

### X3. 打包 / 安装脚本补自动化测试

`build-installer.ps1`、`install.sh`、`build-tarball.sh` 全都没有自动化测试,靠手工跑一次。

值得测的纯逻辑点(不需要真二进制):参数校验与退出码、`.desktop` 模板替换是否两个占位符都换掉、`Exec=` 是否指向 launcher 而非裸二进制名(**这条有回归价值**:`--prefix` 装的目录不在 PATH 上时,`Exec=astro-publisher` 会点得开却起不来)、卸载后的残留文件清单为空。

`build-installer.ps1` 里 `-SkipBuild` 的过期二进制检查**已经有单测**,可以照那个路子做。

---

## L 档:Linux 次之

### L1. Linux 上关窗会留下占着 4321 的 `node`

`process::kill_tree_blocking` 在 Windows 走 `taskkill /T /F`(整棵树);Unix 分支只给**直接子进程**发 `SIGTERM`:

```rust
// crates/core/src/process.rs, kill_tree_blocking
#[cfg(unix)]
// Only the direct child: unlike Windows, `taskkill /T` has no portable
// equivalent here, and signalling a process group we did not create
// would reach our own.
```

「真实预览」起的是 `pnpm dev --port <port>`,所以直接子进程是 `pnpm`,真正监听端口的 `node` 是它的**孙子**,活下来继续占 4321。

而且 `job.rs` 整份都在 `#[cfg(windows)]` 内,所以 Linux 上「强杀应用」这条兜底**根本不存在**。

⚠️ 我做过的 job object 验证(`Stop-Process -Force` 后 dev server 随应用消失、另外 8 个无关 `node` 存活)**只在 Windows 上成立**,Linux 那半边没有任何证据。

**候选做法**(都要在 Linux 上实测):`setsid` 让 `pnpm` 成为新进程组组长再 `kill -- -<PGID>`(最贴近 Windows 语义,但**不能对自己没创建的进程组发信号**);或绕开 `pnpm` 直接起 `node`(会绑死 astro 的调用方式);或补一个 job object 的 Unix 等价物(最彻底也最重)。

**验收**:Linux 上起「真实预览」→ 关窗 → `ss -tlnp | grep 4321` 无输出;强杀应用 → 同样无残留。

### L2. Linux 真机验证整条打包链路

`build-tarball.sh` **从未在 Linux 上跑过**(GPUI 不能从 Windows 交叉编译)。

我在 Windows 上只做了两件都是模拟的事:`bash -n` 语法校验(只查语法,证明不了行为);把一个手搭的假二进制目录喂给 `install.sh` 跑通装/卸闭环。

**本机模拟里 `install.sh --uninstall` 留下了一个 `.astro-publisher-install`**,已确认不是脚本 bug——Git Bash 里的 `sudo` 是 Windows 自带的 `sudo.exe`,它会静默拒绝删除点开头的文件(rc=0、文件仍在)。Linux 上没这行为。但这意味着**卸载分支的真实验证从未发生**。

**验收**:在办公室那台 Linux Mint 上 `build-tarball.sh` → 拷贝 tarball → 解开 + `install.sh` → 从启动器能起 → `install.sh --uninstall` 后**列出残留文件**(应只剩 `~/.config/astro-publisher`)。

顺带把 L1 一起验掉。

### L3. `PLAN.md` §9.2 第 7 条的双平台验收

要求「Win11 与 Linux Mint 各跑通新增章节与发布」。Win11 那半边做过,Linux 那半边没做。做 L1 / L2 时顺带覆盖,不必单排。

---

## 不做

- **macOS 打包**(`.app` / dmg / 公证)—— 用户 2026-10-19 明确不考虑。README「状态」一节里那条 macOS 缺口随之作废,可以改成「本项目只支持 Windows 与 Linux」。
- **EO 部署状态轮询** —— 2026-10-16 已决定不做:需要腾讯云 / EdgeOne 的 API 凭据,工具拿不到,`PLAN.md` §10 原本就标成「MVP 接受」。

---

## 一条经验,留给下一次核实

L1 这类条目是**读 `#[cfg]` 边界**才发现的,不是跑测试跑出来的。凡是「某功能在某平台宣称可用」的说法,都值得顺着 `cfg(windows)` / `cfg(unix)` / `cfg(target_os)` 扫一遍——平台特化很容易只写一半,而另一半既不报错也不消失。

同一个道理反过来也成立:**编译产物的时间戳会骗人**。W1 里那个安装包比三个 PR 都新(时间戳上是「最新的」),却比三个 PR 都旧;`target\release` 干脆不存在,一眼就能看出从没编过。判断「装出来的东西是不是当前的」不能只看文件时间,要回到 commit——**而版本号也救不了你,因为它同样停在 0.1.0 没动过**(W2)。
