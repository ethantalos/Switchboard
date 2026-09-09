use serde::Serialize;
use std::path::Path;
use std::process::Command;

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

/// Keep the Switchboard window above other windows, so it stays visible after
/// focus moves to an editor or browser.
#[tauri::command]
fn set_always_on_top(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window
        .set_always_on_top(enabled)
        .map_err(|e| format!("Could not change window layering: {e}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_worktrees,
            open_in_vscode,
            set_always_on_top
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
