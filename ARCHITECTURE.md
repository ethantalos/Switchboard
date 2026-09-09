# Architecture

Current state: lists the worktrees of one Git repository and opens a checkout
in VS Code. No agent sessions, no browser/chat/music integrations, and no
storage beyond remembering the last repository path in the webview.

## Stack
Tauri 2 desktop shell, React 19 + TypeScript frontend built by Vite, Rust
backend. See docs/STACK.md.

## Data flow
The frontend has no OS access; it calls Rust through Tauri's `invoke`, and
Rust returns serde-serialized values.

| Command | Argument | Returns | Does |
| --- | --- | --- | --- |
| `list_worktrees` | `repoPath` | `Worktree[]` | Runs `git worktree list --porcelain` in that directory and parses it |
| `open_in_vscode` | `path` | nothing | Launches VS Code on that checkout |

`Worktree` carries `path`, `head`, `branch`, and the `detached`, `bare`,
`locked`, and `prunable` flags. Rust field names are serialized as camelCase.

Both commands return an error string the interface displays rather than
failing silently. Launching VS Code goes through `cmd /C` on Windows because
`code` is a shim script that cannot be executed directly.

## Files
- README.md: project context, scope, and open questions.
- TODO.md: current status and implementation tasks.
- docs/STACK.md: the major technologies and prerequisites.
- AGENTS.md and CLAUDE.md: identical, short working rules.
- LICENSE: MIT.
- .editorconfig, .gitattributes, .gitignore: formatting, line endings, exclusions.
- .oxlintrc.json: lint config from the Vite template.

## Layout
```text
index.html            Vite HTML entry
package.json          Frontend dependencies and scripts
vite.config.ts        Vite config
tsconfig*.json        TypeScript config
public/               Static assets served as-is
src/                  React frontend
  main.tsx            React entry point
  App.tsx             Repository input, worktree list, open action
  index.css           Design tokens and base styles
  App.css             Component styles
src-tauri/            Rust backend
  Cargo.toml          Rust dependencies
  build.rs            Tauri build script
  tauri.conf.json     App identifier, window, and build commands
  capabilities/       Permissions granted to the frontend
  icons/              Application icons
  src/main.rs         Binary entry point
  src/lib.rs          Tauri commands, worktree parsing, and its unit tests
```
`dist/` and `src-tauri/target/` are generated and ignored.

## Commands
- `npm install` once, then `npm run tauri dev` to run the desktop app.
- `npm run build` builds the frontend; `cargo test` runs the Rust tests.

Keep this map current in the same change as file, engine, config, or data-flow
changes. Plans belong in TODO.md; this file describes only what exists.
