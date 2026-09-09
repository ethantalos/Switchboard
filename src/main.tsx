import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import App from "./App.tsx";
import Settings from "./Settings.tsx";

// One bundle, two windows. Rust opens the settings window with ?window=settings.
const view = new URLSearchParams(window.location.search).get("window");

createRoot(document.getElementById("root")!).render(
  <StrictMode>{view === "settings" ? <Settings /> : <App />}</StrictMode>,
);
