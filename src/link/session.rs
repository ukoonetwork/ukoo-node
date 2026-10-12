use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{ClientTlsConfig, Endpoint};

use crate::engine::EngineClient;
use crate::error::NodeError;
use crate::setup::NodeState;
use crate::verify::Fingerprints;

use super::jobs::{self, Outcome};
use super::pb::node_link_client::NodeLinkClient;
use super::pb::node_message::Msg as Out;
use super::pb::relayer_message::Msg as In;
use super::pb::{self};

const AUTH_CONTEXT: &[u8] = b"ukoo-node-auth-v1";
/// §6.2: long prompts travel inside a single gRPC message.
const MAX_MESSAGE_BYTES: usize = 16 << 20;
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// The terminal gets one line per minute of work, not one per request.
const SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// The running engine, or `None` while the node is paused and its engine is stopped.
pub type EngineSlot = Arc<RwLock<Option<EngineClient>>>;

/// Everything the link needs about this machine and what it serves.
pub struct Node {
    pub state: NodeState,
    pub identity: SigningKey,
    pub engine: EngineSlot,
    pub platform: String,
    pub usable_memory_bytes: u64,
    /// Whether the node is serving (not paused, inside its serving hours); sent as `available`.
    pub serving: watch::Receiver<bool>,
    /// Jobs running right now, so pausing can wait for them to finish.
    pub active: Arc<AtomicU32>,
    /// Ask per 1M input and output tokens; `ukoo-node price` changes it while running.
    pub price: watch::Receiver<(String, String)>,
}

enum Ended {
    /// The relayer turned this node away; reconnecting won't help.
    Rejected(String),
    /// The connection dropped or never came up; worth retrying.
    Lost(String),
}

/// Stays connected to the relayer, reconnecting with backoff, until the relayer rejects the
/// node. Only returns with that rejection. Connects only while serving, because the relayer
/// benchmarks a node as soon as it connects.
pub async fn run(node: Node, mut fingerprints: Fingerprints) -> NodeError {
    let node = Arc::new(node);
    let mut backoff = Duration::from_secs(1);
    let mut connected_before = false;
    let mut serving = node.serving.clone();
    loop {
        let _ = serving.wait_for(|on| *on).await;
        let mut welcomed = false;
        match connect(&node, &mut fingerprints, &mut welcomed, connected_before).await {
            Ended::Rejected(message) => return NodeError::Rejected(message),
            Ended::Lost(reason) => {
                if welcomed {
                    connected_before = true;
                    backoff = Duration::from_secs(1);
                }
                println!(
                    "Lost connection to Ukoo ({reason}). Reconnecting in {}s…",
                    backoff.as_secs()
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

async fn channel(url: &str) -> Result<tonic::transport::Channel, String> {
    let mut endpoint = Endpoint::from_shared(url.to_string()).map_err(|e| e.to_string())?;
    if url.starts_with("https://") {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| e.to_string())?;
    }
    endpoint
        .connect_timeout(Duration::from_secs(10))
        .connect()
        .await
        .map_err(|e| format!("couldn't reach {url}: {e}"))
}

async fn connect(
    node: &Arc<Node>,
    fingerprints: &mut Fingerprints,
    welcomed: &mut bool,
    connected_before: bool,
) -> Ended {
    let lost = |reason: &str| Ended::Lost(reason.to_string());
    let channel = match channel(&node.state.relayer_url).await {
        Ok(channel) => channel,
        Err(reason) => return Ended::Lost(reason),
    };
    let mut client = NodeLinkClient::new(channel)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES);

    let (out, rx) = mpsc::channel::<pb::NodeMessage>(256);
    let send = |msg: Out| {
        let out = out.clone();
        async move { out.send(pb::NodeMessage { msg: Some(msg) }).await.is_ok() }
    };
    let public_key = node.identity.verifying_key().to_bytes();
    send(Out::Hello(pb::Hello {
        public_key: public_key.to_vec(),
        node_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: node.platform.clone(),
        usable_memory_bytes: node.usable_memory_bytes,
    }))
    .await;

    let mut inbound = match client.connect(ReceiverStream::new(rx)).await {
        Ok(response) => response.into_inner(),
        Err(status) => return Ended::Lost(status.message().to_string()),
    };
    let mut next = async || match inbound.message().await {
        Ok(Some(m)) => m.msg,
        _ => None,
    };

    let nonce = match next().await {
        Some(In::Challenge(c)) => c.nonce,
        Some(In::Rejected(r)) => return Ended::Rejected(r.message),
        _ => return lost("the relayer didn't send an identity challenge"),
    };
    let signature = node
        .identity
        .sign(&[AUTH_CONTEXT, &nonce, &public_key].concat());
    send(Out::ChallengeResponse(pb::ChallengeResponse {
        signature: signature.to_bytes().to_vec(),
    }))
    .await;

    let welcome = match next().await {
        Some(In::Welcome(w)) => w,
        Some(In::Rejected(r)) => return Ended::Rejected(r.message),
        _ => return lost("the relayer didn't accept the identity proof"),
    };
    *welcomed = true;

    let mut price = node.price.clone();
    let serve = async |fingerprints: &mut Fingerprints, (ask_in, ask_out): (String, String)| {
        let state = &node.state;
        Out::Serve(pb::Serve {
            model_id: state.model_id.clone(),
            model_version: state.version.clone(),
            file_sha256s: fingerprints.current().await,
            ask_price_per_1m_input_usd: ask_in,
            ask_price_per_1m_output_usd: ask_out,
            parallel_slots: state.parallel_slots as u32,
        })
    };
    let asked = price.borrow_and_update().clone();
    send(serve(fingerprints, asked).await).await;
    if connected_before {
        println!("Reconnected to Ukoo.");
    } else {
        println!(
            "Connected to Ukoo as {}. Ukoo checks the model and benchmarks this machine before sending it requests…",
            welcome.node_id
        );
    }

    let active = node.active.clone();
    let mut serving = node.serving.clone();
    let running: Arc<Mutex<HashMap<String, AbortHandle>>> = Arc::default();
    let interval = Duration::from_secs(u64::from(welcome.heartbeat_interval_secs.max(1)));
    let mut heartbeat = tokio::time::interval(interval);
    let mut summary = tokio::time::interval(SUMMARY_EVERY);
    let served = Arc::new(Served::default());
    let mut seq = 0u64;

    let ended = loop {
        tokio::select! {
            // Tell the relayer about a pause or resume right away, not at the next beat.
            _ = serving.changed() => heartbeat.reset_immediately(),
            Ok(()) = price.changed() => {
                let asked = price.borrow_and_update().clone();
                println!("Now asking ${} in / ${} out per 1M tokens for new requests.", asked.0, asked.1);
                if !send(serve(fingerprints, asked).await).await {
                    break lost("the connection closed");
                }
            }
            _ = summary.tick() => served.print(),
            _ = heartbeat.tick() => {
                seq += 1;
                let beat = Out::Heartbeat(pb::Heartbeat {
                    seq,
                    active_jobs: active.load(Ordering::Acquire),
                    available: *serving.borrow(),
                    file_sha256s: fingerprints.current().await,
                });
                if !send(beat).await {
                    break lost("the connection closed");
                }
            }
            msg = next() => match msg {
                Some(In::Job(job)) => {
                    let job_id = job.job_id.clone();
                    let task = tokio::spawn(answer(node.clone(), job, out.clone(), active.clone(), running.clone(), served.clone()));
                    running.lock().expect("jobs lock").insert(job_id, task.abort_handle());
                }
                Some(In::CancelJob(cancel)) => {
                    if let Some(task) = running.lock().expect("jobs lock").remove(&cancel.job_id) {
                        task.abort();
                    }
                }
                Some(In::Rejected(r)) => break Ended::Rejected(r.message),
                Some(_) => {}
                None => break lost("the relayer closed the connection"),
            },
        }
    };
    for (_, task) in running.lock().expect("jobs lock").drain() {
        task.abort();
    }
    ended
}

async fn answer(
    node: Arc<Node>,
    job: pb::Job,
    out: mpsc::Sender<pb::NodeMessage>,
    active: Arc<AtomicU32>,
    running: Arc<Mutex<HashMap<String, AbortHandle>>>,
    served: Arc<Served>,
) {
    let job_id = job.job_id.clone();
    let _busy = Busy::new(active);
    let engine = node.engine.read().expect("engine lock").clone();
    let outcome = match engine {
        Some(engine) => jobs::run(&engine, job, &out).await,
        None => jobs::refuse(job, &out, "this node is paused").await,
    };
    running.lock().expect("jobs lock").remove(&job_id);
    match outcome {
        Outcome::Answered { completion_tokens } => served.add(completion_tokens),
        Outcome::Failed(reason) => println!("A request failed on this machine: {reason}"),
    }
}

#[derive(Default)]
struct Served {
    requests: AtomicU32,
    tokens: AtomicU64,
}

impl Served {
    fn add(&self, tokens: u32) {
        self.requests.fetch_add(1, Ordering::AcqRel);
        self.tokens.fetch_add(u64::from(tokens), Ordering::AcqRel);
    }

    fn print(&self) {
        let requests = self.requests.swap(0, Ordering::AcqRel);
        let tokens = self.tokens.swap(0, Ordering::AcqRel);
        if requests > 0 {
            println!("Answered {requests} request(s) in the last minute ({tokens} tokens).");
        }
    }
}

/// Counts a running job for heartbeats, including one cancelled mid-answer.
struct Busy(Arc<AtomicU32>);

impl Busy {
    fn new(active: Arc<AtomicU32>) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        Self(active)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
