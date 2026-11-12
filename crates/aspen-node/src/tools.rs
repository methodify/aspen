//! The bus tools served to agents over the in-process MCP path.
//!
//! Tool descriptions ARE the contract — guidance delivered at the point of
//! use, traveling with the session (plumb's lesson). They also have to earn
//! their load: on current builds SDK-served tools arrive deferred and the
//! model reads the description to decide whether to load them.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::node::{NodeInner, TurnState};

const BUS_SEND_DESC: &str = "Send a message to a peer agent, a repo channel, or the human operator \
on the aspen bus. `to` is '@name' (a peer in your repo), '@name@repo' (a peer in another repo; \
add '@node' only if that repo exists on several nodes), '#channel', or '@operator'. `urgency` is delivery timing, \
nothing else: 'gating' interrupts the recipient mid-turn; 'normal' (default) is delivered now \
if they are idle (waking them) or at their turn boundary if they are busy; 'notice' is an \
ambient fact that never wakes or interrupts anyone. A peer who is not running receives at \
their next session start. Delivery always carries everything pending in send order. There is \
no subject line; put everything in body. Silence from a peer never means your message was \
lost — check bus_status before concluding anything from silence.";
const BUS_STATUS_DESC: &str = "Who is on this bus: every agent, their repo channel, whether their \
session is running, whether they are mid-turn or idle, and how many messages are pending for \
them. Check here when a peer seems unresponsive before concluding a message was lost — \
silence usually means mid-turn or not running; it never means the bus dropped something.";
const BUS_INBOX_DESC: &str =
    "Read messages addressed to you that have not been delivered yet. You \
rarely need this: gating messages interrupt you and everything else arrives at your turn \
boundaries. Reach for it to drain deliberately — e.g. before reporting status, so you are not \
reporting against a ruling you have not read.";

const PROJECT_LIST_DESC: &str = "The operator's projects you are on: each one's name, status, \
charter (its start) and team size. A project is a team the operator put together for one piece \
of work; its charter (in your instructions too) says what it is for.";
const PROJECT_GET_DESC: &str =
    "One of your projects in full: its charter, the team (each member's \
role from its own charter, node and state: running, idle, stopped or benched), the decisions \
pinned so far, where its shared files are, and its files.";
const PROJECT_POST_DESC: &str = "Post once to everyone else on a project, on the project's thread \
(instead of messaging each member). Delivered like bus_send: 'normal' (default) wakes an idle \
member, 'notice' rides along with their next delivery. kind 'decision' also pins the post as a \
decision: it heads the project's activity and every new member's briefing. Use it for what the \
team settled, not for progress.";
const PROJECT_FILES_DESC: &str = "List a project's shared files: path, size, who wrote it, when. \
The same files are a folder linked into your repo (.aspen/projects/<name>/, kept out of git); \
your own file tools work there too, and what you change there is shared within seconds.";
const PROJECT_READ_DESC: &str = "Read a project's shared file (text). `version` reads an earlier \
version by its sha (project_get lists none; the operator's console shows a file's history).";
const PROJECT_WRITE_DESC: &str = "Write (create or replace) a project's shared file, text. Every \
member's node gets it, and the others are told (without being woken). Two writes to one path \
made apart are both kept: the later one keeps the name, the other becomes `name (from @x).ext`.";
const PROJECT_DELETE_DESC: &str = "Delete a project's shared file (its history is kept for the \
operator to restore).";

fn project_arg() -> Value {
    json!({ "type": "string", "description": "The project's name or id; leave it out when you are on just one." })
}

fn bus_send_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "to": { "type": "string", "description": "'@agent', '#channel', or '@operator'. bus_status lists who exists." },
            "body": { "type": "string", "description": "The whole message." },
            "urgency": { "type": "string", "enum": ["gating", "normal", "notice"], "default": "normal" },
            "thread": { "type": "string", "description": "Optional thread id grouping an exchange; no effect on delivery." },
            "record": { "type": "string", "description": "Durable record this is about (issue id, doc path), if any — the message is the notification; the record is where it lives." }
        },
        "required": ["to", "body"]
    })
}

/// The bus tools as a harness-neutral provider (aspen-core `ToolProvider`):
/// Claude mounts it as an in-process MCP server, Codex reaches it through
/// the `aspen mcp` stdio bridge.
pub struct BusTools {
    inner: Arc<NodeInner>,
    me: String,
}

pub fn build_tools(inner: Arc<NodeInner>, me: String) -> Arc<dyn aspen_core::ToolProvider> {
    Arc::new(BusTools { inner, me })
}

impl aspen_core::ToolProvider for BusTools {
    fn list(&self) -> Vec<aspen_core::ToolDef> {
        vec![
            aspen_core::ToolDef {
                name: "bus_send",
                description: BUS_SEND_DESC.into(),
                input_schema: bus_send_schema(),
            },
            aspen_core::ToolDef {
                name: "bus_status",
                description: BUS_STATUS_DESC.into(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            aspen_core::ToolDef {
                name: "bus_inbox",
                description: BUS_INBOX_DESC.into(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            aspen_core::ToolDef {
                name: "project_list",
                description: PROJECT_LIST_DESC.into(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            aspen_core::ToolDef {
                name: "project_get",
                description: PROJECT_GET_DESC.into(),
                input_schema: json!({ "type": "object", "properties": { "project": project_arg() } }),
            },
            aspen_core::ToolDef {
                name: "project_post",
                description: PROJECT_POST_DESC.into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "project": project_arg(),
                        "body": { "type": "string", "description": "The whole post." },
                        "kind": { "type": "string", "enum": ["message", "decision"], "default": "message" },
                        "urgency": { "type": "string", "enum": ["normal", "notice"], "default": "normal" }
                    },
                    "required": ["body"]
                }),
            },
            aspen_core::ToolDef {
                name: "project_files",
                description: PROJECT_FILES_DESC.into(),
                input_schema: json!({ "type": "object", "properties": { "project": project_arg() } }),
            },
            aspen_core::ToolDef {
                name: "project_read",
                description: PROJECT_READ_DESC.into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "project": project_arg(),
                        "path": { "type": "string", "description": "Relative to the project's files, e.g. 'specs/api.md'." },
                        "version": { "type": "string", "description": "An earlier version's sha." }
                    },
                    "required": ["path"]
                }),
            },
            aspen_core::ToolDef {
                name: "project_write",
                description: PROJECT_WRITE_DESC.into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "project": project_arg(),
                        "path": { "type": "string", "description": "Relative, e.g. 'specs/api.md'; folders are made as needed." },
                        "content": { "type": "string", "description": "The whole file." }
                    },
                    "required": ["path", "content"]
                }),
            },
            aspen_core::ToolDef {
                name: "project_delete",
                description: PROJECT_DELETE_DESC.into(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "project": project_arg(), "path": { "type": "string" } },
                    "required": ["path"]
                }),
            },
        ]
    }
    fn call(&self, name: &str, args: Value) -> std::result::Result<String, String> {
        match name {
            "bus_send" => bus_send(&self.inner, &self.me, args),
            "bus_status" => bus_status(&self.inner, &self.me),
            "bus_inbox" => bus_inbox(&self.inner, &self.me),
            "project_list" => project_list(&self.inner, &self.me),
            "project_get" => project_get(&self.inner, &self.me, &args),
            "project_post" => {
                let b = pick(&self.inner, &self.me, &args)?;
                let body = args
                    .get("body")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'body'")?;
                let urgency = args
                    .get("urgency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("normal");
                if urgency == "gating" {
                    return Err(
                        "a project post is normal or notice; bus_send interrupts one peer".into(),
                    );
                }
                let decision = args.get("kind").and_then(|v| v.as_str()) == Some("decision");
                crate::projects::post(&self.inner, &self.me, &b, body, urgency, decision)
                    .map(|n| n.join("\n"))
            }
            "project_files" => project_files(&self.inner, &self.me, &args),
            "project_read" => project_read(&self.inner, &self.me, &args),
            "project_write" => {
                let b = pick(&self.inner, &self.me, &args)?;
                let path = str_arg(&args, "path")?;
                let content = str_arg(&args, "content")?;
                let e =
                    crate::projfiles::write(&self.inner, &b.id, path, content.as_bytes(), &self.me)
                        .map_err(|e| e.to_string())?;
                crate::federation::broadcast_roster(&self.inner);
                Ok(format!(
                    "Wrote {} ({} bytes) in \u{201c}{}\u{201d}; the team is told.",
                    e.path, e.size, b.name
                ))
            }
            "project_delete" => {
                let b = pick(&self.inner, &self.me, &args)?;
                let path = str_arg(&args, "path")?;
                crate::projfiles::delete(&self.inner, &b.id, path, &self.me)
                    .map_err(|e| e.to_string())?;
                crate::federation::broadcast_roster(&self.inner);
                Ok(format!("Deleted {path} in \u{201c}{}\u{201d}.", b.name))
            }
            other => Err(format!("unknown tool {other:?}")),
        }
    }
}

fn str_arg<'a>(args: &'a Value, k: &str) -> Result<&'a str, String> {
    args.get(k)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing '{k}'"))
}

fn pick(inner: &Arc<NodeInner>, me: &str, args: &Value) -> Result<crate::store::Board, String> {
    crate::projects::pick(inner, me, args.get("project").and_then(|v| v.as_str()))
}

fn size_text(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn project_list(inner: &Arc<NodeInner>, me: &str) -> Result<String, String> {
    let mine = crate::projects::of_agent(inner, me);
    if mine.is_empty() {
        return Ok("You are on no project.".into());
    }
    let mut out = Vec::new();
    for b in mine {
        let team = crate::topology::board_panes(inner, &b).len();
        let charter = crate::projects::charter(&b)
            .map(|c| {
                let short: String = c.chars().take(200).collect();
                format!(
                    "\n  charter: {}{}",
                    short.replace('\n', " "),
                    if c.chars().count() > 200 { "…" } else { "" }
                )
            })
            .unwrap_or_default();
        out.push(format!(
            "\u{201c}{}\u{201d} (id {}) — {}, {} on the team{}",
            b.name,
            b.id,
            crate::projects::status(&b),
            team,
            charter
        ));
    }
    Ok(out.join("\n"))
}

fn project_get(inner: &Arc<NodeInner>, me: &str, args: &Value) -> Result<String, String> {
    let b = pick(inner, me, args)?;
    let mut out = vec![format!(
        "Project \u{201c}{}\u{201d} (id {}), {}.",
        b.name,
        b.id,
        crate::projects::status(&b)
    )];
    match crate::projects::charter(&b) {
        Some(c) => out.push(format!("\nCharter:\n{c}")),
        None => out.push("\nNo charter.".into()),
    }
    out.push("\nTeam:".into());
    for m in crate::projects::team(inner, &b) {
        let agent = m["agent"].as_str().unwrap_or("?");
        let you = if agent == me { " (you)" } else { "" };
        let role = m["role"]
            .as_str()
            .map(|r| format!(" — {r}"))
            .unwrap_or_default();
        let state = if m["benched"].as_bool() == Some(true) {
            "benched".to_owned()
        } else {
            m["state"].as_str().unwrap_or("?").to_owned()
        };
        out.push(format!(
            "- @{agent}{you}{role} [{}, {state}]",
            m["node"].as_str().unwrap_or("?")
        ));
    }
    let decisions = crate::projects::decisions(&b);
    if !decisions.is_empty() {
        out.push("\nDecisions:".into());
        for d in decisions.iter().rev().take(20) {
            out.push(format!(
                "- {} (@{})",
                d["text"].as_str().unwrap_or(""),
                crate::addr::bare(d["by"].as_str().unwrap_or("?"))
            ));
        }
    }
    if let Some(f) = crate::projfiles::folder_for(inner, me, &b) {
        out.push(format!("\nShared files: {f}"));
    }
    let files = crate::projfiles::listing(inner, &b.id);
    if !files.is_empty() {
        out.push(format!(
            "{} file(s); project_files lists them.",
            files.len()
        ));
    }
    Ok(out.join("\n"))
}

fn project_files(inner: &Arc<NodeInner>, me: &str, args: &Value) -> Result<String, String> {
    let b = pick(inner, me, args)?;
    let files = crate::projfiles::listing(inner, &b.id);
    let at = crate::projfiles::folder_for(inner, me, &b)
        .map(|f| format!(" (folder: {f})"))
        .unwrap_or_default();
    if files.is_empty() {
        return Ok(format!("\u{201c}{}\u{201d} has no files yet{at}.", b.name));
    }
    let mut out = vec![format!("Files of \u{201c}{}\u{201d}{at}:", b.name)];
    for e in files {
        let both = if e.conflict { " [both kept]" } else { "" };
        out.push(format!(
            "- {} — {} — {}{both}",
            e.path,
            size_text(e.size),
            if e.by.starts_with('(') || e.by == "operator" {
                e.by.clone()
            } else {
                format!("@{}", crate::addr::bare(&e.by))
            }
        ));
    }
    Ok(out.join("\n"))
}

fn project_read(inner: &Arc<NodeInner>, me: &str, args: &Value) -> Result<String, String> {
    let b = pick(inner, me, args)?;
    let path = crate::projfiles::clean_path(str_arg(args, "path")?).map_err(|e| e.to_string())?;
    let e = inner
        .store
        .file_entry(&b.id, &path)
        .map_err(|e| e.to_string())?
        .filter(|e| !e.deleted)
        .ok_or_else(|| {
            format!(
                "no file {path:?} in \u{201c}{}\u{201d}; project_files lists them",
                b.name
            )
        })?;
    let sha = match args.get("version").and_then(|v| v.as_str()) {
        Some(v) if v == e.sha || e.history.iter().any(|h| h.sha == v) => v.to_owned(),
        Some(v) => return Err(format!("{path:?} has no version {v}")),
        None => e.sha.clone(),
    };
    if !crate::projfiles::has_blob(inner, &sha) {
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            let inner = inner.clone();
            let node = e.node.clone();
            let sha = sha.clone();
            h.spawn(async move {
                let _ = crate::projfiles::fetch_blob(&inner, &sha, Some(&node)).await;
            });
        }
        return Err(format!(
            "{path} is not on this node yet (written on {}); fetching it now — read again in a few seconds",
            e.node
        ));
    }
    let bytes = crate::projfiles::read_blob(inner, &sha).map_err(|e| e.to_string())?;
    match crate::projfiles::as_text(&bytes) {
        Some(t) => Ok(t.to_owned()),
        None => Err(format!(
            "{path} is binary ({}); open it in the folder{}",
            size_text(bytes.len() as u64),
            crate::projfiles::folder_for(inner, me, &b)
                .map(|f| format!(" {f}"))
                .unwrap_or_default()
        )),
    }
}

fn bus_send(inner: &Arc<NodeInner>, me: &str, args: Value) -> Result<String, String> {
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or("missing 'to'")?
        .trim()
        .to_owned();
    let body = args
        .get("body")
        .and_then(|v| v.as_str())
        .ok_or("missing 'body'")?;
    let urgency = args
        .get("urgency")
        .and_then(|v| v.as_str())
        .unwrap_or("normal");
    let thread = args.get("thread").and_then(|v| v.as_str());
    let record = args.get("record").and_then(|v| v.as_str());
    let notes = send_message(inner, me, &to, body, urgency, thread, record)?;
    Ok(format!("Sent ({urgency}) to {}.\n{}", to, notes.join("\n")))
}

/// The one send path, shared by the agent-facing MCP tool and the operator
/// API. Returns per-recipient delivery notes.
pub fn send_message(
    inner: &Arc<NodeInner>,
    from: &str,
    to: &str,
    body: &str,
    urgency: &str,
    thread: Option<&str>,
    record: Option<&str>,
) -> Result<Vec<String>, String> {
    if body.trim().is_empty() {
        return Err("refusing to send an empty message".into());
    }
    if !["gating", "normal", "notice"].contains(&urgency) {
        return Err(format!(
            "urgency must be gating|normal|notice, not {urgency:?}"
        ));
    }

    let mut topology_note: Option<String> = None;
    // Resolve the address to concrete recipients.
    let recipients: Vec<String> = if to == "@operator" {
        vec!["operator".into()]
    } else if let Some(name) = to.strip_prefix('@') {
        let target = resolve_agent(inner, from, name)?;
        // Topology: inside the sender's neighborhood, or a reply to someone
        // who messaged them — fine. Outside it: note (open) or refuse (closed).
        if from != "operator"
            && target != "operator"
            && !crate::topology::in_neighborhood(inner, from, &target)
            && !inner.store.has_messaged(&target, from).unwrap_or(false)
        {
            let closed = inner
                .data_dir
                .as_deref()
                .map(crate::settings::load)
                .and_then(|s| s.topology)
                .as_deref()
                == Some("closed");
            if closed {
                return Err(format!(
                    "no declared link between you and {target} (topology is closed) — ask the operator to link you, or use a shared channel"
                ));
            }
            topology_note = Some(format!(
                "note: {target} is outside your declared links/channels — delivered anyway (open topology); the operator can declare a link to make this a standing pathway."
            ));
        }
        vec![target]
    } else if let Some(channel) = to.strip_prefix('#') {
        let mut members: Vec<String> = Vec::new();
        // A custom channel carries explicit members (which may span repos
        // and nodes, and may include @operator); a bare repo name falls
        // back to that repo's auto-membership.
        if inner.store.channel_exists(channel).unwrap_or(false) {
            for m in inner
                .store
                .custom_channel_members(channel)
                .map_err(|e| e.to_string())?
            {
                members.push(canonical_member(&m));
            }
        } else {
            members = inner
                .store
                .channel_members(channel)
                .map_err(|e| e.to_string())?;
            if let Some(mesh) = inner.mesh() {
                for (name, _node) in mesh.remote_channel_members(channel) {
                    if !members.contains(&name) {
                        members.push(name);
                    }
                }
            }
        }
        let others: Vec<String> = members.into_iter().filter(|m| m != from).collect();
        if others.is_empty() {
            return Err(format!(
                "channel #{channel} has no members besides you — bus_status lists who exists"
            ));
        }
        others
    } else {
        return Err(format!(
            "'to' must be '@agent', '#channel', or '@operator', not {to:?}"
        ));
    };

    // One post id groups a fan-out (channel → N recipients) into a single
    // logical message for conversation views.
    let post = uuid::Uuid::new_v4().to_string();
    let mut out = Vec::new();
    if recipients.iter().any(|r| r == "operator") && from != "operator" {
        crate::notify::raise(
            inner,
            from,
            "inbox",
            &format!("@{from} wrote to you"),
            Some(&body.chars().take(200).collect::<String>()),
            Some("/"),
        );
    }
    for recipient in &recipients {
        inner
            .store
            .insert_message(
                from,
                recipient,
                to,
                urgency,
                body,
                thread,
                record,
                Some(&post),
            )
            .map_err(|e| e.to_string())?;
        inner.tick_delivery(recipient);
        out.push(delivery_note(inner, recipient, urgency));
    }
    if let Some(n) = topology_note {
        out.push(n);
    }
    Ok(out)
}

/// The self node's mesh name, if in a mesh.
fn self_node(inner: &Arc<NodeInner>) -> Option<String> {
    inner.mesh().map(|m| m.identity.node.clone())
}

/// Every agent this node knows about, as (key, node) — local agents with
/// node = None, roster-known remote agents with their node.
fn known_agents(inner: &Arc<NodeInner>) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = inner
        .store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.name, None))
        .collect();
    if let Some(mesh) = inner.mesh() {
        for (node, agents) in mesh.remote.lock().unwrap().iter() {
            for a in agents {
                out.push((a.name.clone(), Some(node.clone())));
            }
        }
    }
    out
}

/// Resolve an agent address from the sender's context to a recipient key:
/// a local key (`arch@nl`) or a fully qualified remote one (`arch@nl@beta`).
///
/// - `name@repo@node`: taken literally (local if node is us).
/// - `name@repo`: local if such an agent (or at least the repo) is here;
///   else the one node whose roster has it; ambiguous → error.
/// - `name`: the sender's own repo first; else the single agent of that
///   name anywhere; else the single one sharing a custom channel with the
///   sender; else refused with the candidates — never a silent far guess.
pub fn resolve_agent(inner: &Arc<NodeInner>, from: &str, to: &str) -> Result<String, String> {
    let a = crate::addr::Addr::parse(to)?;
    if a.is_operator() {
        return Ok("operator".into());
    }
    let me = self_node(inner);
    if let (Some(repo), Some(node)) = (&a.repo, &a.node) {
        return Ok(if me.as_deref() == Some(node.as_str()) {
            crate::addr::local_key(&a.name, repo)
        } else {
            format!("{}@{}@{}", a.name, repo, node)
        });
    }
    let known = known_agents(inner);
    if let Some(repo) = &a.repo {
        let key = crate::addr::local_key(&a.name, repo);
        if known.iter().any(|(k, n)| k == &key && n.is_none()) {
            return Ok(key);
        }
        let remote: Vec<&String> = known
            .iter()
            .filter(|(k, n)| k == &key && n.is_some())
            .map(|(_, n)| n.as_ref().unwrap())
            .collect();
        return match remote.as_slice() {
            [node] => Ok(format!("{key}@{node}")),
            [] => {
                // The repo is here but the agent isn't registered yet — let
                // the message wait for it, as bare names always could.
                let repo_here = inner
                    .store
                    .repos()
                    .unwrap_or_default()
                    .iter()
                    .any(|r| r.handle == *repo);
                if repo_here {
                    Ok(key)
                } else {
                    Err(format!(
                        "no agent {key} on this node or any linked node — bus_status lists who exists"
                    ))
                }
            }
            many => Err(format!(
                "{key} exists on several nodes ({}) — say which: {key}@<node>",
                many.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
    // Bare name: the sender's own repo first.
    if let Some(my_repo) = crate::addr::repo_of(from) {
        let key = crate::addr::local_key(&a.name, my_repo);
        if known.iter().any(|(k, n)| k == &key && n.is_none()) {
            return Ok(key);
        }
    }
    // Then a single match reachable through the sender's links — the
    // hotline case: `triage` from alpha resolves via alpha→triage@nl.
    {
        let via_links: Vec<String> = crate::topology::link_targets(inner, from)
            .into_iter()
            .filter(|t| crate::addr::bare(t) == a.name)
            .collect();
        if let [one] = via_links.as_slice() {
            return Ok(one.clone());
        }
    }
    let matches: Vec<&(String, Option<String>)> = known
        .iter()
        .filter(|(k, _)| crate::addr::bare(k) == a.name)
        .collect();
    let qualify = |(k, n): &(String, Option<String>)| match n {
        Some(node) => format!("{k}@{node}"),
        None => k.clone(),
    };
    match matches.as_slice() {
        [] => Err(format!(
            "no agent named '{}' anywhere on the bus — bus_status lists who exists",
            a.name
        )),
        [one] => Ok(qualify(one)),
        many => {
            // Disambiguate through shared custom channels with the sender.
            let mine: std::collections::HashSet<String> = inner
                .store
                .channels_of(from)
                .unwrap_or_default()
                .into_iter()
                .collect();
            let shared: Vec<String> = many
                .iter()
                .filter(|(k, n)| {
                    let full = match n {
                        Some(node) => format!("{k}@{node}"),
                        None => k.clone(),
                    };
                    inner
                        .store
                        .channels_of(&full)
                        .unwrap_or_default()
                        .iter()
                        .any(|c| mine.contains(c))
                })
                .map(|m| qualify(m))
                .collect();
            if let [one] = shared.as_slice() {
                return Ok(one.clone());
            }
            Err(format!(
                "'{}' is ambiguous — choose one of: {}",
                a.name,
                many.iter()
                    .map(|m| qualify(m))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

/// Canonicalize a channel-member address to a recipient key: `@operator`
/// and `operator` → "operator"; `@name` → "name"; `name@node` kept as-is
/// (the delivery engine forwards it).
pub fn canonical_member(addr: &str) -> String {
    let a = addr.trim();
    if a == "@operator" || a == "operator" {
        return "operator".into();
    }
    a.strip_prefix('@').unwrap_or(a).to_owned()
}

/// What the sender is told about how their message will actually land — the
/// signal no discipline ever gave them, at the moment they can act on it.
fn delivery_note(inner: &Arc<NodeInner>, recipient: &str, urgency: &str) -> String {
    if recipient == "operator" {
        return "→ @operator: lands in the operator inbox; the operator reads it on their own schedule.".into();
    }
    match inner.live(recipient) {
        None => {
            // Homed on a peer node? Say what will actually happen. Both
            // node-qualified (`name@node`) and roster-known bare names.
            if let Some(mesh) = inner.mesh() {
                // `key@node` names its home outright; a bare key may be
                // homed on a peer whose roster lists it.
                let found = match crate::addr::node_of(recipient) {
                    Some(node) => {
                        let key = crate::addr::strip_node(recipient, node);
                        let ra = mesh
                            .remote
                            .lock()
                            .unwrap()
                            .get(node)
                            .and_then(|v| v.iter().find(|a| a.name == key).cloned());
                        Some((
                            node.to_owned(),
                            ra.unwrap_or(crate::federation::RemoteAgent {
                                name: key.to_owned(),
                                channel: String::new(),
                                live: false,
                                turn_state: None,
                                summary: None,
                                title: None,
                                activities: None,
                                harness: Default::default(),
                                mcp: None,
                                plugins: None,
                                plugin_updates: None,
                                last_active: None,
                                effort: None,
                                role: None,
                            }),
                        ))
                    }
                    None => mesh.find_remote(recipient),
                };
                if let Some((node, ra)) = found {
                    return if mesh.link_up(&node) {
                        let state = if ra.live {
                            ra.turn_state.as_deref().unwrap_or("running").to_owned()
                        } else {
                            "not running there either".to_owned()
                        };
                        format!(
                            "→ @{recipient}: homed on node '{node}' ({state}) — forwarding \
                             over the mesh now; their node takes it from there."
                        )
                    } else {
                        format!(
                            "→ @{recipient}: homed on node '{node}', but that node is \
                             UNREACHABLE right now — held here and forwarded when the link \
                             returns."
                        )
                    };
                }
            }
            format!(
                "→ @{recipient}: NOT RUNNING — nothing can wake them; this is held and arrives at \
                 their next session start. If it needs them now, tell the operator."
            )
        }
        Some(sess) => match (sess.turn_state(), urgency) {
            (_, "notice") => format!(
                "→ @{recipient}: notice recorded; it rides along with their next delivery or turn."
            ),
            (TurnState::Idle, _) => format!("→ @{recipient}: idle — delivered now (this wakes them)."),
            (TurnState::Busy, "gating") => {
                format!("→ @{recipient}: mid-turn — interrupting them now.")
            }
            (TurnState::Busy, _) => format!(
                "→ @{recipient}: mid-turn — arrives when their current turn ends, which can take a while."
            ),
        },
    }
}

fn bus_status(inner: &Arc<NodeInner>, me: &str) -> Result<String, String> {
    let agents = inner.store.agents().map_err(|e| e.to_string())?;
    if agents.is_empty() {
        return Ok("No agents registered on this bus yet.".into());
    }
    let my_repo = crate::addr::repo_of(me);
    let self_node = self_node(inner).unwrap_or_else(|| "local".into());
    let mut lines = vec![format!("You are {me}.")];
    // Topology first: this is who you're wired to, and why.
    let guidance = crate::topology::guidance(inner, me);
    if !guidance.is_empty() {
        lines.push("Your neighborhood (declared by the operator):".into());
        for g in guidance.lines() {
            lines.push(format!("  {g}"));
        }
    }
    let neighborhood = crate::topology::neighborhood(inner, me);
    let near: std::collections::HashSet<String> = neighborhood
        .repo_mates
        .iter()
        .cloned()
        .chain(neighborhood.links.iter().flat_map(|r| r.targets.clone()))
        .chain(neighborhood.channels.iter().flat_map(|(_, m)| m.clone()))
        .chain(neighborhood.boards.iter().flat_map(|(_, m, _)| m.clone()))
        .collect();
    lines.push(
        "Everyone on the bus (addresses as you should write them; * = in your neighborhood):"
            .into(),
    );
    for a in agents {
        let state = match inner.live(&a.name) {
            None => "not running".to_owned(),
            Some(s) => match s.turn_state() {
                TurnState::Idle => "running, idle".into(),
                TurnState::Busy => "running, mid-turn".into(),
            },
        };
        let pending = inner.store.pending_count(&a.name).unwrap_or(0);
        let shown = crate::addr::display_for(&a.name, my_repo, &self_node, false);
        let star = if near.contains(&a.name) { "*" } else { "" };
        let mut line = if a.name == me {
            format!("  {shown} (you) — #{}", a.channel)
        } else {
            format!("  {shown}{star} — #{} — {}", a.channel, state)
        };
        if pending > 0 && a.name != me {
            line.push_str(&format!(", {pending} pending"));
        }
        lines.push(line);
    }
    if let Some(mesh) = inner.mesh() {
        for (node, agents) in mesh.remote_snapshot() {
            let node = &node;
            let reachable = mesh.link_up(node);
            for a in agents {
                let state = if !reachable {
                    "node unreachable".to_owned()
                } else if a.live {
                    format!(
                        "running, {}",
                        a.turn_state.as_deref().unwrap_or("state unknown")
                    )
                } else {
                    "not running".to_owned()
                };
                let pending = inner
                    .store
                    .pending_count(&format!("{}@{}", a.name, node))
                    .unwrap_or(0);
                let star = if near.contains(&format!("{}@{}", a.name, node)) {
                    "*"
                } else {
                    ""
                };
                let mut line = format!(
                    "  {}@{}{star} — #{} — on node '{}' — {}",
                    a.name, node, a.channel, node, state
                );
                if pending > 0 {
                    line.push_str(&format!(", {pending} pending here"));
                }
                lines.push(line);
            }
        }
    }
    let op_pending = inner.store.pending_count("operator").unwrap_or(0);
    lines.push(format!(
        "  @operator — the human — {op_pending} unread in their inbox"
    ));
    Ok(lines.join("\n"))
}

fn bus_inbox(inner: &Arc<NodeInner>, me: &str) -> Result<String, String> {
    let pending = inner.store.pending_for(me).map_err(|e| e.to_string())?;
    if pending.is_empty() {
        return Ok("No undelivered messages.".into());
    }
    let text = crate::delivery::compose(&pending);
    let ids: Vec<i64> = pending.iter().map(|m| m.id).collect();
    inner
        .store
        .mark_delivered(&ids, "inbox", None)
        .map_err(|e| e.to_string())?;
    Ok(text)
}
