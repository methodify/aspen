//! The app-server process and its JSON-RPC line client
//! (CODEX_RUNTIME_REFERENCE.md §2). Schema-ignorant: it spawns
//! `codex app-server --listen stdio://`, correlates our requests with their
//! responses, hands server→client requests to a handler, and streams
//! notifications. Nothing here knows what a thread is.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

/// How to launch the app server.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    pub codex_bin: String,
    pub cwd: PathBuf,
    /// `-c key=value` overrides (TOML values), e.g. the trust for the cwd.
    pub config_overrides: Vec<(String, String)>,
    pub extra_env: Vec<(String, String)>,
    pub extra_args: Vec<String>,
}

/// A message from the server: a notification or a request we must answer.
#[derive(Debug, Clone)]
pub enum Inbound {
    Notification { method: String, params: Value },
    Request { id: Value, method: String, params: Value },
    /// A stderr line.
    Stderr(String),
    /// The process exited.
    Exited(Option<i32>),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

pub struct RpcClient {
    stdin_tx: mpsc::Sender<String>,
    pending: Pending,
    next_id: AtomicU64,
    kill_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub pid: Option<u32>,
}

impl RpcClient {
    /// Spawn the process. Returns the client and the inbound stream.
    pub fn spawn(spec: &ProcessSpec) -> Result<(Arc<Self>, mpsc::Receiver<Inbound>)> {
        let mut cmd = Command::new(&spec.codex_bin);
        cmd.arg("app-server").arg("--listen").arg("stdio://");
        for (k, v) in &spec.config_overrides {
            cmd.arg("-c").arg(format!("{k}={v}"));
        }
        cmd.args(&spec.extra_args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("ASPEN_DETACHED")
            .kill_on_drop(true);
        for (k, v) in &spec.extra_env {
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !v.is_empty() {
                cmd.env(k, v);
            }
        }
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning {} app-server in {}", spec.codex_bin, spec.cwd.display()))?;
        let stdin = child.stdin.take().context("child stdin missing")?;
        let stdout = child.stdout.take().context("child stdout missing")?;
        let stderr = child.stderr.take().context("child stderr missing")?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(1024);
        let (inbound_tx, inbound_rx) = mpsc::channel::<Inbound>(4096);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let client = Arc::new(Self {
            stdin_tx,
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            kill_tx: Mutex::new(Some(kill_tx)),
            pid: child.id(),
        });

        // Writer.
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // Reader: responses settle pending calls; everything else flows out.
        {
            let pending = pending.clone();
            let inbound_tx = inbound_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let v: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => {
                            tracing::debug!(target: "codex_stdout", "non-json line: {line}");
                            continue;
                        }
                    };
                    let method = v.get("method").and_then(|m| m.as_str()).map(str::to_owned);
                    match (method, v.get("id")) {
                        (Some(method), Some(id)) => {
                            let _ = inbound_tx
                                .send(Inbound::Request {
                                    id: id.clone(),
                                    method,
                                    params: v.get("params").cloned().unwrap_or(Value::Null),
                                })
                                .await;
                        }
                        (Some(method), None) => {
                            let _ = inbound_tx
                                .send(Inbound::Notification { method, params: v.get("params").cloned().unwrap_or(Value::Null) })
                                .await;
                        }
                        (None, Some(id)) => {
                            let Some(id) = id.as_u64() else { continue };
                            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                let r = match v.get("error") {
                                    Some(e) if !e.is_null() => Err(e
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| e.to_string())),
                                    _ => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                                };
                                let _ = tx.send(r);
                            }
                        }
                        (None, None) => {}
                    }
                }
            });
        }

        // Stderr.
        {
            let inbound_tx = inbound_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "codex_stderr", "{line}");
                    let _ = inbound_tx.send(Inbound::Stderr(line)).await;
                }
            });
        }

        // Supervisor.
        {
            let pending = pending.clone();
            tokio::spawn(async move {
                let code = tokio::select! {
                    status = child.wait() => status.ok().and_then(|s| s.code()),
                    _ = kill_rx => {
                        let _ = child.kill().await;
                        None
                    }
                };
                let drained: Vec<_> = pending.lock().unwrap().drain().collect();
                for (_, tx) in drained {
                    let _ = tx.send(Err("app-server exited".into()));
                }
                let _ = inbound_tx.send(Inbound::Exited(code)).await;
            });
        }

        Ok((client, inbound_rx))
    }

    async fn write(&self, v: Value) -> Result<()> {
        let line = serde_json::to_string(&v)?;
        self.stdin_tx.send(line).await.map_err(|_| anyhow!("app-server stdin closed"))
    }

    /// One request, awaited with a timeout (a dead child must not leak
    /// promises).
    pub async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        match tokio::time::timeout(timeout, async {
            self.write(frame).await?;
            Ok::<_, anyhow::Error>(rx.await)
        })
        .await
        {
            Ok(Err(e)) => {
                self.pending.lock().unwrap().remove(&id);
                Err(e)
            }
            Ok(Ok(Ok(Ok(v)))) => Ok(v),
            Ok(Ok(Ok(Err(e)))) => Err(anyhow!("{method}: {e}")),
            Ok(Ok(Err(_))) => Err(anyhow!("{method}: app-server closed before responding")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("{method}: timed out after {}s", timeout.as_secs()))
            }
        }
    }

    /// A notification to the server (no response).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write(json!({ "jsonrpc": "2.0", "method": method, "params": params })).await
    }

    /// Answer a server→client request.
    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        self.write(json!({ "jsonrpc": "2.0", "id": id, "result": result })).await
    }

    pub async fn respond_error(&self, id: Value, message: &str) -> Result<()> {
        self.write(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } }))
            .await
    }

    /// Kill the child (idempotent).
    pub fn kill(&self) {
        if let Some(tx) = self.kill_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}
