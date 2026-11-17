"""A todo API. DATABASE_URL is wired to the template's PostgreSQL service."""

import os
from contextlib import asynccontextmanager

import psycopg
import uvicorn
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

DATABASE_URL = os.environ["DATABASE_URL"]


def connect():
    return psycopg.connect(DATABASE_URL, autocommit=True)


@asynccontextmanager
async def lifespan(_app):
    with connect() as conn:
        conn.execute(
            "CREATE TABLE IF NOT EXISTS todos ("
            " id SERIAL PRIMARY KEY, title TEXT NOT NULL, done BOOLEAN NOT NULL DEFAULT false)"
        )
    yield


app = FastAPI(title="fastapi-postgres-example", lifespan=lifespan)


class NewTodo(BaseModel):
    title: str


@app.get("/health")
def health():
    with connect() as conn:
        conn.execute("SELECT 1")
    return {"ok": True}


@app.get("/todos")
def list_todos():
    with connect() as conn:
        rows = conn.execute("SELECT id, title, done FROM todos ORDER BY id").fetchall()
    return [{"id": r[0], "title": r[1], "done": r[2]} for r in rows]


@app.post("/todos", status_code=201)
def create_todo(todo: NewTodo):
    with connect() as conn:
        row = conn.execute(
            "INSERT INTO todos (title) VALUES (%s) RETURNING id, title, done", (todo.title,)
        ).fetchone()
    return {"id": row[0], "title": row[1], "done": row[2]}


@app.post("/todos/{todo_id}/done")
def complete(todo_id: int):
    with connect() as conn:
        row = conn.execute(
            "UPDATE todos SET done = true WHERE id = %s RETURNING id", (todo_id,)
        ).fetchone()
    if row is None:
        raise HTTPException(404, "no such todo")
    return {"id": todo_id, "done": True}


if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=int(os.environ.get("PORT", "8080")))
