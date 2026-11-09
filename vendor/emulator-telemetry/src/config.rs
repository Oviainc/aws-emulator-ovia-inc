// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Local switch resolution, config-file path resolution, and the server-side
//! telemetry configuration (fetch, ETag/304, TTL cache, sampling, minimum
//! client version). Every policy value referenced here lives in
//! [`crate::policy`].

use crate::outbox::fnv1a;
use crate::policy::{self, TELEMETRY_DEFAULT_ENABLED};
use crate::types::{
    version_at_least, ConfigCacheError, ConfigEtag, InstallationId, MinClientVersion, ProjectId,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Where the effective on/off decision came from, for the startup notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchSource {
    /// A valid [`policy::TELEMETRY_SWITCH_ENV`] value.
    Env,
    /// A valid enabled flag in the emulator config file.
    ConfigFile,
    /// The embedding API (`TelemetryOptions::embedding_opt_out`).
    Api,
    /// The compiled-in [`policy::TELEMETRY_DEFAULT_ENABLED`] default.
    Default,
    /// The environment value was present but invalid — failed safe to off.
    InvalidEnv,
    /// The config file was present but invalid — failed safe to off.
    InvalidFile,
}

/// The resolved local master switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    pub enabled: bool,
    pub source: SwitchSource,
}

/// How one switch source answered.
enum SwitchLayer {
    Silent,
    Resolved(bool),
    Invalid,
}

/// A non-Unicode [`policy::TELEMETRY_SWITCH_ENV`] value is kept (lossily) so
/// it resolves as invalid — off with a warning — rather than as unset.
pub(crate) fn switch_env_value(raw: Option<OsString>) -> Option<String> {
    raw.map(|value| value.to_string_lossy().into_owned())
}

fn env_switch_layer(env_value: Option<&str>) -> SwitchLayer {
    let Some(raw) = env_value else {
        return SwitchLayer::Silent;
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if policy::SWITCH_ON_TOKENS.contains(&normalized.as_str()) {
        SwitchLayer::Resolved(true)
    } else if policy::SWITCH_OFF_TOKENS.contains(&normalized.as_str()) {
        SwitchLayer::Resolved(false)
    } else {
        SwitchLayer::Invalid
    }
}

/// The config file is SHARED emulator configuration: a file without the
/// `[telemetry]` table or the `enabled` key is silent, not invalid. An
/// unreadable file, unparseable TOML or a wrongly-typed section/key fails
/// safe to off.
fn config_file_switch_layer(config_file: Option<Result<&str, &io::Error>>) -> SwitchLayer {
    let contents = match config_file {
        None => return SwitchLayer::Silent,
        Some(Err(_)) => return SwitchLayer::Invalid,
        Some(Ok(contents)) => contents,
    };
    let Ok(table) = contents.parse::<toml::Table>() else {
        return SwitchLayer::Invalid;
    };
    match table.get(policy::CONFIG_TOML_TELEMETRY_SECTION) {
        None => SwitchLayer::Silent,
        Some(toml::Value::Table(telemetry)) => {
            match telemetry.get(policy::CONFIG_FILE_ENABLED_KEY) {
                None => SwitchLayer::Silent,
                Some(toml::Value::Boolean(enabled)) => SwitchLayer::Resolved(*enabled),
                Some(_) => SwitchLayer::Invalid,
            }
        }
        Some(_) => SwitchLayer::Invalid,
    }
}

/// Resolve the local master switch against the compiled-in policy default.
///
/// OFF WINS across the three equal sources — the [`policy::TELEMETRY_SWITCH_ENV`]
/// environment variable, the `[telemetry] enabled` flag in the emulator config
/// file, and the embedding API (`TelemetryOptions::embedding_opt_out`). Any
/// source saying off (or holding an invalid value, which fails safe to off)
/// disables telemetry regardless of the others; otherwise any source saying on
/// enables it; only when every source is silent does the default apply. The
/// embedding layer is DISABLE-ONLY: an embedding application can never force
/// telemetry on over the end user's sources. The reported source is the first
/// deciding layer in env → file → api order.
pub fn resolve_switch(
    env_value: Option<&str>,
    config_file: Option<Result<&str, &io::Error>>,
    embedding_opt_out: bool,
) -> Switch {
    resolve_switch_with_default(
        env_value,
        config_file,
        embedding_opt_out,
        TELEMETRY_DEFAULT_ENABLED,
    )
}

/// [`resolve_switch`] with an explicit default, so tests can prove both
/// values of [`policy::TELEMETRY_DEFAULT_ENABLED`] behave.
pub fn resolve_switch_with_default(
    env_value: Option<&str>,
    config_file: Option<Result<&str, &io::Error>>,
    embedding_opt_out: bool,
    default_enabled: bool,
) -> Switch {
    let api_layer = if embedding_opt_out {
        SwitchLayer::Resolved(false)
    } else {
        SwitchLayer::Silent
    };
    let layers = [
        (
            env_switch_layer(env_value),
            SwitchSource::Env,
            SwitchSource::InvalidEnv,
        ),
        (
            config_file_switch_layer(config_file),
            SwitchSource::ConfigFile,
            SwitchSource::InvalidFile,
        ),
        (api_layer, SwitchSource::Api, SwitchSource::Api),
    ];
    for (layer, source, invalid_source) in &layers {
        match layer {
            SwitchLayer::Resolved(false) => {
                return Switch {
                    enabled: false,
                    source: *source,
                }
            }
            SwitchLayer::Invalid => {
                return Switch {
                    enabled: false,
                    source: *invalid_source,
                }
            }
            _ => {}
        }
    }
    for (layer, source, _) in &layers {
        if let SwitchLayer::Resolved(true) = layer {
            return Switch {
                enabled: true,
                source: *source,
            };
        }
    }
    Switch {
        enabled: default_enabled,
        source: SwitchSource::Default,
    }
}

/// Resolve the emulator config file path: the [`policy::EMULATOR_CONFIG_ENV`]
/// override when set, else the default that constant documents.
pub fn resolve_config_file(env_value: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = env_value.map(str::trim).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    user_config_dir().map(|dir| dir.join(policy::EMULATOR_CONFIG_FILE_NAME))
}

/// Resolve the telemetry state directory: `telemetry/` in the
/// `emulator_paths::emulator_home` folder, regardless of
/// [`policy::EMULATOR_CONFIG_ENV`].
pub(crate) fn resolve_state_dir() -> Option<PathBuf> {
    user_config_dir().map(|dir| dir.join(policy::TELEMETRY_STATE_DIR_NAME))
}

fn user_config_dir() -> Option<PathBuf> {
    emulator_paths::emulator_home()
}

/// Why an endpoint URL was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointError {
    /// The value was empty or whitespace.
    Empty,
    /// The value exceeded [`policy::ENDPOINT_MAX_LEN`].
    TooLong,
    /// The value did not parse as an absolute URL.
    Parse,
    /// The scheme was not `https` (plaintext `http` is allowed only on
    /// loopback hosts, for tests and local development).
    Scheme,
    /// The URL carried credentials (userinfo).
    UserInfo,
    /// The URL carried an explicit port on a non-loopback host.
    Port,
    /// The URL carried no host.
    NoHost,
    /// The URL carried a query string or fragment.
    QueryOrFragment,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            EndpointError::Empty => "must not be empty",
            EndpointError::TooLong => "exceeds maximum length",
            EndpointError::Parse => "must be an absolute URL",
            EndpointError::Scheme => "must use https (http is allowed only on loopback)",
            EndpointError::UserInfo => "must not carry credentials",
            EndpointError::Port => "must not carry an explicit port on a non-loopback host",
            EndpointError::NoHost => "must carry a host",
            EndpointError::QueryOrFragment => "must not carry a query string or fragment",
        };
        write!(f, "invalid telemetry endpoint: {reason}")
    }
}

impl std::error::Error for EndpointError {}

/// A validated telemetry endpoint URL. Parsing enforces the transport rules
/// ONCE — https only (plaintext only on loopback), no credentials, no
/// explicit port off loopback — and every request URL is then built by
/// joining on the validated value, so validation always precedes the request.
/// Host pinning happens against [`Endpoint::same_host`]: the client pins to
/// the injected config endpoint's host and refuses ingest or override
/// endpoints pointing anywhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    url: String,
    host: String,
    loopback: bool,
}

impl Endpoint {
    /// Parse a service BASE endpoint: trailing slashes and an explicit
    /// [`policy::CONFIG_PATH`] suffix are stripped before validation.
    pub fn parse(raw: &str) -> Result<Self, EndpointError> {
        let mut base = raw.trim().trim_end_matches('/').to_string();
        if let Some(stripped) = base.strip_suffix(policy::CONFIG_PATH) {
            base = stripped.trim_end_matches('/').to_string();
        }
        Self::validated(base)
    }

    /// Parse a full ingest endpoint URL (its path is kept as-is).
    pub fn parse_ingest(raw: &str) -> Result<Self, EndpointError> {
        Self::validated(raw.trim().trim_end_matches('/').to_string())
    }

    fn validated(url_string: String) -> Result<Self, EndpointError> {
        if url_string.is_empty() {
            return Err(EndpointError::Empty);
        }
        if url_string.len() > policy::ENDPOINT_MAX_LEN {
            return Err(EndpointError::TooLong);
        }
        let url = reqwest::Url::parse(&url_string).map_err(|_| EndpointError::Parse)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointError::UserInfo);
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(EndpointError::QueryOrFragment);
        }
        let host = url
            .host_str()
            .ok_or(EndpointError::NoHost)?
            .to_ascii_lowercase();
        let bare_host = host.trim_start_matches('[').trim_end_matches(']');
        let loopback = bare_host.eq_ignore_ascii_case("localhost")
            || IpAddr::from_str(bare_host).is_ok_and(|address| address.is_loopback());
        match url.scheme() {
            "https" => {}
            "http" if loopback => {}
            _ => return Err(EndpointError::Scheme),
        }
        if url.port().is_some() && !loopback {
            return Err(EndpointError::Port);
        }
        Ok(Self {
            url: url_string,
            host,
            loopback,
        })
    }

    /// The validated endpoint URL.
    pub fn as_str(&self) -> &str {
        &self.url
    }

    /// The endpoint's lowercased host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether both endpoints point at the same host — the pinning rule for
    /// every ingest endpoint and endpoint override.
    pub fn same_host(&self, other: &Endpoint) -> bool {
        self.host == other.host
    }

    /// The config URL under this base endpoint.
    pub(crate) fn config_url(&self) -> Option<reqwest::Url> {
        reqwest::Url::parse(&format!("{}{}", self.url, policy::CONFIG_PATH)).ok()
    }

    /// The default ingest URL under this base endpoint.
    pub(crate) fn ingest_url(&self) -> String {
        format!("{}{}", self.url, policy::INGEST_PATH)
    }
}

fn lenient_ingest_endpoint<'de, D>(deserializer: D) -> Result<Option<Endpoint>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = serde::Deserialize::deserialize(deserializer)?;
    Ok(raw
        .as_deref()
        .and_then(|value| Endpoint::parse_ingest(value).ok()))
}

fn lenient_min_client_version<'de, D>(deserializer: D) -> Result<Option<MinClientVersion>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = serde::Deserialize::deserialize(deserializer)?;
    Ok(raw.as_deref().and_then(MinClientVersion::new))
}

fn serialize_ingest_endpoint<S: serde::Serializer>(
    value: &Option<Endpoint>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(&value.as_ref().map(Endpoint::as_str), serializer)
}

fn serialize_min_client_version<S: serde::Serializer>(
    value: &Option<MinClientVersion>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(&value.as_ref().map(MinClientVersion::as_str), serializer)
}

/// The server-side telemetry configuration served by the config endpoint.
/// Unknown fields are ignored; out-of-range values are clamped by
/// [`ServerConfig::clamped`]. Defaults come from the [`crate::policy`]
/// fallbacks and fill in only fields a server response leaves absent; with
/// neither a response nor a cached config there is no config at all and the
/// client collects nothing. Serialization is the canonical form the cache
/// persists: known fields only, validated values only.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    pub enabled: bool,
    pub flush_interval_seconds: u64,
    pub max_rows_per_batch: usize,
    pub max_body_bytes: usize,
    pub sampling_rate: f64,
    #[serde(
        deserialize_with = "lenient_min_client_version",
        serialize_with = "serialize_min_client_version"
    )]
    pub min_supported_client_version: Option<MinClientVersion>,
    #[serde(
        deserialize_with = "lenient_ingest_endpoint",
        serialize_with = "serialize_ingest_endpoint"
    )]
    pub ingest_endpoint: Option<Endpoint>,
    pub config_ttl_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            enabled: policy::FALLBACK_ENABLED,
            flush_interval_seconds: policy::FALLBACK_FLUSH_INTERVAL_SECONDS,
            max_rows_per_batch: policy::FALLBACK_MAX_ROWS_PER_BATCH,
            max_body_bytes: policy::FALLBACK_MAX_BODY_BYTES,
            sampling_rate: policy::SAMPLING_DEFAULT,
            min_supported_client_version: None,
            ingest_endpoint: None,
            config_ttl_seconds: policy::FALLBACK_CONFIG_TTL_SECONDS,
        }
    }
}

impl ServerConfig {
    /// Clamp every tunable into its [`crate::policy`] range, so a hostile or
    /// corrupt config value cannot wedge the flush loop or the batch builder.
    pub fn clamped(mut self) -> Self {
        let (flush_min, flush_max) = policy::FLUSH_INTERVAL_CLAMP_SECONDS;
        self.flush_interval_seconds = self.flush_interval_seconds.clamp(flush_min, flush_max);
        let (rows_min, rows_max) = policy::ROWS_PER_BATCH_CLAMP;
        self.max_rows_per_batch = self.max_rows_per_batch.clamp(rows_min, rows_max);
        let (bytes_min, bytes_max) = policy::BODY_BYTES_CLAMP;
        self.max_body_bytes = self.max_body_bytes.clamp(bytes_min, bytes_max);
        let (rate_min, rate_max) = policy::SAMPLING_RATE_CLAMP;
        if !self.sampling_rate.is_finite() {
            self.sampling_rate = rate_min;
        }
        self.sampling_rate = self.sampling_rate.clamp(rate_min, rate_max);
        let (ttl_min, ttl_max) = policy::CONFIG_TTL_CLAMP_SECONDS;
        self.config_ttl_seconds = self.config_ttl_seconds.clamp(ttl_min, ttl_max);
        self
    }

    /// Whether this client version satisfies `minSupportedClientVersion`.
    /// Fail-open: an absent or unparseable minimum never disables the client.
    pub fn client_version_supported(&self, client_version: &str) -> bool {
        match &self.min_supported_client_version {
            Some(min) => version_at_least(client_version, min.as_str()).unwrap_or(true),
            None => true,
        }
    }

    /// Why this config forbids collection for `installation_id`, if it does.
    /// The three server-side gates in precedence order: the client version
    /// gate, the kill switch, and per-installation sampling. `None` means the
    /// session may record. Decided BEFORE anything is stored: a denial gates
    /// collection, not merely transmission.
    pub(crate) fn collection_denial(
        &self,
        installation_id: &InstallationId,
    ) -> Option<CollectionDenial> {
        if !self.client_version_supported(policy::CLIENT_VERSION) {
            return Some(CollectionDenial::UnsupportedVersion);
        }
        if !self.enabled {
            return Some(CollectionDenial::RemoteDisabled);
        }
        if !is_sampled(installation_id, self.sampling_rate) {
            return Some(CollectionDenial::NotSampled);
        }
        None
    }
}

/// Why the server config denies collection for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectionDenial {
    /// `minSupportedClientVersion` is above this client.
    UnsupportedVersion,
    /// The server kill switch (`enabled: false`).
    RemoteDisabled,
    /// This installation falls outside `samplingRate`.
    NotSampled,
}

/// Deterministic per-installation sampling: the same installation id always
/// lands on the same side of the rate, so participation is stable across
/// sessions rather than re-rolled per flush. The FNV-1a hash is passed
/// through a splitmix64 finalizer because raw FNV-1a of short strings has
/// badly biased high bits.
pub fn is_sampled(installation_id: &InstallationId, sampling_rate: f64) -> bool {
    if sampling_rate >= 1.0 {
        return true;
    }
    if sampling_rate <= 0.0 {
        return false;
    }
    let hash = mix64(fnv1a(installation_id.as_str().as_bytes()));
    ((hash as f64) / (u64::MAX as f64)) < sampling_rate
}

/// splitmix64 finalizer: full-avalanche mixing of a 64-bit value.
fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// The server's last-seen configuration, persisted as
/// [`policy::SERVER_CONFIG_CACHE_FILE`] next to the outbox but deliberately
/// NOT inside it: every effective-off purge deletes the installation id and
/// the outbox, and the server's decision has to survive that so a server last
/// seen disabling collection, not sampling this installation, or no longer
/// supporting this client stays in force while offline.
///
/// The file maps project ids to [`CacheEntry`] values and holds only the
/// parsed configuration re-serialized canonically (never the server's raw
/// bytes), a bounded ETag, and timestamps — nothing about this installation
/// or its traffic. Every update is a read-modify-write of the current file
/// under an exclusive advisory lock on [`policy::SERVER_CONFIG_LOCK_FILE`],
/// committed through a synced staging file and an atomic rename, so a reader
/// (which needs no lock) always sees one complete generation and concurrent
/// emulator processes can neither lose each other's writes nor revive a stale
/// decision. The only fail-open read is an ABSENT cache; anything that stops
/// the client from trusting the file is an error the caller must act on.
pub(crate) struct ServerConfigCache {
    path: PathBuf,
    lock_path: PathBuf,
    staged_path: PathBuf,
}

type CacheEntries = BTreeMap<String, CacheEntry>;

impl ServerConfigCache {
    pub(crate) fn open(state_dir: &Path) -> Self {
        Self {
            path: state_dir.join(policy::SERVER_CONFIG_CACHE_FILE),
            lock_path: state_dir.join(policy::SERVER_CONFIG_LOCK_FILE),
            staged_path: state_dir.join(policy::SERVER_CONFIG_STAGED_FILE),
        }
    }

    /// The project's entry from one complete generation of the file.
    /// `Ok(None)` when the cache is absent or has no entry for the project;
    /// `Err` when the file exists but cannot be read or is not this client's
    /// bounded, canonical shape.
    pub(crate) fn load(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<CacheEntry>, ConfigCacheError> {
        Ok(self
            .read_entries()?
            .and_then(|mut entries| entries.remove(project_id.as_str())))
    }

    /// Apply `change` to the project's entry AS IT IS NOW, under the exclusive
    /// lock, and commit the whole file atomically. `change` decides what to
    /// keep — a refresh that raced a newer decision from another process
    /// compares against its own snapshot and leaves the newer one in place.
    /// A file that is not this client's shape is replaced, not trusted; an
    /// update that would exceed [`policy::SERVER_CONFIG_CACHE_MAX_BYTES`] is
    /// refused and the previous file stays.
    pub(crate) fn update<R>(
        &self,
        project_id: &ProjectId,
        change: impl FnOnce(&mut CacheEntry) -> R,
    ) -> Result<R, ConfigCacheError> {
        let dir = self.path.parent().ok_or(ConfigCacheError {
            kind: io::ErrorKind::NotFound,
        })?;
        std::fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)?;
        lock.lock()?;
        let mut entries = match self.read_entries() {
            Ok(entries) => entries.unwrap_or_default(),
            Err(error) if error.kind == io::ErrorKind::InvalidData => CacheEntries::new(),
            Err(error) => return Err(error),
        };
        let result = change(entries.entry(project_id.as_str().to_string()).or_default());
        let encoded = serde_json::to_vec(&entries).map_err(|_| ConfigCacheError {
            kind: io::ErrorKind::InvalidData,
        })?;
        if encoded.len() > policy::SERVER_CONFIG_CACHE_MAX_BYTES {
            return Err(ConfigCacheError {
                kind: io::ErrorKind::FileTooLarge,
            });
        }
        self.commit(&encoded)?;
        Ok(result)
    }

    fn read_entries(&self) -> Result<Option<CacheEntries>, ConfigCacheError> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let limit = policy::SERVER_CONFIG_CACHE_MAX_BYTES;
        let mut raw = Vec::new();
        file.take(limit.saturating_add(1) as u64)
            .read_to_end(&mut raw)?;
        if raw.len() > limit {
            return Err(ConfigCacheError {
                kind: io::ErrorKind::InvalidData,
            });
        }
        serde_json::from_slice(&raw)
            .map(Some)
            .map_err(|_| ConfigCacheError {
                kind: io::ErrorKind::InvalidData,
            })
    }

    fn commit(&self, encoded: &[u8]) -> Result<(), ConfigCacheError> {
        let written = File::create(&self.staged_path).and_then(|mut staged| {
            staged.write_all(encoded)?;
            staged.sync_all()
        });
        let committed = written.and_then(|()| std::fs::rename(&self.staged_path, &self.path));
        if committed.is_err() {
            let _ = std::fs::remove_file(&self.staged_path);
        }
        Ok(committed?)
    }
}

/// One project's cache entry: the last fetched configuration (absent until
/// the first successful fetch) plus the earliest time a failed fetch may be
/// retried.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CacheEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched: Option<CachedConfig>,
    #[serde(default)]
    pub retry_after: u64,
}

/// The cached server config plus its cache metadata.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CachedConfig {
    pub config: ServerConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<ConfigEtag>,
    pub fetched_at: u64,
}

/// Load the cached server config: `Ok(None)` when none was ever fetched.
pub(crate) fn load_cached_config(
    cache: &ServerConfigCache,
    project_id: &ProjectId,
) -> Result<Option<CachedConfig>, ConfigCacheError> {
    Ok(cache.load(project_id)?.and_then(|entry| entry.fetched))
}

/// The cached decision, if any: without one the server has never allowed
/// collection, so there is nothing to fall back to.
fn cached_decision(cached: &Option<CachedConfig>) -> Option<ServerConfig> {
    cached.as_ref().map(|entry| entry.config.clone())
}

fn read_config_response(
    response: reqwest::blocking::Response,
) -> Option<(String, Option<ConfigEtag>)> {
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .and_then(ConfigEtag::new);
    let limit = policy::CONFIG_RESPONSE_MAX_BODY_BYTES.saturating_add(1) as u64;
    let mut body = Vec::new();
    response.take(limit).read_to_end(&mut body).ok()?;
    if body.len() > policy::CONFIG_RESPONSE_MAX_BODY_BYTES {
        return None;
    }
    Some((String::from_utf8(body).ok()?, etag))
}

/// The outcome of one config refresh. `config` is the configuration in force
/// — `None` when there is no trustworthy decision at all: the server gave no
/// usable response AND no cached decision exists or it could not be read, in
/// which case the caller must not collect. `cache_error` is the first cache
/// read or write failure met on the way, whether or not a decision was still
/// reached.
#[derive(Debug)]
pub(crate) struct ConfigRefresh {
    pub config: Option<ServerConfig>,
    pub cache_error: Option<ConfigCacheError>,
}

impl ConfigRefresh {
    fn decided(config: Option<ServerConfig>) -> Self {
        Self {
            config,
            cache_error: None,
        }
    }

    fn from_update(
        applied: Result<Option<ServerConfig>, ConfigCacheError>,
        fallback: Option<ServerConfig>,
    ) -> Self {
        match applied {
            Ok(config) => Self::decided(config),
            Err(error) => Self {
                config: fallback,
                cache_error: Some(error),
            },
        }
    }
}

/// A refresh that obtained nothing new from the server: pace the next retry
/// and answer with the decision currently on disk (which another process may
/// have refreshed meanwhile). With no cached decision, or none readable, there
/// is no decision.
fn settle_without_response(
    cache: &ServerConfigCache,
    project_id: &ProjectId,
    now: u64,
    snapshot: &Option<CachedConfig>,
    load_error: Option<ConfigCacheError>,
) -> ConfigRefresh {
    if let Some(error) = load_error {
        return ConfigRefresh {
            config: None,
            cache_error: Some(error),
        };
    }
    let retry_after = now.saturating_add(policy::CONFIG_FETCH_RETRY_INTERVAL.as_secs());
    let applied = cache.update(project_id, |entry| {
        entry.retry_after = retry_after;
        cached_decision(&entry.fetched)
    });
    ConfigRefresh::from_update(applied, cached_decision(snapshot))
}

/// Return the current server config, refreshing over HTTP when the TTL has
/// lapsed. Honors `ETag`/`If-None-Match` (a 304 revalidates the cache without
/// a body). On a fetch or parse failure the cached decision stays in force;
/// with no cache at all there is no decision — the client never collects on
/// terms the server did not set, and fails silent on a broken config
/// endpoint. A response is applied to the cache only if the entry is still
/// the one the request was made from; otherwise the newer decision another
/// process stored meanwhile wins and is returned. A cache that exists but cannot be read is never mistaken for an
/// absent one: a fresh server response still decides, but without one the
/// refresh yields no decision.
pub(crate) fn current_config(
    http: &reqwest::blocking::Client,
    cache: &ServerConfigCache,
    base_endpoint: &Endpoint,
    project_id: &ProjectId,
    now_epoch_seconds: u64,
) -> ConfigRefresh {
    let (entry, load_error) = match cache.load(project_id) {
        Ok(entry) => (entry.unwrap_or_default(), None),
        Err(error) => (CacheEntry::default(), Some(error)),
    };
    let snapshot = entry.fetched;
    if load_error.is_none() {
        if let Some(cached) = &snapshot {
            let ttl = cached.config.config_ttl_seconds;
            if now_epoch_seconds < cached.fetched_at.saturating_add(ttl) {
                return ConfigRefresh::decided(Some(cached.config.clone()));
            }
        }
        if now_epoch_seconds < entry.retry_after {
            return ConfigRefresh::decided(cached_decision(&snapshot));
        }
    }

    let Some(url) = base_endpoint.config_url() else {
        return settle_without_response(
            cache,
            project_id,
            now_epoch_seconds,
            &snapshot,
            load_error,
        );
    };
    let mut request = http.get(url);
    if let Some(etag) = snapshot.as_ref().and_then(|entry| entry.etag.as_ref()) {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag.as_str());
    }
    match request.send() {
        Ok(response) if response.status() == reqwest::StatusCode::NOT_MODIFIED => {
            if snapshot.is_none() {
                return settle_without_response(
                    cache,
                    project_id,
                    now_epoch_seconds,
                    &snapshot,
                    load_error,
                );
            }
            let applied = cache.update(project_id, |entry| {
                if entry.fetched == snapshot {
                    if let Some(fetched) = &mut entry.fetched {
                        fetched.fetched_at = now_epoch_seconds;
                    }
                    entry.retry_after = 0;
                }
                cached_decision(&entry.fetched)
            });
            ConfigRefresh::from_update(applied, cached_decision(&snapshot))
        }
        Ok(response) if response.status().is_success() => {
            let parsed = read_config_response(response).and_then(|(body, etag)| {
                serde_json::from_str::<ServerConfig>(&body)
                    .ok()
                    .map(|config| (config.clamped(), etag))
            });
            let Some((config, etag)) = parsed else {
                return settle_without_response(
                    cache,
                    project_id,
                    now_epoch_seconds,
                    &snapshot,
                    load_error,
                );
            };
            let fetched = CachedConfig {
                config: config.clone(),
                etag,
                fetched_at: now_epoch_seconds,
            };
            let applied = cache.update(project_id, |entry| {
                if entry.fetched == snapshot {
                    entry.fetched = Some(fetched);
                    entry.retry_after = 0;
                }
                cached_decision(&entry.fetched)
            });
            ConfigRefresh::from_update(applied, Some(config))
        }
        _ => settle_without_response(cache, project_id, now_epoch_seconds, &snapshot, load_error),
    }
}

/// Build the blocking HTTP client used for config and ingest calls: short,
/// bounded [`crate::policy`] timeouts so telemetry can never stall emulator
/// shutdown for long, and NO redirect following — a redirecting endpoint can
/// never steer telemetry toward an unvalidated host.
///
/// Safe to call from inside a tokio runtime: the client is built on a
/// dedicated thread with no runtime context, because reqwest's blocking
/// builder creates and drops a runtime on the calling thread, which panics
/// inside an async context.
pub fn build_http_client() -> Option<reqwest::blocking::Client> {
    std::thread::Builder::new()
        .name(policy::HTTP_CLIENT_BUILD_THREAD_NAME.to_string())
        .spawn(|| {
            reqwest::blocking::Client::builder()
                .connect_timeout(policy::HTTP_CONNECT_TIMEOUT)
                .timeout(policy::HTTP_REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok()
        })
        .ok()?
        .join()
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{refused_base_url, TestServer};

    #[test]
    fn any_off_wins_across_all_switch_source_combinations() {
        let states = [None, Some(true), Some(false)];
        for default_enabled in [false, true] {
            for env in states {
                for file in states {
                    for opt_out in [false, true] {
                        let env_value = env.map(|on| {
                            if on {
                                policy::SWITCH_ON
                            } else {
                                policy::SWITCH_OFF
                            }
                        });
                        let file_contents = file.map(|on| {
                            format!(
                                "[{}]\n{} = {on}\n",
                                policy::CONFIG_TOML_TELEMETRY_SECTION,
                                policy::CONFIG_FILE_ENABLED_KEY
                            )
                        });
                        let resolved = resolve_switch_with_default(
                            env_value,
                            file_contents.as_deref().map(Ok),
                            opt_out,
                            default_enabled,
                        );
                        let api = opt_out.then_some(false);
                        let any_off = [env, file, api].contains(&Some(false));
                        let any_on = [env, file].contains(&Some(true));
                        let expected = if any_off {
                            false
                        } else if any_on {
                            true
                        } else {
                            default_enabled
                        };
                        assert_eq!(
                            resolved.enabled, expected,
                            "off wins: env={env:?} file={file:?} opt_out={opt_out} \
                             default={default_enabled}"
                        );
                        assert_eq!(
                            resolved.source == SwitchSource::Default,
                            env.is_none() && file.is_none() && !opt_out,
                            "the default layer answers only when every source is silent"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_switch_token_is_matched_case_insensitively() {
        for (tokens, expected) in [
            (policy::SWITCH_ON_TOKENS, true),
            (policy::SWITCH_OFF_TOKENS, false),
        ] {
            for token in tokens {
                for variant in [
                    token.to_string(),
                    token.to_ascii_uppercase(),
                    format!("  {token}  "),
                    format!("{}{}", token[..1].to_ascii_uppercase(), &token[1..]),
                ] {
                    let resolved = resolve_switch(Some(&variant), None, false);
                    assert_eq!(
                        (resolved.enabled, resolved.source),
                        (expected, SwitchSource::Env),
                        "token variant {variant:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_embedding_api_cannot_force_telemetry_on() {
        for default_enabled in [false, true] {
            let silent = resolve_switch_with_default(None, None, false, default_enabled);
            assert_eq!(
                silent.enabled, default_enabled,
                "a non-opted-out embedding is SILENT, not an enable vote"
            );
            assert_eq!(silent.source, SwitchSource::Default);
        }
        let opted_out = resolve_switch(Some(policy::SWITCH_ON), None, true);
        assert_eq!(
            opted_out,
            Switch {
                enabled: false,
                source: SwitchSource::Api
            },
            "embedding opt-out wins even against an explicit env on"
        );
        let api_decides = resolve_switch(None, None, true);
        assert_eq!(
            api_decides,
            Switch {
                enabled: false,
                source: SwitchSource::Api
            }
        );
    }

    #[test]
    fn the_notice_source_is_the_first_deciding_layer() {
        let file_on = "[telemetry]\nenabled = true\n";
        let file_off = "[telemetry]\nenabled = false\n";
        let cases = [
            (
                Some("on"),
                Some(file_off),
                false,
                false,
                SwitchSource::ConfigFile,
            ),
            (Some("off"), Some(file_on), false, false, SwitchSource::Env),
            (None, Some(file_on), true, false, SwitchSource::Api),
            (Some("on"), None, false, true, SwitchSource::Env),
            (None, Some(file_on), false, true, SwitchSource::ConfigFile),
            (Some("true"), None, false, true, SwitchSource::Env),
        ];
        for (env, file, opt_out, enabled, source) in cases {
            assert_eq!(
                resolve_switch(env, file.map(Ok), opt_out),
                Switch { enabled, source },
                "env={env:?} file={file:?} opt_out={opt_out:?}"
            );
        }
    }

    #[test]
    fn flipping_the_compiled_default_flips_only_the_default_layer() {
        for default_enabled in [false, true] {
            let resolved = resolve_switch_with_default(None, None, false, default_enabled);
            assert_eq!(
                resolved,
                Switch {
                    enabled: default_enabled,
                    source: SwitchSource::Default
                }
            );
            assert!(
                !resolve_switch_with_default(Some("off"), None, false, default_enabled).enabled,
                "an explicit off always wins over the default"
            );
            assert!(
                resolve_switch_with_default(Some("on"), None, false, default_enabled).enabled,
                "an explicit on always wins over the default"
            );
            assert!(
                !resolve_switch_with_default(None, None, true, default_enabled).enabled,
                "an embedding opt-out always wins over the default"
            );
        }
    }

    #[test]
    fn invalid_switch_layers_fail_safe_to_off() {
        let file_on = "[telemetry]\nenabled = true\n";
        for bad_env in ["yes", "no", "enabled", "2", "onn", "off please", ""] {
            let switch = resolve_switch(Some(bad_env), Some(Ok(file_on)), false);
            assert_eq!(
                switch,
                Switch {
                    enabled: false,
                    source: SwitchSource::InvalidEnv
                },
                "env {bad_env:?} must fail safe, not fall through"
            );
        }
        for bad_file in [
            "not toml [",
            "[telemetry]\nenabled = \"yes\"",
            "[telemetry]\nenabled = 1",
            "telemetry = 5",
        ] {
            let switch = resolve_switch(None, Some(Ok(bad_file)), false);
            assert_eq!(
                switch,
                Switch {
                    enabled: false,
                    source: SwitchSource::InvalidFile
                },
                "file {bad_file:?} must fail safe, not fall through"
            );
        }
        let unreadable = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(
            resolve_switch(None, Some(Err(&unreadable)), false),
            Switch {
                enabled: false,
                source: SwitchSource::InvalidFile
            },
            "an unreadable config file may hold an opt-out, so it fails safe"
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let non_unicode = switch_env_value(Some(OsString::from_vec(b"of\xff".to_vec())));
            assert_eq!(
                resolve_switch(non_unicode.as_deref(), Some(Ok(file_on)), false),
                Switch {
                    enabled: false,
                    source: SwitchSource::InvalidEnv
                },
                "a non-Unicode env value is another value, not an unset one"
            );
        }
    }

    #[test]
    fn a_shared_config_file_without_the_telemetry_flag_is_silent_not_invalid() {
        for silent_file in [
            "",
            "# just a comment\n",
            "[telemetry]\n",
            "[other]\nkey = true\n",
            "[telemetry]\nsampling_note = \"ignored\"\n",
        ] {
            let switch = resolve_switch(None, Some(Ok(silent_file)), false);
            assert_eq!(
                switch.source,
                SwitchSource::Default,
                "file {silent_file:?} carries no telemetry decision"
            );
        }
    }

    #[test]
    fn the_default_layer_matches_the_compiled_policy_default() {
        let resolved = resolve_switch(None, None, false);
        assert_eq!(
            resolved,
            Switch {
                enabled: TELEMETRY_DEFAULT_ENABLED,
                source: SwitchSource::Default
            },
            "the default layer derives from policy::TELEMETRY_DEFAULT_ENABLED alone \
             (flip it in policy.rs; no test edit required)"
        );
    }

    #[test]
    fn the_config_file_resolves_from_the_env_override_else_the_home_convention() {
        assert_eq!(
            resolve_config_file(Some("/tmp/custom/config.toml")),
            Some(PathBuf::from("/tmp/custom/config.toml"))
        );
        assert_eq!(resolve_config_file(Some("  ")), resolve_config_file(None));
        if let Some(default_path) = resolve_config_file(None) {
            let rendered = default_path.to_string_lossy().into_owned();
            assert!(
                rendered.ends_with(&format!(
                    "{}/{}",
                    policy::CONFIG_DIR_NAME,
                    policy::EMULATOR_CONFIG_FILE_NAME
                )),
                "default path {rendered} must live in the home config directory"
            );
        }
    }

    #[test]
    fn the_state_dir_stays_in_the_home_config_directory_when_the_config_file_moves() {
        let home_config_dir =
            resolve_config_file(None).and_then(|path| path.parent().map(Path::to_path_buf));
        assert_eq!(
            resolve_state_dir(),
            home_config_dir.map(|dir| dir.join(policy::TELEMETRY_STATE_DIR_NAME))
        );
        assert_ne!(
            resolve_state_dir(),
            Some(PathBuf::from("/tmp/custom").join(policy::TELEMETRY_STATE_DIR_NAME)),
            "EMULATOR_CONFIG moves only the config file"
        );
    }

    #[test]
    fn endpoints_parse_once_and_enforce_the_transport_rules() {
        assert_eq!(
            Endpoint::parse("https://t.example.com/v1/config")
                .unwrap()
                .as_str(),
            "https://t.example.com"
        );
        assert_eq!(
            Endpoint::parse("https://t.example.com/v1/config/")
                .unwrap()
                .as_str(),
            "https://t.example.com"
        );
        assert_eq!(
            Endpoint::parse("http://127.0.0.1:8080/").unwrap().as_str(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            Endpoint::parse("http://localhost:8080").unwrap().as_str(),
            "http://localhost:8080"
        );
        assert_eq!(
            Endpoint::parse("http://t.example.com").unwrap_err(),
            EndpointError::Scheme
        );
        assert_eq!(
            Endpoint::parse("t.example.com").unwrap_err(),
            EndpointError::Parse
        );
        assert_eq!(Endpoint::parse("  ").unwrap_err(), EndpointError::Empty);
        assert_eq!(
            Endpoint::parse("https://api.example.com:8443").unwrap_err(),
            EndpointError::Port,
            "an explicit port is allowed only on loopback"
        );
        assert_eq!(
            Endpoint::parse("https://user@api.example.com").unwrap_err(),
            EndpointError::UserInfo
        );
        assert_eq!(
            Endpoint::parse("https://user:pw@api.example.com").unwrap_err(),
            EndpointError::UserInfo
        );
        assert_eq!(
            Endpoint::parse(&format!("https://t.example.com/{}", "a".repeat(2100))).unwrap_err(),
            EndpointError::TooLong
        );
    }

    #[test]
    fn endpoint_host_pinning_is_case_insensitive_and_scheme_agnostic() {
        let base = Endpoint::parse("https://T.Example.com").unwrap();
        assert_eq!(base.host(), "t.example.com");
        let same = Endpoint::parse_ingest("https://t.example.com/v1/events").unwrap();
        let other = Endpoint::parse_ingest("https://evil.example.com/v1/events").unwrap();
        assert!(base.same_host(&same));
        assert!(!base.same_host(&other));
    }

    #[test]
    fn ingest_endpoints_keep_their_path_and_follow_the_same_rules() {
        assert_eq!(
            Endpoint::parse_ingest("https://ingest.example.com/v1/events")
                .unwrap()
                .as_str(),
            "https://ingest.example.com/v1/events"
        );
        assert_eq!(
            Endpoint::parse_ingest("https://ingest.example.com:8443/v1/events").unwrap_err(),
            EndpointError::Port
        );
        assert_eq!(
            Endpoint::parse_ingest("http://127.0.0.1:9000/v1/events")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:9000/v1/events"
        );
    }

    #[test]
    fn request_urls_are_built_from_the_validated_endpoint() {
        let base = Endpoint::parse("https://t.example.com").unwrap();
        let config_url = base.config_url().unwrap();
        assert_eq!(config_url.as_str(), "https://t.example.com/v1/config");
        assert_eq!(base.ingest_url(), "https://t.example.com/v1/events");
    }

    #[test]
    fn endpoints_carrying_a_query_or_fragment_are_rejected() {
        for raw in [
            "https://t.example.com/?product=other",
            "https://t.example.com/#frag",
            "https://t.example.com/v1/events?x=1",
        ] {
            assert_eq!(
                Endpoint::parse(raw).unwrap_err(),
                EndpointError::QueryOrFragment,
                "{raw}"
            );
            assert_eq!(
                Endpoint::parse_ingest(raw).unwrap_err(),
                EndpointError::QueryOrFragment,
                "{raw}"
            );
        }
        assert!(Endpoint::parse("https://t.example.com/base/").is_ok());
    }

    const TEST_PROJECT_ID: &str = "proj-0123456789abcdef0123456789abcdef";

    fn test_project() -> ProjectId {
        ProjectId::new(TEST_PROJECT_ID).unwrap()
    }

    fn fetched(config: ServerConfig, etag: Option<&str>, fetched_at: u64) -> CachedConfig {
        CachedConfig {
            config,
            etag: etag.and_then(ConfigEtag::new),
            fetched_at,
        }
    }

    #[test]
    fn the_cache_persists_only_the_canonical_parsed_config() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ServerConfigCache::open(dir.path());
        let raw = r#"{
            "enabled": false,
            "configTtlSeconds": 3600,
            "operatorNote": "SENTINEL-NOT-FOR-DISK user@example.com",
            "minSupportedClientVersion": "1.0.0"
        }"#;
        let config = serde_json::from_str::<ServerConfig>(raw).unwrap().clamped();
        let entry = fetched(config.clone(), Some("\"etag-1\""), 1_000);
        cache
            .update(&test_project(), |current| {
                current.fetched = Some(entry.clone())
            })
            .unwrap();

        let on_disk =
            std::fs::read_to_string(dir.path().join(policy::SERVER_CONFIG_CACHE_FILE)).unwrap();
        assert!(
            !on_disk.contains("SENTINEL") && !on_disk.contains("example.com"),
            "unknown server fields never reach the disk: {on_disk}"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        let mut keys: Vec<&str> = parsed[TEST_PROJECT_ID]["fetched"]["config"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "configTtlSeconds",
                "enabled",
                "flushIntervalSeconds",
                "ingestEndpoint",
                "maxBodyBytes",
                "maxRowsPerBatch",
                "minSupportedClientVersion",
                "samplingRate",
            ]
        );
        let loaded = load_cached_config(&cache, &test_project())
            .unwrap()
            .unwrap();
        assert_eq!(loaded, entry, "the canonical form round-trips exactly");
        assert!(!loaded.config.enabled);
    }

    #[test]
    fn etags_are_bounded_visible_ascii_or_dropped() {
        assert_eq!(
            ConfigEtag::new(" \"abc-123\" ").map(|e| e.as_str().to_string()),
            Some("\"abc-123\"".to_string())
        );
        assert!(ConfigEtag::new("").is_none());
        assert!(ConfigEtag::new("has space").is_none());
        assert!(ConfigEtag::new("tab\there").is_none());
        assert!(ConfigEtag::new("caf\u{e9}").is_none());
        assert!(ConfigEtag::new("x".repeat(policy::CONFIG_ETAG_MAX_LEN)).is_some());
        assert!(ConfigEtag::new("x".repeat(policy::CONFIG_ETAG_MAX_LEN + 1)).is_none());
        let rejected: Result<ConfigEtag, _> = serde_json::from_str("\"has space\"");
        assert!(
            rejected.is_err(),
            "a stored invalid etag invalidates the entry"
        );
    }

    #[test]
    fn concurrent_cache_writers_never_lose_or_mix_a_stored_decision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let disabled = fetched(
            ServerConfig {
                enabled: false,
                ..ServerConfig::default()
            },
            Some("\"disabled\""),
            5_000,
        );
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let store = {
            let path = path.clone();
            let disabled = disabled.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let cache = ServerConfigCache::open(&path);
                barrier.wait();
                cache
                    .update(&test_project(), |entry| entry.fetched = Some(disabled))
                    .unwrap();
            })
        };
        let pacers: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let cache = ServerConfigCache::open(&path);
                    barrier.wait();
                    for tick in 1..=200u64 {
                        cache
                            .update(&test_project(), |entry| entry.retry_after = tick)
                            .unwrap();
                        if let Ok(Some(entry)) = cache.load(&test_project()) {
                            if let Some(seen) = &entry.fetched {
                                assert_eq!(seen.etag.as_ref().unwrap().as_str(), "\"disabled\"");
                                assert!(!seen.config.enabled);
                            }
                        }
                    }
                })
            })
            .collect();
        store.join().unwrap();
        for pacer in pacers {
            pacer.join().unwrap();
        }
        let cache = ServerConfigCache::open(&path);
        let entry = cache.load(&test_project()).unwrap().unwrap();
        assert_eq!(
            entry.fetched,
            Some(disabled),
            "the stored denial survives every concurrent retry-pacing write"
        );
        assert!((1..=200).contains(&entry.retry_after));
    }

    #[test]
    fn an_oversized_or_corrupt_cache_file_is_an_error_not_a_decision() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ServerConfigCache::open(dir.path());
        let path = dir.path().join(policy::SERVER_CONFIG_CACHE_FILE);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(
            cache.load(&test_project()).unwrap_err().kind,
            io::ErrorKind::InvalidData
        );
        std::fs::write(&path, "x".repeat(policy::SERVER_CONFIG_CACHE_MAX_BYTES + 1)).unwrap();
        assert_eq!(
            cache.load(&test_project()).unwrap_err().kind,
            io::ErrorKind::InvalidData
        );
        cache
            .update(&test_project(), |entry| entry.retry_after = 7)
            .unwrap();
        assert_eq!(
            cache.load(&test_project()).unwrap().unwrap().retry_after,
            7,
            "a fresh write replaces a file that is not this client's shape"
        );
        assert!(
            std::fs::metadata(&path).unwrap().len() as usize
                <= policy::SERVER_CONFIG_CACHE_MAX_BYTES
        );
        assert!(
            !dir.path().join(policy::SERVER_CONFIG_STAGED_FILE).exists(),
            "the staging file is renamed away on commit"
        );
    }

    #[test]
    fn an_unwritable_cache_reports_a_typed_error_without_a_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(policy::SERVER_CONFIG_CACHE_FILE)).unwrap();
        let cache = ServerConfigCache::open(dir.path());
        let error = cache
            .update(&test_project(), |entry| entry.retry_after = 1)
            .unwrap_err();
        assert_ne!(error.kind, io::ErrorKind::Other);
        assert!(!error.to_string().contains(dir.path().to_str().unwrap()));
        assert!(
            cache.load(&test_project()).is_err(),
            "a cache that exists but cannot be read is never an empty one"
        );
    }

    #[test]
    fn an_oversized_update_is_refused_and_keeps_the_previous_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ServerConfigCache::open(dir.path());
        let denial = fetched(
            ServerConfig {
                enabled: false,
                ..ServerConfig::default()
            },
            Some("\"disabled\""),
            5,
        );
        cache
            .update(&test_project(), |entry| {
                entry.fetched = Some(denial.clone())
            })
            .unwrap();
        let wide_etag = "e".repeat(policy::CONFIG_ETAG_MAX_LEN);
        let refused = (0..10_000u64).find_map(|n| {
            let project = ProjectId::new(format!("proj-{n:032x}")).unwrap();
            let entry = fetched(ServerConfig::default(), Some(&wide_etag), n);
            cache
                .update(&project, |current| current.fetched = Some(entry))
                .err()
        });
        let error = refused.expect("growth past the cache bound is refused");
        assert_eq!(error.kind, io::ErrorKind::FileTooLarge);
        let path = dir.path().join(policy::SERVER_CONFIG_CACHE_FILE);
        assert!(
            std::fs::metadata(&path).unwrap().len() as usize
                <= policy::SERVER_CONFIG_CACHE_MAX_BYTES
        );
        assert_eq!(
            cache.load(&test_project()).unwrap().unwrap().fetched,
            Some(denial),
            "the refused write leaves the previous generation readable"
        );
    }

    fn expired_allow_entry(etag: &str) -> CachedConfig {
        fetched(
            ServerConfig {
                enabled: true,
                config_ttl_seconds: policy::CONFIG_TTL_CLAMP_SECONDS.0,
                ..ServerConfig::default()
            },
            Some(etag),
            1,
        )
    }

    fn newer_denial_entry(fetched_at: u64) -> CachedConfig {
        fetched(
            ServerConfig {
                enabled: false,
                ..ServerConfig::default()
            },
            Some("\"disabled\""),
            fetched_at,
        )
    }

    /// One refresh whose response is held at the server while another
    /// process (a second cache handle) stores a newer denial. Returns the
    /// refresh outcome and the entry left on disk.
    fn refresh_racing_a_newer_denial(
        server: &TestServer,
        seeded: CachedConfig,
    ) -> (ConfigRefresh, CachedConfig) {
        let dir = tempfile::tempdir().unwrap();
        let cache = ServerConfigCache::open(dir.path());
        cache
            .update(&test_project(), |entry| entry.fetched = Some(seeded))
            .unwrap();
        let release = server.hold_config_responses();
        let accepted_before = server.accepted_connections();
        let refresher = {
            let cache = ServerConfigCache::open(dir.path());
            let endpoint = Endpoint::parse(&server.base_url).unwrap();
            std::thread::spawn(move || {
                let http = build_http_client().unwrap();
                current_config(
                    &http,
                    &cache,
                    &endpoint,
                    &test_project(),
                    crate::wire::epoch_seconds_now(),
                )
            })
        };
        server.wait_for_accepted(accepted_before + 1);
        let newer = newer_denial_entry(777);
        cache
            .update(&test_project(), |entry| entry.fetched = Some(newer))
            .unwrap();
        drop(release);
        let refresh = refresher.join().unwrap();
        let on_disk = cache
            .load(&test_project())
            .unwrap()
            .unwrap()
            .fetched
            .unwrap();
        (refresh, on_disk)
    }

    #[test]
    fn a_refresh_that_raced_a_newer_decision_keeps_and_returns_the_newer_one() {
        let server = TestServer::start(r#"{"enabled": true, "configTtlSeconds": 3600}"#);
        let (refresh, on_disk) =
            refresh_racing_a_newer_denial(&server, expired_allow_entry("\"old\""));
        assert!(refresh.cache_error.is_none());
        assert!(
            !refresh.config.unwrap().enabled,
            "a delayed 200 never overrides the denial stored meanwhile"
        );
        assert_eq!(on_disk, newer_denial_entry(777));

        *server.behavior.etag.lock().unwrap() = "\"old\"".to_string();
        let (refresh, on_disk) =
            refresh_racing_a_newer_denial(&server, expired_allow_entry("\"old\""));
        assert!(refresh.cache_error.is_none());
        assert!(
            !refresh.config.unwrap().enabled,
            "a delayed 304 neither revalidates nor returns the stale entry"
        );
        assert_eq!(
            on_disk,
            newer_denial_entry(777),
            "the newer entry's fetch time is left untouched"
        );
    }

    #[test]
    fn an_unreadable_cache_yields_no_decision_offline_but_a_fresh_response_still_decides() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ServerConfigCache::open(dir.path());
        std::fs::write(
            dir.path().join(policy::SERVER_CONFIG_CACHE_FILE),
            "{not json",
        )
        .unwrap();
        let http = build_http_client().unwrap();
        let now = crate::wire::epoch_seconds_now();

        let offline = current_config(
            &http,
            &cache,
            &Endpoint::parse(&refused_base_url()).unwrap(),
            &test_project(),
            now,
        );
        assert!(
            offline.config.is_none(),
            "no readable cache and no server means no decision, never the defaults"
        );
        assert_eq!(
            offline.cache_error.map(|error| error.kind),
            Some(io::ErrorKind::InvalidData)
        );

        let server = TestServer::start(r#"{"enabled": false, "configTtlSeconds": 3600}"#);
        let online = current_config(
            &http,
            &cache,
            &Endpoint::parse(&server.base_url).unwrap(),
            &test_project(),
            now,
        );
        assert!(online.cache_error.is_none());
        assert!(!online.config.unwrap().enabled);
        let repaired = load_cached_config(&cache, &test_project())
            .unwrap()
            .unwrap();
        assert!(
            !repaired.config.enabled,
            "the fresh decision replaced the unreadable file"
        );
        assert_eq!(repaired.fetched_at, now);
    }

    #[test]
    fn no_cache_and_no_response_yields_no_decision_even_while_pacing_retries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ServerConfigCache::open(dir.path());
        let http = build_http_client().unwrap();
        let endpoint = Endpoint::parse(&refused_base_url()).unwrap();
        let now = crate::wire::epoch_seconds_now();

        let first = current_config(&http, &cache, &endpoint, &test_project(), now);
        assert!(
            first.config.is_none(),
            "the shipped defaults never stand in for a server that never answered"
        );
        assert!(first.cache_error.is_none());
        let paced = current_config(&http, &cache, &endpoint, &test_project(), now);
        assert!(
            paced.config.is_none(),
            "a paced retry with nothing cached is still no decision"
        );
    }

    #[test]
    fn server_config_parses_the_shipped_shape_and_ignores_unknown_fields() {
        let raw = r#"{
            "enabled": true,
            "flushIntervalSeconds": 300,
            "maxRowsPerBatch": 1000,
            "maxBodyBytes": 262144,
            "samplingRate": 0.5,
            "minSupportedClientVersion": "1.0.0",
            "ingestEndpoint": "https://ingest.example.com/v1/events",
            "configTtlSeconds": 3600,
            "someFutureField": {"nested": true}
        }"#;
        let config = serde_json::from_str::<ServerConfig>(raw).unwrap().clamped();
        assert!(config.enabled);
        assert_eq!(config.flush_interval_seconds, 300);
        assert_eq!(config.max_rows_per_batch, 1000);
        assert_eq!(config.max_body_bytes, 262_144);
        assert_eq!(config.sampling_rate, 0.5);
        assert_eq!(
            config
                .min_supported_client_version
                .as_ref()
                .map(MinClientVersion::as_str),
            Some("1.0.0")
        );
        assert_eq!(
            config.ingest_endpoint,
            Some(Endpoint::parse_ingest("https://ingest.example.com/v1/events").unwrap())
        );
        assert_eq!(config.config_ttl_seconds, 3600);
    }

    #[test]
    fn the_fallback_config_matches_the_shipped_backend_defaults() {
        let fallback = ServerConfig::default();
        assert!(
            fallback.enabled,
            "a config response that omits `enabled` keeps collecting — the LOCAL \
             master switch is what gates consent"
        );
        assert_eq!(fallback.flush_interval_seconds, 300);
        assert_eq!(fallback.max_rows_per_batch, 1000);
        assert_eq!(fallback.max_body_bytes, 262_144);
        assert_eq!(fallback.sampling_rate, 1.0);
        assert_eq!(fallback.config_ttl_seconds, 3600);
    }

    #[test]
    fn server_config_clamps_hostile_values() {
        let raw = r#"{
            "enabled": true,
            "flushIntervalSeconds": 0,
            "maxRowsPerBatch": 0,
            "maxBodyBytes": 1,
            "samplingRate": 99.0,
            "configTtlSeconds": 0
        }"#;
        let config = serde_json::from_str::<ServerConfig>(raw).unwrap().clamped();
        assert_eq!(config.flush_interval_seconds, 5);
        assert_eq!(config.max_rows_per_batch, 2);
        assert_eq!(config.max_body_bytes, 1024);
        assert_eq!(config.sampling_rate, 1.0);
        assert_eq!(config.config_ttl_seconds, 1);

        let invalid = serde_json::from_str::<ServerConfig>(r#"{"samplingRate": null}"#);
        assert!(
            invalid.is_err(),
            "a null sampling rate is invalid JSON config"
        );

        let plaintext_ingest = serde_json::from_str::<ServerConfig>(
            r#"{"ingestEndpoint": "http://evil.example.com/v1/events"}"#,
        )
        .unwrap();
        assert_eq!(
            plaintext_ingest.ingest_endpoint, None,
            "a server-pushed ingest endpoint violating the transport rules is dropped"
        );
    }

    #[test]
    fn version_gate_disables_only_below_the_minimum() {
        let mut config = ServerConfig::default();
        assert!(config.client_version_supported("1.0.0"));
        config.min_supported_client_version = MinClientVersion::new("1.0.0");
        assert!(config.client_version_supported("1.0.0"));
        assert!(config.client_version_supported("1.2.0"));
        config.min_supported_client_version = MinClientVersion::new("2.0.0");
        assert!(!config.client_version_supported("1.0.0"));
        config.min_supported_client_version = MinClientVersion::new("garbage");
        assert!(config.client_version_supported("1.0.0"), "fail-open");
    }

    #[test]
    fn sampling_is_deterministic_per_installation() {
        let id = InstallationId::new("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
        assert!(is_sampled(&id, 1.0));
        assert!(!is_sampled(&id, 0.0));
        let first = is_sampled(&id, 0.5);
        for _ in 0..100 {
            assert_eq!(is_sampled(&id, 0.5), first, "same id, same decision");
        }
        let sampled = (0..1000)
            .filter(|i| {
                let id = InstallationId::new(format!("00000000-0000-4000-8000-{i:012}")).unwrap();
                is_sampled(&id, 0.5)
            })
            .count();
        assert!(
            (400..=600).contains(&sampled),
            "rate 0.5 over 1000 ids landed at {sampled}"
        );
    }
}
