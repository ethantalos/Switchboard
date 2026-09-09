# Architecture

Current state: a Tauri 2 desktop shell that builds and runs the default
template UI. No Switchboard features are wired up yet: no repository
registration, no worktree view, no agent sessions, no persistence.

## Stack
Tauri 2 desktop shell, React 19 + TypeScript frontend built by Vite,
Rust backend. Frontend calls Rust through Tauri commands.

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
  App.tsx             Root component (template default)
  index.css, App.css  Styles
  assets/             Images imported by components
  worktrees.mjs       Parses `git worktree list --porcelain`; not yet wired to UI
src-tauri/            Rust backend
  Cargo.toml          Rust dependencies
  build.rs            Tauri build script
  tauri.conf.json     App identifier, window, and build commands
  capabilities/       Permissions granted to the frontend
  icons/              Application icons
  src/main.rs         Binary entry point
  src/lib.rs          Application setup and Tauri commands
```
`dist/` (frontend build) and `src-tauri/target/` (Rust build) are generated
and ignored.

## Commands
- `npm install` once, then `npm run tauri dev` to run the desktop app.
- `npm run build` builds the frontend only.
- `npm run tauri build` produces a packaged app.

Keep this map current in the same change as file, engine, config, or data-flow
changes. Plans belong in TODO.md; this file describes only what exists.
