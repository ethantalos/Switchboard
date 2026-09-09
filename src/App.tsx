import { useEffect, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type Worktree = {
  path: string;
  head: string | null;
  branch: string | null;
  detached: boolean;
  bare: boolean;
  locked: boolean;
  prunable: boolean;
};

type Pinned = {
  name: string;
  path: string;
  note?: string;
};

// Hardcoded for now. These become user-managed workspaces once registration
// and storage exist (TODO sections 2 and 5).
const PINNED: Pinned[] = [
  { name: "Themis", path: "C:\Themis" },
  { name: "Themis B", path: "C:\Themis B" },
  { name: "Themis C", path: "C:\Themis C" },
  { name: "Switchboard", path: "C:\Switchboard", note: "this session" },
];

const LAST_REPO_KEY = "switchboard.lastRepo";
const ON_TOP_KEY = "switchboard.alwaysOnTop";

function describe(worktree: Worktree): string {
  if (worktree.branch) return worktree.branch;
  if (worktree.detached) return "detached HEAD";
  if (worktree.bare) return "bare";
  return "unknown";
}

export default function App() {
  const [repoPath, setRepoPath] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [worktrees, setWorktrees] = useState<Worktree[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [onTop, setOnTop] = useState(false);

  useEffect(() => {
    const savedRepo = localStorage.getItem(LAST_REPO_KEY);
    if (savedRepo) {
      setRepoPath(savedRepo);
      void load(savedRepo);
    }
    if (localStorage.getItem(ON_TOP_KEY) === "true") void toggleOnTop(true);
  }, []);

  async function toggleOnTop(enabled: boolean) {
    setOnTop(enabled);
    localStorage.setItem(ON_TOP_KEY, String(enabled));
    try {
      await invoke("set_always_on_top", { enabled });
    } catch (err) {
      setError(String(err));
    }
  }

  async function load(path: string) {
    const target = path.trim();
    if (!target) return;

    setBusy(true);
    setError(null);
    setSelected(target);
    try {
      const found = await invoke<Worktree[]>("list_worktrees", { repoPath: target });
      setWorktrees(found);
      localStorage.setItem(LAST_REPO_KEY, target);
    } catch (err) {
      setError(String(err));
      setWorktrees([]);
    } finally {
      setBusy(false);
      setLoaded(true);
    }
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    void load(repoPath);
  }

  function selectPinned(pinned: Pinned) {
    setRepoPath(pinned.path);
    void load(pinned.path);
  }

  async function openWorktree(path: string) {
    setError(null);
    try {
      await invoke("open_in_vscode", { path });
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="layout">
      <aside className="sidebar">
        <p className="sidebar-title">Workspaces</p>
        <nav>
          {PINNED.map((pinned) => (
            <button
              key={pinned.path}
              className={`pinned${selected === pinned.path ? " active" : ""}`}
              onClick={() => selectPinned(pinned)}
            >
              <span className="pinned-name">{pinned.name}</span>
              {pinned.note && <span className="pinned-note">{pinned.note}</span>}
            </button>
          ))}
        </nav>
      </aside>

      <main className="content">
        <header className="header">
          <div>
            <h1>Switchboard</h1>
            <p className="subtitle">
              {selected ? selected : "Select a workspace or enter a path"}
            </p>
          </div>
          <label className="on-top" title="Keep this window above others">
            <input
              type="checkbox"
              checked={onTop}
              onChange={(event) => toggleOnTop(event.target.checked)}
            />
            Stay on top
          </label>
        </header>

        <form className="repo-form" onSubmit={submit}>
          <input
            value={repoPath}
            onChange={(event) => setRepoPath(event.target.value)}
            placeholder="C:\Themis"
            spellCheck={false}
            aria-label="Repository path"
          />
          <button type="submit" disabled={busy || !repoPath.trim()}>
            {busy ? "Loading" : "Load"}
          </button>
        </form>

        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}

        {loaded && !error && worktrees.length === 0 && (
          <p className="empty">No worktrees found.</p>
        )}

        <ul className="worktrees">
          {worktrees.map((worktree) => (
            <li key={worktree.path}>
              <button className="worktree" onClick={() => openWorktree(worktree.path)}>
                <span className="branch">{describe(worktree)}</span>
                <span className="path">{worktree.path}</span>
              </button>
            </li>
          ))}
        </ul>
      </main>
    </div>
  );
}
