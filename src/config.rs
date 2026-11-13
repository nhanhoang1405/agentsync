//! Local application configuration and first-run onboarding.

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::db::{Database, normalize_database_url, validate_database_url};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub database_url: String,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_ca_cert: Option<PathBuf>,
}

impl Config {
    pub fn load_optional() -> Result<Option<Self>> {
        let has_url = env::var("AGENTSYNC_DATABASE_URL").is_ok_and(|url| !url.trim().is_empty());
        if has_url || config_path()?.exists() {
            Self::load().map(Some)
        } else {
            Ok(None)
        }
    }

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
                "AgentSync has not been configured; run agentsync and enter a SQL connection URL (expected {})",
                path.display()
            )
        };
        let database_url = env::var("AGENTSYNC_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .or_else(|| saved.as_ref().map(|config| config.database_url.clone()))
            .with_context(missing_config)?;
        let email = saved
            .as_ref()
            .map(|config| config.email.clone())
            .unwrap_or_default();
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
        validate_database_url(&self.database_url)?;
        if !self.email.is_empty() {
            validate_email(&self.email)?;
        }
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

    pub fn from_url(database_url: String) -> Self {
        Self {
            database_url,
            email: String::new(),
            tls_ca_cert: None,
        }
    }

    /// Resolve the author from the authenticated SQL role without another prompt.
    pub fn resolve_identity(mut self) -> Result<Self> {
        self.validate()?;
        if self.email.is_empty() {
            let mut database = Database::connect(&self.database_url, self.tls_ca_cert.as_deref())?;
            database.migrate()?;
            self.email = database.author_identity()?;
        }
        Ok(self)
    }

    /// Validate credentials, initialize the schema, and persist the connection.
    pub fn connect_and_save(mut self) -> Result<Self> {
        self.email = self.email.trim().to_owned();
        self.database_url = normalize_database_url(&self.database_url)?;
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
        if self.email.is_empty() {
            self.email = database.author_identity()?;
        }
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
    let Ok(mut parsed) = url::Url::parse(url) else {
        return "<configured>".into();
    };
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some("***"));
    }
    let params = parsed
        .query_pairs()
        .map(|(key, value)| {
            let value = if matches!(key.as_ref(), "authToken" | "auth_token") {
                "***".to_owned()
            } else {
                value.into_owned()
            };
            (key.into_owned(), value)
        })
        .collect::<Vec<_>>();
    if !params.is_empty() {
        parsed.set_query(None);
        parsed.query_pairs_mut().extend_pairs(params);
    }
    parsed.to_string()
}

fn validate_email(email: &str) -> Result<()> {
    let trimmed = email.trim();
    let valid = !trimmed.is_empty()
        && !trimmed.contains(char::is_whitespace)
        && trimmed
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty());
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
    fn libsql_inline_tokens_are_redacted() {
        let redacted = redact_database_url("libsql://example.turso.io?authToken=private-token");
        assert!(!redacted.contains("private-token"));
        assert!(redacted.contains("authToken=***"));
        Config::from_url("libsql://example.turso.io".into())
            .validate()
            .unwrap();
    }

    #[test]
    fn url_only_connections_do_not_require_an_author_email() {
        let config: Config =
            toml::from_str("database_url = 'postgres://localhost/agentsync'").unwrap();
        assert!(config.email.is_empty());
        config.validate().unwrap();
        assert!(
            Config::from_url("unsupported://local.db".into())
                .validate()
                .is_err()
        );
    }

    #[test]
    fn saved_connections_are_atomic_private_and_preserve_legacy_authors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let mut config = Config::from_url("postgres://alice:secret@localhost/agentsync".into());
        config.email = "author@example.com".into();
        config.save(&path).unwrap();
        let saved: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.email, "author@example.com");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
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
