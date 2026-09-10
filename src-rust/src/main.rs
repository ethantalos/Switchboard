// Prevents an extra console window on Windows in release. DO NOT REMOVE.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Held open regardless of where the pointer is.
///
/// Hover alone means the panel vanishes the moment you look away, which is
/// wrong as soon as you want to read it while doing something else, or drag a
/// worktree without the list closing under the cursor.
#[derive(Default)]
pub struct Pinned(AtomicBool);

/// Hold the panel open, or let hover decide again.
#[tauri::command]
fn set_pinned(app: AppHandle, pinned: bool) -> Result<(), String> {
    app.state::<Pinned>().0.store(pinned, Ordering::Relaxed);

    // Take the cursor back immediately rather than waiting for the next poll,
    // so the click that pinned it does not land on whatever is behind.
    if let Some(window) = app.get_webview_window("main") {
        if pinned {
            let _ = window.set_ignore_cursor_events(false);
        }
    }
    let _ = app.emit("pinned-changed", pinned);
    Ok(())
}

#[tauri::command]
fn is_pinned(state: tauri::State<Pinned>) -> bool {
    state.0.load(Ordering::Relaxed)
}

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
            // the whole window, so the edges are not a knife edge. Pinned
            // overrides both.
            let pinned = app.state::<Pinned>().0.load(Ordering::Relaxed);
            let want = if pinned {
                true
            } else if expanded {
                over_window
            } else {
                over_badge
            };
            if want != expanded {
                expanded = want;
                let _ = window.set_ignore_cursor_events(!expanded);
                let _ = app.emit("hover-changed", expanded);
            }
        }
    });
}

/// Hook events Switchboard needs in order to track session state.
const HOOK_EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    // A long turn is silent between the prompt and the stop, so tool events
    // are what keep "working" honest instead of letting it look hung.
    "PostToolUse",
    "SubagentStop",
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

/// One hook delivery, kept so the settings window can show what actually
/// arrived. "The hooks don't work" is impossible to diagnose from a widget
/// that only ever shows its conclusions.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookRecord {
    at: u64,
    event: String,
    session_id: String,
    cwd: String,
    /// What the widget did with it: applied, or why it was ignored.
    outcome: String,
    /// The fields present on the payload, so a schema change is visible
    /// rather than silently dropping information.
    fields: Vec<String>,
}

/// Most recent deliveries, newest last.
#[derive(Default)]
pub struct HookLog(Mutex<VecDeque<HookRecord>>);

/// Enough to cover a couple of turns without growing without bound.
const HOOK_LOG_LIMIT: usize = 200;

fn record_hook(app: &AppHandle, body: &str, outcome: &str) {
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let get = |key: &str| {
        value
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let mut fields: Vec<String> = value
        .as_ref()
        .and_then(|v| v.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    fields.sort();

    let record = HookRecord {
        at: now_ms(),
        event: get("hook_event_name"),
        session_id: get("session_id"),
        cwd: get("cwd"),
        outcome: outcome.to_string(),
        fields,
    };

    let log = app.state::<HookLog>();
    let mut entries = log.0.lock().unwrap();
    if entries.len() >= HOOK_LOG_LIMIT {
        entries.pop_front();
    }
    entries.push_back(record);
}

/// Recent hook deliveries, newest last. Drives the settings window's
/// diagnostics, and answers "is Claude Code actually talking to me".
#[tauri::command]
fn recent_hooks(log: tauri::State<HookLog>) -> Vec<HookRecord> {
    log.0.lock().unwrap().iter().cloned().collect()
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

/// Sessions that ended properly, and when.
///
/// A transcript keeps its last-written time after the session exits, so the
/// disk scan would otherwise resurrect anything that ended recently and show
/// it as working forever. SessionEnd is the one unambiguous signal that a
/// session is gone; it has to outlive the session record itself, or the scan
/// simply puts it back.
#[derive(Default)]
pub struct Ended(Mutex<HashMap<String, u64>>);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How much of a message is kept. The row shows one clipped line; clicking
/// it opens the rest, so this is the length worth expanding to rather than
/// the length that fits.
const DETAIL_CHARS: usize = 400;

/// First line, whitespace collapsed, clipped. Assistant messages are markdown
/// paragraphs; a row has one line.
fn one_line(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let clipped: String = flat.chars().take(limit).collect();
    match clipped.rsplit_once(' ') {
        Some((head, _)) if head.chars().count() > limit / 2 => format!("{head}..."),
        _ => format!("{clipped}..."),
    }
}

/// Is `inner` the same directory as `outer`, or somewhere beneath it?
///
/// Windows paths arrive with either separator and either case, so both are
/// normalised before comparing.
fn is_inside(inner: &str, outer: &str) -> bool {
    let norm = |p: &str| {
        p.replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    let (inner, outer) = (norm(inner), norm(outer));
    inner == outer || inner.starts_with(&format!("{outer}/"))
}

/// A one-line summary of what just happened, from whichever field the event
/// actually carries.
///
/// There is no single `message` field. Verified against live payloads from
/// Claude Code 2.1.267: `Stop` carries `last_assistant_message`,
/// `UserPromptSubmit` carries `prompt`, and `Notification` carries
/// `notification_type` rather than prose.
fn detail_for(event: &str, value: &serde_json::Value) -> Option<String> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
    };

    let picked = match event {
        // What it last said - the reason you would go back to this session.
        "Stop" | "StopFailure" => text("last_assistant_message"),
        // What it is working on.
        "UserPromptSubmit" => text("prompt"),
        "Notification" => {
            // The row already says "needs you"; repeating the machine name
            // for it is noise. Only say something the state does not.
            return text("message")
                .map(|m| one_line(m, DETAIL_CHARS))
                .or_else(|| notification_reason(value));
        }
        "PermissionRequest" | "PreToolUse" | "PostToolUse" => text("tool_name"),
        "SubagentStop" => text("agent_type"),
        _ => None,
    }?;

    Some(one_line(picked, DETAIL_CHARS))
}

/// Plain English for a notification kind, or nothing when the state label
/// already says it.
fn notification_reason(value: &serde_json::Value) -> Option<String> {
    let kind = value.get("notification_type").and_then(|v| v.as_str())?;
    Some(match kind {
        "permission_prompt" => "waiting on permission".to_string(),
        "agent_needs_input" => "needs input".to_string(),
        "elicitation_dialog" | "elicitation_url_dialog" => "asking you something".to_string(),
        // "your turn" already covers this one.
        "idle_prompt" => return None,
        // Something new. Show it rather than hide it, just tidied up.
        other => other.replace('_', " "),
    })
}

/// Notifications that mean "I am blocked on you" rather than "I have been
/// sitting here a while".
///
/// Claude Code fires an idle notification about a minute after it finishes a
/// turn. Treating that as a blocking prompt turned the badge red for sessions
/// that simply had nothing to do, which is the fastest way to train someone
/// to ignore it.
fn notification_blocks(value: &serde_json::Value) -> bool {
    match value.get("notification_type").and_then(|v| v.as_str()) {
        Some("idle_prompt") => false,
        // Unknown kinds are treated as blocking: a missed prompt is worse
        // than an extra one.
        _ => true,
    }
}

/// Map a hook event onto the state the widget shows. Events we do not care
/// about return None and leave the session untouched.
///
/// `Stop` is the one worth thinking about. It fires when the assistant
/// finishes replying, which means the session is now sitting there waiting
/// for its human - not that the work is done. Calling that "done" and
/// painting it green said "nothing to see here" about the sessions most
/// likely to want you, which is the opposite of what this widget is for.
fn state_for(event: &str, value: &serde_json::Value) -> Option<&'static str> {
    match event {
        "SessionStart" => Some("idle"),
        // Anything that proves the turn is still moving.
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "SubagentStop" => Some("working"),
        "PermissionRequest" => Some("needs_you"),
        "Notification" => Some(if notification_blocks(value) {
            "needs_you"
        } else {
            "waiting"
        }),
        "Stop" => Some("waiting"),
        "StopFailure" => Some("failed"),
        _ => None,
    }
}

/// A session with no hook traffic for this long is treated as quiet: shown,
/// but dimmed and left out of the badge count.
const QUIET_AFTER_MS: u64 = 30 * 60 * 1000;

/// Sessions die without warning - a closed VS Code window or a killed
/// terminal fires no SessionEnd - so anything this old is dropped outright.
const FORGET_AFTER_MS: u64 = 12 * 60 * 60 * 1000;

/// How often to re-check for sessions that have gone silent.
const PRUNE_EVERY_MS: u64 = 60 * 1000;

/// Drop sessions that have been silent long enough to be certainly gone.
/// Returns true when something was removed.
fn prune_dead(sessions: &mut HashMap<String, Session>, now: u64) -> bool {
    let before = sessions.len();
    sessions.retain(|_, s| now.saturating_sub(s.updated_at) < FORGET_AFTER_MS);
    sessions.len() != before
}

/// How far back a transcript is worth reading at all.
const TRANSCRIPT_WINDOW_MS: u64 = 4 * 60 * 60 * 1000;
/// Written this recently means the turn is still moving.
const TRANSCRIPT_ACTIVE_MS: u64 = 90 * 1000;
/// A Stop writes its own last lines, so writes within this long after the
/// last hook are that hook's own tail rather than new work.
const TRANSCRIPT_GRACE_MS: u64 = 15 * 1000;
/// How often to re-read the transcript directory.
const TRANSCRIPT_SCAN_MS: u64 = 5000;
/// The cwd is on the first user line; no need to read further.
const TRANSCRIPT_HEAD_LINES: usize = 40;

/// A session found on disk rather than announced by a hook.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    session_id: String,
    cwd: String,
    last_write: u64,
}

/// The working directory a session started in.
///
/// Only the first few lines are read: Claude Code puts `cwd` on the first
/// user entry, and transcripts run to hundreds of thousands of lines.
fn transcript_cwd(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(TRANSCRIPT_HEAD_LINES) {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// Sessions with recent transcript activity.
///
/// Only files sitting directly in a project directory are sessions; the
/// `<session>/subagents/` tree underneath holds the agents a session spawned,
/// which are not sessions of their own.
fn scan_transcripts(
    home: &Path,
    now: u64,
    cwds: &mut HashMap<std::path::PathBuf, String>,
) -> Vec<Transcript> {
    let root = home.join(".claude").join("projects");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(session_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            let last_write = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            if now.saturating_sub(last_write) > TRANSCRIPT_WINDOW_MS {
                continue;
            }
            // A session's starting directory never changes, so read it once
            // and remember it. Otherwise every live transcript is reopened
            // every five seconds for a value that cannot have moved.
            let cwd = match cwds.get(&path) {
                Some(known) => known.clone(),
                None => {
                    let Some(found) = transcript_cwd(&path) else {
                        continue;
                    };
                    cwds.insert(path.clone(), found.clone());
                    found
                }
            };

            found.push(Transcript {
                session_id: session_id.to_string(),
                cwd,
                last_write,
            });
        }
    }

    keep_current(found, now)
}

/// Reduce a directory full of history to the sessions worth showing.
///
/// Every transcript a worktree has ever had lives in the same folder, so a
/// raw scan resurrects months of dead sessions. Keep anything still being
/// written - concurrent sessions in one worktree are real - plus the newest
/// per worktree, which is the one you would actually return to.
fn keep_current(found: Vec<Transcript>, now: u64) -> Vec<Transcript> {
    let mut newest: HashMap<String, u64> = HashMap::new();
    for t in &found {
        let key = t.cwd.replace('\\', "/").to_lowercase();
        let slot = newest.entry(key).or_insert(0);
        *slot = (*slot).max(t.last_write);
    }

    found
        .into_iter()
        .filter(|t| {
            let active = now.saturating_sub(t.last_write) < TRANSCRIPT_ACTIVE_MS;
            let key = t.cwd.replace('\\', "/").to_lowercase();
            active || newest.get(&key) == Some(&t.last_write)
        })
        .collect()
}

/// Fold what is on disk into what the hooks have said.
///
/// Hooks describe transitions precisely but only while Switchboard is
/// running, and a long turn fires nothing at all between the prompt and the
/// stop. Transcripts are the opposite: no state, but they survive a restart
/// and they are written throughout a turn. Together they cover each other.
///
/// Returns true when anything changed.
fn merge_transcripts(
    sessions: &mut HashMap<String, Session>,
    ended: &HashMap<String, u64>,
    found: &[Transcript],
    now: u64,
) -> bool {
    let mut changed = false;

    for t in found {
        // It said goodbye. Its transcript still looks freshly written, but
        // bringing it back would leave a session that no longer exists
        // sitting there claiming to be working.
        if ended.contains_key(&t.session_id) {
            continue;
        }
        let active = now.saturating_sub(t.last_write) < TRANSCRIPT_ACTIVE_MS;

        match sessions.get_mut(&t.session_id) {
            // Already known from hooks. Hooks own the state; disk only keeps
            // the clock honest so a long turn is not mistaken for silence.
            Some(session) => {
                if t.last_write > session.updated_at {
                    // Writing well after the last hook means a turn is under
                    // way that we never saw start.
                    let overdue = t.last_write - session.updated_at > TRANSCRIPT_GRACE_MS;
                    if overdue && active && session.state != "needs_you" {
                        session.state = "working";
                    }
                    session.updated_at = t.last_write;
                    changed = true;
                }
            }
            // Never heard of it. This is the session that started before
            // Switchboard did, or survived its restart.
            None => {
                sessions.insert(
                    t.session_id.clone(),
                    Session {
                        session_id: t.session_id.clone(),
                        cwd: t.cwd.clone(),
                        // Disk says it exists and when it last moved. It
                        // cannot say what it is doing, so do not pretend.
                        state: if active { "working" } else { "unknown" },
                        detail: None,
                        updated_at: t.last_write,
                    },
                );
                changed = true;
            }
        }
    }

    changed
}

/// Watch the transcript directory so sessions Switchboard never saw start
/// still show up, and long turns keep looking alive.
fn start_transcript_watch(app: AppHandle) {
    std::thread::spawn(move || {
        let Ok(home) = app.path().home_dir() else {
            log::warn!("No home directory; cannot read session transcripts");
            return;
        };
        // Path -> starting directory, so each transcript head is read once.
        let mut cwds: HashMap<std::path::PathBuf, String> = HashMap::new();
        loop {
            let now = now_ms();
            let found = scan_transcripts(&home, now, &mut cwds);
            let store = app.state::<SessionStore>();
            let mut sessions = store.0.lock().unwrap();
            let ended = app.state::<Ended>();
            let mut ended = ended.0.lock().unwrap();
            // Forget the tombstones once no scan could reach them anyway.
            ended.retain(|_, at| now.saturating_sub(*at) < TRANSCRIPT_WINDOW_MS);
            if merge_transcripts(&mut sessions, &ended, &found, now) {
                let _ = app.emit("sessions-changed", snapshot(&sessions));
            }
            drop(sessions);
            std::thread::sleep(Duration::from_millis(TRANSCRIPT_SCAN_MS));
        }
    });
}

/// Forget sessions nothing has been heard from in half a day, and keep the
/// frontend's idea of "quiet" honest by re-emitting on a slow tick.
fn start_session_reaper(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(PRUNE_EVERY_MS));
        let store = app.state::<SessionStore>();
        let mut sessions = store.0.lock().unwrap();
        if prune_dead(&mut sessions, now_ms()) {
            log::info!("Forgot sessions with no traffic for 12h");
            let _ = app.emit("sessions-changed", snapshot(&sessions));
        }
    });
}

/// Apply one hook payload to the store. Returns true when something changed.
fn apply_hook(
    sessions: &mut HashMap<String, Session>,
    ended: &mut HashMap<String, u64>,
    body: &str,
) -> bool {
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
        ended.insert(session_id.to_string(), now_ms());
        return sessions.remove(session_id).is_some();
    }

    let Some(state) = state_for(event, &value) else {
        return false;
    };

    // Not every event carries cwd, and the ones that do report the process's
    // *current* directory. A session that cd's into a subfolder would
    // otherwise appear as a second worktree, so a deeper path never replaces
    // one already known - only a shallower one does.
    let known = sessions.get(session_id).map(|s| s.cwd.clone());
    let cwd = match (value.get("cwd").and_then(|v| v.as_str()), known) {
        (Some(fresh), Some(old)) if !fresh.is_empty() => {
            if is_inside(fresh, &old) {
                old
            } else {
                fresh.to_string()
            }
        }
        (Some(fresh), None) if !fresh.is_empty() => fresh.to_string(),
        (_, Some(old)) => old,
        (_, None) => String::new(),
    };

    sessions.insert(
        session_id.to_string(),
        Session {
            session_id: session_id.to_string(),
            cwd,
            state,
            detail: detail_for(event, &value),
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
        // Restarting leaves the old process holding the port for a moment,
        // and a second Switchboard would hold it for good. Retry briefly,
        // then give up loudly - a listener that quietly never bound makes
        // every session invisible with no sign anything is wrong.
        let mut server = None;
        for attempt in 0..LISTEN_RETRIES {
            match tiny_http::Server::http(("127.0.0.1", HOOK_PORT)) {
                Ok(bound) => {
                    server = Some(bound);
                    break;
                }
                Err(e) => {
                    log::warn!(
                        "Hook port {HOOK_PORT} busy (attempt {}/{LISTEN_RETRIES}): {e}",
                        attempt + 1
                    );
                    std::thread::sleep(Duration::from_millis(LISTEN_RETRY_MS));
                }
            }
        }

        let Some(server) = server else {
            log::error!(
                "Hook listener could not bind {HOOK_PORT}. Another Switchboard is probably running; this one will never see a session."
            );
            app.state::<ListenerUp>().0.store(false, Ordering::Relaxed);
            let _ = app.emit("hooks-changed", ());
            return;
        };

        app.state::<ListenerUp>().0.store(true, Ordering::Relaxed);
        let _ = app.emit("hooks-changed", ());
        log::info!("Hook listener ready on 127.0.0.1:{HOOK_PORT}");

        for mut request in server.incoming_requests() {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);

            let changed = {
                let store = app.state::<SessionStore>();
                let mut sessions = store.0.lock().unwrap();
                // Bind the state before locking; the guard cannot outlive a
                // temporary.
                let tombstones = app.state::<Ended>();
                let mut ended = tombstones.0.lock().unwrap();
                let changed = apply_hook(&mut sessions, &mut ended, &body);
                if changed {
                    let _ = app.emit("sessions-changed", snapshot(&sessions));
                }
                changed
            };

            // Log the ignored ones too - an event the widget silently drops
            // is exactly the thing that makes hooks look broken.
            record_hook(&app, &body, if changed { "applied" } else { "ignored" });
            let _ = app.emit("hooks-changed", ());

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

/// How long a session may be silent before the widget dims it. The frontend
/// decides that per render off its own clock, so it needs the same number.
#[tauri::command]
fn quiet_after_ms() -> u64 {
    QUIET_AFTER_MS
}

/// The order the user dragged their worktrees into.
///
/// Everything else in the widget is derived from what Claude Code is doing,
/// so it is the one piece of state worth keeping on disk: it is a preference,
/// and losing it on every restart would make dragging pointless.
#[derive(Default)]
pub struct OrderStore(Mutex<Vec<String>>);

/// Where the order lives, next to whatever else the app keeps per user.
fn order_file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Could not find a config directory: {e}"))?;
    Ok(dir.join("worktree-order.json"))
}

/// Read the saved order, or an empty one. A missing or corrupt file is not
/// worth failing over - the widget just falls back to sorting by urgency.
fn load_order(app: &AppHandle) -> Vec<String> {
    let Ok(path) = order_file(app) else {
        return Vec::new();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
}

/// Worktree keys in the order the user arranged them, most-preferred first.
#[tauri::command]
fn list_order(store: tauri::State<OrderStore>) -> Vec<String> {
    store.0.lock().unwrap().clone()
}

/// Save a new order. Keys are the frontend's normalised paths, so the two
/// sides agree on identity regardless of drive-letter case or separator.
#[tauri::command]
fn set_order(app: AppHandle, order: Vec<String>) -> Result<(), String> {
    let path = order_file(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }

    let text = serde_json::to_string_pretty(&order).map_err(|e| e.to_string())?;
    // Write beside the target and rename over it, so an interrupted write
    // cannot leave half an order behind.
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text + "\n")
        .map_err(|e| format!("Could not save the order: {e}"))?;
    std::fs::rename(&temp, &path)
        .map_err(|e| format!("Could not replace the order: {e}"))?;

    *app.state::<OrderStore>().0.lock().unwrap() = order.clone();
    let _ = app.emit("order-changed", order);
    Ok(())
}

/// How many times to retry binding the hook port, and how long between.
const LISTEN_RETRIES: u32 = 10;
const LISTEN_RETRY_MS: u64 = 500;

/// Whether the hook listener actually got the port.
pub struct ListenerUp(AtomicBool);

impl Default for ListenerUp {
    fn default() -> Self {
        // Assume it will bind; the listener corrects this either way within
        // a few seconds of startup.
        Self(AtomicBool::new(true))
    }
}

/// Whether Claude Code is actually wired up to talk to Switchboard.
///
/// Hooks failing quietly is the worst case for this widget: it looks like it
/// is working and simply reports nothing. Adding an event to HOOK_EVENTS in a
/// new version has the same effect, because the user's settings.json still
/// holds the old list.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookHealth {
    endpoint: String,
    /// False when the port could not be taken, which makes every other
    /// field meaningless - nothing can arrive at all.
    listening: bool,
    /// Events Switchboard needs that settings.json is missing.
    missing: Vec<String>,
    registered: usize,
    /// Events pointed somewhere other than this Switchboard.
    misdirected: Vec<String>,
    /// When a hook last arrived, or None if never.
    last_delivery: Option<u64>,
    deliveries: usize,
}

#[tauri::command]
fn hook_health(app: AppHandle, log: tauri::State<HookLog>) -> Result<HookHealth, String> {
    let endpoint = format!("http://127.0.0.1:{HOOK_PORT}");

    let mut missing: Vec<String> = Vec::new();
    let mut misdirected: Vec<String> = Vec::new();
    let mut registered = 0usize;

    let file = app
        .path()
        .home_dir()
        .map_err(|e| format!("Could not find your home directory: {e}"))?
        .join(".claude")
        .join("settings.json");

    let settings: serde_json::Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let hooks = settings.get("hooks").and_then(|h| h.as_object());

    for event in HOOK_EVENTS {
        let groups = hooks.and_then(|h| h.get(event)).and_then(|g| g.as_array());
        let Some(groups) = groups else {
            missing.push(event.to_string());
            continue;
        };
        registered += 1;

        let points_here = groups.iter().any(|group| {
            group
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry.get("url").and_then(|u| u.as_str()) == Some(endpoint.as_str())
                    })
                })
        });
        if !points_here {
            misdirected.push(event.to_string());
        }
    }

    let entries = log.0.lock().unwrap();
    Ok(HookHealth {
        endpoint,
        listening: app.state::<ListenerUp>().0.load(Ordering::Relaxed),
        missing,
        registered,
        misdirected,
        last_delivery: entries.back().map(|r| r.at),
        deliveries: entries.len(),
    })
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
        .manage(HookLog::default())
        .manage(OrderStore::default())
        .manage(Pinned::default())
        .manage(Ended::default())
        .manage(ListenerUp::default())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            *app.state::<OrderStore>().0.lock().unwrap() = load_order(&app.handle().clone());
            start_hook_listener(app.handle().clone());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_ignore_cursor_events(true);
            }
            start_hover_watch(app.handle().clone());
            start_ide_watch(app.handle().clone());
            start_session_reaper(app.handle().clone());
            start_transcript_watch(app.handle().clone());
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
            quiet_after_ms,
            recent_hooks,
            hook_health,
            list_order,
            set_order,
            set_pinned,
            is_pinned,
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
    use serde_json::json;

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

    #[test]
    fn a_finished_turn_is_waiting_for_you_not_done() {
        // The whole point: Stop means the assistant stopped talking, so it is
        // now your move. Reporting that as "done" hid the sessions that most
        // wanted attention.
        assert_eq!(state_for("Stop", &json!({})), Some("waiting"));
        assert_eq!(state_for("StopFailure", &json!({})), Some("failed"));
    }

    #[test]
    fn tool_events_keep_a_long_turn_looking_alive() {
        for event in ["UserPromptSubmit", "PreToolUse", "PostToolUse", "SubagentStop"] {
            assert_eq!(state_for(event, &json!({})), Some("working"), "{event}");
        }
    }

    #[test]
    fn attention_events_win_and_unknown_events_are_ignored() {
        assert_eq!(state_for("Notification", &json!({})), Some("needs_you"));
        assert_eq!(state_for("PermissionRequest", &json!({})), Some("needs_you"));
        assert_eq!(state_for("SessionStart", &json!({})), Some("idle"));
        assert_eq!(state_for("Compact", &json!({})), None);
        assert_eq!(state_for("", &json!({})), None);
    }

    #[test]
    fn every_event_the_states_need_is_actually_registered() {
        // A state the widget can show but never subscribes to would simply
        // never appear, silently.
        for event in ["SessionStart", "UserPromptSubmit", "PostToolUse", "SubagentStop",
                      "Stop", "StopFailure", "Notification", "SessionEnd"] {
            assert!(HOOK_EVENTS.contains(&event), "{event} is not registered");
        }
    }

    #[test]
    fn silent_sessions_are_forgotten_but_recent_ones_are_kept() {
        let now = 100 * 60 * 60 * 1000;
        let mut sessions = HashMap::new();
        for (id, age) in [("fresh", 0), ("quiet", QUIET_AFTER_MS), ("ancient", FORGET_AFTER_MS + 1)] {
            sessions.insert(
                id.to_string(),
                Session {
                    session_id: id.to_string(),
                    cwd: "C:/x".to_string(),
                    state: "waiting",
                    detail: None,
                    updated_at: now - age,
                },
            );
        }

        assert!(prune_dead(&mut sessions, now));
        assert!(sessions.contains_key("fresh"));
        // Quiet is dimmed, not dropped - it may still be a live session.
        assert!(sessions.contains_key("quiet"));
        assert!(!sessions.contains_key("ancient"));
        // Nothing left to do the second time.
        assert!(!prune_dead(&mut sessions, now));
    }

    #[test]
    fn a_notification_message_reaches_the_session() {
        let mut sessions = HashMap::new();
        let body = r#"{"hook_event_name":"Notification","session_id":"s1",
                       "cwd":"C:/Switchboard","message":"needs your permission"}"#;
        assert!(apply_hook(&mut sessions, &mut HashMap::new(), body));
        let s = &sessions["s1"];
        assert_eq!(s.state, "needs_you");
        assert_eq!(s.detail.as_deref(), Some("needs your permission"));
    }

    #[test]
    fn session_end_removes_and_is_idempotent() {
        let mut sessions = HashMap::new();
        apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"C:/x"}"#);
        assert!(apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"SessionEnd","session_id":"s1"}"#));
        assert!(!apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"SessionEnd","session_id":"s1"}"#));
    }

    #[test]
    fn a_later_event_without_cwd_keeps_the_one_we_know() {
        let mut sessions = HashMap::new();
        apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"C:/Themis B"}"#);
        apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"Stop","session_id":"s1"}"#);
        assert_eq!(sessions["s1"].cwd, "C:/Themis B");
        assert_eq!(sessions["s1"].state, "waiting");
    }

    #[test]
    fn detail_comes_from_the_field_each_event_actually_carries() {
        // Field names captured from live Claude Code 2.1.267 deliveries.
        assert_eq!(
            detail_for("Stop", &json!({"last_assistant_message": "Just says \"hello\""})),
            Some("Just says \"hello\"".to_string())
        );
        assert_eq!(
            detail_for("UserPromptSubmit", &json!({"prompt": "Read note.txt"})),
            Some("Read note.txt".to_string())
        );
        assert_eq!(
            detail_for("PostToolUse", &json!({"tool_name": "Bash"})),
            Some("Bash".to_string())
        );
        // There is no `message` field on these events; inventing one gave
        // every row a blank detail.
        assert_eq!(detail_for("Stop", &json!({"message": "nope"})), None);
        assert_eq!(detail_for("SessionEnd", &json!({"reason": "logout"})), None);
    }

    #[test]
    fn an_assistant_paragraph_is_flattened_to_one_clipped_line() {
        let long = "line one\nline two ".repeat(40);
        let out = one_line(&long, DETAIL_CHARS);
        assert!(!out.contains('\n'));
        assert!(out.chars().count() <= DETAIL_CHARS + 3, "{}", out.chars().count());
        assert!(out.ends_with("..."));
        // Short text is left exactly alone.
        assert_eq!(one_line("all good", DETAIL_CHARS), "all good");
    }

    #[test]
    fn an_idle_notification_is_not_treated_as_blocking() {
        // Claude Code nudges about a minute after finishing. Painting that
        // red trains you to ignore the badge.
        let idle = json!({"notification_type": "idle_prompt"});
        assert_eq!(state_for("Notification", &idle), Some("waiting"));

        let permission = json!({"notification_type": "permission_prompt"});
        assert_eq!(state_for("Notification", &permission), Some("needs_you"));

        // Unknown kinds stay blocking: a missed prompt is worse than a spare.
        let unknown = json!({"notification_type": "something_new"});
        assert_eq!(state_for("Notification", &unknown), Some("needs_you"));
        assert_eq!(state_for("Notification", &json!({})), Some("needs_you"));
    }

    #[test]
    fn a_real_stop_payload_lands_as_waiting_with_its_last_message() {
        // Shape taken verbatim from a live delivery.
        let body = r#"{
            "hook_event_name": "Stop",
            "session_id": "e091acf2-0000-0000-0000-000000000000",
            "cwd": "C:/Switchboard",
            "transcript_path": "C:/Users/x/.claude/projects/p/s.jsonl",
            "permission_mode": "default",
            "stop_hook_active": false,
            "last_assistant_message": "Just says \"hello\"",
            "effort": "high"
        }"#;
        let mut sessions = HashMap::new();
        assert!(apply_hook(&mut sessions, &mut HashMap::new(), body));
        let s = &sessions["e091acf2-0000-0000-0000-000000000000"];
        assert_eq!(s.state, "waiting");
        assert_eq!(s.detail.as_deref(), Some("Just says \"hello\""));
        assert_eq!(s.cwd, "C:/Switchboard");
    }

    fn at(now: u64, state: &'static str, age: u64) -> Session {
        Session {
            session_id: "s".to_string(),
            cwd: "c:/Themis B".to_string(),
            state,
            detail: None,
            updated_at: now - age,
        }
    }

    #[test]
    fn a_session_switchboard_never_saw_start_is_discovered_from_disk() {
        // The reported failure: an agent running in another worktree stayed
        // invisible, because hooks only describe sessions that fire while
        // Switchboard happens to be up, and a restart empties the store.
        let now = 10_000_000;
        let mut sessions = HashMap::new();
        let found = vec![Transcript {
            session_id: "themis-b".to_string(),
            cwd: "c:/Themis B".to_string(),
            last_write: now - 5_000,
        }];

        assert!(merge_transcripts(&mut sessions, &HashMap::new(), &found, now));
        let s = &sessions["themis-b"];
        // Being written to right now means the turn is still moving.
        assert_eq!(s.state, "working");
        assert_eq!(s.cwd, "c:/Themis B");
    }

    #[test]
    fn a_transcript_gone_cold_is_shown_but_not_claimed_to_be_working() {
        let now = 10_000_000;
        let mut sessions = HashMap::new();
        let found = vec![Transcript {
            session_id: "old".to_string(),
            cwd: "c:/Themis".to_string(),
            last_write: now - 30 * 60 * 1000,
        }];
        merge_transcripts(&mut sessions, &HashMap::new(), &found, now);
        // Disk knows it exists and when it last moved, not what it is doing.
        assert_eq!(sessions["old"].state, "unknown");
    }

    #[test]
    fn disk_activity_keeps_a_long_turn_from_looking_silent() {
        // Between UserPromptSubmit and Stop no hook fires, so a turn that
        // runs for an hour used to age into "quiet" while working fine.
        let now = 10_000_000;
        let mut sessions = HashMap::new();
        sessions.insert("s".to_string(), at(now, "working", 45 * 60 * 1000));
        let found = vec![Transcript {
            session_id: "s".to_string(),
            cwd: "c:/Themis B".to_string(),
            last_write: now - 2_000,
        }];

        assert!(merge_transcripts(&mut sessions, &HashMap::new(), &found, now));
        assert_eq!(sessions["s"].state, "working");
        assert_eq!(sessions["s"].updated_at, now - 2_000);
    }

    #[test]
    fn a_stops_own_tail_does_not_flip_it_back_to_working() {
        // Stop writes its last lines a moment after the hook arrives. That
        // must not read as a new turn.
        let now = 10_000_000;
        let mut sessions = HashMap::new();
        sessions.insert("s".to_string(), at(now, "waiting", 1_000));
        let found = vec![Transcript {
            session_id: "s".to_string(),
            cwd: "c:/Themis B".to_string(),
            last_write: now - 500,
        }];

        merge_transcripts(&mut sessions, &HashMap::new(), &found, now);
        assert_eq!(sessions["s"].state, "waiting", "a stop tail is not new work");
    }

    #[test]
    fn writing_long_after_the_last_hook_is_a_turn_we_missed() {
        let now = 10_000_000;
        let mut sessions = HashMap::new();
        sessions.insert("s".to_string(), at(now, "waiting", 10 * 60 * 1000));
        let found = vec![Transcript {
            session_id: "s".to_string(),
            cwd: "c:/Themis B".to_string(),
            last_write: now - 3_000,
        }];

        merge_transcripts(&mut sessions, &HashMap::new(), &found, now);
        assert_eq!(sessions["s"].state, "working");
    }

    #[test]
    fn a_blocked_session_is_never_talked_over_by_disk_activity() {
        let now = 10_000_000;
        let mut sessions = HashMap::new();
        sessions.insert("s".to_string(), at(now, "needs_you", 10 * 60 * 1000));
        let found = vec![Transcript {
            session_id: "s".to_string(),
            cwd: "c:/Themis B".to_string(),
            last_write: now - 1_000,
        }];

        merge_transcripts(&mut sessions, &HashMap::new(), &found, now);
        assert_eq!(sessions["s"].state, "needs_you");
    }

    #[test]
    fn cd_into_a_subfolder_does_not_split_a_worktree_in_two() {
        // Hook payloads carry the process's current directory, so a session
        // that cd'd into src-rust reported a second worktree.
        assert!(is_inside(r"C:\Switchboard\src-rust", r"C:\Switchboard"));
        assert!(is_inside("c:/switchboard/src-rust", r"C:\Switchboard"));
        assert!(is_inside(r"C:\Switchboard", r"C:\Switchboard\"));
        assert!(!is_inside(r"C:\Themis", r"C:\Themis B"));
        assert!(!is_inside(r"C:\Switchboard", r"C:\Switchboard\src-rust"));

        let mut sessions = HashMap::new();
        apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"SessionStart","session_id":"s","cwd":"C:/Switchboard"}"#);
        apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"PostToolUse","session_id":"s","cwd":"C:/Switchboard/src-rust","tool_name":"Bash"}"#);
        assert_eq!(sessions["s"].cwd, "C:/Switchboard");

        // Moving genuinely elsewhere still updates.
        apply_hook(&mut sessions, &mut HashMap::new(), r#"{"hook_event_name":"PostToolUse","session_id":"s","cwd":"C:/Themis","tool_name":"Bash"}"#);
        assert_eq!(sessions["s"].cwd, "C:/Themis");
    }

    #[test]
    fn only_current_sessions_survive_a_directory_full_of_history() {
        // Comfortably larger than the oldest age below, so the test's own
        // arithmetic cannot underflow.
        let now = 100_000_000;
        let t = |id: &str, cwd: &str, age: u64| Transcript {
            session_id: id.to_string(),
            cwd: cwd.to_string(),
            last_write: now - age,
        };
        let found = vec![
            t("live", "c:/Themis B", 3_000),               // still writing
            t("also-live", "c:/Themis B", 10_000),         // concurrent, also writing
            t("newest-cold", "c:/Themis", 40 * 60 * 1000), // newest for its worktree
            t("older", "c:/Themis", 90 * 60 * 1000),       // superseded
            t("ancient", "c:/Themis B", 3 * 60 * 60 * 1000),
        ];

        let kept: Vec<String> = keep_current(found, now)
            .into_iter()
            .map(|t| t.session_id)
            .collect();

        assert!(kept.contains(&"live".to_string()));
        assert!(kept.contains(&"also-live".to_string()), "concurrent sessions are real");
        assert!(kept.contains(&"newest-cold".to_string()));
        assert!(!kept.contains(&"older".to_string()), "superseded history is noise");
        assert!(!kept.contains(&"ancient".to_string()));
    }

    #[test]
    fn a_session_that_said_goodbye_is_not_resurrected_from_disk() {
        // Caught by running a real `claude -p` session end to end: the hooks
        // were all correct and SessionEnd removed it, then the transcript
        // scan put it straight back as "working", because the file still
        // looked freshly written. A finished session became a permanent
        // ghost claiming to be busy.
        let now = 100_000_000;
        let mut sessions = HashMap::new();
        let mut ended = HashMap::new();

        apply_hook(
            &mut sessions,
            &mut ended,
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"lab","cwd":"c:/lab/alpha","prompt":"go"}"#,
        );
        assert_eq!(sessions["lab"].state, "working");

        apply_hook(
            &mut sessions,
            &mut ended,
            r#"{"hook_event_name":"SessionEnd","session_id":"lab","reason":"other"}"#,
        );
        assert!(sessions.is_empty(), "SessionEnd removes it");
        assert!(ended.contains_key("lab"), "and is remembered");

        // The transcript is still there, written seconds ago.
        let found = vec![Transcript {
            session_id: "lab".to_string(),
            cwd: "c:/lab/alpha".to_string(),
            last_write: now - 2_000,
        }];
        assert!(!merge_transcripts(&mut sessions, &ended, &found, now));
        assert!(sessions.is_empty(), "and it stays gone");

        // A session that never said goodbye is still discovered.
        let other = vec![Transcript {
            session_id: "still-running".to_string(),
            cwd: "c:/lab/beta".to_string(),
            last_write: now - 2_000,
        }];
        assert!(merge_transcripts(&mut sessions, &ended, &other, now));
        assert_eq!(sessions["still-running"].state, "working");
    }
}
