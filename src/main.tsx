import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/ibm-plex-sans";
import "@fontsource/ibm-plex-mono/400.css";
import App from "./App";
import "./app.css";
import "./theme.css";
import "./glass-effects.css";
import "./visual-refinements.css";
import "./glass-materials.css";
import "./interaction-refinements.css";
import "./dashboard.css";
import "./feedback.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><App /></React.StrictMode>,
);
