//! Agent addresses: `name@repo[@node]`.
//!
//! Agents are named per repo, so `arch` can exist in every project. The
//! repo segment is the repo's *handle* (defaults to the directory basename,
//! operator-renamable, unique per node), which is also the repo channel
//! name. The node segment is only needed when the same handle exists on
//! more than one node (the same clone on two machines).
//!
//! Internally the local key of an agent is `name@repo`; a remote agent is
//! `name@repo@node`. `operator` is global and has no segments.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Addr {
    pub name: String,
    pub repo: Option<String>,
    pub node: Option<String>,
}

impl Addr {
    /// Parse `name`, `name@repo`, or `name@repo@node` (leading `@` allowed).
    /// More than three segments is an error; empty segments are errors.
    pub fn parse(s: &str) -> Result<Addr, String> {
        let s = s.trim().trim_start_matches('@');
        if s.is_empty() {
            return Err("empty address".into());
        }
        let parts: Vec<&str> = s.split('@').collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err(format!("malformed address {s:?}"));
        }
        match parts.as_slice() {
            [n] => Ok(Addr {
                name: n.to_string(),
                repo: None,
                node: None,
            }),
            [n, r] => Ok(Addr {
                name: n.to_string(),
                repo: Some(r.to_string()),
                node: None,
            }),
            [n, r, d] => Ok(Addr {
                name: n.to_string(),
                repo: Some(r.to_string()),
                node: Some(d.to_string()),
            }),
            _ => Err(format!(
                "address {s:?} has too many segments (name@repo@node at most)"
            )),
        }
    }

    pub fn is_operator(&self) -> bool {
        self.name == "operator" && self.repo.is_none()
    }

    /// The local key form `name@repo` (node dropped).
    pub fn local_key(&self) -> Option<String> {
        self.repo.as_ref().map(|r| format!("{}@{}", self.name, r))
    }

    /// The fully qualified form with the given node.
    pub fn with_node(&self, node: &str) -> Addr {
        Addr {
            name: self.name.clone(),
            repo: self.repo.clone(),
            node: Some(node.to_string()),
        }
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(r) = &self.repo {
            write!(f, "@{r}")?;
        }
        if let Some(n) = &self.node {
            write!(f, "@{n}")?;
        }
        Ok(())
    }
}

/// Build a local key from parts.
pub fn local_key(name: &str, repo: &str) -> String {
    format!("{name}@{repo}")
}

/// The bare name of a key/address (`arch@nonlinear@beta` → `arch`).
pub fn bare(key: &str) -> &str {
    key.split('@').next().unwrap_or(key)
}

/// The repo segment of a key/address, if any.
pub fn repo_of(key: &str) -> Option<&str> {
    key.split('@').nth(1)
}

/// The node segment of a fully qualified address, if any.
pub fn node_of(key: &str) -> Option<&str> {
    key.split('@').nth(2)
}

/// Strip a trailing `@node` from a fully qualified address, leaving the
/// local key — what the owning node uses.
pub fn strip_node<'a>(addr: &'a str, node: &str) -> &'a str {
    addr.strip_suffix(&format!("@{node}"))
        .filter(|rest| rest.matches('@').count() == 1)
        .unwrap_or(addr)
}

/// How an address should be shown to a reader: the shortest form that is
/// unambiguous *from where the reader sits*.
///
/// - same repo (and node) as the reader → `name`
/// - another repo → `name@repo`
/// - the repo handle exists on several nodes (`ambiguous_repo`) → full
///
/// A reader with no repo (the operator console) sees `name@repo`, plus the
/// node when the handle is ambiguous.
pub fn display_for(
    addr: &str,
    reader_repo: Option<&str>,
    self_node: &str,
    ambiguous_repo: bool,
) -> String {
    let Ok(a) = Addr::parse(addr) else {
        return addr.to_string();
    };
    if a.is_operator() {
        return "operator".into();
    }
    let Some(repo) = &a.repo else {
        return a.name;
    };
    let node = a.node.as_deref().unwrap_or(self_node);
    let same_node = node == self_node;
    if same_node && reader_repo == Some(repo.as_str()) {
        return a.name;
    }
    if ambiguous_repo || !same_node {
        return format!("{}@{}@{}", a.name, repo, node);
    }
    format!("{}@{}", a.name, repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_forms() {
        assert_eq!(Addr::parse("arch").unwrap().to_string(), "arch");
        assert_eq!(Addr::parse("@arch@nl").unwrap().to_string(), "arch@nl");
        let a = Addr::parse("arch@nl@beta").unwrap();
        assert_eq!(a.node.as_deref(), Some("beta"));
        assert_eq!(a.local_key().as_deref(), Some("arch@nl"));
        assert!(Addr::parse("a@b@c@d").is_err());
        assert!(Addr::parse("a@@b").is_err());
        assert!(Addr::parse("").is_err());
    }

    #[test]
    fn segment_helpers() {
        assert_eq!(bare("arch@nl@beta"), "arch");
        assert_eq!(repo_of("arch@nl@beta"), Some("nl"));
        assert_eq!(node_of("arch@nl@beta"), Some("beta"));
        assert_eq!(node_of("arch@nl"), None);
        assert_eq!(strip_node("arch@nl@beta", "beta"), "arch@nl");
        assert_eq!(strip_node("arch@nl", "beta"), "arch@nl");
    }

    #[test]
    fn display_relative_to_reader() {
        assert_eq!(display_for("arch@nl", Some("nl"), "alpha", false), "arch");
        assert_eq!(
            display_for("arch@nl", Some("other"), "alpha", false),
            "arch@nl"
        );
        assert_eq!(display_for("arch@nl", None, "alpha", false), "arch@nl");
        assert_eq!(
            display_for("arch@nl@beta", Some("nl"), "alpha", false),
            "arch@nl@beta"
        );
        assert_eq!(display_for("arch@nl", None, "alpha", true), "arch@nl@alpha");
        assert_eq!(display_for("operator", None, "alpha", false), "operator");
    }
}
