import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./Settings.css";

type HookHealth = {
  endpoint: string;
  missing: string[];
  registered: number;
  misdirected: string[];
  lastDelivery: number | null;
  deliveries: number;
};

type HookRecord = {
  at: number;
  event: string;
  sessionId: string;
  cwd: string;
  outcome: string;
  fields: string[];
};

function ago(at: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  return `${Math.round(minutes / 60)}h ago`;
}

function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/// A plain settings window. No hover behaviour, no transparency, no widget
/// tricks - it is an ordinary window with an ordinary title bar.
export default function Settings() {
  const [health, setHealth] = useState<HookHealth | null>(null);
  const [hooks, setHooks] = useState<HookRecord[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());

  const refresh = useCallback(() => {
    void invoke<HookHealth>("hook_health").then(setHealth).catch(() => {});
    void invoke<HookRecord[]>("recent_hooks").then(setHooks).catch(() => {});
  }, []);

  useEffect(() => {
    // The widget window is transparent; this one must paint its own background.
    document.body.style.background = "#000";
    refresh();

    // Every delivery updates the log, so the window shows traffic as it lands.
    const off = listen("hooks-changed", refresh);
    const ticker = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      void off.then((stop) => stop());
      clearInterval(ticker);
    };
  }, [refresh]);

  async function connect() {
    setBusy(true);
    setMessage(null);
    try {
      setMessage(await invoke<string>("connect_claude_code"));
      refresh();
    } catch (err) {
      setMessage(String(err));
    } finally {
      setBusy(false);
    }
  }

  const incomplete =
    health !== null &&
    (health.missing.length > 0 || health.misdirected.length > 0);
  const silent = health !== null && health.lastDelivery === null;

  return (
    <main className="page">
      <section className="group">
        <h2>Claude Code</h2>
        <p className="copy">
          Switchboard listens on <code>{health?.endpoint ?? "..."}</code> and
          Claude Code reports to it through hooks. Connecting adds those hooks
          to your Claude Code settings, keeps anything already configured, and
          backs the file up before the first change.
        </p>

        {health && (
          <div className={`status ${incomplete ? "bad" : silent ? "warn" : "good"}`}>
            {incomplete ? (
              <>
                <strong>Hooks are out of date.</strong>{" "}
                {health.missing.length > 0 && (
                  <>
                    Not registered: <code>{health.missing.join(", ")}</code>.{" "}
                  </>
                )}
                {health.misdirected.length > 0 && (
                  <>
                    Registered but not pointed here:{" "}
                    <code>{health.misdirected.join(", ")}</code>.{" "}
                  </>
                )}
                Connect again to add them, then start a new session.
              </>
            ) : silent ? (
              <>
                <strong>All {health.registered} hooks registered</strong>, but
                nothing has arrived yet. Hooks are read when a session starts,
                so open a new Claude Code session.
              </>
            ) : (
              <>
                <strong>All {health.registered} hooks registered.</strong> Last
                delivery {ago(health.lastDelivery ?? 0, now)}, {health.deliveries}{" "}
                kept.
              </>
            )}
          </div>
        )}

        <div className="row">
          <button className="action" onClick={connect} disabled={busy}>
            {busy ? "Connecting" : "Connect Claude Code"}
          </button>
          {message && <span className="message">{message}</span>}
        </div>
      </section>

      <section className="group">
        <h2>Recent hooks</h2>
        <p className="copy">
          What Claude Code has actually sent, newest first. Events Switchboard
          ignores are listed too, so a hook that arrives but changes nothing is
          visible rather than looking like silence.
        </p>
        {hooks.length === 0 ? (
          <p className="note">Nothing yet.</p>
        ) : (
          <ul className="hooks">
            {[...hooks].reverse().slice(0, 40).map((h, i) => (
              <li key={`${h.at}-${i}`} className={h.outcome}>
                <span className="hook-event">{h.event || "(no name)"}</span>
                <span className="hook-where">
                  {h.cwd ? basename(h.cwd) : "-"}
                </span>
                <span className="hook-outcome">{h.outcome}</span>
                <span className="hook-when">{ago(h.at, now)}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="group">
        <h2>Display</h2>
        <p className="copy">
          Reported by this window. Useful when Eve looks the wrong size on a
          monitor with different scaling.
        </p>
        <p className="metrics">
          <code>
            devicePixelRatio {window.devicePixelRatio} · css{" "}
            {window.innerWidth}x{window.innerHeight}
          </code>
        </p>
      </section>

      <div className="footer">
        <button
          className="action"
          onClick={() => void invoke("close_settings").catch(() => {})}
        >
          Close
        </button>
      </div>
    </main>
  );
}
