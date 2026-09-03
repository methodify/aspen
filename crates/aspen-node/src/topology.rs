//! Topology: endpoints, links, and an agent's neighborhood.
//!
//! An **endpoint** is one of: an agent (`agent:arch@nl[@node]`), a repo
//! (`repo:nl[@node]` — whoever is working in it, resolved live), a node
//! (`node:alpha` — every agent on it), or the operator. A **link** is a
//! declared pathway between two endpoints with a direction and a purpose —
//! the purpose is prose the agents on the `from` side are told, so wiring
//! the mesh also explains it. Channels remain rooms (fan-out).
//!
//! Visibility follows topology: an agent's `bus_status` leads with its
//! neighborhood (repo-mates, links, channels), and in *closed* topology a
//! send outside it is refused. In *open* topology (default) it still
//! delivers, with a note.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use crate::node::NodeInner;
use crate::store::Link;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Agent {
        key: String,
        node: Option<String>,
    },
    Repo {
        handle: String,
        node: Option<String>,
    },
    Node(String),
    Operator,
}

impl Endpoint {
    /// Parse the stored/wire form: `agent:…`, `repo:…`, `node:…`, `operator`.
    /// Bare forms are accepted for convenience: `@x` / `x@y` → agent,
    /// `#x` → repo.
    pub fn parse(s: &str) -> Result<Endpoint, String> {
        let s = s.trim();
        if s == "operator" || s == "@operator" {
            return Ok(Endpoint::Operator);
        }
        if let Some(rest) = s.strip_prefix("agent:") {
            let a = crate::addr::Addr::parse(rest)?;
            let key = a
                .local_key()
                .ok_or_else(|| format!("agent endpoint needs name@repo, got {rest:?}"))?;
            return Ok(Endpoint::Agent { key, node: a.node });
        }
        if let Some(rest) = s.strip_prefix("repo:") {
            let (h, n) = match rest.split_once('@') {
                Some((h, n)) => (h.to_string(), Some(n.to_string())),
                None => (rest.to_string(), None),
            };
            if h.is_empty() {
                return Err("empty repo handle".into());
            }
            return Ok(Endpoint::Repo { handle: h, node: n });
        }
        if let Some(rest) = s.strip_prefix("node:") {
            if rest.is_empty() {
                return Err("empty node name".into());
            }
            return Ok(Endpoint::Node(rest.to_string()));
        }
        if let Some(rest) = s.strip_prefix('#') {
            return Endpoint::parse(&format!("repo:{rest}"));
        }
        // bare agent address
        Endpoint::parse(&format!("agent:{}", s.trim_start_matches('@')))
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Agent { key, node } => match node {
                Some(n) => write!(f, "agent:{key}@{n}"),
                None => write!(f, "agent:{key}"),
            },
            Endpoint::Repo { handle, node } => match node {
                Some(n) => write!(f, "repo:{handle}@{n}"),
                None => write!(f, "repo:{handle}"),
            },
            Endpoint::Node(n) => write!(f, "node:{n}"),
            Endpoint::Operator => write!(f, "operator"),
        }
    }
}

/// Human form for rosters/charters: `@arch@nl`, `#nl`, `node alpha`, `@operator`.
pub fn human(e: &Endpoint) -> String {
    match e {
        Endpoint::Agent { key, node } => match node {
            Some(n) => format!("@{key}@{n}"),
            None => format!("@{key}"),
        },
        Endpoint::Repo { handle, node } => match node {
            Some(n) => format!("#{handle}@{n}"),
            None => format!("#{handle}"),
        },
        Endpoint::Node(n) => format!("node {n}"),
        Endpoint::Operator => "@operator".into(),
    }
}

fn self_node(inner: &Arc<NodeInner>) -> Option<String> {
    inner.mesh().map(|m| m.identity.node.clone())
}

/// Every agent address this node knows: local keys and remote `key@node`.
fn all_agents(inner: &Arc<NodeInner>) -> Vec<(String, String, Option<String>)> {
    // (address, channel, node) — node None = local
    let mut out: Vec<(String, String, Option<String>)> = inner
        .store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.name, a.channel, None))
        .collect();
    if let Some(mesh) = inner.mesh() {
        for (node, agents) in mesh.remote.lock().unwrap().iter() {
            for a in agents {
                out.push((
                    format!("{}@{}", a.name, node),
                    a.channel.clone(),
                    Some(node.clone()),
                ));
            }
        }
    }
    out
}

/// The agent addresses an endpoint stands for right now. Addresses are
/// local keys for local agents and `key@node` for remote ones.
pub fn members(inner: &Arc<NodeInner>, e: &Endpoint) -> Vec<String> {
    let me = self_node(inner);
    let is_local_node = |n: &Option<String>| n.is_none() || *n == me;
    match e {
        Endpoint::Operator => vec!["operator".into()],
        Endpoint::Agent { key, node } => {
            if is_local_node(node) {
                vec![key.clone()]
            } else {
                vec![format!("{}@{}", key, node.as_deref().unwrap_or(""))]
            }
        }
        Endpoint::Repo { handle, node } => all_agents(inner)
            .into_iter()
            .filter(|(_, ch, n)| {
                ch == handle
                    && match node {
                        None => true,
                        Some(want) => {
                            n.as_deref() == Some(want.as_str())
                                || (n.is_none() && me.as_deref() == Some(want.as_str()))
                        }
                    }
            })
            .map(|(addr, _, _)| addr)
            .collect(),
        Endpoint::Node(name) => all_agents(inner)
            .into_iter()
            .filter(|(_, _, n)| match n {
                None => me.as_deref() == Some(name.as_str()),
                Some(n) => n == name,
            })
            .map(|(addr, _, _)| addr)
            .collect(),
    }
}

/// Does this endpoint contain the agent (a local key)?
pub fn contains(inner: &Arc<NodeInner>, e: &Endpoint, agent: &str) -> bool {
    members(inner, e).iter().any(|m| m == agent)
}

/// One side of an agent's neighborhood: a link and the addresses it reaches.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Reach {
    pub link: Link,
    /// Which end the agent is on ("from" or "to").
    pub side: &'static str,
    /// The other end, human form.
    pub other: String,
    /// Concrete addresses the agent can reach through it.
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Neighborhood {
    pub repo_mates: Vec<String>,
    pub links: Vec<Reach>,
    pub channels: Vec<(String, Vec<String>)>,
}

/// Everything an agent can see/reach by topology.
pub fn neighborhood(inner: &Arc<NodeInner>, agent: &str) -> Neighborhood {
    let my_repo = crate::addr::repo_of(agent).unwrap_or("").to_string();
    let repo_mates: Vec<String> = inner
        .store
        .channel_members(&my_repo)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m != agent)
        .collect();
    let mut links = Vec::new();
    for l in inner.store.links().unwrap_or_default() {
        let (Ok(from), Ok(to)) = (Endpoint::parse(&l.src), Endpoint::parse(&l.dst)) else {
            continue;
        };
        if contains(inner, &from, agent) {
            let targets: Vec<String> = members(inner, &to)
                .into_iter()
                .filter(|t| t != agent)
                .collect();
            links.push(Reach {
                link: l.clone(),
                side: "from",
                other: human(&to),
                targets,
            });
        } else if l.two_way && contains(inner, &to, agent) {
            let targets: Vec<String> = members(inner, &from)
                .into_iter()
                .filter(|t| t != agent)
                .collect();
            links.push(Reach {
                link: l.clone(),
                side: "to",
                other: human(&from),
                targets,
            });
        }
    }
    let mut channels = Vec::new();
    for c in inner.store.channels_of(agent).unwrap_or_default() {
        let members: Vec<String> = inner
            .store
            .custom_channel_members(&c)
            .unwrap_or_default()
            .into_iter()
            .map(|m| crate::tools::canonical_member(&m))
            .filter(|m| m != agent)
            .collect();
        channels.push((c, members));
    }
    Neighborhood {
        repo_mates,
        links,
        channels,
    }
}

/// Is `to` (a resolved recipient) inside `from`'s neighborhood? The operator
/// always is, and replies to whoever messaged you are always allowed (the
/// caller checks that separately, from the trail).
pub fn in_neighborhood(inner: &Arc<NodeInner>, from: &str, to: &str) -> bool {
    if to == "operator" || from == "operator" {
        return true;
    }
    let n = neighborhood(inner, from);
    if n.repo_mates.iter().any(|m| m == to) {
        return true;
    }
    if n.links.iter().any(|r| r.targets.iter().any(|t| t == to)) {
        return true;
    }
    n.channels.iter().any(|(_, ms)| ms.iter().any(|m| m == to))
}

/// The set of link targets for bare-name resolution: addresses reachable
/// through links only.
pub fn link_targets(inner: &Arc<NodeInner>, from: &str) -> BTreeSet<String> {
    neighborhood(inner, from)
        .links
        .into_iter()
        .flat_map(|r| r.targets)
        .collect()
}

/// The paragraph derived from topology for an agent's charter/roster:
/// links are instructions.
pub fn guidance(inner: &Arc<NodeInner>, agent: &str) -> String {
    let n = neighborhood(inner, agent);
    let mut lines = Vec::new();
    if !n.repo_mates.is_empty() {
        lines.push(format!(
            "In your repo with you: {}.",
            n.repo_mates
                .iter()
                .map(|m| format!("@{}", crate::addr::bare(m)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for r in &n.links {
        let arrow = if r.link.two_way {
            "↔"
        } else if r.side == "from" {
            "→"
        } else {
            "←"
        };
        let who = if r.targets.is_empty() {
            format!("{} (nobody there right now)", r.other)
        } else if r.targets.len() == 1 {
            format!("@{}", r.targets[0])
        } else {
            format!(
                "{} ({})",
                r.other,
                r.targets
                    .iter()
                    .map(|t| format!("@{t}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let purpose = r
            .link
            .purpose
            .as_deref()
            .map(|p| format!(" — {p}"))
            .unwrap_or_default();
        let urg = r
            .link
            .urgency
            .as_deref()
            .map(|u| format!(" [default urgency: {u}]"))
            .unwrap_or_default();
        lines.push(format!("Link {arrow} {who}{purpose}{urg}"));
    }
    for (c, ms) in &n.channels {
        lines.push(format!(
            "Channel #{c} with {}.",
            ms.iter()
                .map(|m| format!("@{m}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_forms() {
        assert_eq!(Endpoint::parse("operator").unwrap(), Endpoint::Operator);
        assert_eq!(
            Endpoint::parse("#nl").unwrap(),
            Endpoint::Repo {
                handle: "nl".into(),
                node: None
            }
        );
        assert_eq!(
            Endpoint::parse("repo:nl@beta").unwrap(),
            Endpoint::Repo {
                handle: "nl".into(),
                node: Some("beta".into())
            }
        );
        assert_eq!(
            Endpoint::parse("@arch@nl").unwrap(),
            Endpoint::Agent {
                key: "arch@nl".into(),
                node: None
            }
        );
        assert_eq!(
            Endpoint::parse("agent:arch@nl@beta").unwrap().to_string(),
            "agent:arch@nl@beta"
        );
        assert_eq!(
            Endpoint::parse("node:alpha").unwrap(),
            Endpoint::Node("alpha".into())
        );
        assert!(Endpoint::parse("agent:arch").is_err());
        assert_eq!(human(&Endpoint::parse("repo:nl").unwrap()), "#nl");
    }
}
