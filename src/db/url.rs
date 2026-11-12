use anyhow::{Context, Result, bail};
use sqlx::{ConnectOptions, sqlite::SqliteConnectOptions};
use std::{path::Path, str::FromStr, time::Duration};

pub fn validate_database_url(url: &str) -> Result<()> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        postgres::Config::from_str(url).context("invalid Postgres connection URL")?;
    } else if is_libsql_url(url) {
        validate_libsql_url(url)?;
    } else if url.starts_with("sqlite:") {
        sqlite_options(url)?;
    } else if url.starts_with("mysql://") || url.starts_with("mariadb://") {
        sqlx::mysql::MySqlConnectOptions::from_str(&url.replacen("mariadb://", "mysql://", 1))
            .context("invalid MySQL/MariaDB connection URL")?;
    } else {
        bail!(
            "database URL must start with postgres://, postgresql://, sqlite:, mysql://, mariadb:// or libsql://"
        );
    }
    Ok(())
}

pub(super) fn is_libsql_url(url: &str) -> bool {
    ["libsql://", "https://", "http://"]
        .iter()
        .any(|prefix| url.starts_with(prefix))
}

fn validate_libsql_url(url: &str) -> Result<::url::Url> {
    let parsed = ::url::Url::parse(url).map_err(|_| anyhow::anyhow!("invalid libSQL URL"))?;
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || !matches!(parsed.path(), "" | "/")
    {
        bail!(
            "libSQL URL must specify a host without credentials, a path or a fragment; use ?authToken=... for authentication"
        );
    }
    let loopback = match parsed.host() {
        Some(::url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(::url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(::url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if parsed.scheme() == "http" && !loopback {
        bail!("libSQL requires TLS; use libsql:// or https:// for remote servers");
    }
    let tokens = parsed.query_pairs().collect::<Vec<_>>();
    if tokens.len() > 1
        || tokens.iter().any(|(key, value)| {
            !matches!(key.as_ref(), "authToken" | "auth_token") || value.trim().is_empty()
        })
    {
        bail!(
            "libSQL URLs support one nonempty authToken query parameter; alternatively set TURSO_AUTH_TOKEN"
        );
    }
    Ok(parsed)
}

pub(super) fn libsql_connection(url: &str) -> Result<(String, String)> {
    let mut parsed = validate_libsql_url(url)?;
    let inline = parsed
        .query_pairs()
        .next()
        .map(|(_, token)| token.into_owned());
    let token = inline
        .or_else(|| std::env::var("TURSO_AUTH_TOKEN").ok())
        .unwrap_or_default()
        .trim()
        .to_owned();
    if token.chars().any(char::is_control) {
        bail!("libSQL auth token contains invalid control characters");
    }
    parsed.set_query(None);
    Ok((parsed.to_string(), token))
}

pub(super) fn sqlite_options(url: &str) -> Result<SqliteConnectOptions> {
    let raw = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .context("invalid SQLite URL")?;
    let (filename, query) = raw.split_once('?').unwrap_or((raw, ""));
    let params = ::url::form_urlencoded::parse(query.as_bytes()).collect::<Vec<_>>();
    if filename.is_empty()
        || filename == ":memory:"
        || params.iter().any(|(k, v)| k == "mode" && v == "memory")
    {
        bail!(
            "SQLite sync requires a file URL such as sqlite://agentsync.db; in-memory databases are not supported"
        );
    }
    if params.iter().any(|(k, v)| {
        (k == "mode" && v == "ro") || (k == "immutable" && matches!(v.as_ref(), "true" | "1"))
    }) {
        bail!("SQLite sync requires a writable database");
    }
    if params.iter().any(|(key, _)| key == "vfs") {
        bail!(
            "custom SQLite VFS options are not supported; sync requires a persistent file database"
        );
    }
    let options = SqliteConnectOptions::from_str(url).context("invalid SQLite connection URL")?;
    if options.get_filename() == Path::new(":memory:") {
        bail!("SQLite sync requires a file database");
    }
    let path = std::path::absolute(options.get_filename())
        .context("could not resolve SQLite database path")?;
    let create = !params
        .iter()
        .any(|(key, value)| key == "mode" && value == "rw");
    Ok(options
        .filename(path)
        .create_if_missing(create)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(10)))
}

/// Save absolute SQLite paths so changing the working directory does not select another archive.
pub fn normalize_database_url(url: &str) -> Result<String> {
    validate_database_url(url)?;
    if url.starts_with("sqlite:") {
        Ok(sqlite_options(url)?.to_url_lossy().to_string())
    } else {
        Ok(url.to_owned())
    }
}

pub(super) fn portable_url(url: &str, ca: Option<&Path>) -> Result<String> {
    if url.starts_with("sqlite:") {
        return normalize_database_url(url);
    }
    let mut parsed = ::url::Url::parse(&url.replacen("mariadb://", "mysql://", 1))
        .context("invalid MySQL/MariaDB URL")?;
    if !parsed
        .query_pairs()
        .any(|(key, _)| matches!(key.as_ref(), "sslmode" | "ssl-mode"))
    {
        parsed
            .query_pairs_mut()
            .append_pair("ssl-mode", "VERIFY_IDENTITY");
    }
    if let Some(ca) = ca {
        parsed
            .query_pairs_mut()
            .append_pair("ssl-ca", &ca.to_string_lossy());
    }
    Ok(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_supported_engines_and_rejects_transient_sqlite_databases() {
        for url in [
            "postgres://localhost/agentsync",
            "postgresql://localhost/agentsync",
            "mysql://localhost/agentsync",
            "mariadb://localhost/agentsync",
            "sqlite://archive.db",
            "sqlite:///tmp/archive.db",
        ] {
            validate_database_url(url).unwrap();
        }
        for url in [
            "mongodb://localhost/db",
            "sqlite::memory:",
            "sqlite://:memory:",
            "sqlite://foo?mode=memory",
            "sqlite://foo?mode=ro",
            "sqlite://foo?vfs=memdb",
            "sqlite:",
        ] {
            assert!(validate_database_url(url).is_err(), "{url}");
        }
    }
    #[test]
    fn sqlite_relative_urls_are_saved_as_absolute_and_keep_encoded_paths() {
        let normalized = normalize_database_url("sqlite://test%20archive%3F.db").unwrap();
        let options = sqlite_options(&normalized).unwrap();
        assert!(options.get_filename().is_absolute());
        assert!(options.get_filename().ends_with("test archive?.db"));
    }
    #[test]
    fn mysql_checks_tls_identity_by_default_and_keeps_explicit_url_options() {
        let secure = portable_url("mariadb://localhost/agentsync", None).unwrap();
        assert!(secure.starts_with("mysql://"));
        assert!(secure.contains("VERIFY_IDENTITY"));
        assert_eq!(
            portable_url("mysql://localhost/agentsync?ssl-mode=DISABLED", None).unwrap(),
            "mysql://localhost/agentsync?ssl-mode=DISABLED"
        );
    }
}

#[cfg(test)]
mod libsql_tests {
    use super::*;
    #[test]
    fn validates_turso_urls_and_keeps_tokens_out_of_the_client_url() {
        let target = "libsql://agentsync-chinhan1405.aws-ap-southeast-1.turso.io";
        validate_database_url(target).unwrap();
        let (endpoint, token) =
            libsql_connection(&format!("{target}?authToken=test-token")).unwrap();
        assert!(!endpoint.contains("test-token"));
        assert_eq!(token, "test-token");
        let (_, alias) =
            libsql_connection("libsql://example.turso.io?auth_token=second-token").unwrap();
        assert_eq!(alias, "second-token");
        for invalid in [
            "libsql://",
            "libsql://user:password@example.turso.io",
            "libsql://example.turso.io/path",
            "libsql://example.turso.io#fragment",
            "libsql://example.turso.io?authToken=",
            "libsql://example.turso.io?authToken=a&authToken=b",
            "libsql://example.turso.io?other=value",
            "http://example.turso.io",
        ] {
            assert!(
                validate_database_url(invalid).is_err(),
                "unexpectedly accepted {invalid}"
            );
        }
        validate_database_url("http://127.0.0.1:8080").unwrap();
        validate_database_url("https://example.turso.io").unwrap();
        assert!(libsql_connection("libsql://example.turso.io?authToken=bad%0Atoken").is_err());
    }
}
