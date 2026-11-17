// Rendered on each request, to show the server is really running.
export const dynamic = "force-dynamic";

export default function Home() {
  return (
    <main style={{ maxWidth: 640, margin: "0 auto", padding: "96px 24px" }}>
      <h1 style={{ fontFamily: "Georgia, serif", fontWeight: 400, fontSize: 40, margin: 0 }}>Next.js</h1>
      <div style={{ width: 40, height: 4, background: "#f0cc71", margin: "18px 0 24px" }} />
      <p>Server-rendered at {new Date().toISOString()} by a Next.js server that Railpack built.</p>
    </main>
  );
}
