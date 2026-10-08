//! Paths and names that arrive from outside — a peer, a bundle file, the
//! console — before they are joined onto a directory. `Path::join` with an
//! absolute path replaces the base, and `..` climbs out of it; both let
//! a crafted name read or write anywhere the daemon can (the 2026-10
//! quality pass found that in bundle import, session moves, bundle reads
//! and transcript lookups).

use std::path::{Component, Path};

/// A relative path made only of ordinary names: no root, drive, `..` or
/// `.`, not empty. Joined onto a directory, it stays inside it.
pub fn safe_rel(rel: &str) -> Result<&Path, String> {
    let p = Path::new(rel);
    if rel.is_empty() || rel.contains('\0') {
        return Err(format!("unsafe path {rel:?}"));
    }
    if p.components().all(|c| matches!(c, Component::Normal(_))) {
        Ok(p)
    } else {
        Err(format!(
            "unsafe path {rel:?}: only plain relative names are allowed"
        ))
    }
}

/// An identifier used as one path name (a session id, a bundle id): ASCII
/// letters, digits, `-` and `_` only, 1–128 long.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths_pass() {
        assert!(safe_rel("memory/notes.md").is_ok());
        assert!(safe_rel("a").is_ok());
        for bad in ["", "/etc/passwd", "../x", "a/../../b", "./a", "a\0b"] {
            assert!(safe_rel(bad).is_err(), "{bad:?} should be refused");
        }
        #[cfg(windows)]
        assert!(safe_rel("C:\\Windows\\x").is_err());
    }

    #[test]
    fn ids_are_single_plain_names() {
        assert!(safe_id("50b5791b-2fd5-4b8a-92f3-191d4cce81d7"));
        assert!(safe_id("bundle_1"));
        for bad in ["", "a/b", "..", "a.b", "x\\y", &"a".repeat(129)] {
            assert!(!safe_id(bad), "{bad:?} should be refused");
        }
    }
}
