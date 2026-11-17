import os

import uvicorn
from fastapi import FastAPI

app = FastAPI(title="fastapi-example")


@app.get("/")
def index():
    return {"service": "fastapi", "docs": "/docs"}


@app.get("/health")
def health():
    return {"ok": True}


@app.get("/hello/{name}")
def hello(name: str):
    return {"message": f"Hello, {name}"}


if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=int(os.environ.get("PORT", "8080")))
