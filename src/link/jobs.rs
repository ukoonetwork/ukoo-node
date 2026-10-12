use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::engine::EngineClient;

use super::pb::job_event::Event;
use super::pb::{self, node_message::Msg};

/// How a job ended, for the operator's terminal.
pub enum Outcome {
    Answered { completion_tokens: u32 },
    Failed(String),
}

/// Sends one relayer job to the local engine and streams every `data:` payload back unchanged.
pub async fn run(
    engine: &EngineClient,
    job: pb::Job,
    out: &mpsc::Sender<pb::NodeMessage>,
) -> Outcome {
    let deadline = Duration::from_millis(u64::from(job.deadline_ms.max(1)));
    let job_id = job.job_id.clone();
    let send = |event: Event| {
        let msg = pb::NodeMessage {
            msg: Some(Msg::JobEvent(pb::JobEvent {
                job_id: job_id.clone(),
                event: Some(event),
            })),
        };
        async move {
            let _ = out.send(msg).await;
        }
    };
    let failed = |code: &str, message: String| {
        Event::Failed(pb::Failed {
            code: code.to_string(),
            message,
        })
    };

    let streamed = tokio::time::timeout(deadline, stream(engine, job.request_json, &send)).await;
    match streamed {
        Ok(Ok(usage)) => {
            send(Event::Completed(usage)).await;
            Outcome::Answered {
                completion_tokens: usage.node_completion_tokens,
            }
        }
        Ok(Err((code, message))) => {
            send(failed(code, message.clone())).await;
            Outcome::Failed(message)
        }
        Err(_) => {
            let message = "the answer took too long".to_string();
            send(failed("deadline_exceeded", message.clone())).await;
            Outcome::Failed(message)
        }
    }
}

/// Turns a job away without running it; the relayer moves it to another node.
pub async fn refuse(job: pb::Job, out: &mpsc::Sender<pb::NodeMessage>, reason: &str) -> Outcome {
    let failed = Event::Failed(pb::Failed {
        code: "engine_error".to_string(),
        message: reason.to_string(),
    });
    let _ = out
        .send(pb::NodeMessage {
            msg: Some(Msg::JobEvent(pb::JobEvent {
                job_id: job.job_id,
                event: Some(failed),
            })),
        })
        .await;
    Outcome::Failed(reason.to_string())
}

type JobError = (&'static str, String);

async fn stream<F, Fut>(
    engine: &EngineClient,
    request_json: Vec<u8>,
    send: &F,
) -> Result<pb::Completed, JobError>
where
    F: Fn(Event) -> Fut,
    Fut: Future<Output = ()>,
{
    let engine_error = |e: String| ("engine_error", e);
    let response = engine
        .chat(request_json)
        .await
        .map_err(|e| engine_error(e.to_string()))?;
    let status = response.status();
    if status.is_client_error() {
        // The engine refused the request itself (e.g. too long for the context): the
        // requester's problem, so the relayer doesn't retry it elsewhere.
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(body);
        return Err(("bad_request", message));
    }
    if !status.is_success() {
        return Err(engine_error(format!("engine answered {status}")));
    }

    let mut usage = pb::Completed::default();
    let mut body = response.bytes_stream();
    let mut buf = Vec::new();
    while let Some(bytes) = body.next().await {
        buf.extend_from_slice(&bytes.map_err(|e| engine_error(e.to_string()))?);
        while let Some(end) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=end).collect();
            let Some(data) = line.trim_ascii().strip_prefix(b"data: ") else {
                continue;
            };
            if data == b"[DONE]" {
                continue;
            }
            if let Ok(chunk) = serde_json::from_slice::<Value>(data) {
                if let Some(message) = chunk["error"]["message"].as_str() {
                    return Err(engine_error(message.to_string()));
                }
                if let Some(u) = chunk.get("usage").filter(|u| u.is_object()) {
                    usage.node_prompt_tokens = u["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                    usage.node_completion_tokens =
                        u["completion_tokens"].as_u64().unwrap_or(0) as u32;
                }
            }
            send(Event::SseData(data.to_vec())).await;
        }
    }
    Ok(usage)
}
