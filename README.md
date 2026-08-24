# AppCast

[English](README.md) | [简体中文](README.cn.md)

Cast a single Android app's screen into its own native window on this desktop,
via one scrcpy virtual-display pipeline. The phone's main screen stays free.

```bash
appcast run adb-scrcpy 10.0.0.8:5555 com.termux
```

## How it works

One scrcpy (>= 3.0) process does everything — creates a virtual display at
the configured resolution, starts the app inside it, mirrors it locally:

```text
scrcpy -s <target> \
    --new-display=<WxH> \
    --start-app=<package> \
    --max-fps <fps> --video-bit-rate <n>M
```

Closing the window or Ctrl+C kills scrcpy, which destroys the virtual display.
No manual display bookkeeping, no `am start` permission pitfalls.

> Design rationale and decision log: [docs/design.md](docs/design.md)
> ([简体中文](docs/design.cn.md)).

## Requirements

- `adb` on PATH
- `scrcpy` >= 3.0 on PATH (4.x recommended)
- Android device with USB/network debugging enabled

## Install

### Nix

```bash
nix run github:AndroidAppsUsedByMyself/appcast -- --help
```

### Debian / Ubuntu (apt repository)

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

Or grab `termux-<arch>.deb` directly from
[Releases](https://github.com/AndroidAppsUsedByMyself/appcast/releases).

### Windows

Download `windows-x86_64.exe` from
[Releases](https://github.com/AndroidAppsUsedByMyself/appcast/releases)
(static CRT, no DLLs; SmartScreen may warn on first run).

## Usage

Addressing slots are per-transporter: adb needs `<TARGET> <APP>`, other
transporters may need fewer (e.g. a web one takes just a URL).

```bash
# Cast an app (stateless one-liner)
appcast run adb-scrcpy <serial|ip:port> <package>

# Expose any scrcpy option via passthrough (appended verbatim)
appcast run adb-scrcpy DUMMY com.termux --param resolution=1280x960 \
    -- --no-vd-destroy-content -x --video-codec=h265

# Print the merged command line instead of running it
appcast snapshot --profile qq

# List apps on a device
appcast list adb-scrcpy 10.0.0.8:5555          # ids only
appcast list adb-scrcpy 10.0.0.8:5555 -l       # display name <TAB> id
appcast list adb-scrcpy 10.0.0.8:5555 --json   # full structured entries

# Profiles: save / list / edit ($EDITOR) / rm
appcast profile save qq adb-scrcpy SERIAL com.tencent.mobileqq
appcast profile edit qq
appcast run --profile qq
```

Config merge priority (highest wins):
positional slots > dedicated slot options (`--transporter/--target/--app`)
> `--param` > profile fields (`params`/`raw_args` included).

Options: `--profile`, `--transporter`, `--target`, `--app`,
`--log-level`, `--param KEY=VALUE` (repeatable),
`--` passthrough (overrides profile `raw_args` when non-empty).

Backend params (`--param`): `resolution`, `fps`, `bit_rate`, `adb_path`,
`scrcpy_path`. Unknown keys are warned about and ignored — they are the
extension surface for custom transporters. The `adb-scrcpy` backend requires
**scrcpy >= 3.0** (checked at startup, with an explicit error message).

> `--activity` is intentionally unsupported: apps are started by scrcpy
> itself (`--start-app` targets whole packages).

## Extending

Backends are pluggable behind the `Transporter` trait. To add your own
(e.g. an `am start --display` variant that needs `--activity`):

```rust
// src/core/transporters/mine.rs
pub struct Mine;
impl Transporter for Mine { /* name / run / list_apps */ }

// src/core/transporters/mod.rs
registry.register("mine", || Box::new(mine::Mine));
```

Then `appcast run mine <target> <app>` works with zero CLI changes.
The historical `am start` pipeline is preserved in git history if you want
a starting point.

## Development

```bash
nix develop            # or use rustup toolchain
cargo build && cargo test && cargo clippy --all-targets
nix build              # verify packaging
```

## License

Apache-2.0 — see [LICENSE](LICENSE) and [NOTICE](NOTICE). appcast is an
independent tool that merely *invokes* external programs at runtime;
scrcpy (Genymobile) and adb (Android Open Source Project) remain under
their own Apache-2.0 terms.
