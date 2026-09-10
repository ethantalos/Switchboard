import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./Settings.css";

/// A plain settings window. No hover behaviour, no transparency, no widget
/// tricks — it is an ordinary window with an ordinary title bar.
export default function Settings() {
  const [endpoint, setEndpoint] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    // The widget window is transparent; this one must paint its own background.
    document.body.style.background = "#000";
    void invoke<string>("hook_endpoint").then(setEndpoint).catch(() => {});
  }, []);

  async function connect() {
    setBusy(true);
    setMessage(null);
    try {
      setMessage(await invoke<string>("connect_claude_code"));
    } catch (err) {
      setMessage(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="page">
      <section className="group">
        <h2>Claude Code</h2>
        <p className="copy">
          Switchboard listens on <code>{endpoint}</code> and Claude Code reports
          to it through hooks. This adds those hooks to your Claude Code
          settings, keeps anything already configured, and backs the file up
          before the first change.
        </p>
        <div className="row">
          <button className="action" onClick={connect} disabled={busy}>
            {busy ? "Connecting" : "Connect Claude Code"}
          </button>
          {message && <span className="message">{message}</span>}
        </div>
        <p className="note">
          Hooks are read when a session starts, so open a new Claude Code session
          after connecting.
        </p>
      </section>

      <section className="group">
        <h2>Display</h2>
        <p className="copy">
          Reported by this window. Useful when Eve looks the wrong size on
          a monitor with different scaling.
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
