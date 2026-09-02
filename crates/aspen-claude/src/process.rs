//! The thin native layer: spawn, line I/O, kill. Schema-ignorant by design —
//! this module never inspects the meaning of a frame. That split survived a
//! complete protocol rebuild in the reference implementation; we keep it.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

/// How to launch the `claude` binary. Argv per the verified contract
/// (reference §2.1); cwd IS the project root — no flag sets it.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub claude_bin: String,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub resume: Option<String>,
    pub fork_session: bool,
    /// With resume: truncate to this assistant message (branch from a point).
    pub resume_at: Option<String>,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    /// Stamped as CLAUDE_CODE_ENTRYPOINT — marks every transcript line so hub
    /// sessions are distinguishable on disk forever (reference §2.2).
    pub entrypoint: String,
    pub extra_env: Vec<(String, String)>,
    pub extra_args: Vec<String>,
}

impl SpawnSpec {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            claude_bin: "claude".into(),
            cwd,
            session_id: None,
            resume: None,
            fork_session: false,
            resume_at: None,
            permission_mode: None,
            model: None,
            entrypoint: "aspen".into(),
            extra_env: Vec::new(),
            extra_args: Vec::new(),
        }
    }

    fn argv(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "--print",
            "--verbose", // mandatory with stream-json output; CLI hard-exits without it
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--replay-user-messages",
            // THE FLAG THE DOCS DON'T TELL YOU ABOUT (reference §2.1): without
            // it, can_use_tool never reaches the host.
            "--permission-prompt-tool",
            "stdio",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(id) = &self.resume {
            a.push("-r".into());
            a.push(id.clone());
            if self.fork_session {
                a.push("--fork-session".into());
            }
            if let Some(at) = &self.resume_at {
                a.push("--resume-session-at".into());
                a.push(at.clone());
            }
        } else if let Some(id) = &self.session_id {
            a.push("--session-id".into());
            a.push(id.clone());
        }
        if let Some(m) = &self.permission_mode {
            a.push("--permission-mode".into());
            a.push(m.clone());
        }
        if let Some(m) = &self.model {
            a.push("--model".into());
            a.push(m.clone());
        }
        a.extend(self.extra_args.iter().cloned());
        a
    }
}

/// A running child with line-disciplined channels. Owns nothing about the
/// protocol: outbound is pre-serialized single-line JSON, inbound is raw
/// lines tagged by stream.
pub struct ClaudeProcess {
    pub child: Child,
    pub stdin_tx: mpsc::Sender<String>,
    pub stdout_rx: mpsc::Receiver<String>,
    pub stderr_rx: mpsc::Receiver<String>,
}

pub fn spawn(spec: &SpawnSpec) -> Result<ClaudeProcess> {
    let mut cmd = Command::new(&spec.claude_bin);
    cmd.args(spec.argv())
        .current_dir(&spec.cwd)
        .env("CLAUDE_CODE_ENTRYPOINT", &spec.entrypoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in &spec.extra_env {
        // A UI bug must never be able to blank PATH (reference §2.2): names
        // are validated, empty values refused.
        if !k.is_empty()
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !v.is_empty()
        {
            cmd.env(k, v);
        }
    }
    // Windows: CREATE_NO_WINDOW, or a console flashes (reference §2.2).
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning {} in {}", spec.claude_bin, spec.cwd.display()))?;

    let stdin = child.stdin.take().context("child stdin missing")?;
    let stdout = child.stdout.take().context("child stdout missing")?;
    let stderr = child.stderr.take().context("child stderr missing")?;

    let (stdin_tx, stdin_rx) = mpsc::channel::<String>(256);
    let (stdout_tx, stdout_rx) = mpsc::channel::<String>(1024);
    let (stderr_tx, stderr_rx) = mpsc::channel::<String>(256);

    tokio::spawn(write_loop(stdin, stdin_rx));
    tokio::spawn(read_loop(BufReader::new(stdout), stdout_tx));
    tokio::spawn(read_loop(BufReader::new(stderr), stderr_tx));

    Ok(ClaudeProcess {
        child,
        stdin_tx,
        stdout_rx,
        stderr_rx,
    })
}

/// Stdin discipline (reference §2.3): one complete JSON value per line,
/// `\n`-terminated, UTF-8. A malformed line kills the child with no
/// resynchronization, so this — the last writer before the pipe — enforces.
async fn write_loop(mut stdin: ChildStdin, mut rx: mpsc::Receiver<String>) {
    while let Some(line) = rx.recv().await {
        if let Err(e) = validate_line(&line) {
            // Refuse rather than kill the child. The sender's request will
            // time out, which is loud; a dead child is a catastrophe.
            tracing::error!(error = %e, "refusing to write malformed line to claude stdin");
            continue;
        }
        if stdin.write_all(line.as_bytes()).await.is_err()
            || stdin.write_all(b"\n").await.is_err()
            || stdin.flush().await.is_err()
        {
            tracing::debug!("claude stdin closed; writer exiting");
            return;
        }
    }
    // Channel closed: dropping stdin sends EOF — the clean-shutdown rung.
}

pub fn validate_line(line: &str) -> Result<()> {
    if line.contains('\n') || line.contains('\r') {
        bail!("embedded newline");
    }
    serde_json::from_str::<serde_json::Value>(line).context("not valid JSON")?;
    Ok(())
}

async fn read_loop<R>(reader: BufReader<R>, tx: mpsc::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = reader.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.is_empty() {
                    continue;
                }
                if tx.send(line).await.is_err() {
                    return; // consumer gone
                }
            }
            Ok(None) => return, // EOF
            Err(e) => {
                tracing::debug!(error = %e, "read error on claude pipe");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_discipline_rejects_embedded_newlines() {
        assert!(validate_line("{\"a\":1}\n{\"b\":2}").is_err());
        assert!(validate_line("{\"a\":\"x\ry\"}").is_err());
    }

    #[test]
    fn line_discipline_rejects_non_json() {
        assert!(validate_line("not json").is_err());
        assert!(validate_line("{\"a\":1} {\"b\":2}").is_err()); // two values, one line: fatal to the CLI
    }

    #[test]
    fn line_discipline_accepts_serialized_json() {
        // serde_json escapes newlines inside strings, so serialization is
        // inherently single-line — this asserts that assumption holds.
        let v = serde_json::json!({"text": "line1\nline2"});
        assert!(validate_line(&serde_json::to_string(&v).unwrap()).is_ok());
    }
}
