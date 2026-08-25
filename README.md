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

# Profiles: save / list / edit ($VISUAL/$EDITOR, fallback notepad/vi) / rm
appcast profile save qq adb-scrcpy SERIAL com.tencent.mobileqq \
    --param resolution=1280x960 -- --no-vd-destroy-content
appcast profile edit qq
appcast run --profile qq

# Derive a variant from an existing profile (inherits everything)
appcast profile save qq-lan --profile qq --target 192.168.1.20:5555
```

Config merge priority (highest wins):
positional slots > dedicated slot options (`--transporter/--target/--app`)
> `--param` > profile fields (`params`/`raw_args` included).

Options: `--profile`, `--transporter`, `--target`, `--app`,
`--log-level`, `--param KEY=VALUE` (repeatable),
`--` passthrough (appended to profile `raw_args`; `--clear-raw` resets).

Backend params (`--param`): `resolution`, `fps`, `bit_rate`, `adb_path`,
`scrcpy_path`. Unknown keys are warned about and ignored — they are the
extension surface for custom transporters.

The virtual display always uses `--display-ime-policy=local`, so device
input methods appear inside the casted window. For typing CJK with your
PC keyboard, switch scrcpy to HID keyboard mode:

```bash
appcast run adb-scrcpy SERIAL com.tencent.mobileqq -- --keyboard=uhid
```

(Plain key injection cannot inject CJK characters — you would see
`Could not inject char` warnings.) The `adb-scrcpy` backend requires
**scrcpy >= 3.0** (checked at startup, with an explicit error message).

> `--activity` is intentionally unsupported: apps are started by scrcpy
> itself (`--start-app` targets whole packages).

### Casting web apps

Two transporters cover web content, differing only in the window mechanism:

| transporter | window | needs | params |
|---|---|---|---|
| `web-browser` (built-in) | system browser in app mode (`--app`) — no tabs, no address bar | any Chromium-family browser; Firefox via `kiosk=true` | `browser_path`, `window_size`, `kiosk` |
| `web-webview` (plugin) | embedded WebView (WebKitGTK / WebView2 / WKWebView) in its own window | plugin installed (see below); Linux build needs WebKitGTK | `window_size`, `title` |

```bash
appcast run web-browser https://excalidraw.com --param window_size=1600x900
appcast run web-webview https://excalidraw.com --param title=Draw
appcast transporters   # list every backend and where it came from
```

## Extending

Backends plug in behind the `Transporter` trait, two ways:

**In-tree (Rust, compiled into the binary)** — right for backends that
shell out to external tools like adb-scrcpy does:

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

Every built-in adapter is an individually toggleable cargo feature whose
exclusive dependencies travel with it (`adapter-adb = ["dep:regex"]`,
plus `adapter-browser`, `adapter-ssh`, `adapter-waypipe`). Dropping one
removes its code *and* its deps, and it disappears from CLI validation
and completions:

```bash
cargo build --no-default-features --features cli,adapter-browser  # slim web-only CLI
cargo check --no-default-features --features cli                  # plugins only, no built-ins
```

For a future heavy in-tree adapter: declare the dependency `optional`,
wire it as `adapter-<name> = ["dep:<crate>"]`, and gate the module behind
the same feature — nothing else changes.

**Out-of-tree (`.so`/`.dll` plugin, own dependency tree)** — right for
backends that need heavy dependencies. Rust has no stable ABI, so plugins
speak a narrow C ABI (JSON payloads, version handshake) generated entirely
by the [`sdk/appcast-plugin`](sdk/appcast-plugin) SDK — you implement one
blocking trait, zero `unsafe`:

```rust
use appcast_plugin::{export_appcast_transporter, SimpleTransporter};

struct Mine;
impl SimpleTransporter for Mine { /* name / run / list_apps */ }

export_appcast_transporter!(Mine);
```

Build as a cdylib named `libappcast_tpt_<name>.{so,dylib,dll}` and drop it
into `~/.config/appcast/transporters/` (or point `$APPCAST_TRANSPORTER_DIR`
at a PATH-style list of dirs). Plugins may override same-named built-ins;
`appcast transporters` shows what loaded and from where. See
[plugins/webview](plugins/webview) for a complete real-world plugin.

## Development

```bash
nix develop            # or use rustup toolchain
cargo build && cargo test && cargo clippy --all-targets
nix build              # verify packaging
```

### NixOS workflows

| task | shell | loop |
|---|---|---|
| core / in-tree adapter | `nix develop` | edit `src/core/transporters/*.rs`, register in `mod.rs`, `cargo test` |
| plugin (bare base) | `nix develop .#plugin` | edit `plugins/<name>/src/lib.rs`, then `install-plugin <crate>` (build + copy to `~/.config/appcast/transporters/`), verify with `appcast transporters` |
| webview plugin | `nix develop .#plugin-webview` (adds the WebKitGTK stack) | same loop |

Each plugin gets its own shell because backends link against different
native stacks; add one entry to `mkPluginShell` calls in `flake.nix` when
introducing a new backend. Workspace-wide runs that include GUI-linked
members must go through their shell (`nix develop .#plugin-webview -c
cargo test --workspace`); the bare shell covers core + SDK via workspace
`default-members`.

Built plugin `.so` files carry Nix store rpaths to their WebKitGTK
runtime, so they load anywhere — no `LD_LIBRARY_PATH` needed.

Declarative system-wide install:

```nix
# flake.nix
inputs.appcast.url = "github:AndroidAppsUsedByMyself/appcast";

# configuration.nix
imports = [ inputs.appcast.nixosModules.default ];
nixpkgs.overlays = [ inputs.appcast.overlays.default ];
programs.appcast = {
  enable = true;
  plugins = [ pkgs.appcast-tpt-webview ];  # or programs.appcast.package = pkgs.appcast;
};
```

The module wraps `appcast` so the declarative plugin store is searched
after any per-user `$APPCAST_TRANSPORTER_DIR` — users can still shadow a
system plugin by dropping a same-named one into their own directory.

## License

Apache-2.0 — see [LICENSE](LICENSE) and [NOTICE](NOTICE). appcast is an
independent tool that merely *invokes* external programs at runtime;
scrcpy (Genymobile) and adb (Android Open Source Project) remain under
their own Apache-2.0 terms.
