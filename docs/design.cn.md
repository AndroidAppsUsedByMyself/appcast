# AppCast 设计说明

[English](design.md) | [简体中文](design.cn.md)

记录架构决策及其理由。新决策可能覆盖旧决策，每条均注明变更内容与原因。

## 架构总览

```text
CLI 前端（clap）              ── 薄：寻址三元组 + 两条不透明通道
   │  优先级栈合并
   ▼
ResolvedConfig               ── {transporter, target, app, params, raw_args}
   │  registry.get(name)
   ▼
Transporter trait（dyn）     ── name / run / list_apps
   ├── adb-scrcpy  （scrcpy >=3 虚拟显示；参数语义与默认值归它所有）
   ├── ssh-x11  （占位）
   └── waypipe  （占位）
```

核心原则：**薄通用内核 + 厚后端**。核心只认识寻址三元组；一切后端相关的
旋钮都通过 `params` 传递，由被选中的后端解释并拥有默认值。

## 决策记录

### D1 · 寻址槽位由后端定义 schema，核心仅不透明承载

本工具的寻址模型是"一个协议 + 两个可选槽位"：

```text
appcast run <TRANSPORTER> [<TARGET>] [<APP>]
#                        └─ "哪里"     └─ "在那里打开什么"
```

不同 transporter 需要的元数不同：

| transporter | 需要的槽位 | 映射 |
|---|---|---|
| adb-scrcpy | target + app | 设备序列号 → 包名 |
| ssh-x11 / waypipe | target + app | host → 可执行路径 |
| web/webview（未来） | 仅 target | URL |
| vnc（未来） | 仅 target | `host:display` |
| 本地窗口捕获（未来） | 仅 app | 窗口标题/id |

因此**核心绝不校验元数**：`target`/`app` 端到端都是 `Option<String>`，
每个后端自行拒绝无法处理的输入，并使用自己的 usage 文案
（[`AppError::Usage`]）。核心层唯一的寻址错误是
`MissingTransporter`（registry 查找需要它）。

核心保证的是：槽位位置稳定、以固定优先级合并（D5），肌肉记忆可跨后端迁移。
曾考虑并否决的两个方向：把 `app` 挪进 `--param`（把寻址降格为字符串配置）、
在 CLI 层强制三元组齐全（虚构了并不存在的"普适真理"）。

### D2 · 强类型显示旋钮降级为 params

`--resolution/--fps/--bit-rate` 曾是强类型 CLI 选项、默认值放在 Profile 层。
现已移除：它们是 scrcpy 的概念而非通用概念（waypipe/VNC/RDP 后端要么没有
对应物、要么完全不同）。现在它们作为 params 传递：

```bash
appcast run adb SERIAL com.app --param resolution=1280x960 --param fps=90
```

adb/scrcpy 后端负责解释并在缺省时应用内置默认值（`1920x1080`、`60`、
`8` Mbps）。**默认值住在语义所在的地方**。非法值会显式报错
（`InvalidParamValue`），不再被静默转换。

携带旧顶层键（`resolution:` 等）的 Profile 仍可加载，旧键被忽略。

### D3 · 单条 scrcpy 流水线，而不是 am-start 组合拳

最初设计镜像 Android 原语（`dumpsys display` 找空闲 ID →
`am start --display N -n pkg/Act` → `scrcpy --display-id N`）。真机上被击穿：
ROM 拒绝 shell 在虚拟显示上拉起受保护 Activity
（`SecurityException: Permission Denial`），且手工 display 簿记在不同
Android 版本间极其脆弱。

scrcpy 3.0 起 server 可以自建虚拟显示并自行启动应用，整条流水线坍缩成
一个进程：

```text
scrcpy -s <t> --new-display=<WxH> --start-app=<pkg> [--max-fps F] [--video-bit-rate NM]
```

杀死 scrcpy 即销毁虚拟显示——不需要任何清理协议。旧的 am-start 实现保留在
git 历史中，可作为独立 `adb-intent` transporter 的起点（intent 级启动必须走
`am start`，scrcpy 做不到）。

### D4 · 两条不透明通道，两种契约

| 通道 | 谁解析 | 失败模式 | 存入 Profile |
|---|---|---|---|
| `--param KEY=VALUE` | appcast 后端代码 | 未识别键 → 警告并忽略 | ✅ `params:` |
| `-- RAW_ARGS` | 后端二进制（scrcpy） | 未知选项 → 二进制立即报错 | ✅ `raw_args:` |

param 是语义化的（一个键可以映射为二进制路径、一个 flag、或纯行为）；
透传是语法性的 argv 原样追加，零封装直达 scrcpy 全部选项。每个后端公布自己
认识的 param 键集并对陌生键告警——拼写错误可见，同时不破坏 fork 扩展性。

### D5 · 合并优先级栈

高者胜：

```text
位置三元组 > 三元组专用选项（--transporter/--target/--app）
    > --param 逐键覆盖
    > Profile 字段（transporter/target/app/params/raw_args）
```

列表类型的 `raw_args` 遵循与标量相同的"显式即胜"规则：CLI 非空透传整体替换
Profile 值。

### D6 · 无状态容忍

除非给定 `--profile`，`appcast run` 绝不触碰文件系统。日志目录不可写时静默
降级为仅 stderr。profiles 目录不存在等价于"没有 Profile"，绝不是错误。

### D7 · 名字编码技术，不编码版本

transporter 的注册名是 `adb-scrcpy`——平台加流水线技术——因此未来 fork 的
`adb-amstart` 变体不会与它冲突。名字里刻意不放版本号：本后端依赖的是
**能力集**（虚拟显示，scrcpy >= 3），而不是"scrcpy 3"；上游大版本持续演进，
能力集却岿然不动。该要求由运行时守卫强制（会话启动时探测 `scrcpy --version`），
失败时给出明确报错，而非事后一条难以理解的 flag 错误。

## Fork 扩展指南

实现 `Transporter` trait，在 `src/core/transporters/mod.rs` 注册即可——
CLI 零改动：

```rust
registry.register("adb-intent", || Box::new(my::AdbIntent));
// appcast run adb-intent <目标> <应用> --param action=... --param data=...
```

你的后端自定义理解哪些 params（并对陌生键告警）；`ResolvedConfig.raw_args`
的去向也由你决定。动态 `.so` 插件目前刻意不做——扩展是源码级的。
