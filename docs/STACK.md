# Stack

| Part | Tech | Role |
| --- | --- | --- |
| Desktop shell | Tauri 2 | Native Windows/macOS window using the OS webview |
| Native logic | Rust | Git discovery, process launching, OS access behind Tauri commands |
| Interface | React 19 | Worktree lists, sessions, activity feed |
| Types | TypeScript | Makes action targets and session states explicit |
| Frontend build | Vite | Dev server and production UI build |
| Styling | Plain CSS | No framework until real UI needs one |
| Packages | npm + Cargo | Frontend and Rust dependencies |

Exact versions live in `package.json` and `src-tauri/Cargo.toml`.

## Prerequisites
- Node.js and npm
- Rust via rustup, `stable-msvc` toolchain
- Windows: MSVC C++ Build Tools (the linker Rust needs) and the WebView2 runtime
- macOS: Xcode Command Line Tools

[Tauri docs](https://v2.tauri.app/) · [Vite docs](https://vite.dev/guide/)
