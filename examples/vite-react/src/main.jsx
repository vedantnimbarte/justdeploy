import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";

const page = {
  minHeight: "100vh",
  margin: 0,
  background: "#f1efed",
  color: "#161513",
  fontFamily: "system-ui, sans-serif",
  display: "grid",
  placeItems: "center",
};

function App() {
  const [count, setCount] = useState(0);
  return (
    <main style={page}>
      <div style={{ maxWidth: 560, padding: 24 }}>
        <h1 style={{ fontFamily: "Georgia, serif", fontWeight: 400, fontSize: 40, margin: 0 }}>
          Vite + React
        </h1>
        <div style={{ width: 40, height: 4, background: "#f0cc71", margin: "18px 0 24px" }} />
        <p>Built by Railpack and served as static files. Change this page and push to redeploy.</p>
        <button
          onClick={() => setCount((c) => c + 1)}
          style={{
            background: "#312d2a",
            color: "#fff",
            border: 0,
            borderRadius: 4,
            padding: "14px 16px",
            fontSize: 14,
            cursor: "pointer",
          }}
        >
          Clicked {count} times
        </button>
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
