//! Library-spec fetching (prototype, #79 / distribution design).
//!
//! A `use "git+<url>@<ref>#<path>"` coordinate names a library spec hosted in a git repo. This
//! module dereferences the coordinate: it resolves the ref to a commit SHA (immutable identity),
//! fetches the repo into a local cache under `.allium/` in the working directory ONCE, and returns
//! the path to the cached spec file. A lockfile pins each coordinate to its resolved SHA, and a
//! later resolve that returns a different SHA is refused — the day-one trust floor from the
//! distribution design (content-hash pinning; no signatures/transparency log yet).
//!
//! The cache lives in the working directory on purpose: the fetched spec is a plain file an agent
//! (or a human) can open and read, not hidden in a system cache.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A parsed git coordinate: where the repo is, which ref, and the file within it.
pub struct GitCoord {
    pub url: String,
    pub reference: String,
    pub path: String,
    pub raw: String,
}

/// Parse `git+<url>@<ref>#<path>`. Returns `None` for anything that is not a git coordinate
/// (e.g. a plain relative path), so the caller falls back to filesystem resolution.
///
/// Split from the right: `#` separates the in-repo path, then the last `@` separates the ref.
/// Splitting the ref from the right keeps `git@host:org/repo` SSH URLs intact.
pub fn parse_git_coord(target: &str) -> Option<GitCoord> {
    let rest = target.strip_prefix("git+")?;
    let (rest, path) = rest.rsplit_once('#')?;
    let (url, reference) = rest.rsplit_once('@')?;
    if url.is_empty() || reference.is_empty() || path.is_empty() {
        return None;
    }
    Some(GitCoord {
        url: url.to_string(),
        reference: reference.to_string(),
        path: path.to_string(),
        raw: target.to_string(),
    })
}

fn is_sha(s: &str) -> bool {
    s.len() >= 7 && s.len() <= 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolve a ref to a commit SHA via `git ls-remote` — verifies the ref exists and pins it to an
/// immutable commit. A full SHA passes through unchanged.
fn resolve_sha(url: &str, reference: &str) -> Result<String, String> {
    if is_sha(reference) {
        return Ok(reference.to_string());
    }
    let out = Command::new("git")
        .args(["ls-remote", url, reference])
        .output()
        .map_err(|e| format!("git ls-remote failed ({e}); is git installed?"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-remote {url} {reference}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let sha = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("ref '{reference}' not found in {url}"))?;
    Ok(sha.to_string())
}

/// Clone the repo at `sha` into the cache once and return the path to the requested file. If the
/// file is already cached, no network happens.
fn fetch_file(coord: &GitCoord, sha: &str, cache_root: &Path) -> Result<PathBuf, String> {
    let repo_dir = cache_root.join(sha).join("repo");
    let file = repo_dir.join(&coord.path);
    if file.exists() {
        return Ok(file); // fetch-once: already cached
    }
    // The file is not cached. (Re)fetch, wiping any partial/broken repo dir first so an interrupted
    // earlier clone cannot leave us serving nothing — the fetch is self-healing.
    if repo_dir.exists() {
        let _ = std::fs::remove_dir_all(&repo_dir);
    }
    std::fs::create_dir_all(&repo_dir).map_err(|e| format!("cache mkdir: {e}"))?;
    let clone = Command::new("git")
        .args(["clone", "--quiet", &coord.url])
        .arg(&repo_dir)
        .output()
        .map_err(|e| format!("git clone failed ({e})"))?;
    if !clone.status.success() {
        let _ = std::fs::remove_dir_all(&repo_dir);
        return Err(format!(
            "git clone {}: {}",
            coord.url,
            String::from_utf8_lossy(&clone.stderr).trim()
        ));
    }
    let checkout = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .args(["checkout", "--quiet", sha])
        .output()
        .map_err(|e| format!("git checkout failed ({e})"))?;
    if !checkout.status.success() {
        let _ = std::fs::remove_dir_all(&repo_dir);
        return Err(format!(
            "git checkout {sha}: {}",
            String::from_utf8_lossy(&checkout.stderr).trim()
        ));
    }
    if !file.exists() {
        return Err(format!("path '{}' not found in repo at {sha}", coord.path));
    }
    Ok(file)
}

/// The lockfile, mapping a coordinate string to its resolved commit SHA. JSON for legibility and
/// hand-review. Kept minimal for the prototype.
fn lock_path(cache_root: &Path) -> PathBuf {
    cache_root
        .parent()
        .unwrap_or(cache_root)
        .join("allium.lock")
}

fn read_lock(cache_root: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(lock_path(cache_root))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("specs").cloned())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_lock(cache_root: &Path, specs: &serde_json::Map<String, serde_json::Value>) {
    let doc = serde_json::json!({ "version": 1, "specs": specs });
    if let Ok(s) = serde_json::to_string_pretty(&doc) {
        let _ = std::fs::write(lock_path(cache_root), s + "\n");
    }
}

/// Resolve a git coordinate to a local cached file and return its contents. `cache_root` is
/// `<workdir>/.allium/cache`.
///
/// Lockfile semantics — the fetch-once and trust floor from the distribution design:
/// - **Locked** (the coordinate is already pinned to a SHA in `.allium/allium.lock`): fetch that
///   exact SHA. If it is cached, no network happens at all; the project resolves offline. Because
///   a locked coordinate never consults the remote ref, moving or rewriting the ref upstream cannot
///   change what this project sees — reproducible and tamper-immune by construction.
/// - **Unlocked** (first resolve): consult the remote ref via `ls-remote` to pin it to an immutable
///   commit SHA, fetch that, and record the pin. Re-pinning is an explicit act (delete the lock).
pub fn resolve(coord: &GitCoord, cache_root: &Path) -> Result<String, String> {
    let mut specs = read_lock(cache_root);
    let pinned = specs
        .get(&coord.raw)
        .and_then(|v| v.get("sha"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let sha = match &pinned {
        Some(sha) => sha.clone(),                       // locked: use the pin, no ls-remote
        None => resolve_sha(&coord.url, &coord.reference)?, // first resolve: pin the ref
    };

    let file = fetch_file(coord, &sha, cache_root)?;
    let content = std::fs::read_to_string(&file).map_err(|e| format!("reading cached spec: {e}"))?;

    if pinned.is_none() {
        specs.insert(
            coord.raw.clone(),
            serde_json::json!({ "sha": sha, "path": file.display().to_string() }),
        );
        write_lock(cache_root, &specs);
    }

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_git_coordinate() {
        let c = parse_git_coord("git+file:///tmp/lib@v1#kafka.allium").unwrap();
        assert_eq!(c.url, "file:///tmp/lib");
        assert_eq!(c.reference, "v1");
        assert_eq!(c.path, "kafka.allium");
    }

    #[test]
    fn keeps_ssh_url_at_intact() {
        let c = parse_git_coord("git+git@github.com:org/repo@v2#dir/contract.allium").unwrap();
        assert_eq!(c.url, "git@github.com:org/repo");
        assert_eq!(c.reference, "v2");
        assert_eq!(c.path, "dir/contract.allium");
    }

    #[test]
    fn a_plain_path_is_not_a_coordinate() {
        assert!(parse_git_coord("./kafka.allium").is_none());
        assert!(parse_git_coord("kafka.allium").is_none());
    }
}
