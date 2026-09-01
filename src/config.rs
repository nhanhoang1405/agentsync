//! Local application configuration and first-run onboarding.

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub database_url: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_ca_cert: Option<PathBuf>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        let saved = if path.is_file() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))?;
            Some(
                toml::from_str::<Self>(&text)
                    .with_context(|| format!("could not parse {}", path.display()))?,
            )
        } else {
            None
        };

        let missing_config = || {
            format!(
                "AgentSync has not been configured; open Settings to add a connection (expected {})",
                path.display()
            )
        };
        let database_url = env::var("AGENTSYNC_DATABASE_URL")
            .ok()
            .or_else(|| saved.as_ref().map(|config| config.database_url.clone()))
            .with_context(missing_config)?;
        let email = env::var("AGENTSYNC_EMAIL")
            .ok()
            .or_else(|| saved.as_ref().map(|config| config.email.clone()))
            .with_context(missing_config)?;
        let tls_ca_cert = env::var_os("AGENTSYNC_TLS_CA_CERT")
            .map(PathBuf::from)
            .or_else(|| saved.as_ref().and_then(|config| config.tls_ca_cert.clone()));

        let config = Self {
            database_url,
            email,
            tls_ca_cert,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !looks_like_postgres_url(&self.database_url) {
            bail!("database URL must start with postgres:// or postgresql://");
        }
        validate_email(&self.email)?;
        if let Some(path) = &self.tls_ca_cert
            && !path.is_file()
        {
            bail!("TLS CA certificate {} is not a file", path.display());
        }
        Ok(())
    }

    fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .context("configuration path has no parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;

        let text = toml::to_string_pretty(self).context("could not serialize configuration")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
            format!("could not create a temporary file in {}", parent.display())
        })?;
        temporary
            .write_all(text.as_bytes())
            .with_context(|| format!("could not write configuration in {}", parent.display()))?;
        restrict_permissions(temporary.path())?;
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("could not replace {}", path.display()))?;
        Ok(())
    }

    /// Validate credentials, initialize the schema, and persist the settings.
    pub fn connect_and_save(mut self) -> Result<Self> {
        self.email = self.email.trim().to_lowercase();
        self.tls_ca_cert = self
            .tls_ca_cert
            .map(|path| {
                fs::canonicalize(&path).with_context(|| {
                    format!("could not find TLS CA certificate {}", path.display())
                })
            })
            .transpose()?;
        self.validate()?;

        let mut database = Database::connect(&self.database_url, self.tls_ca_cert.as_deref())?;
        database.migrate()?;
        database.register_user(&self.email)?;
        self.save(&config_path()?)?;
        Ok(self)
    }
}

pub fn config_path() -> Result<PathBuf> {
    let directories = ProjectDirs::from("dev", "agentsync", "agentsync")
        .context("could not locate the operating system configuration directory")?;
    Ok(directories.config_dir().join("config.toml"))
}

pub fn redact_database_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return "<configured>".to_owned();
    };
    let authority_start = scheme_end + 3;
    let Some(at_offset) = url[authority_start..].find('@') else {
        return url.to_owned();
    };
    let at = authority_start + at_offset;
    let credentials = &url[authority_start..at];
    let Some(colon) = credentials.find(':') else {
        return url.to_owned();
    };

    format!(
        "{}{}:***{}",
        &url[..authority_start],
        &credentials[..colon],
        &url[at..]
    )
}

fn looks_like_postgres_url(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

fn validate_email(email: &str) -> Result<()> {
    let trimmed = email.trim();
    let valid = !trimmed.is_empty()
        && !trimmed.contains(char::is_whitespace)
        && trimmed
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    if !valid {
        bail!("`{email}` does not look like a valid email address");
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("could not protect {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    // Windows ACL handling belongs in the platform adapter. The per-user config
    // directory is still used, and environment variables avoid local storage.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords_but_preserves_connection_details() {
        assert_eq!(
            redact_database_url("postgres://alice:secret@localhost:5432/db?sslmode=require"),
            "postgres://alice:***@localhost:5432/db?sslmode=require"
        );
        assert_eq!(
            redact_database_url("postgres://localhost/db"),
            "postgres://localhost/db"
        );
    }

    #[test]
    fn old_configuration_files_do_not_require_a_tls_ca_field() {
        let config: Config = toml::from_str(
            r#"
            database_url = "postgres://localhost/agentsync"
            email = "author@example.com"
            "#,
        )
        .unwrap();
        assert!(config.tls_ca_cert.is_none());
    }
}
