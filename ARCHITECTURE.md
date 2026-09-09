# Architecture

Current state: a desktop widget that shows which Claude Code sessions need
attention, grouped by worktree, and opens a worktree in VS Code. Session state
comes from Claude Code hooks. Nothing is persisted; restarting forgets sessions.

## Stack
Tauri 2 desktop shell, React 19 + TypeScript frontend built by Vite, Rust
backend. See docs/STACK.md.

## Windows
Two windows, one frontend bundle. `main.tsx` picks the view from the query
string, so there is no second build.

- **main** — the widget. Fixed 320x320, transparent, undecorated, always on top.
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
the state of its most urgent session.

## Commands
The frontend has no OS access; it calls Rust through `invoke`.

| Command | Does |
| --- | --- |
| `list_worktrees(repoPath)` | Parses `git worktree list --porcelain`. Not currently called by the UI |
| `open_in_vscode(path)` | Launches VS Code on a checkout |
| `list_sessions()` | Current sessions, most urgent first |
| `hook_endpoint()` | The URL hooks should POST to |
| `connect_claude_code()` | Adds Switchboard's hooks to `~/.claude/settings.json`, appending to what is there and backing the file up first |
| `open_settings()` / `close_settings()` | The settings window |
| `close_widget()` | The widget has no title bar |
| `set_always_on_top(enabled)` | Window layering |
| `window_metrics()` | Scale factor and physical size, for mixed-DPI diagnosis |

Events sent the other way: `sessions-changed` and `hover-changed`.

## Files
- README.md: project context, scope, and open questions.
- TODO.md: current status and implementation tasks.
- docs/STACK.md: the major technologies and prerequisites.
- AGENTS.md and CLAUDE.md: identical, short working rules.
- LICENSE: MIT.
- app-icon.svg: icon source. Regenerate with `npx tauri icon app-icon.svg`.
- .editorconfig, .gitattributes, .gitignore: formatting, line endings, exclusions.
- .oxlintrc.json: lint config from the Vite template.

## Layout
```text
index.html            Vite HTML entry
package.json          Frontend dependencies and scripts
vite.config.ts        Vite config, including the src-tauri watch exclusion
tsconfig*.json        TypeScript config
public/favicon.svg    Browser tab icon
src/                  React frontend
  main.tsx            Entry point; chooses widget or settings by query string
  App.tsx             The widget: badge, worktree groups, session rows
  Settings.tsx        The settings window
  index.css           Design tokens shared by both windows
  App.css             Widget styles
  Settings.css        Settings window styles
src-tauri/            Rust backend
  Cargo.toml          Rust dependencies
  tauri.conf.json     Window config, bundle identifier, icons
  capabilities/       Permissions granted to each window
  icons/              Application icons, generated from app-icon.svg
  src/main.rs         Binary entry point
  src/lib.rs          Commands, hook listener, hover watch, worktree parsing
```
`dist/` and `src-tauri/target/` are generated and ignored.

## Commands to run it
- `npm install` once, then `npm run tauri dev`.
- `npm run build` builds the frontend; `cargo test` runs the Rust tests.

## Known limitations
- Mixed-DPI: the widget renders oversized on a monitor with different scaling.
- Sessions are in memory only.
- Codex sessions are not tracked; its CLI has no documented push equivalent.
- macOS is untested.

Keep this map current in the same change as file, engine, config, or data-flow
changes. Plans belong in TODO.md; this file describes only what exists.
