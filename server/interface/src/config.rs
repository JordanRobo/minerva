//! Typed server configuration, owned by the composition root.
//!
//! This crate is the only place that reads files or environment variables;
//! `domain`, `application` and `infrastructure` receive ready-made values.
//! Settings layer from lowest to highest precedence:
//!
//! 1. built-in defaults (see the field docs),
//! 2. an optional TOML file, discovered as `--config <path>`, else
//!    `MINERVA_CONFIG`, else `./minerva.toml`, else `/etc/minerva/minerva.toml`
//!    (the default locations are optional; a missing *explicit* file is an
//!    error),
//! 3. the legacy aliases `DATABASE_URL`, `REDIS_URL` and `PORT`,
//! 4. `MINERVA_`-prefixed environment variables, with `__` separating the
//!    section from the key (e.g. `MINERVA_OIDC__ISSUER_URL`). Arrays arrive as
//!    JSON (`MINERVA_OIDC__SCOPES='["openid","email"]'`).
//!
//! An empty or whitespace-only value counts as unset, preserving the old
//! "empty variable means off" behaviour. Secrets may come from their key or
//! from a `*_file` sibling holding a path to a file whose trimmed contents
//! are the value (the Docker/Kubernetes convention); setting both is an
//! error, and secrets never appear in logs or `Debug` output.

use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Format, Serialized, Toml};
use serde::Deserialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

/// A value that must never appear in logs or `Debug` output.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value. Callers are trusted not to log it; prefer the
    /// `Display`/`Debug` impls, which redact.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(String::deserialize(deserializer)?))
    }
}

/// The whole server configuration. Every section and key is optional at the
/// TOML level; the built-in defaults fill in what is missing, and
/// [`Config::validate`] (run by [`load`]) reports every problem together.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub oidc: OidcConfig,
}

impl Config {
    /// OIDC sign-in is enabled exactly when `oidc.issuer_url` is non-blank.
    pub fn oidc_enabled(&self) -> bool {
        !self.oidc.issuer_url.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// Port to listen on (default 8080).
    pub port: u16,
    /// `Secure` flag for the session and OIDC-state cookies (default false;
    /// set true once the deployment is served over https).
    pub cookie_secure: bool,
    /// Apply pending database migrations at startup (default true); set false
    /// when a dedicated migration job owns the schema.
    pub run_migrations: bool,
    /// Absolute public URL of the API origin (no trailing slash); required
    /// when OIDC is enabled, ignored otherwise. Blank means unset; after a
    /// successful load any trailing slash is stripped.
    pub web_base_url: String,
}

// A manual `Default` rather than a derived one: the container-level
// `#[serde(default)]` fills missing fields from it, and a derived default
// would make `port` 0 instead of 8080.
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            cookie_secure: false,
            run_migrations: true,
            web_base_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DatabaseConfig {
    /// Postgres connection string; required. May contain credentials, so it
    /// belongs in the `DATABASE_URL` environment variable or `url_file`, not
    /// in a committed file. Blank means unset.
    pub url: Secret,
    /// Path to a file containing the connection string (trimmed). Consumed by
    /// the loader; always blank after a successful load.
    url_file: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RedisConfig {
    /// Redis URL for session storage; blank means "store sessions in
    /// Postgres". May contain credentials, so prefer the `REDIS_URL`
    /// environment variable or `url_file`.
    pub url: Secret,
    /// Path to a file containing the URL (trimmed). Consumed by the loader;
    /// always blank after a successful load.
    url_file: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OidcConfig {
    /// The provider's issuer URL (the base of its
    /// `/.well-known/openid-configuration`); blank means OIDC is disabled.
    pub issuer_url: String,
    /// Client ID of the application registered at the provider; required when
    /// enabled.
    pub client_id: String,
    /// Client secret; required when enabled.
    pub client_secret: Secret,
    /// Path to a file containing the client secret (trimmed). Consumed by the
    /// loader; always blank after a successful load.
    client_secret_file: String,
    /// Redirect URI registered at the provider — it must match exactly;
    /// required when enabled.
    pub redirect_url: String,
    /// Name shown for the SSO option (default "Single sign-on").
    pub display_name: String,
    /// Requested scopes as a real array (default `["openid", "email",
    /// "profile"]`); `openid` is always present after a successful load.
    pub scopes: Vec<String>,
    /// ID-token claim holding the user's groups (default "groups").
    pub groups_claim: String,
    /// Key for the encrypted OIDC state cookie; at least 32 bytes when
    /// enabled.
    pub state_secret: Secret,
    /// Path to a file containing the state secret (trimmed). Consumed by the
    /// loader; always blank after a successful load.
    state_secret_file: String,
    /// Create an account for first-time SSO logins (default true); false
    /// restricts sign-in to existing accounts.
    pub auto_create_users: bool,
}

// Manual `Default` for the same reason as `ServerConfig`: missing fields are
// filled from it, and a derived default would leave `auto_create_users` off
// and the name/scopes/claim blank.
impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            issuer_url: String::new(),
            client_id: String::new(),
            client_secret: Secret::default(),
            client_secret_file: String::new(),
            redirect_url: String::new(),
            display_name: DEFAULT_DISPLAY_NAME.to_owned(),
            scopes: default_scopes(),
            groups_claim: DEFAULT_GROUPS_CLAIM.to_owned(),
            state_secret: Secret::default(),
            state_secret_file: String::new(),
            auto_create_users: true,
        }
    }
}

/// Default for `oidc.display_name`.
const DEFAULT_DISPLAY_NAME: &str = "Single sign-on";
/// Default for `oidc.groups_claim`.
const DEFAULT_GROUPS_CLAIM: &str = "groups";

/// Default for `oidc.scopes`.
fn default_scopes() -> Vec<String> {
    ["openid", "email", "profile"]
        .iter()
        .map(|scope| (*scope).to_owned())
        .collect()
}

impl Config {
    /// Replace each `*_file` secret with the trimmed contents of its file.
    /// Setting both a value and its file, or an unreadable file, is recorded
    /// in `problems` — naming the path, never the contents.
    fn resolve_secret_files(&mut self, problems: &mut Vec<String>) {
        resolve_one(
            "database.url",
            &mut self.database.url,
            &mut self.database.url_file,
            problems,
        );
        resolve_one(
            "redis.url",
            &mut self.redis.url,
            &mut self.redis.url_file,
            problems,
        );
        resolve_one(
            "oidc.client_secret",
            &mut self.oidc.client_secret,
            &mut self.oidc.client_secret_file,
            problems,
        );
        resolve_one(
            "oidc.state_secret",
            &mut self.oidc.state_secret,
            &mut self.oidc.state_secret_file,
            problems,
        );
    }

    /// Fill in what a blank value stands for: an empty `display_name` falls
    /// back to the default rather than producing a blank name, an empty scope
    /// list becomes the default list, and `openid` is always requested.
    fn apply_defaults(&mut self) {
        if self.oidc.display_name.trim().is_empty() {
            self.oidc.display_name = DEFAULT_DISPLAY_NAME.to_owned();
        }
        if self.oidc.groups_claim.trim().is_empty() {
            self.oidc.groups_claim = DEFAULT_GROUPS_CLAIM.to_owned();
        }
        if self.oidc.scopes.is_empty() {
            self.oidc.scopes = default_scopes();
        }
        if !self.oidc.scopes.iter().any(|scope| scope == "openid") {
            self.oidc.scopes.insert(0, "openid".to_owned());
        }
    }

    /// Every rule, checked together: all problems come back in one list so a
    /// misconfigured startup reports everything at once instead of one fix
    /// per restart.
    fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.database.url.expose().trim().is_empty() {
            problems.push("database.url is required".to_owned());
        }
        if !self.oidc_enabled() {
            return problems;
        }

        // OIDC is enabled, so everything it needs must be there and parseable.
        if self.oidc.client_id.trim().is_empty() {
            problems.push("oidc.client_id is required when oidc.issuer_url is set".to_owned());
        }
        if self.oidc.client_secret.expose().trim().is_empty() {
            problems.push("oidc.client_secret is required when oidc.issuer_url is set".to_owned());
        }
        if let Err(err) = url::Url::parse(&self.oidc.issuer_url) {
            problems.push(format!(
                "oidc.issuer_url is not a valid URL ({:?}): {err}",
                self.oidc.issuer_url
            ));
        }
        if self.oidc.redirect_url.trim().is_empty() {
            problems.push("oidc.redirect_url is required when oidc.issuer_url is set".to_owned());
        } else if let Err(err) = url::Url::parse(&self.oidc.redirect_url) {
            problems.push(format!(
                "oidc.redirect_url is not a valid URL ({:?}): {err}",
                self.oidc.redirect_url
            ));
        }
        match self.oidc.state_secret.expose() {
            secret if secret.trim().is_empty() => problems
                .push("oidc.state_secret is required when oidc.issuer_url is set".to_owned()),
            secret if secret.len() < 32 => {
                problems.push("oidc.state_secret must be at least 32 bytes".to_owned())
            }
            _ => {}
        }
        if self.server.web_base_url.trim().is_empty() {
            problems.push("server.web_base_url is required when oidc.issuer_url is set".to_owned());
        } else if valid_base_url(&self.server.web_base_url).is_none() {
            problems.push(
                "server.web_base_url must be an absolute http(s) URL, e.g. https://minerva.example.com"
                    .to_owned(),
            );
        }
        problems
    }
}

/// One secret pair: when only the path is set, the value becomes the file's
/// trimmed contents; both set, or an unreadable file, records a problem (naming
/// the path, never the contents). The path is consumed either way so it never
/// survives into the loaded configuration.
fn resolve_one(key: &str, value: &mut Secret, file: &mut String, problems: &mut Vec<String>) {
    let path = file.trim();
    if !value.expose().trim().is_empty() && !path.is_empty() {
        problems.push(format!(
            "{key} and {key}_file are both set; use one or the other"
        ));
    } else if !path.is_empty() {
        match std::fs::read_to_string(path) {
            Ok(contents) => *value = Secret::new(contents.trim().to_owned()),
            Err(err) => problems.push(format!("could not read {key}_file at {path}: {err}")),
        }
    }
    file.clear();
}

/// An absolute http(s) URL with any trailing slash stripped, so redirects can
/// be built as `{base}{path}` where `path` always starts with '/'.
pub(crate) fn valid_base_url(raw: &str) -> Option<String> {
    let host = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))?;
    if host.is_empty() || host.starts_with('/') {
        return None;
    }
    Some(raw.trim_end_matches('/').to_owned())
}

/// Load the configuration: discover the file, layer defaults/file/aliases/env,
/// resolve `*_file` secrets and validate everything. All problems are
/// collected and returned together; an empty list means success.
pub fn load() -> Result<Config, Vec<String>> {
    let args: Vec<String> = std::env::args().collect();
    let vars: Vec<(String, String)> = std::env::vars().collect();

    let file = discover_config_file(&args, std::env::var("MINERVA_CONFIG").ok().as_deref())
        .and_then(resolved_file)
        .map_err(|err| vec![err])?;
    if let Some(path) = &file {
        println!("loading configuration from {}", path.display());
    } else {
        println!("no configuration file found; using built-in defaults and environment variables");
    }

    let (mut config, problems) = from_sources(file, alias_layer(&vars), minerva_env_layer(&vars))
        .map_err(|err| vec![err])?;
    // A blank `web_base_url` is "unset"; a valid one is stored without its
    // trailing slash so redirects can be built as `{base}{path}`.
    if let Some(stripped) = valid_base_url(&config.server.web_base_url) {
        config.server.web_base_url = stripped;
    }

    if problems.is_empty() {
        if let Some(warning) = legacy_env_warning(&vars) {
            eprintln!("{warning}");
        }
        Ok(config)
    } else {
        Err(problems)
    }
}

/// The pure core of the loader, over injectable sources so tests never touch
/// the process environment or the real filesystem outside their temp files.
/// Returns the loaded configuration together with every validation problem;
/// an extract/parse failure is a single `Err` message.
pub(crate) fn from_sources(
    file: Option<PathBuf>,
    aliases: JsonValue,
    minerva_env: JsonValue,
) -> Result<(Config, Vec<String>), String> {
    let mut figment = Figment::new();
    if let Some(path) = &file {
        // A missing default-location file is never passed in; an explicit one
        // was checked by `resolved_file`. An unparsable file surfaces here.
        figment = figment.merge(Toml::file(path));
    }
    // Layering order is merge order: later providers win, so the env layers
    // override the file and the aliases sit between them.
    let figment = figment
        .merge(Serialized::defaults(aliases))
        .merge(Serialized::defaults(minerva_env));

    let mut config: Config = figment.extract().map_err(|err| err.to_string())?;
    let mut problems = Vec::new();
    config.resolve_secret_files(&mut problems);
    config.apply_defaults();
    problems.extend(config.validate());
    Ok((config, problems))
}

/// `--config <path>` / `--config=<path>`, else `MINERVA_CONFIG`, else the
/// first existing default location. Returns the path and whether it was named
/// explicitly (a missing explicit file is an error; a missing default one is
/// not). Other command-line arguments are ignored.
fn discover_config_file(
    args: &[String],
    minerva_config: Option<&str>,
) -> Result<Option<(PathBuf, bool)>, String> {
    if let Some(path) = config_path_arg(args)? {
        return Ok(Some((path.into(), true)));
    }
    if let Some(path) = minerva_config.filter(|value| !value.trim().is_empty()) {
        return Ok(Some((path.trim().into(), true)));
    }
    for path in ["./minerva.toml", "/etc/minerva/minerva.toml"] {
        if Path::new(path).is_file() {
            return Ok(Some((path.into(), false)));
        }
    }
    Ok(None)
}

/// `--config <path>` / `--config=<path>` from the command line.
fn config_path_arg(args: &[String]) -> Result<Option<String>, String> {
    let mut rest = args.iter().peekable();
    while let Some(arg) = rest.next() {
        if arg == "--config" {
            return rest
                .next()
                .cloned()
                .map(Some)
                .ok_or_else(|| "--config requires a path".to_owned());
        }
        if let Some(path) = arg.strip_prefix("--config=") {
            return Ok(Some(path.to_owned()));
        }
    }
    Ok(None)
}

/// An explicitly named file that does not exist is an error; the optional
/// default locations were already filtered by existence.
fn resolved_file(discovered: Option<(PathBuf, bool)>) -> Result<Option<PathBuf>, String> {
    match discovered {
        Some((path, true)) if !path.is_file() => {
            Err(format!("config file not found: {}", path.display()))
        }
        other => Ok(other.map(|(path, _)| path)),
    }
}

/// The legacy alias variables, as a layer below the `MINERVA_` ones:
/// `DATABASE_URL` -> database.url, `REDIS_URL` -> redis.url, `PORT` ->
/// server.port. Blank values are dropped (unset).
fn alias_layer(vars: &[(String, String)]) -> JsonValue {
    let mut root = JsonMap::new();
    for (name, path) in [
        ("DATABASE_URL", ["database", "url"]),
        ("REDIS_URL", ["redis", "url"]),
        ("PORT", ["server", "port"]),
    ] {
        if let Some(value) = non_blank(
            vars.iter()
                .find(|(key, _)| key == name)
                .map(|(_, v)| v.as_str()),
        ) {
            insert_at(
                &mut root,
                path.iter().map(|part| (*part).to_owned()).collect(),
                parse_value(value),
            );
        }
    }
    JsonValue::Object(root)
}

/// The `MINERVA_`-prefixed variables as a nested map: the prefix is stripped,
/// keys are lowercased and split on `__` into section/key. `MINERVA_CONFIG`
/// names the config file, not a setting, so it is skipped; blank values are
/// dropped (unset).
fn minerva_env_layer(vars: &[(String, String)]) -> JsonValue {
    let mut root = JsonMap::new();
    for (name, raw) in vars {
        let Some(key) = name.strip_prefix("MINERVA_") else {
            continue;
        };
        if key.is_empty() || key.eq_ignore_ascii_case("CONFIG") {
            continue;
        }
        let Some(value) = non_blank(Some(raw.as_str())) else {
            continue;
        };
        let path: Vec<String> = key
            .to_ascii_lowercase()
            .split("__")
            .map(|part| part.to_owned())
            .collect();
        insert_at(&mut root, path, parse_value(value));
    }
    JsonValue::Object(root)
}

/// A variable value that is not empty or whitespace-only; trimmed, since a
/// stray newline from `$(cat file)` must not become part of the setting.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
}

/// Environment values are JSON when they parse as such (that is how numbers,
/// booleans and arrays arrive, e.g. `MINERVA_OIDC__SCOPES='["openid","email"]'`),
/// plain strings otherwise.
fn parse_value(raw: &str) -> JsonValue {
    serde_json::from_str(raw).unwrap_or_else(|_| JsonValue::String(raw.to_owned()))
}

/// Insert `value` at the key path, creating intermediate objects.
fn insert_at(root: &mut JsonMap<String, JsonValue>, path: Vec<String>, value: JsonValue) {
    let (parents, key) = path.split_at(path.len() - 1);
    let mut node = root;
    for parent in parents {
        if !node.contains_key(parent) {
            node.insert(parent.clone(), JsonValue::Object(JsonMap::new()));
        }
        node = node
            .get_mut(parent)
            .and_then(JsonValue::as_object_mut)
            .expect("nested key was just created as an object");
    }
    node.insert(key[0].clone(), value);
}

/// Old variable names that are no longer read, with their replacements.
const LEGACY_VARS: [(&str, &str); 12] = [
    ("OIDC_ISSUER_URL", "MINERVA_OIDC__ISSUER_URL"),
    ("OIDC_CLIENT_ID", "MINERVA_OIDC__CLIENT_ID"),
    ("OIDC_CLIENT_SECRET", "MINERVA_OIDC__CLIENT_SECRET"),
    ("OIDC_REDIRECT_URL", "MINERVA_OIDC__REDIRECT_URL"),
    ("OIDC_DISPLAY_NAME", "MINERVA_OIDC__DISPLAY_NAME"),
    ("OIDC_SCOPES", "MINERVA_OIDC__SCOPES"),
    ("OIDC_GROUPS_CLAIM", "MINERVA_OIDC__GROUPS_CLAIM"),
    ("OIDC_STATE_SECRET", "MINERVA_OIDC__STATE_SECRET"),
    ("OIDC_AUTO_CREATE_USERS", "MINERVA_OIDC__AUTO_CREATE_USERS"),
    ("COOKIE_SECURE", "MINERVA_SERVER__COOKIE_SECURE"),
    ("WEB_BASE_URL", "MINERVA_SERVER__WEB_BASE_URL"),
    ("RUN_MIGRATIONS", "MINERVA_SERVER__RUN_MIGRATIONS"),
];

/// One startup warning naming every legacy variable still present in the
/// environment, with its replacement. `None` when none are set.
fn legacy_env_warning(vars: &[(String, String)]) -> Option<String> {
    let mut found = Vec::new();
    for (name, replacement) in LEGACY_VARS {
        if vars.iter().any(|(key, _)| key == name) {
            found.push(format!("{name} ({replacement})"));
        }
    }
    (!found.is_empty()).then(|| {
        format!(
            "warning: these environment variables are no longer read; use minerva.toml or the \
             MINERVA_-prefixed equivalents instead: {}",
            found.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A temp file with `contents`, uniquely named so parallel tests cannot
    /// collide. Left in the system temp dir; the OS reaps it.
    fn temp_file(name: &str, contents: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "minerva-config-test-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, contents).expect("write temp file");
        path
    }

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// Run the pure loader core over a temp TOML file (or none) and fake
    /// alias/env variable lists — no process environment involved.
    fn sources(
        toml: Option<&str>,
        aliases: &[(&str, &str)],
        env: &[(&str, &str)],
    ) -> Result<(Config, Vec<String>), String> {
        let file = toml.map(|contents| temp_file("config.toml", contents));
        from_sources(
            file,
            alias_layer(&pairs(aliases)),
            minerva_env_layer(&pairs(env)),
        )
    }

    /// A TOML body with a database URL so validation passes on its own.
    const DB_TOML: &str =
        "[database]\nurl = \"postgresql://minerva:minerva@localhost:5432/minerva\"\n";

    #[test]
    fn defaults_apply_without_any_source() {
        let (config, problems) = sources(None, &[], &[]).expect("extract");
        assert_eq!(config.server.port, 8080);
        assert!(!config.server.cookie_secure);
        assert!(config.server.run_migrations);
        assert_eq!(config.server.web_base_url, "");
        assert_eq!(config.redis.url.expose(), "");
        assert!(!config.oidc_enabled());
        assert_eq!(config.oidc.display_name, DEFAULT_DISPLAY_NAME);
        assert_eq!(
            config.oidc.scopes,
            vec![
                "openid".to_owned(),
                "email".to_owned(),
                "profile".to_owned()
            ]
        );
        assert_eq!(config.oidc.groups_claim, DEFAULT_GROUPS_CLAIM);
        assert!(config.oidc.auto_create_users);
        // Only the one genuinely required setting is missing.
        assert_eq!(problems, vec!["database.url is required".to_owned()]);
    }

    #[test]
    fn file_only_loads_everything() {
        let toml = format!("{DB_TOML}[server]\nport = 9000\ncookie_secure = true\n");
        let (config, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.server.port, 9000);
        assert!(config.server.cookie_secure);
        assert_eq!(
            config.database.url.expose(),
            "postgresql://minerva:minerva@localhost:5432/minerva"
        );
    }

    #[test]
    fn env_overrides_the_file() {
        let toml = format!("{DB_TOML}[server]\nport = 9000\n");
        let (config, problems) =
            sources(Some(&toml), &[], &[("MINERVA_SERVER__PORT", "9999")]).expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.server.port, 9999);
    }

    #[test]
    fn aliases_override_the_file() {
        let toml = "[server]\nport = 9000\n";
        let (config, problems) = sources(
            Some(toml),
            &[
                ("PORT", "7777"),
                ("DATABASE_URL", "postgresql://from-alias/db"),
            ],
            &[],
        )
        .expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.server.port, 7777);
        assert_eq!(config.database.url.expose(), "postgresql://from-alias/db");
    }

    #[test]
    fn minerva_env_overrides_the_aliases() {
        let (config, problems) = sources(
            None,
            &[("PORT", "7777"), ("DATABASE_URL", "postgresql://a/db")],
            &[
                ("MINERVA_SERVER__PORT", "9999"),
                ("MINERVA_DATABASE__URL", "postgresql://b/db"),
            ],
        )
        .expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.database.url.expose(), "postgresql://b/db");
    }

    #[test]
    fn scopes_come_from_toml_as_a_real_array() {
        let toml = format!("{DB_TOML}[oidc]\nscopes = [\"email\", \"profile\"]\n");
        let (config, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        // `openid` is ensured even when the configured list omits it.
        assert_eq!(
            config.oidc.scopes,
            vec![
                "openid".to_owned(),
                "email".to_owned(),
                "profile".to_owned()
            ]
        );
    }

    #[test]
    fn scopes_come_from_env_as_json() {
        let (config, problems) = sources(
            Some(DB_TOML),
            &[],
            &[("MINERVA_OIDC__SCOPES", r#"["openid", "email"]"#)],
        )
        .expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            config.oidc.scopes,
            vec!["openid".to_owned(), "email".to_owned()]
        );
    }

    #[test]
    fn blank_values_count_as_unset() {
        // Blank env vars: the issuer stays "unset", so OIDC is disabled even
        // though the variable is present; a blank PORT falls back to default.
        let (config, problems) = sources(
            None,
            &[("PORT", "   "), ("DATABASE_URL", "postgresql://a/db")],
            &[
                ("MINERVA_OIDC__ISSUER_URL", ""),
                ("MINERVA_SERVER__RUN_MIGRATIONS", "  "),
            ],
        )
        .expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!config.oidc_enabled());
        assert_eq!(config.server.port, 8080);
        assert!(config.server.run_migrations);

        // A blank string in the file is "unset" too: no issuer, no OIDC.
        let toml = format!("{DB_TOML}[oidc]\nissuer_url = \"   \"\n");
        let (config, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!config.oidc_enabled());
    }

    #[test]
    fn unknown_keys_are_rejected_with_their_name() {
        let toml = format!("{DB_TOML}[server]\nport = 8080\nissure_url = \"x\"\n");
        let err = sources(Some(&toml), &[], &[]).expect_err("must fail");
        assert!(err.contains("issure_url"), "{err}");
    }

    #[test]
    fn secret_file_is_read_and_trimmed() {
        let secret_path = temp_file("client-secret.txt", "  s3cr3t-value\n");
        let toml = format!(
            "{DB_TOML}[oidc]\nissuer_url = \"https://idp.example\"\nclient_id = \"id\"\n\
             client_secret_file = \"{secret_path}\"\nredirect_url = \"https://app.example/cb\"\n",
            secret_path = secret_path.display()
        );
        let (config, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        // state_secret and web_base_url are still missing: exactly those.
        assert_eq!(
            problems,
            vec![
                "oidc.state_secret is required when oidc.issuer_url is set".to_owned(),
                "server.web_base_url is required when oidc.issuer_url is set".to_owned(),
            ]
        );
        assert_eq!(config.oidc.client_secret.expose(), "s3cr3t-value");
    }

    #[test]
    fn secret_and_secret_file_together_is_an_error() {
        let secret_path = temp_file("state-secret.txt", "x".repeat(32).as_str());
        let toml = format!(
            "{DB_TOML}[oidc]\nissuer_url = \"https://idp.example\"\nclient_id = \"id\"\n\
             client_secret = \"direct-secret\"\nclient_secret_file = \"{secret_path}\"\n\
             redirect_url = \"https://app.example/cb\"\nstate_secret = \"{}\"\n\
             state_secret_file = \"{secret_path}\"\n",
            "y".repeat(32),
            secret_path = secret_path.display()
        );
        let (_, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert!(
            problems.iter().any(|problem| problem
                .contains("oidc.client_secret and oidc.client_secret_file are both set")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|problem| problem
                .contains("oidc.state_secret and oidc.state_secret_file are both set")),
            "{problems:?}"
        );
    }

    #[test]
    fn unreadable_secret_file_is_an_error_naming_the_path() {
        let missing = "/nonexistent/minerva-secret";
        let toml = format!("{DB_TOML}[redis]\nurl_file = \"{missing}\"\n");
        let (_, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(missing), "{problems:?}");
    }

    #[test]
    fn every_validation_problem_is_reported_in_one_run() {
        // An issuer with nothing else: all five missing pieces at once.
        let (config, problems) = sources(
            Some(DB_TOML),
            &[],
            &[("MINERVA_OIDC__ISSUER_URL", "https://idp.example")],
        )
        .expect("extract");
        assert!(config.oidc_enabled());
        for expected in [
            "oidc.client_id is required when oidc.issuer_url is set",
            "oidc.client_secret is required when oidc.issuer_url is set",
            "oidc.redirect_url is required when oidc.issuer_url is set",
            "oidc.state_secret is required when oidc.issuer_url is set",
            "server.web_base_url is required when oidc.issuer_url is set",
        ] {
            assert!(
                problems.iter().any(|problem| problem == expected),
                "{expected} missing from {problems:?}"
            );
        }
    }

    #[test]
    fn state_secret_must_be_at_least_32_bytes() {
        let toml = format!(
            "{DB_TOML}[server]\nweb_base_url = \"https://minerva.example.com\"\n\
             [oidc]\nissuer_url = \"https://idp.example\"\nclient_id = \"id\"\n\
             client_secret = \"secret\"\nredirect_url = \"https://app.example/cb\"\n\
             state_secret = \"short\"\n"
        );
        let (_, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert_eq!(
            problems,
            vec!["oidc.state_secret must be at least 32 bytes".to_owned()]
        );
    }

    #[test]
    fn unparseable_urls_are_rejected() {
        let toml = format!(
            "{DB_TOML}[server]\nweb_base_url = \"not a url\"\n\
             [oidc]\nissuer_url = \"also not a url\"\nclient_id = \"id\"\n\
             client_secret = \"secret\"\nredirect_url = \"https://app.example/cb\"\n\
             state_secret = \"{}\"\n",
            "z".repeat(32)
        );
        let (_, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("oidc.issuer_url is not a valid URL")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem
                    .contains("server.web_base_url must be an absolute http(s) URL")),
            "{problems:?}"
        );
    }

    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let toml = format!("{DB_TOML}[redis]\nurl = \"redis://user:hunter2@localhost:6379\"\n");
        let (config, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        let debug = format!("{config:?}");
        assert!(
            !debug.contains("hunter2"),
            "secret leaked into Debug: {debug}"
        );
        assert!(debug.contains("[redacted]"));
        assert_eq!(format!("{}", config.redis.url), "[redacted]");
    }

    #[test]
    fn web_base_url_keeps_its_value_and_drops_the_trailing_slash() {
        let toml = format!("{DB_TOML}[server]\nweb_base_url = \"https://minerva.example.com/\"\n");
        let (config, problems) = sources(Some(&toml), &[], &[]).expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        // Normalisation happens in `load`; the core keeps the raw value.
        assert_eq!(
            valid_base_url(&config.server.web_base_url).as_deref(),
            Some("https://minerva.example.com")
        );
    }

    #[test]
    fn valid_base_url_requires_an_absolute_http_or_https_url() {
        assert_eq!(
            valid_base_url("https://minerva.example.com").as_deref(),
            Some("https://minerva.example.com")
        );
        // Trailing slashes are stripped so `{base}{path}` never doubles them.
        assert_eq!(
            valid_base_url("https://minerva.example.com/").as_deref(),
            Some("https://minerva.example.com")
        );
        assert_eq!(
            valid_base_url("http://localhost:3010").as_deref(),
            Some("http://localhost:3010")
        );
        for bad in [
            "",
            "minerva.example.com",
            "ftp://x",
            "https://",
            "https:///x",
        ] {
            assert!(valid_base_url(bad).is_none(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn config_path_arg_is_parsed_in_both_forms() {
        let args = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| part.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            config_path_arg(&args(&["minerva-server", "--config", "a.toml"])).unwrap(),
            Some("a.toml".to_owned())
        );
        assert_eq!(
            config_path_arg(&args(&["minerva-server", "--config=b.toml"])).unwrap(),
            Some("b.toml".to_owned())
        );
        assert_eq!(config_path_arg(&args(&["minerva-server"])).unwrap(), None);
        // Other arguments are ignored.
        assert_eq!(
            config_path_arg(&args(&[
                "minerva-server",
                "--other",
                "x",
                "--config",
                "c.toml"
            ]))
            .unwrap(),
            Some("c.toml".to_owned())
        );
        assert!(config_path_arg(&args(&["--config"])).is_err());
    }

    #[test]
    fn discovery_prefers_the_flag_over_minerva_config() {
        let args = vec!["--config=flag.toml".to_owned()];
        let discovered = discover_config_file(&args, Some("env.toml")).unwrap();
        assert_eq!(discovered, Some(("flag.toml".into(), true)));

        let discovered = discover_config_file(&[], Some("  env.toml  ")).unwrap();
        assert_eq!(discovered, Some(("env.toml".into(), true)));

        // A blank MINERVA_CONFIG is not a path: no explicit file comes back.
        let discovered = discover_config_file(&[], Some("   ")).unwrap();
        assert!(!matches!(discovered, Some((_, true))));
    }

    #[test]
    fn an_explicit_missing_file_is_an_error() {
        let err =
            resolved_file(Some(("/nonexistent/minerva.toml".into(), true))).expect_err("must fail");
        assert!(err.contains("/nonexistent/minerva.toml"), "{err}");
        assert!(resolved_file(Some(("/nonexistent/minerva.toml".into(), false))).is_ok());
    }

    #[test]
    fn minerva_config_is_not_treated_as_a_setting() {
        let (config, problems) = sources(
            None,
            &[("DATABASE_URL", "postgresql://a/db")],
            &[
                ("MINERVA_CONFIG", "/etc/minerva/minerva.toml"),
                ("MINERVA_SERVER__PORT", "9000"),
            ],
        )
        .expect("extract");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.server.port, 9000);
    }

    #[test]
    fn legacy_variables_produce_one_warning_with_their_replacements() {
        let vars = pairs(&[
            ("OIDC_CLIENT_ID", "x"),
            ("COOKIE_SECURE", "true"),
            ("MINERVA_SERVER__PORT", "9000"),
            ("DATABASE_URL", "postgresql://a/db"),
        ]);
        let warning = legacy_env_warning(&vars).expect("warning");
        assert!(
            warning.contains("OIDC_CLIENT_ID (MINERVA_OIDC__CLIENT_ID)"),
            "{warning}"
        );
        assert!(
            warning.contains("COOKIE_SECURE (MINERVA_SERVER__COOKIE_SECURE)"),
            "{warning}"
        );
        // The new-scheme and alias variables are not legacy.
        assert!(!warning.contains("MINERVA_SERVER__PORT"), "{warning}");
        assert!(!warning.contains("DATABASE_URL"), "{warning}");

        let vars = pairs(&[("DATABASE_URL", "postgresql://a/db")]);
        assert!(legacy_env_warning(&vars).is_none());
    }
}
