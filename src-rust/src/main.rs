// Prevents an extra console window on Windows in release. DO NOT REMOVE.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
/// The folder name VS Code puts in its title bar.
fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rsplit(['/', '\\']).next() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => trimmed.to_string(),
    }
}

/// Start VS Code on a checkout.
fn launch_vscode(path: &str) -> Result<(), String> {
    // On Windows `code` is a .cmd shim, which CreateProcess cannot launch
    // directly, so go through cmd.exe and suppress its console window.
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .args(["/C", "code"])
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    };
    #[cfg(not(windows))]
    let spawned = Command::new("code").arg(path).spawn();

    spawned
        .map(|_| ())
        .map_err(|e| format!("Could not launch VS Code: {e}"))
}

/// Where a window sat before Switchboard filled a monitor with it.
///
/// Recorded from GetWindowRect, not GetWindowPlacement: the placement struct's
/// rcNormalPosition drifts out of step with the real rect on mixed-DPI setups
/// and cannot be trusted to put a window back.
#[derive(Clone, Copy, Debug)]
pub struct SavedPlacement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

/// Worktrees Switchboard has filled a monitor with, and where their editor
/// was beforehand. A worktree in here can be put back.
#[derive(Default)]
pub struct PlacementStore(Mutex<HashMap<String, SavedPlacement>>);

/// Match the frontend's path normalisation, so the two agree on identity.
fn placement_key(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .replace('\\', "/")
        .to_lowercase()
}

/// Worktrees currently filling a monitor, newest spelling of each path.
#[tauri::command]
fn list_parked(store: tauri::State<PlacementStore>) -> Vec<String> {
    store.0.lock().unwrap().keys().cloned().collect()
}

fn emit_parked(app: &AppHandle) {
    let parked: Vec<String> = app
        .state::<PlacementStore>()
        .0
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let _ = app.emit("parked-changed", parked);
}

/// Finding and placing another application's window.
///
/// Tauri can only move its own windows, so this drops to Win32. Every VS Code
/// window belongs to the same process, so the process id cannot tell them
/// apart; the title can. VS Code titles a window `<edited file> - <folder> -
/// Visual Studio Code`, or just `<folder> - Visual Studio Code` with nothing
/// open, so the folder name plus that suffix identifies it.
#[cfg(windows)]
mod editor_window {
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    type Hwnd = *mut c_void;
    type Lparam = isize;
    type Bool32 = i32;

    const SW_RESTORE: i32 = 9;
    const SW_MAXIMIZE: i32 = 3;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;

    /// 150ms is comfortably past the 17ms the placement itself takes, and
    /// short enough that a user cannot start their own maximise inside it.
    const SETTLE_MS: u64 = 150;
    /// DWMWA_TRANSITIONS_FORCEDISABLED
    const NO_TRANSITIONS: u32 = 3;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            window: Hwnd,
            attribute: u32,
            value: *const i32,
            size: u32,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(Hwnd, Lparam) -> Bool32>,
            param: Lparam,
        ) -> Bool32;
        fn GetWindowTextW(window: Hwnd, buffer: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(window: Hwnd) -> Bool32;
        fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> Bool32;
        fn IsZoomed(window: Hwnd) -> Bool32;
        fn IsIconic(window: Hwnd) -> Bool32;
        fn ShowWindow(window: Hwnd, command: i32) -> Bool32;
        fn SetWindowPos(
            window: Hwnd,
            after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> Bool32;
        fn SetForegroundWindow(window: Hwnd) -> Bool32;
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    struct Search {
        exact: String,
        suffix: String,
        found: Option<Hwnd>,
    }

    unsafe extern "system" fn visit(window: Hwnd, param: Lparam) -> Bool32 {
        let search = &mut *(param as *mut Search);
        if IsWindowVisible(window) == 0 {
            return 1;
        }
        let mut buffer = [0u16; 512];
        let len = GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32);
        if len <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&buffer[..len as usize]);
        if title == search.exact || title.ends_with(&search.suffix) {
            search.found = Some(window);
            return 0; // stop enumerating
        }
        1
    }

    /// The VS Code window showing `folder`, if one is open.
    pub fn find(folder: &str) -> Option<Hwnd> {
        let mut search = Search {
            exact: format!("{folder} - Visual Studio Code"),
            suffix: format!(" - {folder} - Visual Studio Code"),
            found: None,
        };
        unsafe {
            EnumWindows(Some(visit), &mut search as *mut Search as Lparam);
        }
        search.found
    }

    /// Wait for VS Code to put its window up after a cold launch.
    pub fn wait_for(folder: &str, limit: Duration) -> Option<Hwnd> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Some(window) = find(folder) {
                return Some(window);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        None
    }

    /// Turn the window's own restore/maximise animations off, or back on.
    fn set_transitions(window: Hwnd, enabled: bool) {
        let value: i32 = if enabled { 0 } else { 1 };
        unsafe {
            DwmSetWindowAttribute(window, NO_TRANSITIONS, &value, 4);
        }
    }

    /// Switches a window's animations off, and back on however we leave.
    ///
    /// The attribute sticks to the target window for the rest of its life,
    /// and the window belongs to someone else's editor. Returning early or
    /// panicking between the two calls would cost the user their animations
    /// until they close VS Code, so the restore goes in Drop rather than at
    /// the end of the happy path.
    struct Transitions(Hwnd);

    impl Transitions {
        fn suppressed(window: Hwnd) -> Self {
            set_transitions(window, false);
            Transitions(window)
        }
    }

    impl Drop for Transitions {
        fn drop(&mut self) {
            set_transitions(self.0, true);
        }
    }

    /// Fill the monitor whose top-left is (`x`, `y`).
    ///
    /// Three steps are unavoidable: a maximised window will not move until it
    /// is restored, and Windows maximises onto whichever monitor then holds
    /// it. Letting each step animate is what made this stutter - the window
    /// visibly shrank, flew across, and grew again. Suppressing the window's
    /// transitions collapses all three into one frame.
    ///
    /// Maximising is also what keeps this correct across monitors with
    /// different scaling: Windows recomputes the size from the target monitor.
    /// Setting the rect directly instead looks smoother but comes out 1.5x too
    /// large on a 150% display.
    pub fn fill_monitor(window: Hwnd, x: i32, y: i32) {
        let _transitions = Transitions::suppressed(window);

        unsafe {
            // Minimised or maximised, it has to be a normal window before it
            // will move.
            if IsIconic(window) != 0 || IsZoomed(window) != 0 {
                ShowWindow(window, SW_RESTORE);
            }
            SetWindowPos(
                window,
                std::ptr::null_mut(),
                x + 60,
                y + 60,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            ShowWindow(window, SW_MAXIMIZE);
            SetForegroundWindow(window);
        }

        // Hold the suppression until the move has settled; Drop puts the
        // window's animations back.
        std::thread::sleep(Duration::from_millis(SETTLE_MS));
    }

    use super::SavedPlacement;

    /// The window's real screen rect and maximised state.
    ///
    /// A minimised window has no meaningful rect - Windows reports
    /// (-32000, -32000) - and saving that would later "restore" the editor to
    /// somewhere no monitor can show it. Better to record nothing.
    pub fn placement(window: Hwnd) -> Option<SavedPlacement> {
        let mut rect = Rect::default();
        unsafe {
            if IsIconic(window) != 0 || GetWindowRect(window, &mut rect) == 0 {
                return None;
            }
            Some(SavedPlacement {
                x: rect.left,
                y: rect.top,
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
                maximized: IsZoomed(window) != 0,
            })
        }
    }

    /// Does this window's centre sit inside the given monitor?
    ///
    /// Focus cannot answer "is it already filling this screen": clicking the
    /// widget takes focus off the editor, so by the time the second click
    /// arrives the editor is never in front. Geometry does not move.
    pub fn fills(saved: SavedPlacement, x: i32, y: i32, width: i32, height: i32) -> bool {
        let centre_x = saved.x + saved.width / 2;
        let centre_y = saved.y + saved.height / 2;
        saved.maximized
            && centre_x >= x
            && centre_x < x + width
            && centre_y >= y
            && centre_y < y + height
    }

    /// Put a window back exactly where it was.
    ///
    /// The size is restated twice on purpose. Coming off a differently scaled
    /// monitor, the first call crosses the DPI boundary and Windows rescales
    /// the window on the way - a 1456x908 window returning from a 150%
    /// display lands at 971x605. By the second call the window is already on
    /// the destination monitor, so the size it is given is the size it keeps.
    /// One call is not enough; measured on a 100%/150% pair.
    pub fn restore(window: Hwnd, saved: SavedPlacement) {
        let _transitions = Transitions::suppressed(window);

        unsafe {
            if IsIconic(window) != 0 || IsZoomed(window) != 0 {
                ShowWindow(window, SW_RESTORE);
            }
            for _ in 0..2 {
                SetWindowPos(
                    window,
                    std::ptr::null_mut(),
                    saved.x,
                    saved.y,
                    saved.width,
                    saved.height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            if saved.maximized {
                ShowWindow(window, SW_MAXIMIZE);
            }
            SetForegroundWindow(window);
        }

        std::thread::sleep(Duration::from_millis(SETTLE_MS));
    }
}

/// Open a worktree in VS Code, filling the monitor the widget is sitting on -
/// or put it back if it is already filled and in front of you.
///
/// Clicking a worktree you are not currently in always brings it to you, so
/// the toggle can never strand a window somewhere you cannot see. Only a
/// second click, on the editor you are already working in, sends it home.
///
/// Async on purpose: a cold VS Code launch is waited on for up to 20 seconds,
/// and a synchronous command would block the main thread for all of it.
#[tauri::command]
async fn open_in_vscode(
    app: AppHandle,
    window: tauri::Window,
    path: String,
) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err(format!("Path no longer exists: {path}"));
    }

    // Placement follows the widget: whichever monitor it is on is the one the
    // editor fills. Physical pixels, which is what Win32 wants.
    let monitor = window.current_monitor().ok().flatten().map(|m| {
        (
            m.position().x,
            m.position().y,
            m.size().width as i32,
            m.size().height as i32,
        )
    });

    #[cfg(windows)]
    {
        let folder = basename(&path);
        let found = match editor_window::find(&folder) {
            Some(found) => found,
            None => {
                launch_vscode(&path)?;
                editor_window::wait_for(&folder, Duration::from_secs(20)).ok_or_else(|| {
                    format!("Opened {folder}, but no VS Code window appeared to place")
                })?
            }
        };

        let key = placement_key(&path);
        let saved = app
            .state::<PlacementStore>()
            .0
            .lock()
            .unwrap()
            .get(&key)
            .copied();

        // A second click, on a worktree already filling this monitor, sends
        // its editor back where it came from.
        if let (Some(saved), Some((mx, my, mw, mh)), Some(now)) =
            (saved, monitor, editor_window::placement(found))
        {
            if editor_window::fills(now, mx, my, mw, mh) {
                editor_window::restore(found, saved);
                app.state::<PlacementStore>().0.lock().unwrap().remove(&key);
                emit_parked(&app);
                return Ok(());
            }
        }

        // Remember the first position only. Filling twice in a row must not
        // overwrite the real one with a full-monitor rect.
        if saved.is_none() {
            if let Some(before) = editor_window::placement(found) {
                app.state::<PlacementStore>()
                    .0
                    .lock()
                    .unwrap()
                    .insert(key, before);
                emit_parked(&app);
            }
        }

        match monitor {
            Some((x, y, _, _)) => editor_window::fill_monitor(found, x, y),
            // Opened, but we could not place it. Better than failing the click.
            None => log::warn!("No monitor for the widget; left VS Code where it was"),
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = (monitor, app);
        launch_vscode(&path)
    }
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

    // Only a missing file means "nothing configured yet". Any other read
    // error - locked by an editor, permissions, antivirus - must abort. The
    // old code treated every failure as an empty config, so the backup below
    // saved "{}" and the write further down replaced a real settings.json
    // (permissions, MCP servers, other hooks) with only Switchboard's hooks.
    let original = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".to_string(),
        Err(e) => return Err(format!("Could not read {}: {e}", file.display())),
    };
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
    // Write beside the target and rename over it, so an interrupted write
    // cannot leave a half-written settings.json behind.
    let temp = dir.join("settings.json.switchboard-tmp");
    std::fs::write(&temp, text + "\n")
        .map_err(|e| format!("Could not write settings.json: {e}"))?;
    std::fs::rename(&temp, &file)
        .map_err(|e| format!("Could not replace settings.json: {e}"))?;

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
///
/// This MUST stay async. A synchronous command runs on the main thread, and
/// building a webview there deadlocks: creation needs the event loop that the
/// command is blocking. The window appears but never leaves about:blank, the
/// call never returns, and every later command queues behind it -- so dragging
/// the widget and closing either window stop working too.
#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
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

/// How often to re-scan `~/.claude/ide` for editors with Claude Code attached.
const IDE_SCAN_MS: u64 = 4000;

/// An editor that already has Claude Code attached.
///
/// Hooks only report what happens after Switchboard starts, so sessions that
/// were already running are invisible to them. Claude Code also writes one
/// lock file per IDE connection into `~/.claude/ide`, named `<port>.lock`, and
/// those describe the connections that exist right now. Reading them is how
/// Switchboard sees work that predates it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    path: String,
    ide_name: String,
    pid: u32,
    port: String,
}

#[derive(Default)]
pub struct WorkspaceStore(Mutex<Vec<Workspace>>);

/// Read one lock file's body. Returns an entry per workspace folder, since a
/// single connection can carry several.
///
/// The lock also holds an auth token. Only the fields below are read, and the
/// token is never stored, logged, or sent to the frontend.
fn parse_ide_lock(body: &str, port: &str) -> Vec<Workspace> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(pid) = value.get("pid").and_then(|v| v.as_u64()) else {
        return Vec::new();
    };
    let ide_name = value
        .get("ideName")
        .and_then(|v| v.as_str())
        .unwrap_or("Editor")
        .to_string();

    value
        .get("workspaceFolders")
        .and_then(|v| v.as_array())
        .map(|folders| {
            folders
                .iter()
                .filter_map(|f| f.as_str())
                .filter(|f| !f.is_empty())
                .map(|folder| Workspace {
                    path: folder.to_string(),
                    ide_name: ide_name.clone(),
                    pid: pid as u32,
                    port: port.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every running process id, in one call rather than one probe per lock file.
///
/// An empty set means the check itself failed. Callers treat that as "cannot
/// tell" and keep the locks, rather than reporting every editor as closed.
fn live_pids() -> HashSet<u32> {
    #[cfg(windows)]
    let output = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("tasklist")
            .args(["/NH", "/FO", "CSV"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    };
    #[cfg(not(windows))]
    let output = Command::new("ps").args(["-A", "-o", "pid="]).output();

    let Ok(output) = output else {
        return HashSet::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);

    #[cfg(windows)]
    // CSV rows look like "code.exe","27972","Console",... - the pid is field 2.
    let pids = text.lines().filter_map(|line| {
        line.split("\",\"")
            .nth(1)
            .and_then(|field| field.trim().parse::<u32>().ok())
    });
    #[cfg(not(windows))]
    let pids = text
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok());

    pids.collect()
}

/// Scan `~/.claude/ide`, dropping locks whose editor has since exited.
fn scan_ide_workspaces(home: &Path) -> Vec<Workspace> {
    let Ok(entries) = std::fs::read_dir(home.join(".claude").join("ide")) else {
        return Vec::new();
    };
    let live = live_pids();
    let trust_liveness = !live.is_empty();

    let mut found: Vec<Workspace> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let port = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        for workspace in parse_ide_lock(&body, &port) {
            // Closing an editor leaves its lock file behind, so liveness is
            // what separates a real connection from a leftover.
            if trust_liveness && !live.contains(&workspace.pid) {
                continue;
            }
            found.push(workspace);
        }
    }

    found.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.port.cmp(&b.port)));
    found.dedup();
    found
}

/// Watch `~/.claude/ide` and publish the editors currently attached.
fn start_ide_watch(app: AppHandle) {
    std::thread::spawn(move || {
        let Ok(home) = app.path().home_dir() else {
            log::warn!("No home directory; cannot discover editor sessions");
            return;
        };
        let mut last: Vec<Workspace> = Vec::new();
        loop {
            let found = scan_ide_workspaces(&home);
            if found != last {
                log::info!("Editors attached: {}", found.len());
                last.clone_from(&found);
                *app.state::<WorkspaceStore>().0.lock().unwrap() = found.clone();
                let _ = app.emit("workspaces-changed", found);
            }
            std::thread::sleep(Duration::from_millis(IDE_SCAN_MS));
        }
    });
}

/// Editors with Claude Code attached right now, discovered without hooks.
#[tauri::command]
fn list_workspaces(store: tauri::State<WorkspaceStore>) -> Vec<Workspace> {
    store.0.lock().unwrap().clone()
}

fn main() {
    tauri::Builder::default()
        .manage(SessionStore::default())
        .manage(WorkspaceStore::default())
        .manage(PlacementStore::default())
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
            start_ide_watch(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_worktrees,
            open_in_vscode,
            set_always_on_top,
            list_sessions,
            list_workspaces,
            list_parked,
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

    #[test]
    fn reads_one_entry_per_workspace_folder_and_never_exposes_the_token() {
        let body = r#"{
            "pid": 27972,
            "workspaceFolders": ["c:\\Switchboard", "c:\\Themis B"],
            "ideName": "Visual Studio Code",
            "authToken": "super-secret"
        }"#;

        let found = parse_ide_lock(body, "10603");

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].path, "c:\\Switchboard");
        assert_eq!(found[1].path, "c:\\Themis B");
        assert!(found.iter().all(|w| w.pid == 27972));
        assert!(found.iter().all(|w| w.port == "10603"));
        assert!(found.iter().all(|w| w.ide_name == "Visual Studio Code"));

        // The auth token must not survive into anything the frontend sees.
        let json = serde_json::to_string(&found).unwrap();
        assert!(!json.contains("super-secret"));
    }

    #[test]
    fn skips_locks_that_are_incomplete_or_unparsable() {
        assert!(parse_ide_lock(r#"{"workspaceFolders": ["/x"]}"#, "1").is_empty());
        assert!(parse_ide_lock(r#"{"pid": 5}"#, "1").is_empty());
        assert!(parse_ide_lock("not json", "1").is_empty());
        assert!(parse_ide_lock(r#"{"pid": 5, "workspaceFolders": []}"#, "1").is_empty());
    }

    #[test]
    fn takes_the_folder_name_vs_code_shows_in_its_title() {
        assert_eq!(basename("C:\\Themis B"), "Themis B");
        assert_eq!(basename("C:/Switchboard"), "Switchboard");
        assert_eq!(basename("C:\\Themis C\\"), "Themis C");
        assert_eq!(basename("C:/a/b/c/"), "c");
        // A drive root has no folder name to fall back on.
        assert_eq!(basename("C:\\"), "C:");
    }
}
