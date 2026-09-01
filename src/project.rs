//! Project-root and stable project-key detection.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::model::{Scope, SyncContext, sha256};

pub fn context(
    scope: Scope,
    requested_root: Option<&Path>,
    requested_key: Option<&str>,
) -> Result<SyncContext> {
    let start = match requested_root {
        Some(path) => path.to_path_buf(),
        None => env::current_dir().context("could not read the current directory")?,
    };
    let start = fs::canonicalize(&start)
        .with_context(|| format!("project directory {} does not exist", start.display()))?;
    if !start.is_dir() {
        bail!("project path {} is not a directory", start.display());
    }

    let root = if requested_root.is_some() {
        start
    } else {
        find_git_root(&start).unwrap_or(start)
    };
    let project_key = requested_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| derive_project_key(&root));

    if project_key.len() > 160 {
        bail!("project key cannot be longer than 160 characters");
    }
    if project_key.contains(char::is_whitespace) {
        bail!("project key cannot contain whitespace");
    }

    Ok(SyncContext {
        scope,
        project_root: root,
        project_key,
    })
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|path| path.join(".git").exists())
        .map(Path::to_path_buf)
}

fn derive_project_key(root: &Path) -> String {
    if let Some(remote) = git_remote(root) {
        let normalized = normalize_git_remote(&remote);
        let digest = sha256(normalized.as_bytes());
        return format!("git-{}", &digest[..24]);
    }

    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("local-{name}")
}

/// Normalize common HTTPS and SCP-style remotes to the same host/path identity.
/// This also removes embedded Git credentials before hashing.
fn normalize_git_remote(remote: &str) -> String {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    if let Some((_, remainder)) = remote.split_once("://") {
        let remainder = remainder
            .split_once('@')
            .map_or(remainder, |(_, without_credentials)| without_credentials);
        if let Some((host, path)) = remainder.split_once('/') {
            return format!("{}/{}", host.to_lowercase(), path.trim_start_matches('/'));
        }
        return remainder.to_lowercase();
    }
    if let Some((user_and_host, path)) = remote.split_once(':')
        && let Some((_, host)) = user_and_host.rsplit_once('@')
    {
        return format!("{}/{}", host.to_lowercase(), path.trim_start_matches('/'));
    }
    remote.to_owned()
}

fn git_remote(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_project_keys_are_portable_and_readable() {
        assert_eq!(
            derive_project_key(Path::new("/work/My project")),
            "local-My-project"
        );
    }

    #[test]
    fn git_remote_normalization_removes_credentials_and_transport_details() {
        let https = normalize_git_remote("https://token@GitHub.com/team/app.git");
        let ssh = normalize_git_remote("git@github.com:team/app.git");
        assert_eq!(https, "github.com/team/app");
        assert_eq!(https, ssh);
    }
}
