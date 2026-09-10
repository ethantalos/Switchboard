# Architecture

Current state: a desktop widget that shows which Claude Code sessions need
attention, grouped by worktree, and opens a worktree in VS Code. Worktrees come
from two sources: hooks report sessions as they act, and `~/.claude/ide` is
scanned for editors that already have Claude Code attached. Nothing is
persisted; restarting forgets sessions.

## Stack
Tauri 2 desktop shell, React 19 + TypeScript frontend built by Vite, Rust
backend. See docs/STACK.md.

## Names
Switchboard is the application. **Eve** is the widget itself - the always-on-top
badge that expands into the worktree list. The distinction only matters in UI
copy and window titles; nothing in the code is named after either.

## Windows
Two windows, one frontend bundle. `main.tsx` picks the view from the query
string, so there is no second build.

- **main** — Eve, the widget. Fixed 320x320, transparent, undecorated, always
  on top.
  It never moves or resizes: collapsed simply paints a 60x60 box in the middle
  and ignores the cursor, so the transparent margin does not block clicks.
  Resizing on hover was tried and abandoned; Windows applies moves
  asynchronously, so reading geometry back to recentre races, drifts, and shows
  a stale frame mid-resize.
- **settings** — an ordinary decorated window at `index.html?window=settings`.

## Session state
Claude Code fires the same hooks wherever it runs, including sessions inside the
VS Code extension, so Switchboard sees sessions it never started.

1. A thread listens on `127.0.0.1:47823` for hook deliveries.
2. Each payload carries `session_id` and `cwd`. The event maps to a state:
   `SessionStart` idle, `UserPromptSubmit` working, `Notification` and
   `PermissionRequest` needs_you, `Stop` and `StopFailure` done, `SessionEnd`
   removes the session.
3. Sessions live in an in-memory map keyed by `session_id`.
4. Each change emits `sessions-changed` to the frontend.

`cwd` is the join key: the widget groups sessions by it, and a worktree takes
the state of its most urgent session. Hook payloads carry the process's
*current* directory, so a session that cd's into a subfolder would appear as a
second worktree; a deeper path never replaces one already known.

States are named for what they ask of you, not for what the assistant did:

| state | meaning | from |
| --- | --- | --- |
| `needs_you` | blocked on you | `PermissionRequest`, blocking `Notification` |
| `failed` | ended badly | `StopFailure` |
| `waiting` | finished its turn, your move | `Stop`, idle `Notification` |
| `working` | mid-turn | `UserPromptSubmit`, `PostToolUse`, `SubagentStop` |
| `idle` | just started | `SessionStart` |
| `unknown` | known to exist, state unknown | disk or an IDE lock |

`Stop` used to map to "done", painted green. That is the single most useful
state - the session is sitting there waiting for its human - and it was being
reported as nothing to see here.

Detail text comes from whichever field the event actually carries: there is no
common `message` field. `Stop` has `last_assistant_message`,
`UserPromptSubmit` has `prompt`, tool events have `tool_name`, and
`Notification` has `notification_type` rather than prose. An `idle_prompt`
notification is not treated as blocking - Claude Code fires one about a minute
after finishing, and painting that red teaches you to ignore the badge.

## Sessions Switchboard never saw start
Hooks only describe sessions that fire while Switchboard is running, and the
store is in memory, so a restart forgot everything and a session was invisible
until it happened to emit. Worse, a long turn emits nothing at all between the
prompt and the stop, so an agent that had been working for an hour looked
silent.

Transcripts fix both. Claude Code appends to
`~/.claude/projects/<slug>/<session-id>.jsonl` throughout a turn, and the
files survive restarts.

1. Every five seconds the transcript directory is scanned. Files sit directly
   in a project directory; the `<session>/subagents/` tree below holds agents a
   session spawned, not sessions.
2. `cwd` is read from the first user entry, within the first 40 lines - these
   files reach hundreds of thousands of lines.
3. Every worktree keeps every transcript it has ever had, so a raw scan
   resurrects months of dead sessions. Anything still being written is kept,
   plus the newest per worktree.
4. Hooks own state; disk only supplies sessions hooks never mentioned and
   keeps the clock honest so a long turn is not mistaken for silence. Writing
   more than 15 seconds after the last hook is treated as a turn that was
   missed, because a `Stop` writes its own tail a moment after firing.

A session silent for 30 minutes is dimmed and left out of the badge count;
one silent for 12 hours is dropped, since a closed window fires no
`SessionEnd`. Paths are compared case-insensitively
with separators normalised, because hooks report `C:\Switchboard` while IDE
locks report `c:\Switchboard`.

## Editor discovery
Hooks only describe what happens after Switchboard starts, so sessions that
were already running are invisible to them. Claude Code also writes one lock
file per IDE connection into `~/.claude/ide`, named `<port>.lock`, holding the
editor's `pid`, `ideName`, and `workspaceFolders`.

1. A thread rescans that directory every 4 seconds.
2. Closing an editor leaves its lock behind, so each `pid` is checked against
   the running process list (`tasklist` on Windows, `ps` elsewhere) and dead
   entries are dropped. If that check fails outright, locks are kept rather
   than reporting every editor as closed.
3. Changes emit `workspaces-changed`.

A discovered editor with no hook activity shows as a worktree in the `unknown`
state, since presence is not progress. The lock files also contain an auth
token; only `pid`, `ideName`, and `workspaceFolders` are read, and the token is
never stored, logged, or sent to the frontend.

## Opening a worktree
Clicking a worktree fills the monitor the widget is currently on, so the editor
lands where you are looking rather than wherever it was last.

1. `window.current_monitor()` gives the widget's monitor in physical pixels.
2. Every VS Code window belongs to one process, so the process id cannot tell
   them apart. The title can: VS Code writes `<file> - <folder> - Visual Studio
   Code`, or `<folder> - Visual Studio Code` with nothing open, so the folder
   name plus that suffix finds the window.
3. An already-open folder is reused. Otherwise `code <path>` runs and the
   window is waited for, up to 20 seconds.
4. Windows maximises onto whichever monitor holds the window, so the window is
   restored if maximised, moved onto the target monitor, then maximised. The
   window's DWM transitions are suppressed around those three steps and
   restored 150ms later, otherwise each one animates and the move reads as a
   stutter. Measured at 17ms for the placement itself.

Maximising, rather than setting the rect directly, is what keeps this correct
on mixed-DPI setups: Windows recomputes the size from the target monitor. A
single `SetWindowPlacement` to the monitor's work area looks tidier but lands
1.5x too large on a 150% display, and does not change monitor at all when the
window is already maximised - `ptMaxPosition` wins over `rcNormalPosition`.

## Sending a worktree back
Clicking a worktree that is already filling the widget's monitor puts its
editor back exactly where it was. The rect and maximised state are recorded
before the fill, kept in memory keyed by the normalised path, and dropped once
used. `list_parked` and the `parked-changed` event tell the frontend which
worktrees can be sent back, so the row can show it.

Three things this got wrong first, all found by measuring:

- **Focus is the wrong test for "already filled."** Clicking the widget takes
  focus off the editor, so the editor is never in front when the second click
  lands. The test is geometry: maximised, with its centre on that monitor.
- **`GetWindowPlacement` is the wrong source for the saved rect.** Its
  `rcNormalPosition` drifts from the real rect on mixed-DPI setups - one
  window reported 1937 wide while actually being 1294. `GetWindowRect` is the
  truth.
- **One `SetWindowPos` does not restore the size across a DPI boundary.** A
  1456x908 window returning from the 150% display lands at 971x605, exactly
  1/1.5. The size is restated a second time, once the window is already on the
  destination monitor.

A minimised window is handled everywhere: it is never recorded (Windows
reports (-32000, -32000) for one, which would restore it off-screen) and it is
brought back up before being moved.

Suppressing the animation sets `DWMWA_TRANSITIONS_FORCEDISABLED` on a window
belonging to another process, and that sticks for the rest of that window's
life. It is therefore held by a guard whose Drop turns it back on, so an early
return or a panic cannot cost the user their editor's animations.

One related trap avoided rather than fixed: `WINDOWPLACEMENT.rcNormalPosition`
is in workspace coordinates, offset by the *primary* monitor's work area, not
the target's. That offset is (0, 0) until something docks to the top or left
of the primary screen, at which point code reading it as screen coordinates
starts placing windows wrong. Nothing here reads it - `GetWindowRect` and
`SetWindowPos` are both screen coordinates.

This drops to Win32 (`EnumWindows`, `SetWindowPos`, `ShowWindow`) because Tauri
can only move its own windows. It is Windows-only; elsewhere the command just
launches VS Code without placing it. Two worktrees whose folders share a
basename cannot be told apart by title.

The command is async. A cold VS Code launch is waited on for up to 20 seconds,
and a synchronous command would block the main thread for all of it.

## Commands
The frontend has no OS access; it calls Rust through `invoke`.

| Command | Does |
| --- | --- |
| `list_worktrees(repoPath)` | Parses `git worktree list --porcelain`. Not currently called by the UI |
| `open_in_vscode(path)` | Opens a checkout in VS Code, filling the monitor the widget is on |
| `list_sessions()` | Current sessions, most urgent first |
| `list_workspaces()` | Editors with Claude Code attached, from `~/.claude/ide` |
| `list_parked()` | Worktrees currently filling a monitor, which a click can send back |
| `list_order()` / `set_order(order)` | The order worktrees were dragged into |
| `hook_endpoint()` | The URL hooks should POST to |
| `quiet_after_ms()` | How long before a silent session is dimmed |
| `recent_hooks()` | The last 200 hook deliveries, for diagnosing silence |
| `connect_claude_code()` | Adds Switchboard's hooks to `~/.claude/settings.json`, appending to what is there and backing the file up first |
| `open_settings()` / `close_settings()` | The settings window |
| `close_widget()` | The widget has no title bar |
| `set_always_on_top(enabled)` | Window layering |
| `window_metrics()` | Scale factor and physical size, for mixed-DPI diagnosis |

Events sent the other way: `sessions-changed`, `workspaces-changed`,
`parked-changed`, `order-changed`, `hooks-changed`, and `hover-changed`.

`open_settings` is async on purpose. A synchronous command runs on the main
thread, and building a webview there deadlocks the event loop: the window
never leaves `about:blank`, the call never returns, and every later command
queues behind it, which also breaks dragging and closing both windows.

## Files
- README.md: project context, scope, and open questions.
- TODO.md: current status and implementation tasks.
- docs/STACK.md: the major technologies and prerequisites.
- AGENTS.md and CLAUDE.md: identical, short working rules.
- LICENSE: MIT.
- app-icon.svg: icon source. Regenerate with `npx tauri icon app-icon.svg`.
- .editorconfig, .gitattributes, .gitignore: formatting, line endings, exclusions.
- .oxlintrc.json: lint config from the Vite template.

## Two languages, two folders
`src-react/` is the entire UI. It runs inside a WebView (WebView2 on Windows,
WKWebView on macOS), so it is sandboxed like a browser tab: it cannot open a
port, read a file, spawn a process, or create a window.

`src-rust/` is a native binary and does everything the sandbox forbids: owns
the widget and settings windows, listens on 127.0.0.1:47823, reads
`~/.claude/ide`, launches VS Code, and polls the cursor for hover.

They talk over Tauri's IPC bridge in both directions - the frontend calls
`invoke("command")`, the backend calls `emit("event")`. That bridge is the
only way across.

The folders were renamed from the Tauri defaults (`src/` and `src-tauri/`)
because two folders starting with "src" read as one thing split in half
rather than two languages. The Tauri CLI locates its project by finding
`tauri.conf.json`, not by the folder name, so the rename needs no config.
Moving the crate does invalidate the Cargo build cache, which bakes in
absolute paths: delete `src-rust/target` after any such move.

## Layout
```text
index.html            Vite HTML entry
package.json          Frontend dependencies and scripts
vite.config.ts        Vite config, including the src-rust watch exclusion
tsconfig*.json        TypeScript config
public/favicon.svg    Browser tab icon
src-react/            React frontend, the entire UI
  main.tsx            Entry point; chooses widget or settings by query string
  App.tsx             The widget: badge, worktree groups, session rows
  Settings.tsx        The settings window
  index.css           Design tokens shared by both windows
  App.css             Widget styles
  Settings.css        Settings window styles
src-rust/             Rust backend, everything native
  Cargo.toml          Rust dependencies
  tauri.conf.json     Window config, bundle identifier, icons
  capabilities/       Permissions granted to each window
  icons/              Application icons, generated from app-icon.svg
  src/main.rs         All Rust: entry point, commands, hook listener, hover
                      watch, editor scan, worktree parsing
```
`dist/` and `src-rust/target/` are generated and ignored.

## One Rust file
`src-rust/src/main.rs` is the whole backend. The Tauri scaffold splits a
desktop app into `main.rs` plus `lib.rs` so the same code can build for
iOS and Android, which need a library rather than a `main()`. Switchboard
targets Windows and macOS only, so the split was removed along with the
`[lib]` target in Cargo.toml. Adding a mobile target later means putting
both back.

## Ordering
Worktrees sort by urgency, which is right until you have a fixed mental
picture of where each one lives. Dragging a row pins the arrangement, saved to
`worktree-order.json` in the app config directory - the one piece of state
worth persisting, because everything else is derived from what Claude Code is
doing and a preference that resets every restart is not a preference.

A worktree the user has not placed falls in after the placed ones, still
sorted by urgency, so a new one still surfaces on its own.

## Commands to run it
- `npm install` once, then `npm run tauri dev`.
- `npm run build` builds the frontend; `cargo test` runs the Rust tests.

## Known limitations
- Mixed-DPI: the widget renders oversized on a monitor with different scaling.
- Sessions are in memory only.
- Codex sessions are not tracked; its CLI has no documented push equivalent.
- Editor discovery finds that a session exists, not what it is doing. State
  stays `unknown` until a hook arrives.
- macOS is untested.

Keep this map current in the same change as file, engine, config, or data-flow
changes. Plans belong in TODO.md; this file describes only what exists.
