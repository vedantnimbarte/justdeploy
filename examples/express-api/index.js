import express from "express";

const app = express();
app.use(express.json());

const notes = [{ id: 1, text: "Deployed with justdeploy" }];

app.get("/", (_req, res) => res.json({ service: "express-api", endpoints: ["/health", "/notes"] }));
app.get("/health", (_req, res) => res.json({ ok: true }));
app.get("/notes", (_req, res) => res.json(notes));
app.post("/notes", (req, res) => {
  const note = { id: notes.length + 1, text: String(req.body?.text ?? "") };
  notes.push(note);
  res.status(201).json(note);
});

const port = Number(process.env.PORT ?? 8080);
app.listen(port, "0.0.0.0", () => console.log(`listening on ${port}`));
