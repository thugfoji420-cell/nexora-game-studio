import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles.css";

const root = document.getElementById("root");

if (!root) {
  console.error("Application root element is missing!");
  throw new Error("Application root element is missing.");
}

createRoot(root).render(
  <StrictMode>
    <ErrorBoundary fallback={<div style={{padding: '40px', color: '#ff6b6b', textAlign: 'center'}}><h1>React Error</h1><p>Something went wrong during rendering.</p></div>}>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
