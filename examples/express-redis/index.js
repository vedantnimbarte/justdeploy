// A visit counter. REDIS_URL is wired to the template's Redis service.
import express from "express";
import { createClient } from "redis";

const redis = createClient({ url: process.env.REDIS_URL });
redis.on("error", (err) => console.error("redis:", err.message));
await redis.connect();

const app = express();

app.get("/", async (_req, res) => {
  const visits = await redis.incr("visits");
  res.json({ service: "express-redis", visits });
});

app.get("/health", async (_req, res) => {
  await redis.ping();
  res.json({ ok: true });
});

const port = Number(process.env.PORT ?? 8080);
app.listen(port, "0.0.0.0", () => console.log(`listening on ${port}`));
