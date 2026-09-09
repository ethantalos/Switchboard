use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// Port the Claude Code hooks POST to. Localhost only.
const HOOK_PORT: u16 = 47823;

/// One entry from `git worktree list --porcelain`.
#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Parse porcelain output. Records are separated by blank lines and each
/// attribute is its own line, so checkout paths containing spaces stay intact.
fn parse_worktrees(stdout: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;

    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));

        if key == "worktree" {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            current = Some(Worktree {
                path: value.to_string(),
                ..Default::default()
            });
            continue;
        }

        let Some(worktree) = current.as_mut() else {
            continue;
        };
        match key {
            "HEAD" => worktree.head = Some(value.to_string()),
            "branch" => worktree.branch = Some(value.trim_start_matches("refs/heads/").to_string()),
            "detached" => worktree.detached = true,
            "bare" => worktree.bare = true,
            "locked" => worktree.locked = true,
            "prunable" => worktree.prunable = true,
            _ => {}
        }
    }

    if let Some(worktree) = current.take() {
        worktrees.push(worktree);
    }
    worktrees
}

/// List every worktree belonging to the repository that contains `repo_path`.
#[tauri::command]
fn list_worktrees(repo_path: String) -> Result<Vec<Worktree>, String> {
    if !Path::new(&repo_path).is_dir() {
        return Err(format!("Not a directory: {repo_path}"));
    }

    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&repo_path)
        .output()
        .map_err(|e| format!("Could not run git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git failed in {repo_path}")
        } else {
            stderr
        });
    }

    Ok(parse_worktrees(&String::from_utf8_lossy(&output.stdout)))
}

/// Open a checkout in VS Code.
#[tauri::command]
fn open_in_vscode(path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err(format!("Path no longer exists: {path}"));
    }

    // On Windows `code` is a .cmd shim, which CreateProcess cannot launch
    // directly, so go through cmd.exe and suppress its console window.
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .args(["/C", "code"])
            .arg(&path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    };
    #[cfg(not(windows))]
    let spawned = Command::new("code").arg(&path).spawn();

    spawned
        .map(|_| ())
        .map_err(|e| format!("Could not launch VS Code: {e}"))
}

/// The window is a fixed EXPANDED square and never moves or resizes. Collapsed
/// simply paints a COLLAPSED-sized box in the middle of it and lets clicks pass
/// through the rest. Resizing on hover looked right but is unworkable: Windows
/// applies moves asynchronously, so reading geometry back to recentre the box
/// races, drifts, and shows a stale frame mid-resize.
/// Must match `.box.collapsed` in App.css; it is the hover target.
const COLLAPSED: f64 = 60.0;
/// How often to check whether the pointer is over the widget.
const HOVER_POLL_MS: u64 = 60;

/// Watch the pointer and flip the widget between collapsed and expanded.
///
/// While collapsed the window ignores the cursor, so the transparent margin
/// around the badge does not block clicks reaching whatever is behind it.
fn start_hover_watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut expanded = false;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(HOVER_POLL_MS));

            let Some(window) = app.get_webview_window("main") else {
                continue;
            };
            let (Ok(position), Ok(size), Ok(cursor), Ok(scale)) = (
                window.outer_position(),
                window.outer_size(),
                window.cursor_position(),
                window.scale_factor(),
            ) else {
                continue;
            };

            let width = size.width as i32;
            let height = size.height as i32;
            let (x, y) = (cursor.x as i32, cursor.y as i32);

            let over_window = x >= position.x
                && x < position.x + width
                && y >= position.y
                && y < position.y + height;

            let half = (COLLAPSED * scale / 2.0) as i32;
            let centre_x = position.x + width / 2;
            let centre_y = position.y + height / 2;
            let over_badge =
                (x - centre_x).abs() <= half && (y - centre_y).abs() <= half;

            // Grow when the pointer reaches the badge, shrink once it leaves
            // the whole window, so the edges are not a knife edge.
            let want = if expanded { over_window } else { over_badge };
            if want != expanded {
                expanded = want;
                let _ = window.set_ignore_cursor_events(!expanded);
                let _ = app.emit("hover-changed", expanded);
            }
        }
    });
}

/// Hook events Switchboard needs in order to track session state.
const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "Notification",
    "PermissionRequest",
    "SessionEnd",
];

/// Add Switchboard's HTTP hooks to the user's Claude Code settings.
///
/// Appends to whatever is already configured rather than replacing it, backs
/// the file up once before the first change, and is safe to run twice.
#[tauri::command]
fn connect_claude_code(app: AppHandle) -> Result<String, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| format!("Could not find your home directory: {e}"))?;
    let dir = home.join(".claude");
    let file = dir.join("settings.json");

    let original = std::fs::read_to_string(&file).unwrap_or_else(|_| "{}".to_string());
    let mut settings: serde_json::Value = serde_json::from_str(&original)
        .map_err(|e| format!("settings.json is not valid JSON: {e}"))?;

    // Back up once, before anything is changed.
    let backup = dir.join("settings.json.switchboard-backup");
    if file.exists() && !backup.exists() {
        std::fs::write(&backup, &original)
            .map_err(|e| format!("Could not write a backup: {e}"))?;
    }

    let url = format!("http://127.0.0.1:{HOOK_PORT}");
    let root = settings
        .as_object_mut()
        .ok_or("settings.json is not a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("hooks is not a JSON object")?;

    let mut added = 0;
    for event in HOOK_EVENTS {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{event} is not an array"))?;

        let already = groups.iter().any(|group| {
            group
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|e| e.get("url").and_then(|u| u.as_str()) == Some(url.as_str()))
                })
        });
        if already {
            continue;
        }

        groups.push(serde_json::json!({
            "hooks": [{ "type": "http", "url": url, "timeout": 5 }]
        }));
        added += 1;
    }

    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&file, text + "\n").map_err(|e| format!("Could not write settings.json: {e}"))?;

    Ok(if added == 0 {
        "Already connected.".to_string()
    } else {
        format!("Connected. Start a new Claude Code session to see it here.")
    })
}

/// Open the settings window, or focus it if it is already open.
///
/// It loads the same bundle as the widget and picks its view from the query
/// string, so there is one frontend rather than two.
#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("settings") {
        existing.show().map_err(|e| e.to_string())?;
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(
        &app,
        "settings",
        tauri::WebviewUrl::App("index.html?window=settings".into()),
    )
    .title("Switchboard Settings")
    .inner_size(440.0, 420.0)
    .min_inner_size(360.0, 320.0)
    .resizable(true)
    .decorations(true)
    .closable(true)
    .always_on_top(false)
    // The widget window is transparent; this one must not inherit that.
    .transparent(false)
    .background_color(tauri::window::Color(0, 0, 0, 255))
    .center()
    .build()
    .map_err(|e| format!("Could not open settings: {e}"))?;

    Ok(())
}

/// Close the settings window from inside it.
#[tauri::command]
fn close_settings(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("settings") {
        window
            .close()
            .map_err(|e| format!("Could not close settings: {e}"))?;
    }
    Ok(())
}

/// What Rust thinks the window is, for diagnosing mixed-DPI behaviour.
#[tauri::command]
fn window_metrics(window: tauri::Window) -> Result<String, String> {
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    let position = window.outer_position().map_err(|e| e.to_string())?;
    Ok(format!(
        "scale {scale} · {}x{} phys · at {},{}",
        size.width, size.height, position.x, position.y
    ))
}

/// The widget has no title bar, so it needs its own way to close.
#[tauri::command]
fn close_widget(window: tauri::Window) -> Result<(), String> {
    window
        .close()
        .map_err(|e| format!("Could not close widget: {e}"))
}

/// Keep the Switchboard window above other windows, so it stays visible after
/// focus moves to an editor or browser.
#[tauri::command]
fn set_always_on_top(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window
        .set_always_on_top(enabled)
        .map_err(|e| format!("Could not change window layering: {e}"))
}

/// A Claude Code session, as reported by its hooks. Sessions are keyed by
/// session_id and live only in memory; restarting Switchboard forgets them.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    session_id: String,
    cwd: String,
    state: &'static str,
    detail: Option<String>,
    updated_at: u64,
}

#[derive(Default)]
pub struct SessionStore(Mutex<HashMap<String, Session>>);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Map a hook event onto the state the widget shows. Events we do not care
/// about return None and leave the session untouched.
fn state_for(event: &str) -> Option<&'static str> {
    match event {
        "SessionStart" => Some("idle"),
        "UserPromptSubmit" => Some("working"),
        "Notification" | "PermissionRequest" => Some("needs_you"),
        "Stop" | "StopFailure" => Some("done"),
        _ => None,
    }
}

/// Apply one hook payload to the store. Returns true when something changed.
fn apply_hook(sessions: &mut HashMap<String, Session>, body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let event = value
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let Some(session_id) = value.get("session_id").and_then(|v| v.as_str()) else {
        return false;
    };

    if event == "SessionEnd" {
        return sessions.remove(session_id).is_some();
    }

    let Some(state) = state_for(event) else {
        return false;
    };

    // Not every event carries cwd; keep the one we already have.
    let cwd = match value.get("cwd").and_then(|v| v.as_str()) {
        Some(cwd) if !cwd.is_empty() => cwd.to_string(),
        _ => sessions
            .get(session_id)
            .map(|s| s.cwd.clone())
            .unwrap_or_default(),
    };

    sessions.insert(
        session_id.to_string(),
        Session {
            session_id: session_id.to_string(),
            cwd,
            state,
            detail: value
                .get("message")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            updated_at: now_ms(),
        },
    );
    true
}

fn snapshot(sessions: &HashMap<String, Session>) -> Vec<Session> {
    let mut list: Vec<Session> = sessions.values().cloned().collect();
    list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    list
}

/// Listen for Claude Code hook deliveries. Claude Code fires the same hooks
/// wherever it runs, including sessions inside the VS Code extension, so this
/// sees sessions Switchboard never started.
fn start_hook_listener(app: AppHandle) {
    std::thread::spawn(move || {
        let server = match tiny_http::Server::http(("127.0.0.1", HOOK_PORT)) {
            Ok(server) => server,
            Err(e) => {
                log::error!("Hook listener could not bind port {HOOK_PORT}: {e}");
                return;
            }
        };
        log::info!("Hook listener ready on 127.0.0.1:{HOOK_PORT}");

        for mut request in server.incoming_requests() {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);

            let changed = {
                let store = app.state::<SessionStore>();
                let mut sessions = store.0.lock().unwrap();
                let changed = apply_hook(&mut sessions, &body);
                if changed {
                    let _ = app.emit("sessions-changed", snapshot(&sessions));
                }
                changed
            };
            let _ = changed;

            // Claude Code waits on this response, so answer immediately with a
            // no-op decision.
            let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("valid header");
            let _ = request.respond(tiny_http::Response::from_string("{}").with_header(header));
        }
    });
}

#[tauri::command]
fn list_sessions(store: tauri::State<SessionStore>) -> Vec<Session> {
    snapshot(&store.0.lock().unwrap())
}

/// The URL to point Claude Code hooks at, shown in the setup hint.
#[tauri::command]
fn hook_endpoint() -> String {
    format!("http://127.0.0.1:{HOOK_PORT}")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(SessionStore::default())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            start_hook_listener(app.handle().clone());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_ignore_cursor_events(true);
            }
            start_hover_watch(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_worktrees,
            open_in_vscode,
            set_always_on_top,
            list_sessions,
            hook_endpoint,
            close_widget,
            connect_claude_code,
            window_metrics,
            open_settings,
            close_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branches_detached_heads_and_paths_with_spaces() {
        let sample = "worktree C:/Themis\nHEAD abc123\nbranch refs/heads/fleet/demo-tooling\n\
                      \n\
                      worktree C:/Themis B\nHEAD def456\nbranch refs/heads/docs/devlog\n\
                      \n\
                      worktree C:/agent/checkout\nHEAD 789abc\ndetached\n";

        let worktrees = parse_worktrees(sample);

        assert_eq!(worktrees.len(), 3);
        assert_eq!(worktrees[0].branch.as_deref(), Some("fleet/demo-tooling"));
        assert_eq!(worktrees[1].path, "C:/Themis B");
        assert_eq!(worktrees[1].branch.as_deref(), Some("docs/devlog"));
        assert!(worktrees[2].detached);
        assert_eq!(worktrees[2].branch, None);
    }

    #[test]
    fn handles_bare_locked_and_prunable_flags() {
        let sample = "worktree /repo\nHEAD abc\nbare\n\nworktree /wt\nHEAD def\ndetached\nlocked\nprunable\n";

        let worktrees = parse_worktrees(sample);

        assert!(worktrees[0].bare);
        assert!(worktrees[1].locked && worktrees[1].prunable);
    }

    #[test]
    fn returns_nothing_for_empty_output() {
        assert!(parse_worktrees("").is_empty());
    }
}
