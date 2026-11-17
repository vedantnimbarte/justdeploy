//! Live views: deployment logs and runtime logs (SSE), metrics, and the web terminal.

use crate::{
    auth::Admin,
    docker::{self, LABEL_KIND, LABEL_SERVICE},
    error::{bad_request, Result},
    metrics, services,
    state::SharedState,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use bollard::{
    exec::{CreateExecOptions, StartExecResults},
    query_parameters::{LogsOptionsBuilder, ResizeExecOptionsBuilder},
};
use futures::{SinkExt, Stream, StreamExt};
use serde::Deserialize;
use std::{convert::Infallible, time::Duration};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

type EventStream = Response;

/// Deployment log, polled from the database so a reconnect resumes where it left off.
pub async fn deployment_logs(_: Admin, State(state): State<SharedState>, Path(id): Path<Uuid>) -> Result<EventStream> {
    services::get_deployment(&state.db, id).await?;
    let stream = async_stream_logs(state, id);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response())
}

fn async_stream_logs(state: SharedState, id: Uuid) -> impl Stream<Item = Result<Event, Infallible>> + Send {
    futures::stream::unfold((state, 0_i64, false), move |(state, after, done)| async move {
        if done {
            return None;
        }
        loop {
            let lines = sqlx::query!(
                "SELECT id, line FROM deployment_logs WHERE deployment_id = $1 AND id > $2 ORDER BY id LIMIT 500",
                id,
                after
            )
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
            if let Some(last) = lines.last() {
                let last_id = last.id;
                let text = lines.into_iter().map(|l| l.line).collect::<Vec<_>>().join("\n");
                return Some((Ok(Event::default().data(text)), (state, last_id, false)));
            }
            let finished = sqlx::query_scalar!("SELECT status FROM deployments WHERE id = $1", id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .is_none_or(|s| !matches!(s.as_str(), "queued" | "building" | "deploying"));
            if finished {
                return Some((Ok(Event::default().event("end").data("")), (state, after, true)));
            }
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    })
}

#[derive(Deserialize)]
pub struct LogQuery {
    tail: Option<u32>,
}

/// Runtime output of the live container, followed.
pub async fn runtime_logs(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Query(query): Query<LogQuery>,
) -> Result<EventStream> {
    let container = live_container(&state, id).await?;
    let Some(container) = container else {
        let once = futures::stream::once(async {
            Ok::<_, Infallible>(Event::default().data("(no running container)"))
        });
        return Ok(Sse::new(once).into_response());
    };
    let options = LogsOptionsBuilder::new()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .tail(&query.tail.unwrap_or(300).min(5000).to_string())
        .build();
    let docker = state.docker.clone();
    let stream = docker
        .logs(&container, Some(options))
        .scan(docker::LineSplitter::default(), |splitter, chunk| {
            let lines = match chunk {
                Ok(chunk) => splitter.push(&docker::output_bytes(&chunk)),
                Err(_) => vec![],
            };
            futures::future::ready(Some(lines))
        })
        .filter(|lines| futures::future::ready(!lines.is_empty()))
        .map(|lines| Ok::<_, Infallible>(Event::default().data(lines.join("\n"))));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response())
}

async fn live_container(state: &SharedState, service_id: Uuid) -> Result<Option<String>> {
    services::get_service(&state.db, service_id).await?;
    let id = service_id.to_string();
    Ok(docker::list(&state.docker, &[(LABEL_SERVICE, &id), (LABEL_KIND, "production")])
        .await?
        .into_iter()
        .find(docker::is_running)
        .and_then(|c| c.id))
}

#[derive(Deserialize)]
pub struct MetricsQuery {
    minutes: Option<i32>,
}

pub async fn metrics(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Query(query): Query<MetricsQuery>,
) -> Result<Json<Vec<metrics::Sample>>> {
    Ok(Json(metrics::series(&state.db, id, query.minutes.unwrap_or(60)).await?))
}

#[derive(Deserialize)]
struct Resize {
    cols: u16,
    rows: u16,
}

/// A shell in the live container. Binary frames carry terminal bytes both ways;
/// a text frame `{"cols":..,"rows":..}` resizes.
pub async fn terminal(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    let container = live_container(&state, id)
        .await?
        .ok_or_else(|| bad_request("the service has no running container"))?;
    let service = services::get_service(&state.db, id).await?;
    Ok(ws.on_upgrade(move |socket| async move {
        services::audit(&state.db, "terminal.opened", &service.name, None).await;
        if let Err(error) = run_terminal(&state, &container, socket).await {
            tracing::warn!(%error, "terminal session ended with an error");
        }
        services::audit(&state.db, "terminal.closed", &service.name, None).await;
    }))
}

async fn run_terminal(state: &SharedState, container: &str, socket: WebSocket) -> anyhow::Result<()> {
    let exec = state
        .docker
        .create_exec(
            container,
            CreateExecOptions {
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                env: Some(vec!["TERM=xterm-256color"]),
                cmd: Some(vec![
                    "sh",
                    "-c",
                    "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi",
                ]),
                ..Default::default()
            },
        )
        .await?;
    let StartExecResults::Attached { mut output, mut input } = state.docker.start_exec(&exec.id, None).await? else {
        anyhow::bail!("could not attach");
    };
    let (mut to_browser, mut from_browser) = socket.split();

    let outbound = async {
        while let Some(Ok(chunk)) = output.next().await {
            let bytes = docker::output_bytes(&chunk);
            if to_browser.send(Message::Binary(bytes)).await.is_err() {
                break;
            }
        }
        let _ = to_browser.close().await;
    };
    let inbound = async {
        while let Some(Ok(message)) = from_browser.next().await {
            match message {
                Message::Binary(data) => {
                    if input.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Message::Text(text) => {
                    if let Ok(size) = serde_json::from_str::<Resize>(&text) {
                        let options = ResizeExecOptionsBuilder::new()
                            .h(size.rows.into())
                            .w(size.cols.into())
                            .build();
                        let _ = state.docker.resize_exec(&exec.id, options).await;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    tokio::select! {
        _ = outbound => {}
        _ = inbound => {}
    }
    Ok(())
}
