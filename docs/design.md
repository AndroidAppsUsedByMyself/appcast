# AppCast Design Notes

[English](design.md) | [简体中文](design.cn.md)

Architecture decisions and their rationale. Newest decisions may supersede
older ones; each entry states what changed and why.

## Architecture

```text
CLI frontend (clap)          ── thin: addressing trio + two opaque channels
   │  priority-stack merge
   ▼
ResolvedConfig               ── {transporter, target, app, params, raw_args}
   │  registry.get(name)
   ▼
Transporter trait (dyn)      ── name / run / list_apps
   ├── adb-scrcpy  (scrcpy >=3 virtual display; owns params + defaults)
   ├── ssh-x11  (placeholder)
   └── waypipe  (placeholder)
```

Core principle: **thin generic core, fat backends**. The core only knows the
universal addressing trio; every backend-specific knob travels through
`params` and is interpreted — with its defaults — by the selected backend.

## Decision log

### D1 · Addressing slots are backend-defined schema, core carries them opaquely

The tool's addressing model is a pair of optional slots plus a protocol:

```text
appcast run <TRANSPORTER> [<TARGET>] [<APP>]
#                        └─ "where"   └─ "what to open there"
```

Different transporters need different arities:

| transporter | slots needed | mapping |
|---|---|---|
| adb-scrcpy | target + app | device serial → package name |
| ssh-x11 / waypipe | target + app | host → executable path |
| web/webview (future) | target only | URL |
| vnc (future) | target only | `host:display` |
| local-window (future) | app only | window title/id |

Therefore the **core never validates arity**: `target`/`app` are
`Option<String>` end-to-end, and each backend rejects what it cannot work
with using its own usage text (`AppError::Usage`). The only core-level error
is `MissingTransporter` (the registry lookup needs it).

What the core *does* guarantee: slot positions are stable and merge with a
fixed priority (D5), so muscle memory transfers between backends. Moving
`app` into `--param`, or making the trio mandatory in the CLI, were both
considered and rejected — the former demotes addressing into stringly typed
config, the latter invents requirements no universal truth supports.

### D2 · Typed display knobs were demoted to params

`--resolution/--fps/--bit-rate` used to be strongly typed CLI options with
defaults in the profile layer. They were removed because they are scrcpy
concepts, not universal ones (waypipe/VNC/RDP backends have different or no
equivalents). They now travel as params:

```bash
appcast run adb-scrcpy SERIAL com.app --param resolution=1280x960 --param fps=90
```

The adb/scrcpy backend interprets them and applies defaults
(`1920x1080`, `60`, `8` Mbps) when absent. Defaults live where the semantics
live. Malformed values fail loudly (`InvalidParamValue`) instead of being
silently coerced.

Legacy profiles carrying the old top-level keys (`resolution:` etc.) still
load; those keys are ignored.

### D3 · One scrcpy pipeline, not an am-start dance

The original design mirrored Android's own primitives (`dumpsys display`
free-id scan → `am start --display N -n pkg/Act` → `scrcpy --display-id N`).
Real devices broke it: ROMs reject shell-launched intents on virtual displays
(`SecurityException: Permission Denial`), and manual display bookkeeping is
fragile across Android versions.

Since scrcpy 3.0 the server can create the virtual display and start the app
itself, so the whole pipeline collapsed into one process:

```text
scrcpy -s <t> --new-display=<WxH> --start-app=<pkg> [--max-fps F] [--video-bit-rate NM]
```

Killing scrcpy destroys the display — no cleanup protocol needed. The old
am-start implementation lives in git history as a starting point for a
separate `adb-intent` transporter (intent-level launching needs `am start`;
scrcpy cannot do it).

### D4 · Two opaque channels with different contracts

| channel | who parses | failure mode | persisted in profile |
|---|---|---|---|
| `--param KEY=VALUE` | appcast backend code | unknown key → warning + ignore | ✅ `params:` |
| `-- RAW_ARGS` | the backend binary (scrcpy) | unknown flag → binary errors immediately | ✅ `raw_args:` |

Params are semantic (a key can map to a binary path, a flag, or pure
behavior); passthrough is syntactic argv appended verbatim, giving full
access to every scrcpy option with zero wrapping. Each backend publishes its
known param set and warns on strangers — typos become visible without
breaking fork extensibility.

### D5 · Merge priority stack

Highest wins:

```text
positional trio > dedicated trio options (--transporter/--target/--app)
    > --param overrides (per-key)
    > profile fields (transporter/target/app/params/raw_args)
```

List-typed `raw_args` uses append semantics: final list = profile args
++ CLI args, in that order (scrcpy's last-wins parser makes tail overrides
possible). `--clear-raw` discards the profile base for cases where the
stored flags are unwanted.

### D6 · Stateless tolerance

`appcast run` never touches the filesystem unless `--profile` is given.
Logging degrades to stderr-only if `$XDG_STATE_HOME` is unwritable. A missing
profiles directory reads as "no profiles", never an error.

### D7 · Names encode technique; versions do not

The transporter id is `adb-scrcpy` — platform plus pipeline technique — so a
future fork's `adb-amstart` variant never collides with it. Version numbers
are deliberately excluded from names: this backend depends on a *capability
set* (virtual displays, scrcpy >= 3), not on "scrcpy 3", and upstream majors
keep moving while the capability stays put. The requirement is enforced by a
runtime guard (`scrcpy --version` probe at session start) that fails with an
explicit message instead of an inscrutable flag error later.

## Extension guide for forks

Implement the `Transporter` trait, register in
`src/core/transporters/mod.rs`, done — no CLI changes:

```rust
registry.register("adb-intent", || Box::new(my::AdbIntent));
// appcast run adb-intent <target> <app> --param action=... --param data=...
```

Your backend defines which params it understands (and should warn on
strangers); `ResolvedConfig.raw_args` is yours to route wherever appropriate.
Dynamic `.so` plugins are intentionally out of scope for now; extension is
source-level by design.
