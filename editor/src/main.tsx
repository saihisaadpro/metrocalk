import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./theme/global.css"; // the design-system stylesheet (M14.1 / ADR-057) — palette vars + mtk-* classes
import { App } from "./app/App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
