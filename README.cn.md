# AppCast

[English](README.md) | [简体中文](README.cn.md)

通过单条 scrcpy 虚拟显示流水线，把单个 Android 应用的画面"流转"到本桌面的
独立原生窗口中，不影响手机主屏。

```bash
appcast run adb-scrcpy 10.0.0.8:5555 com.termux
```

## 工作原理

一个 scrcpy（>= 3.0）进程完成全部工作——在指定分辨率上创建虚拟显示、
在其中启动应用、并镜像到本地窗口：

```text
scrcpy -s <目标> \
    --new-display=<宽x高> \
    --start-app=<包名> \
    --max-fps <帧率> --video-bit-rate <码率>M
```

关闭窗口或 Ctrl+C 即杀死 scrcpy，虚拟显示随之销毁。
无手工 display 管理，也没有 `am start` 的权限坑。

> 设计理由与决策记录：[docs/design.cn.md](docs/design.cn.md)
> （[English](docs/design.md)）。

## 环境要求

- PATH 中有 `adb`
- PATH 中有 `scrcpy` >= 3.0（推荐 4.x；启动时会检查并给出明确报错）
- Android 设备已开启 USB/网络调试

## 安装

### Nix

```bash
nix run github:AndroidAppsUsedByMyself/appcast -- --help
```

### Debian / Ubuntu（apt 源）

```bash
echo "deb [trusted=yes] https://androidappsusedbymyself.github.io/appcast/debian stable main" \
  | sudo tee /etc/apt/sources.list.d/appcast.list
sudo apt update && sudo apt install appcast
```

### Termux

```bash
echo "deb [trusted=yes] https://androidappsusedbymyself.github.io/appcast/termux stable main" \
  >> $PREFIX/etc/apt/sources.list
pkg update && pkg install appcast
```

也可从 [Releases](https://github.com/AndroidAppsUsedByMyself/appcast/releases)
直接下载 `termux-<arch>.deb` 手动安装。

### Windows

从 [Releases](https://github.com/AndroidAppsUsedByMyself/appcast/releases)
下载 `windows-x86_64.exe`（静态 CRT，无需 DLL；首次运行 SmartScreen 可能告警）。

## 使用

寻址槽位因 transporter 而异：adb 需要 `<TARGET> <APP>`，其他后端可能更少
（如未来的 web 只需一个 URL）。

```bash
# 流转应用（无状态一行命令）
appcast run adb-scrcpy <序列号|ip:端口> <包名>

# 通过透传使用任意 scrcpy 参数（原样追加）
appcast run adb-scrcpy DUMMY com.termux --param resolution=1280x960 \
    -- --no-vd-destroy-content -x --video-codec=h265

# 只输出合并后的完整命令行，不执行
appcast snapshot --profile qq

# 列出设备上的应用
appcast list adb-scrcpy 10.0.0.8:5555          # 仅包名
appcast list adb-scrcpy 10.0.0.8:5555 -l       # 应用名 <TAB> 包名
appcast list adb-scrcpy 10.0.0.8:5555 --json   # 完整结构化条目

# Profile：保存 / 列出 / 编辑（$VISUAL/$EDITOR，回退 notepad/vi）/ 删除
appcast profile save qq adb-scrcpy 序列号 com.tencent.mobileqq \
    --param resolution=1280x960 -- --no-vd-destroy-content
appcast profile edit qq
appcast run --profile qq

# 从现有 Profile 派生变体（其余全部继承）
appcast profile save qq-lan --profile qq --target 192.168.1.20:5555
```

配置合并优先级（高者胜）：
位置槽位 > 槽位专用选项（`--transporter/--target/--app`）
> `--param` > Profile 字段（含 `params`/`raw_args`）。

选项：`--profile`、`--transporter`、`--target`、`--app`、
`--log-level`、`--param KEY=VALUE`（可重复）、
`--` 透传（追加到 Profile 的 `raw_args` 之后；`--clear-raw` 重置）。

后端扩展参数（`--param`）：`resolution`、`fps`、`bit_rate`、`adb_path`、
`scrcpy_path`。未识别的键会警告并忽略——它们是自定义 transporter 的扩展通道。

虚拟显示固定使用 `--display-ime-policy=local`，设备输入法的候选框会出现在
投屏窗口内。用电脑键盘打中文请切换 scrcpy 到 HID 键盘模式：

```bash
appcast run adb-scrcpy 序列号 com.tencent.mobileqq -- --keyboard=uhid
```

（默认按键注入无法输入 CJK 字符，会出现 `Could not inject char` 警告。）
`adb-scrcpy` 后端要求 **scrcpy >= 3.0**（启动时检查，报错信息明确）。

> 不支持 `--activity`：应用由 scrcpy 自行启动（`--start-app` 以整包为单位）。

## 二次开发

所有后端都挂在 `Transporter` trait 之后。添加自定义实现
（例如需要 `--activity` 的 `am start --display` 变体）：

```rust
// src/core/transporters/mine.rs
pub struct Mine;
impl Transporter for Mine { /* name / run / list_apps */ }

// src/core/transporters/mod.rs
registry.register("mine", || Box::new(mine::Mine));
```

之后 `appcast run mine <目标> <应用>` 直接可用，CLI 层零改动。
历史版本中的 `am start` 流水线保留在 git 历史里，可作为起点。

每个内置适配器都是独立可开关的 cargo feature，独占依赖随特性走
（`adapter-adb = ["dep:regex"]`，另有 `adapter-browser`、
`adapter-ssh`、`adapter-waypipe`）。关掉即同时移除代码与依赖，
并从 CLI 校验与补全中消失：

```bash
cargo build --no-default-features --features cli,adapter-browser  # 精简纯网页版
cargo check --no-default-features --features cli                  # 仅插件机制，零内置
```

未来要写依赖较重的树内适配器：依赖声明为 `optional`，以
`adapter-<名字> = ["dep:<crate>"]` 挂接，模块用同一 feature 门控——
其余一切不变。

### 投网页应用

两个适配器覆盖网页内容，仅窗口机制不同：

| 适配器 | 窗口 | 依赖 | 参数 |
|---|---|---|---|
| `web-browser`（内置） | 系统浏览器 App 模式（`--app`），无标签页无地址栏 | 任一 Chromium 系浏览器；Firefox 需 `kiosk=true` | `browser_path`、`window_size`、`kiosk`、`profile` |
| `web-webview`(插件) | 自有窗口内嵌 WebView（WebKitGTK / WebView2 / WKWebView） | 安装插件（见下）；Linux 构建需 WebKitGTK | `window_size`、`title` |

```bash
appcast run web-browser https://excalidraw.com --param window_size=1600x900
appcast run web-webview https://excalidraw.com --param title=Draw
appcast transporters   # 列出所有后端及其来源
```

Chromium 系浏览器按 profile 单实例：你的浏览器正在运行时，直接启动只会把
URL 移交过去然后立刻退出。因此本后端默认使用**专属 profile**（appcast
数据目录下的 `--user-data-dir`）加 `--new-window`——被拉起的进程归 Ctrl+C
管，登录态跨次持久，且不碰日常浏览器。`--param profile=default` 改为共享
真实浏览器配置（受上述移交行为影响）；`--param profile=<路径>` 用自定义。

**树外插件（`.so`/`.dll`，独立依赖树）**——适合需要重依赖的后端。Rust
没有稳定 ABI，因此插件走一条窄 C ABI（JSON 载荷 + 版本握手），全部 FFI
由 [sdk/appcast-plugin](sdk/appcast-plugin) SDK 生成：你只需实现一个阻塞
trait，零 `unsafe`。以 cdylib 形式构建为
`libappcast_tpt_<名字>.{so,dylib,dll}`，放进插件目录——Linux:
`~/.config/appcast/transporters/`；Windows:
`%APPDATA%\appcast\transporters`（字面 `~/.config` 路径同样会被扫描）；
macOS: `~/Library/Application Support/appcast/transporters`。或用
`$APPCAST_TRANSPORTER_DIR` 指向 PATH 式目录列表。`appcast transporters`
会打印实际扫描过的所有目录。同名插件可覆盖内置后端；`appcast transporters` 可查
加载结果与来源。完整实例见 [plugins/webview](plugins/webview)。

## 开发

```bash
nix develop            # 或使用 rustup 工具链
cargo build && cargo test && cargo clippy --all-targets
nix build              # 验证打包
```

### NixOS 工作流

| 任务 | Shell | 循环 |
|---|---|---|
| 核心 / 树内适配器 | `nix develop` | 改 `src/core/transporters/*.rs`，在 `mod.rs` 注册，`cargo test` |
| 插件（裸基础环境） | `nix develop .#plugin` | 改 `plugins/<名字>/src/lib.rs`，然后 `install-plugin <crate名>`（构建并拷到 `~/.config/appcast/transporters/`），用 `appcast transporters` 验证 |
| webview 插件 | `nix develop .#plugin-webview`（追加 WebKitGTK 全家桶） | 同上 |

不同插件链接的原生栈不同，因此每个插件有独立 shell 而非一个大而全
环境；新增后端时在 `flake.nix` 里加一条 `mkPluginShell` 即可。涉及
GUI 链接成员的 workspace 级命令须进对应 shell（如 `nix develop
.#plugin-webview -c cargo test --workspace`）；裸 shell 经 workspace
`default-members` 覆盖核心 + SDK。

构建出的插件 `.so` 通过 Nix store rpath 直接链到 WebKitGTK 运行时，
任何环境都能 dlopen，无需 `LD_LIBRARY_PATH`。

声明式系统级安装：

```nix
# flake.nix
inputs.appcast.url = "github:AndroidAppsUsedByMyself/appcast";

# configuration.nix
imports = [ inputs.appcast.nixosModules.default ];
nixpkgs.overlays = [ inputs.appcast.overlays.default ];
programs.appcast = {
  enable = true;
  plugins = [ pkgs.appcast-tpt-webview ];  # 或 programs.appcast.package = pkgs.appcast;
};
```

模块会对 `appcast` 做一层 wrapper：声明式插件目录在用户级
`$APPCAST_TRANSPORTER_DIR` 之后搜索——用户仍可在自己的目录放同名插件
覆盖系统版本。

## 许可证

Apache-2.0 —— 见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。appcast 是独立工具，
运行时仅*调用*外部程序；scrcpy（Genymobile）与 adb（Android Open Source
Project）仍遵循其自身的 Apache-2.0 条款。
