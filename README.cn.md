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

# 列出设备上的包
appcast list adb-scrcpy 10.0.0.8:5555

# Profile：保存 / 列出 / 编辑（$EDITOR）/ 删除
appcast profile save qq adb-scrcpy 序列号 com.tencent.mobileqq
appcast profile edit qq
appcast run --profile qq
```

配置合并优先级（高者胜）：
位置槽位 > 槽位专用选项（`--transporter/--target/--app`）
> `--param` > Profile 字段（含 `params`/`raw_args`）。

选项：`--profile`、`--transporter`、`--target`、`--app`、
`--log-level`、`--param KEY=VALUE`（可重复）、
`--` 透传（非空时覆盖 Profile 的 `raw_args`）。

后端扩展参数（`--param`）：`resolution`、`fps`、`bit_rate`、`adb_path`、
`scrcpy_path`。未识别的键会警告并忽略——它们是自定义 transporter 的扩展通道。
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

## 开发

```bash
nix develop            # 或使用 rustup 工具链
cargo build && cargo test && cargo clippy --all-targets
nix build              # 验证打包
```

## 许可证

Apache-2.0 —— 见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。appcast 是独立工具，
运行时仅*调用*外部程序；scrcpy（Genymobile）与 adb（Android Open Source
Project）仍遵循其自身的 Apache-2.0 条款。
