// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Tenant-agnostic telemetry policy: consent posture, privacy rules, closed
//! taxonomies, and resource limits in one place.
//!
//! Project identity and deployment endpoints are deliberately absent. The
//! scaffold injects those values from each persisted project and stage. No
//! other file in this crate may hard-code a policy value — the
//! `no_policy_literals_outside_this_module` test scans every sibling module's
//! production code and fails on any policy literal it finds.
//!
//! Changing the telemetry policy: edit `policy.rs`. The consent posture
//! is exactly two flags,
//! [`TELEMETRY_DEFAULT_ENABLED`] and [`TELEMETRY_CONSENT_MODE`] — the
//! user-facing startup notice re-words itself from those flags via
//! [`startup_notice_line`].

use crate::config::SwitchSource;
use crate::types::{
    AgentClient, Arch, ConfigCacheError, ContainerRuntime, CrashType, ErrorType, InstallChannel,
    MemoryBucket, MetricType, OsFamily, Outcome, RunMode, SdkDimension, SdkName, StartupOutcome,
    StatePurgeError,
};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Consent & the local master switch
//
// OFF WINS: telemetry runs only when NO switch source disables it. The four
// sources are the [`TELEMETRY_SWITCH_ENV`] environment variable, the
// `[telemetry] enabled` flag in the emulator config file, the
// [`TELEMETRY_CLI_FLAG`] command-line flag, and the embedding API
// (`TelemetryOptions::embedding_opt_out`). Any source saying off (or holding an
// invalid value, which fails safe to off) disables telemetry regardless of
// the other sources; only when every source is silent does
// [`TELEMETRY_DEFAULT_ENABLED`] apply.
// ---------------------------------------------------------------------------

/// Whether telemetry is enabled when no switch source (environment, config
/// file, or embedding API) says otherwise.
///
/// Opt-out posture: enabled by default; any
/// switch source can turn it off.
pub const TELEMETRY_DEFAULT_ENABLED: bool = true;

/// Whether the startup notice ASKS the user to enable telemetry
/// ([`ConsentMode::OptIn`]) or INFORMS an already-enabled user how to disable
/// it ([`ConsentMode::OptOut`]). Paired with [`TELEMETRY_DEFAULT_ENABLED`]:
/// opt-out posture.
pub const TELEMETRY_CONSENT_MODE: ConsentMode = ConsentMode::OptOut;

/// The consent posture toward the user. Drives the wording of
/// [`startup_notice_line`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentMode {
    /// Telemetry is presented as something the user turns ON.
    OptIn,
    /// Telemetry is presented as ON by default with a clear way OUT.
    OptOut,
}

/// The environment master switch: any [`SWITCH_ON_TOKENS`] value enables, any
/// [`SWITCH_OFF_TOKENS`] value disables (case-insensitive), anything else
/// fails safe to disabled and the startup notice carries a
/// [`invalid_switch_warning_line`] warning. One of the four off-wins switch
/// sources; `off` here disables telemetry no matter what the config file or
/// the embedding API say.
pub const TELEMETRY_SWITCH_ENV: &str = "EMULATOR_TELEMETRY";

/// The canonical enabling value of [`TELEMETRY_SWITCH_ENV`].
pub const SWITCH_ON: &str = "on";

/// The canonical disabling value of [`TELEMETRY_SWITCH_ENV`].
pub const SWITCH_OFF: &str = "off";

/// The emulator command-line flag that switches telemetry for one run:
/// `--telemetry off` or `--telemetry on`. One of the four off-wins switch
/// sources and disable-only: the router turns `off` into
/// `TelemetryOptions::embedding_opt_out`, so `on` cannot override the others.
pub const TELEMETRY_CLI_FLAG: &str = "--telemetry";

/// The short form of [`TELEMETRY_CLI_FLAG`].
pub const TELEMETRY_CLI_SHORT_FLAG: &str = "-t";

/// Every enabling [`TELEMETRY_SWITCH_ENV`] token, matched case-insensitively.
pub const SWITCH_ON_TOKENS: &[&str] = &[SWITCH_ON, "true", "1"];

/// Every disabling [`TELEMETRY_SWITCH_ENV`] token, matched case-insensitively.
pub const SWITCH_OFF_TOKENS: &[&str] = &[SWITCH_OFF, "false", "0"];

/// Environment override for the emulator config file path. When unset, the
/// config file is `config.toml` in the `emulator_paths::emulator_home` folder
/// (deliberately no XDG or Library/Application Support variance).
pub const EMULATOR_CONFIG_ENV: &str = "EMULATOR_CONFIG";

/// The per-user emulator configuration directory under the home directory.
/// Telemetry state (installation id, outbox) always lives under this
/// directory, even when [`EMULATOR_CONFIG_ENV`] moves the config file.
pub const CONFIG_DIR_NAME: &str = emulator_paths::DIR_NAME;

/// The emulator config file name inside [`CONFIG_DIR_NAME`]. The telemetry
/// switch is the [`CONFIG_TOML_TELEMETRY_SECTION`] table's
/// [`CONFIG_FILE_ENABLED_KEY`] boolean:
///
/// ```toml
/// [telemetry]
/// enabled = false
/// ```
///
/// One of the four off-wins switch sources; `enabled = false` disables
/// telemetry no matter what the environment or the embedding API say. A file
/// without the section or the key is silent, not invalid: the file is shared
/// emulator configuration, not telemetry-owned.
pub const EMULATOR_CONFIG_FILE_NAME: &str = "config.toml";

/// The TOML table in [`EMULATOR_CONFIG_FILE_NAME`] holding the telemetry
/// switch.
pub const CONFIG_TOML_TELEMETRY_SECTION: &str = "telemetry";

/// The boolean key read from the [`CONFIG_TOML_TELEMETRY_SECTION`] table.
pub const CONFIG_FILE_ENABLED_KEY: &str = "enabled";

// ---------------------------------------------------------------------------
// Injected version validation and closed environment taxonomy
// ---------------------------------------------------------------------------

/// The version this client reports and compares against
/// `minSupportedClientVersion`. Fixed at `1.0.0` by contract with the
/// telemetry backend — deliberately NOT the generated workspace's
/// `CARGO_PKG_VERSION` (0.1.0), which would fail the backend's minimum.
pub const CLIENT_VERSION: &str = "1.0.0";

/// The exact wire token for each closed [`RunMode`] value.
pub const fn run_mode_token(run_mode: RunMode) -> &'static str {
    match run_mode {
        RunMode::Standalone => "standalone",
        RunMode::Embedded => "embedded",
    }
}

/// Environment variables whose presence identifies CI execution. Most CI
/// systems set `CI`; the others are for the systems that never set it
/// (AWS CodeBuild, Jenkins, Azure Pipelines).
pub const CI_ENVS: &[&str] = &["CI", "CODEBUILD_BUILD_ID", "JENKINS_URL", "TF_BUILD"];

/// SDK token before any recognized request has been observed.
pub const SDK_UNKNOWN: &str = "unknown";

/// SDK token after requests from more than one recognized SDK are observed.
pub const SDK_MIXED: &str = "mixed";

/// The exact wire token for each recognized calling SDK.
pub const fn sdk_name_token(sdk: SdkName) -> &'static str {
    match sdk {
        SdkName::AwsSdkRust => "aws-sdk-rust",
        SdkName::AwsSdkJava => "aws-sdk-java",
        SdkName::Boto3 => "boto3",
        SdkName::AwsSdkJsV3 => "aws-sdk-js-v3",
        SdkName::AwsCli => "aws-cli",
    }
}

/// The exact wire token for the `environment.sdkName` dimension.
pub const fn sdk_dimension_token(sdk: SdkDimension) -> &'static str {
    match sdk {
        SdkDimension::Unknown => SDK_UNKNOWN,
        SdkDimension::Mixed => SDK_MIXED,
        SdkDimension::Sdk(name) => sdk_name_token(name),
    }
}

/// Lower-cased `User-Agent` markers identifying each recognized SDK. A
/// marker match also binds the bounded version fragment that follows it.
pub const SDK_USER_AGENT_MARKERS: &[(SdkName, &[&str])] = &[
    (SdkName::AwsSdkRust, &["aws-sdk-rust/"]),
    (SdkName::AwsSdkJava, &["aws-sdk-java/"]),
    (SdkName::Boto3, &["boto3/"]),
    (SdkName::AwsSdkJsV3, &["aws-sdk-js-v3/", "aws-sdk-js/3."]),
    (SdkName::AwsCli, &["aws-cli/"]),
];

/// Lower-cased `User-Agent` markers whose following version fragment is the
/// caller's language version.
pub const LANGUAGE_VERSION_MARKERS: &[&str] = &[
    "lang/rust/",
    "rust/",
    "python/",
    "java/",
    "nodejs/",
    "node/",
];

/// Maximum characters kept from a `User-Agent` version fragment.
pub const VERSION_FRAGMENT_MAX_LEN: usize = 24;

/// Closed agent-client taxonomy accepted by the backend.
pub const AGENT_AMAZON_Q: &str = "amazon-q";
pub const AGENT_CLAUDE_CODE: &str = "claude-code";
pub const AGENT_KIRO: &str = "kiro";
pub const AGENT_CURSOR: &str = "cursor";
pub const AGENT_OTHER: &str = "other";

/// The exact wire token for each closed [`AgentClient`] value.
pub const fn agent_client_token(agent: AgentClient) -> &'static str {
    match agent {
        AgentClient::AmazonQ => AGENT_AMAZON_Q,
        AgentClient::ClaudeCode => AGENT_CLAUDE_CODE,
        AgentClient::Kiro => AGENT_KIRO,
        AgentClient::Cursor => AGENT_CURSOR,
        AgentClient::Other => AGENT_OTHER,
    }
}

/// Lower-cased `User-Agent` markers identifying each recognized agent
/// client, checked in order.
pub const AGENT_CLIENT_MARKERS: &[(AgentClient, &[&str])] = &[
    (AgentClient::AmazonQ, &["amazon-q", "amazonq"]),
    (AgentClient::ClaudeCode, &["claude-code", "claude_code"]),
    (AgentClient::Kiro, &["kiro"]),
    (AgentClient::Cursor, &["cursor"]),
];

/// Lower-cased `User-Agent` markers indicating SOME agent drives the caller
/// without identifying which; they fold to [`AgentClient::Other`].
pub const AGENT_GENERIC_MARKERS: &[&str] = &["md/agent", " agent"];

/// Closed operating-system and CPU-architecture taxonomy accepted by the backend.
pub const OS_LINUX: &str = "linux";
pub const OS_MACOS: &str = "macos";
pub const OS_WINDOWS: &str = "windows";
pub const OS_OTHER: &str = "other";
pub const ARCH_X86_64: &str = "x86_64";
pub const ARCH_AARCH64: &str = "aarch64";
pub const ARCH_OTHER: &str = "other";

/// The exact wire token for each closed [`OsFamily`] value.
pub const fn os_token(os: OsFamily) -> &'static str {
    match os {
        OsFamily::Linux => OS_LINUX,
        OsFamily::Macos => OS_MACOS,
        OsFamily::Windows => OS_WINDOWS,
        OsFamily::Other => OS_OTHER,
    }
}

/// The exact wire token for each closed [`Arch`] value.
pub const fn arch_token(arch: Arch) -> &'static str {
    match arch {
        Arch::Amd64 => ARCH_X86_64,
        Arch::Aarch64 => ARCH_AARCH64,
        Arch::Other => ARCH_OTHER,
    }
}

/// Install channel used when no closed compile-time channel is injected.
pub const INSTALL_CHANNEL_UNKNOWN: &str = "unknown";

/// Whether this is the project's release build: the release pipeline stamps
/// `EMULATOR_VERSION`, and nothing else sets it. Only a release build is the
/// downloadable binary.
pub const RELEASE_BUILD: bool = option_env!("EMULATOR_VERSION").is_some();

/// The exact wire token for each closed [`InstallChannel`] value.
pub const fn install_channel_token(channel: InstallChannel) -> &'static str {
    match channel {
        InstallChannel::Cargo => "cargo",
        InstallChannel::Npm => "npm",
        InstallChannel::Pip => "pip",
        InstallChannel::Docker => "docker",
        InstallChannel::Binary => "binary",
        InstallChannel::Source => "source",
        InstallChannel::Other => "other",
        InstallChannel::Unknown => INSTALL_CHANNEL_UNKNOWN,
    }
}

/// Filesystem marker used to detect Docker.
pub const DOCKER_MARKER_PATH: &str = "/.dockerenv";

/// Filesystem marker used to detect Podman.
pub const PODMAN_MARKER_PATH: &str = "/run/.containerenv";

/// Environment marker used to detect Kubernetes.
pub const KUBERNETES_ENV: &str = "KUBERNETES_SERVICE_HOST";

/// Environment marker used to detect Amazon ECS.
pub const ECS_ENV: &str = "ECS_CONTAINER_METADATA_URI_V4";

/// Detected Docker runtime and backend wire token.
pub const CONTAINER_RUNTIME_DOCKER: &str = "docker";

/// Detected Kubernetes runtime and backend wire token.
pub const CONTAINER_RUNTIME_KUBERNETES: &str = "kubernetes";

/// Detected Amazon ECS runtime and backend wire token.
pub const CONTAINER_RUNTIME_ECS: &str = "ecs";

/// Backend wire token for detected runtimes without a dedicated enum value.
pub const CONTAINER_RUNTIME_OTHER: &str = "other";

/// Backend wire token when no container runtime is detected.
pub const CONTAINER_RUNTIME_NONE: &str = "none";

/// The exact wire token for each closed [`ContainerRuntime`] value. Podman
/// detection folds to [`ContainerRuntime::Other`] before this point.
pub const fn container_runtime_token(runtime: ContainerRuntime) -> &'static str {
    match runtime {
        ContainerRuntime::Docker => CONTAINER_RUNTIME_DOCKER,
        ContainerRuntime::Kubernetes => CONTAINER_RUNTIME_KUBERNETES,
        ContainerRuntime::Ecs => CONTAINER_RUNTIME_ECS,
        ContainerRuntime::Other => CONTAINER_RUNTIME_OTHER,
        ContainerRuntime::None => CONTAINER_RUNTIME_NONE,
    }
}

/// KiB-to-MiB divisor used before closed bucketing.
pub const KIBIBYTES_PER_MIB: u64 = 1024;

/// Byte-to-MiB divisor used before closed bucketing.
pub const BYTES_PER_MIB: u64 = KIBIBYTES_PER_MIB * KIBIBYTES_PER_MIB;

/// Closed memory-usage buckets; raw byte counts never reach the wire.
pub const MEMORY_LESS_THAN_256_MIB: &str = "less_than_256_mib";
pub const MEMORY_256_TO_512_MIB: &str = "256_to_512_mib";
pub const MEMORY_512_MIB_TO_1_GIB: &str = "512_mib_to_1_gib";
pub const MEMORY_1_TO_2_GIB: &str = "1_to_2_gib";
pub const MEMORY_2_TO_4_GIB: &str = "2_to_4_gib";
pub const MEMORY_GREATER_THAN_4_GIB: &str = "greater_than_4_gib";

/// Fold a process's peak resident-set size into the closed memory taxonomy.
pub const fn memory_usage_bucket(mebibytes: u64) -> MemoryBucket {
    match mebibytes {
        0..=255 => MemoryBucket::LessThan256Mib,
        256..=511 => MemoryBucket::From256To512Mib,
        512..=1023 => MemoryBucket::From512MibTo1Gib,
        1024..=2047 => MemoryBucket::From1To2Gib,
        2048..=4095 => MemoryBucket::From2To4Gib,
        _ => MemoryBucket::GreaterThan4Gib,
    }
}

/// The exact wire token for each closed [`MemoryBucket`] value.
pub const fn memory_bucket_token(bucket: MemoryBucket) -> &'static str {
    match bucket {
        MemoryBucket::LessThan256Mib => MEMORY_LESS_THAN_256_MIB,
        MemoryBucket::From256To512Mib => MEMORY_256_TO_512_MIB,
        MemoryBucket::From512MibTo1Gib => MEMORY_512_MIB_TO_1_GIB,
        MemoryBucket::From1To2Gib => MEMORY_1_TO_2_GIB,
        MemoryBucket::From2To4Gib => MEMORY_2_TO_4_GIB,
        MemoryBucket::GreaterThan4Gib => MEMORY_GREATER_THAN_4_GIB,
    }
}

/// The `performance.startupOutcome` token reported once the emulator is ready
/// to serve.
pub const STARTUP_OUTCOME_SUCCESS: &str = "success";

/// The `performance.startupOutcome` token reported when the emulator stopped
/// before it was ready to serve.
pub const STARTUP_OUTCOME_FAILURE: &str = "failure";

/// The exact wire token for each closed [`StartupOutcome`] value.
pub const fn startup_outcome_token(outcome: StartupOutcome) -> &'static str {
    match outcome {
        StartupOutcome::Success => STARTUP_OUTCOME_SUCCESS,
        StartupOutcome::Failure => STARTUP_OUTCOME_FAILURE,
    }
}

/// The wire schema version this client emits.
pub const SCHEMA_VERSION: &str = "1";

/// The name of the single background worker thread that owns all telemetry
/// I/O (fail-silent and non-blocking).
pub const WORKER_THREAD_NAME: &str = "emulator-telemetry";

/// The name of the short-lived thread that builds the blocking HTTP client
/// outside any async runtime context.
pub const HTTP_CLIENT_BUILD_THREAD_NAME: &str = "emulator-telemetry-http-init";

// ---------------------------------------------------------------------------
// Protocol paths. The scaffold injects deployment endpoints per stage.
// ---------------------------------------------------------------------------

/// Environment override for the telemetry service base endpoint (the client
/// requests `{base}`[`CONFIG_PATH`]; a value ending in [`CONFIG_PATH`] is
/// accepted and normalized).
pub const TELEMETRY_ENDPOINT_ENV: &str = "EMULATOR_TELEMETRY_CONFIG_ENDPOINT";

/// The config resource, appended to the base endpoint (the server-directed
/// configuration fetched by `GET /v1/config`).
pub const CONFIG_PATH: &str = "/v1/config";

/// The ingest resource, appended to the base endpoint when the server config
/// does not carry an explicit `ingestEndpoint` (`POST /v1/events`).
pub const INGEST_PATH: &str = "/v1/events";

/// The ingest content type (`POST /v1/events`).
pub const INGEST_CONTENT_TYPE: &str = "application/x-ndjson";

// ---------------------------------------------------------------------------
// Local state (the durable outbox)
//
// Telemetry state (installation id, outbox) lives in the
// [`TELEMETRY_STATE_DIR_NAME`] subdirectory of the home
// [`CONFIG_DIR_NAME`] directory. The embedding API's
// `TelemetryOptions::state_dir` is the only override. Opting out DELETES
// this state rather than merely ceasing to write it.
// ---------------------------------------------------------------------------

/// The telemetry state subdirectory of the home [`CONFIG_DIR_NAME`]
/// (`<emulator home>/telemetry/`): holds the installation id file
/// and the outbox database.
pub const TELEMETRY_STATE_DIR_NAME: &str = "telemetry";

/// The file next to the outbox holding the stable random installation UUID
/// (the `installationId` identifier).
pub const INSTALLATION_ID_FILE: &str = "installation-id";

/// Bounded attempts to read-or-atomically-claim the installation id file
/// under cross-process contention.
pub const INSTALLATION_ID_CLAIM_ATTEMPTS: usize = 4;

/// The outbox database file name inside the telemetry state directory.
pub const OUTBOX_FILE: &str = "outbox.sqlite";

/// The file next to the outbox holding the server's last-seen configuration
/// (plus its ETag and fetch time). Kept in its own file so that every
/// effective-off purge — which deletes the installation id and the outbox —
/// leaves the server's decision in place: a server last seen disabling
/// collection, not sampling this installation, or no longer supporting this
/// client stays in force while offline. Holds only the parsed configuration
/// re-serialized canonically (never the server's raw bytes), its bounded
/// ETag, and the fetch time — nothing about this installation or its traffic.
pub const SERVER_CONFIG_CACHE_FILE: &str = "server-config.json";

/// The advisory lock file serializing read-modify-write updates of
/// [`SERVER_CONFIG_CACHE_FILE`] across emulator processes. Always empty.
pub const SERVER_CONFIG_LOCK_FILE: &str = "server-config.lock";

/// The staging file an update of [`SERVER_CONFIG_CACHE_FILE`] is written and
/// synced to before being renamed into place, so a reader never sees a
/// partially written cache.
pub const SERVER_CONFIG_STAGED_FILE: &str = "server-config.json.staged";

/// Maximum size of [`SERVER_CONFIG_CACHE_FILE`]: an update that would exceed
/// it is refused (the previous file stays), and a larger file on disk is not
/// trusted as a decision.
pub const SERVER_CONFIG_CACHE_MAX_BYTES: usize = 65_536;

/// Maximum length of a config-endpoint `ETag` the client keeps for
/// `If-None-Match`; longer or non-visible-ASCII values are dropped.
pub const CONFIG_ETAG_MAX_LEN: usize = 128;

/// How long the worker waits on a locked SQLite database before giving up.
pub const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause between the worker's attempts to open local state another process
/// holds locked; commands arriving meanwhile are buffered in memory.
pub const STATE_OPEN_RETRY_PAUSE: Duration = Duration::from_secs(1);

/// The panic hook never waits for the worker's SQLite writer lock.
pub const SQLITE_CRASH_BUSY_TIMEOUT: Duration = Duration::ZERO;

/// SQLite journal mode used for the worker and panic-hook connections.
pub const SQLITE_JOURNAL_MODE: &str = "WAL";

// ---------------------------------------------------------------------------
// Server-config fallbacks and clamps
// (the fallbacks mirror the shipped backend's Prod values and fill in ONLY
// fields a /v1/config response leaves absent; with no response and no cached
// config there is no decision and nothing is collected)
// ---------------------------------------------------------------------------

/// Fallback for `enabled` when a `/v1/config` response omits the field. It
/// never stands in for a missing decision: with `/v1/config` unreachable and
/// no cached config the client collects nothing, so no installation id is
/// written before the server allows collection. The LOCAL master switch
/// still gates everything before this fallback can apply.
pub const FALLBACK_ENABLED: bool = true;

/// Fallback for `flushIntervalSeconds` (backend Prod default: 300).
pub const FALLBACK_FLUSH_INTERVAL_SECONDS: u64 = 300;

/// Fallback for `maxRowsPerBatch` (backend Prod default: 1000).
pub const FALLBACK_MAX_ROWS_PER_BATCH: usize = 1000;

/// Fallback for `maxBodyBytes` (backend Prod default: 262144 = 256 KiB, the
/// ingest body cap above which the server answers 400 split-the-batch).
pub const FALLBACK_MAX_BODY_BYTES: usize = 262_144;

/// Fallback for `samplingRate` (backend Prod default: 1.0 — everyone
/// participates).
pub const SAMPLING_DEFAULT: f64 = 1.0;

/// Fallback for `configTtlSeconds` (backend Prod default: 3600).
pub const FALLBACK_CONFIG_TTL_SECONDS: u64 = 3600;

/// Clamp (min, max) applied to `flushIntervalSeconds` so a hostile or corrupt
/// config cannot spin the flush loop or silence it for ever.
pub const FLUSH_INTERVAL_CLAMP_SECONDS: (u64, u64) = (5, 86_400);

/// Clamp (min, max) applied to `maxRowsPerBatch`. The minimum keeps every
/// batch able to carry its session row plus at least one metric row.
pub const ROWS_PER_BATCH_CLAMP: (usize, usize) = (2, 100_000);

/// The smallest `maxRowsPerBatch` the batch builder operates at (the session
/// row plus one metric row) — the lower bound of [`ROWS_PER_BATCH_CLAMP`].
pub const MIN_ROWS_PER_BATCH: usize = ROWS_PER_BATCH_CLAMP.0;

/// Clamp (min, max) applied to `maxBodyBytes`.
pub const BODY_BYTES_CLAMP: (usize, usize) = (1024, 1_048_576);

/// Clamp (min, max) applied to `samplingRate` (non-finite values collapse to
/// the minimum — fail closed, send nothing).
pub const SAMPLING_RATE_CLAMP: (f64, f64) = (0.0, 1.0);

/// Clamp (min, max) applied to `configTtlSeconds`.
pub const CONFIG_TTL_CLAMP_SECONDS: (u64, u64) = (1, 604_800);

// ---------------------------------------------------------------------------
// Flush, queueing and delivery (fail-silent and non-blocking; delivery,
// retries and deduplication)
// ---------------------------------------------------------------------------

/// Bound of the in-process record channel; a full queue drops records rather
/// than block the caller.
pub const RECORD_QUEUE_BOUND: usize = 8192;

/// Cap on distinct in-memory aggregation keys per flush window; further NEW
/// keys are dropped (existing keys keep aggregating).
pub const MAX_AGGREGATED_KEYS: usize = 10_000;

/// Maximum outbox batches attempted per drain pass.
pub const DRAIN_BATCH_LIMIT: usize = 32;

/// Best-effort final-flush budget when the client is dropped without an
/// explicit `shutdown` (the channel disconnects under the worker).
pub const DISCONNECT_FINAL_FLUSH_GRACE: Duration = Duration::from_secs(2);

/// The bounded budget embedders pass to `TelemetryClient::shutdown` for the
/// final flush at emulator exit (fail-silent and non-blocking; delivery must
/// never stall shutdown for long).
pub const SHUTDOWN_FLUSH_BUDGET: Duration = Duration::from_secs(5);

/// The final-flush budget at the exit of an embedding process (a test binary), kept short
/// because the host waits for it once as it exits.
pub const EXIT_FLUSH_BUDGET: Duration = Duration::from_secs(1);

/// HTTP connect timeout for config and ingest calls — short and bounded so
/// telemetry can never stall emulator shutdown for long.
pub const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// End-to-end HTTP request timeout for config and ingest calls.
pub const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum accepted response body for `GET /v1/config`.
pub const CONFIG_RESPONSE_MAX_BODY_BYTES: usize = 65_536;

/// Delay before retrying a failed config fetch, including when no cache exists.
pub const CONFIG_FETCH_RETRY_INTERVAL: Duration = Duration::from_secs(60);

/// Pause between bounded attempts to enqueue the final shutdown command.
pub const SHUTDOWN_QUEUE_RETRY_PAUSE: Duration = Duration::from_millis(1);

// ---------------------------------------------------------------------------
// Outbox retention and retry (the backend deduplicates on
// installationId#batchId with a 14-day window)
// ---------------------------------------------------------------------------

/// Outbox retention: queued batches older than this are dropped LOCALLY.
/// Matches the backend's 14-day dedup window — anything older can no longer
/// be deduplicated safely.
pub const OUTBOX_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Cap on pending batches; beyond it the oldest are evicted. Bounds local
/// disk usage when the ingest endpoint is unreachable for a long time.
pub const OUTBOX_MAX_BATCHES: usize = 256;

/// Retry backoff base delay, doubled per attempt.
pub const BACKOFF_BASE: Duration = Duration::from_secs(60);

/// Retry backoff ceiling.
pub const BACKOFF_CAP: Duration = Duration::from_secs(3600);

/// Number of doublings after which the backoff stops growing (before the
/// [`BACKOFF_CAP`] is applied).
pub const BACKOFF_MAX_DOUBLINGS: u32 = 6;

// ---------------------------------------------------------------------------
// Crash reporting (crash TYPE only, never a location, message, or backtrace)
// ---------------------------------------------------------------------------

/// Row cap for the crash-annotated final batch built inside the panic hook
/// (the session row alone carries the crash block).
pub const CRASH_FLUSH_MAX_ROWS: usize = MIN_ROWS_PER_BATCH;

// ---------------------------------------------------------------------------
// Generated-emulator operation classification (outcomes are classified from
// protocol-level facts alone)
// ---------------------------------------------------------------------------

/// The HTTP status a generated emulator's scaffolded (not-implemented)
/// operation answers. A failure is a coverage gap only when this status and
/// [`NOT_EMULATED_SHAPE_NAME`] both match (`not_emulated` is the single most
/// important prioritization signal).
pub const NOT_EMULATED_HTTP_STATUS: u16 = 501;

/// The synthetic wire error shape emitted by a scaffolded operation. Requiring
/// this together with [`NOT_EMULATED_HTTP_STATUS`] prevents a modeled 501 from
/// being misclassified as a coverage gap.
pub const NOT_EMULATED_SHAPE_NAME: &str = "NotImplemented";

// ---------------------------------------------------------------------------
// The field allowlist — what is sent (the wire schema)
// ---------------------------------------------------------------------------

/// The `recordType` of the envelope row leading every batch.
pub const RECORD_TYPE_BATCH: &str = "batch";

/// The `recordType` of aggregate metric rows.
pub const RECORD_TYPE_METRIC: &str = "metric";

/// The exact wire token for each metric row `type` (`operation`,
/// `coverage_gap`, or `error`).
pub const fn metric_type_token(metric_type: MetricType) -> &'static str {
    match metric_type {
        MetricType::Operation => "operation",
        MetricType::CoverageGap => "coverage_gap",
        MetricType::Error => "error",
    }
}

/// The exact wire token for each closed [`Outcome`] value (how the call
/// ended).
pub const fn outcome_token(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Success => "success",
        Outcome::ClientError => "client_error",
        Outcome::ServerError => "server_error",
        Outcome::NotEmulated => "not_emulated",
        Outcome::Error => "error",
        Outcome::Crashed => "crashed",
    }
}

/// The exact wire token for each closed [`CrashType`] value (the crash *type*
/// alone, `panic`, `oom`, or `sigkill`).
pub const fn crash_type_token(crash_type: CrashType) -> &'static str {
    match crash_type {
        CrashType::Panic => "panic",
        CrashType::Oom => "oom",
        CrashType::Sigkill => "sigkill",
    }
}

/// The wire key patched with the real send time immediately before each POST.
pub const KEY_SENT_AT: &str = "sentAt";

/// The wire key re-keyed when a batch is split on the server's 400 response.
pub const KEY_BATCH_ID: &str = "batchId";

/// The complete, closed set of JSON object keys this client may ever emit —
/// the field allowlist. `wire::tests` re-derives the full wire output and
/// asserts every emitted key is in this list; adding any field to the wire
/// therefore requires editing this allowlist in the same diff.
pub const ALLOWED_WIRE_KEYS: &[&str] = &[
    // session row
    "recordType",
    "schemaVersion",
    "installationId",
    "sessionId",
    "projectId",
    "batchId",
    "flushSequence",
    "isFinal",
    "sentAt",
    "sessionDurationMs",
    "productVersion",
    "previousVersion",
    "firstRun",
    "environment",
    "performance",
    "crash",
    // environment block
    "os",
    "arch",
    "sdkName",
    "sdkVersion",
    "languageVersion",
    "agentClient",
    "region",
    "runMode",
    "installChannel",
    "isCi",
    "containerRuntime",
    "featureFlags",
    "persistence",
    "multiRegion",
    "authEmulation",
    "strictMode",
    "requestLogging",
    "fidelityGating",
    // performance block
    "memoryUsage",
    "throughput",
    "peakConcurrency",
    "coldStartTime",
    "startupOutcome",
    // crash block
    "crashType",
    // metric rows
    "type",
    "namespace",
    "operation",
    "count",
    "outcome",
    "errorType",
    "latency",
    // latency block
    "min",
    "max",
    "sumMillis",
];

/// The structural container keys inside a wire row; every other entry of
/// [`ALLOWED_WIRE_KEYS`] is a leaf field.
pub const WIRE_KEY_CONTAINERS: &[&str] = &[
    "environment",
    "featureFlags",
    "performance",
    "crash",
    "latency",
];

/// The README's field-allowlist table rows: (group label, rendered fields
/// cell). [`render_readme_telemetry_section`] renders these rows verbatim and
/// a policy test proves the cells cover [`ALLOWED_WIRE_KEYS`] completely, so
/// documentation cannot drift from the allowlist.
pub const README_ALLOWLIST_GROUPS: &[(&str, &str)] = &[
    (
        "A — framing and session",
        "`recordType`, `schemaVersion`, `installationId`, `sessionId`, `projectId`, \
         `batchId`, `flushSequence`, `isFinal`, `sentAt`, `sessionDurationMs`",
    ),
    (
        "B — product version",
        "`productVersion`, `previousVersion`, `firstRun`",
    ),
    (
        "C — calling client",
        "`environment.sdkName`, `environment.sdkVersion`, `environment.languageVersion`, \
         `environment.agentClient`, `environment.region`",
    ),
    (
        "D — host and runtime",
        "`environment.os`, `environment.arch`, `environment.runMode`, \
         `environment.installChannel`, `environment.isCi`, `environment.containerRuntime`",
    ),
    (
        "E — feature state",
        "`environment.featureFlags.persistence`, `.multiRegion`, `.authEmulation`, \
         `.strictMode`, `.requestLogging`, `.fidelityGating`; values are booleans only",
    ),
    (
        "F — aggregate performance",
        "`performance.memoryUsage`, `performance.throughput`, `performance.peakConcurrency`, \
         `performance.coldStartTime`, `performance.startupOutcome`",
    ),
    (
        "G — crash category",
        "`crash.crashType` only; `crashLocation`, messages, frames, and backtraces \
         are excluded",
    ),
    (
        "H — aggregate metric row",
        "`recordType`, `type`, `namespace`, `operation`, `count`, `outcome`, \
         `errorType`, `latency`",
    ),
    (
        "I — aggregate latency",
        "`latency.min`, `latency.max`, `latency.sumMillis`",
    ),
];

/// The prose paragraph closing the README's field-allowlist section.
pub const README_ALLOWLIST_FOOTER: &str = "\
The structural `environment`, `featureFlags`, `performance`, `crash`,
and `latency` object keys are the only containers. `ALLOWED_WIRE_KEYS` in `policy.rs` pins
this complete vocabulary, and `wire_schema_carries_no_forbidden_fields` proves
that serialized output equals it exactly. Request/response bodies, raw headers,
credentials, paths, ARNs, account or personal identifiers, resource names and
keys, raw environment/config values, raw User-Agent strings, customer code,
and error messages or frames are never sent.
";

/// The distributor-responsibility sentence both README renderers embed
/// verbatim.
pub const README_REDISTRIBUTION_NOTE: &str = "If you distribute an application that includes \
     this emulator, you are responsible for informing your users and giving them a way to opt \
     out, or ship it with telemetry disabled.";

/// The local outbox-retention sentence both README renderers embed, derived
/// from [`OUTBOX_MAX_AGE`].
pub fn outbox_retention_note() -> String {
    let retention_days = OUTBOX_MAX_AGE.as_secs() / (24 * 60 * 60);
    format!(
        "Queued batches older than {retention_days} days are dropped locally: the \
         backend remembers sent batches for that long to drop duplicates, so an older \
         one could be counted twice."
    )
}

/// Render the README's "Exact telemetry field allowlist" section from the
/// policy constants above. The package README embeds this rendered output and
/// a drift test asserts it verbatim, so the documented policy is generated
/// FROM the code rather than duplicated by hand.
pub fn render_readme_telemetry_section() -> String {
    let mut section =
        String::from("## Exact telemetry field allowlist\n\n| Group | Fields |\n|---|---|\n");
    for (group, fields) in README_ALLOWLIST_GROUPS {
        section.push_str(&format!("| {group} | {fields} |\n"));
    }
    section.push('\n');
    section.push_str(README_ALLOWLIST_FOOTER);
    section.push('\n');
    section.push_str(&render_switch_controls_block(
        "## Turning telemetry on or off",
    ));
    section.push('\n');
    section.push_str(&outbox_retention_note());
    section.push_str("\n\n");
    section.push_str(README_REDISTRIBUTION_NOTE);
    section.push('\n');
    section
}

/// The switch-controls block both README renderers embed: the four off-wins
/// sources, the accepted tokens, per-shell examples, the standard config
/// path, state deletion on opt-out, and the notice/restart semantics — all
/// derived from the policy constants above.
fn render_switch_controls_block(heading: &str) -> String {
    format!(
        "{heading}\n\
         \n\
         OFF WINS: telemetry runs only when NO source disables it. The four switch\n\
         sources are equal — any one saying off (or holding an invalid value, which\n\
         fails safe to off with a startup warning) disables telemetry regardless of\n\
         the others:\n\
         \n\
         1. the `{env}` environment variable — `{on_tokens}` enables, `{off_tokens}`\n   \
         disables (case-insensitive); anything else is invalid:\n\
         \n   \
         - bash: `export {env}={off}`\n   \
         - PowerShell: `$env:{env} = \"{off}\"`\n   \
         - cmd: `set {env}={off}`\n\
         \n\
         2. the emulator config file, `{file}` in the emulator's folder in your home\n   \
         (the startup notice prints its full path; override the path with\n   \
         `{config_env}=<path>`):\n\
         \n   \
         ```toml\n   \
         [{section_name}]\n   \
         {key} = false\n   \
         ```\n\
         \n\
         3. the emulator's command-line flag, `{cli_flag} {off}` (or `{cli_short_flag} {off}`),\n   \
         for that run only — disable-only: `{cli_flag} {on}` cannot override the other\n   \
         sources. An invalid value is refused and the emulator does not start.\n\
         \n\
         4. the embedding API (`TelemetryOptions::embedding_opt_out`) — disable-only:\n   \
         an embedding application can switch telemetry off but can never force it\n   \
         on over the end user's environment variable or config file.\n\
         \n\
         Every effective-off state deletes the stored installation id and outbox\n\
         in `{state_dir}/` under the emulator's folder: an explicit opt-out from any source, an invalid\n\
         switch value, a missing endpoint, and the server disabling collection or\n\
         no longer supporting this client version. Nothing is recorded while off, so\n\
         nothing can be sent later. No installation id is written until the server\n\
         allows collection for this installation; one the server does not sample\n\
         records nothing and keeps only an installation id it already had. Only\n\
         the server's own last-seen configuration (`{cache_file}`, holding nothing\n\
         about this installation) stays behind, so a server-side disable stays in\n\
         force while offline. If deleting fails, the startup notice carries a\n\
         warning naming what is left and the client stays off; if that\n\
         configuration cannot be read or saved, the notice carries a warning and the\n\
         client follows the live server decision or stays off until the next start\n\
         — it never falls back to its shipped defaults over a decision it cannot\n\
         read.\n\
         Nothing is transmitted before the startup notice is emitted; local settings\n\
         take effect on restart, server settings at the next configuration refresh.\n",
        env = TELEMETRY_SWITCH_ENV,
        on_tokens = SWITCH_ON_TOKENS.join("|"),
        off_tokens = SWITCH_OFF_TOKENS.join("|"),
        off = SWITCH_OFF,
        on = SWITCH_ON,
        cli_flag = TELEMETRY_CLI_FLAG,
        cli_short_flag = TELEMETRY_CLI_SHORT_FLAG,
        file = EMULATOR_CONFIG_FILE_NAME,
        config_env = EMULATOR_CONFIG_ENV,
        section_name = CONFIG_TOML_TELEMETRY_SECTION,
        key = CONFIG_FILE_ENABLED_KEY,
        state_dir = TELEMETRY_STATE_DIR_NAME,
        cache_file = SERVER_CONFIG_CACHE_FILE,
    )
}

/// Render the tenant-facing README "Telemetry" policy block the workspace
/// scaffold embeds into every generated workspace README: what is collected,
/// what is never sent, and how to change the state. Every value comes from the
/// policy constants in this module, so generated documentation cannot drift
/// from the code.
///
/// The checked-in `telemetry_readme_section.md` artifact pins this output (see
/// the `readme_policy_drift` wrapper test), and the embedder package exports
/// that artifact to the scaffold — which never links this crate — as
/// `TENANT_TELEMETRY_README_SECTION`.
pub fn render_tenant_readme_telemetry_section() -> String {
    let mut section = String::from(
        "### What is collected\n\
         \n\
         The wire schema is closed and pinned by the telemetry crate's backend-contract\n\
         tests. Each flush sends one `batch` envelope followed by aggregate metric rows,\n\
         and every field belongs to the exact allowlist below.\n\
         \n\
         | Group | Fields |\n|---|---|\n",
    );
    for (group, fields) in README_ALLOWLIST_GROUPS {
        section.push_str(&format!("| {group} | {fields} |\n"));
    }
    section.push('\n');
    section.push_str(README_ALLOWLIST_FOOTER);
    section.push('\n');
    section.push_str(&render_switch_controls_block(
        "### How to turn it on or off",
    ));
    section.push_str(&format!(
        "\n\
         Only when every source is silent do `TELEMETRY_DEFAULT_ENABLED` and\n\
         `TELEMETRY_CONSENT_MODE` in `vendor/emulator-telemetry/src/policy.rs` apply.\n\
         The startup notice prints the resolved config path.\n\
         \n\
         The generated router receives its stage endpoint explicitly. A distributor may\n\
         override it with `{TELEMETRY_ENDPOINT_ENV}` only toward the same backend host;\n\
         any other value is ignored, and without a stage endpoint the client is inert.\n\
         When enabled, the client obeys the `{CONFIG_PATH}` kill switch, sampling, and\n\
         batch caps. {retention_note}\n\
         \n\
         {README_REDISTRIBUTION_NOTE}\n\
         \n\
         To change tenant-agnostic telemetry policy, edit\n\
         `vendor/emulator-telemetry/src/policy.rs`.\n",
        retention_note = outbox_retention_note(),
    ));
    section
}

// ---------------------------------------------------------------------------
// Redaction rules (the privacy boundary — no request bodies, headers,
// credentials, ARNs, account or personal identifiers, resource names/keys,
// file paths, environment/config values, raw User-Agent, error messages, or
// customer code)
// ---------------------------------------------------------------------------

/// Lower-case fragments no wire key may contain, guarding the allowlist
/// itself against ever growing a payload- or identity-carrying field.
pub const FORBIDDEN_WIRE_KEY_FRAGMENTS: &[&str] = &[
    "body",
    "header",
    "arn",
    "account",
    "credential",
    "secret",
    "token",
    "password",
    "payload",
    "message",
    "stack",
    "email",
    "principal",
    "identity",
    "resourcename",
    "path",
    "file",
    "useragent",
];

/// Maximum length of the `projectId` dimension.
pub const PROJECT_ID_MAX_LEN: usize = 128;

/// Maximum length of every other string dimension (ids, namespace,
/// operation).
pub const DIMENSION_MAX_LEN: usize = 64;

/// Maximum length of the product version dimension, mirroring the backend
/// ingest contract's `maxProductVersionLength`.
pub const PRODUCT_VERSION_MAX_LEN: usize = 32;

/// Maximum numeric value of the product version's `major` and `minor`
/// components, mirroring the backend ingest contract's `maxVersionComponent`.
pub const PRODUCT_VERSION_COMPONENT_MAX: u64 = 99;

/// The only bytes a dimension may contain besides ASCII alphanumerics. The
/// set deliberately excludes `:`, `/`, `@`, and whitespace so ARNs, URLs,
/// file paths, emails, and prose structurally cannot pass as a dimension.
pub const DIMENSION_EXTRA_ALLOWED_BYTES: &[u8] = b"._-";

/// The exact `proj-` prefix of every canonical project id.
pub const PROJECT_ID_PREFIX: &str = "proj-";

/// The number of lowercase hex characters after [`PROJECT_ID_PREFIX`].
pub const PROJECT_ID_HEX_LEN: usize = 32;

/// The `environment.region` token reported when an observed region is not a
/// known AWS region: the closed vocabulary folds, it never forwards raw
/// values.
pub const REGION_UNKNOWN: &str = "unknown";

/// The `namespace` / `operation` token stored when a model-derived value does
/// not satisfy the dimension grammar ([`DIMENSION_MAX_LEN`],
/// [`DIMENSION_EXTRA_ALLOWED_BYTES`]): the recording API folds, it never
/// stores a raw identifier-like string.
pub const DIMENSION_UNKNOWN: &str = "unknown";

/// Every AWS region the wire may carry, sorted for binary search. A region
/// must match the closed grammar `^[a-z]{2}(-gov|-iso[a-z]?)?-[a-z]+-\d$` AND
/// appear here, else it folds to [`REGION_UNKNOWN`]. Additions are policy
/// edits reviewed with the backend contract.
pub const KNOWN_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-east-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-6",
    "ap-southeast-7",
    "ca-central-1",
    "ca-west-1",
    "cn-north-1",
    "cn-northwest-1",
    "eu-central-1",
    "eu-central-2",
    "eu-isoe-west-1",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-gov-east-1",
    "us-gov-west-1",
    "us-iso-east-1",
    "us-iso-west-1",
    "us-isob-east-1",
    "us-isof-east-1",
    "us-isof-south-1",
    "us-west-1",
    "us-west-2",
];

/// Maximum length of the `environment.region` dimension.
pub const REGION_MAX_LEN: usize = 32;

/// Maximum length of the `environment.sdkVersion` and
/// `environment.languageVersion` dimensions.
pub const VERSION_DIMENSION_MAX_LEN: usize = 64;

/// Maximum accepted length of an injected or server-pushed endpoint URL.
pub const ENDPOINT_MAX_LEN: usize = 2048;

// ---------------------------------------------------------------------------
// The errorType taxonomy (a closed 12-value vocabulary plus Other; raw error
// codes and messages never reach the wire)
// ---------------------------------------------------------------------------

/// Every taxonomy value the backend accepts.
pub const ERROR_TAXONOMY: [ErrorType; 13] = [
    ErrorType::ValidationError,
    ErrorType::ResourceNotFound,
    ErrorType::ResourceConflict,
    ErrorType::AccessDenied,
    ErrorType::AuthFailure,
    ErrorType::Throttling,
    ErrorType::LimitExceeded,
    ErrorType::UnsupportedOperation,
    ErrorType::SerializationError,
    ErrorType::Timeout,
    ErrorType::ServiceUnavailable,
    ErrorType::InternalError,
    ErrorType::Other,
];

/// The exact wire token for each closed [`ErrorType`] taxonomy value (an
/// `errorType` category label only, never the underlying error text).
pub const fn error_type_token(error_type: ErrorType) -> &'static str {
    match error_type {
        ErrorType::ValidationError => "ValidationError",
        ErrorType::ResourceNotFound => "ResourceNotFound",
        ErrorType::ResourceConflict => "ResourceConflict",
        ErrorType::AccessDenied => "AccessDenied",
        ErrorType::AuthFailure => "AuthFailure",
        ErrorType::Throttling => "Throttling",
        ErrorType::LimitExceeded => "LimitExceeded",
        ErrorType::UnsupportedOperation => "UnsupportedOperation",
        ErrorType::SerializationError => "SerializationError",
        ErrorType::Timeout => "Timeout",
        ErrorType::ServiceUnavailable => "ServiceUnavailable",
        ErrorType::InternalError => "InternalError",
        ErrorType::Other => "Other",
    }
}

/// Substring folding rules mapping raw error codes into the closed taxonomy,
/// most-specific families first (a code like `RequestThrottledException` must
/// fold to `Throttling`, not fall through to `Other`). Matching is
/// case-insensitive and ignores `_`/`-`, mirroring the backend's own
/// normalization; anything unmatched becomes [`ErrorType::Other`].
pub const ERROR_CODE_FOLDS: &[(&[&str], ErrorType)] = &[
    (&["throttl", "toomanyrequests"], ErrorType::Throttling),
    (
        &["limitexceeded", "quotaexceeded"],
        ErrorType::LimitExceeded,
    ),
    (
        &["accessdenied", "forbidden", "unauthorized"],
        ErrorType::AccessDenied,
    ),
    (
        &[
            "auth",
            "signature",
            "expiredtoken",
            "invalidtoken",
            "credential",
        ],
        ErrorType::AuthFailure,
    ),
    (&["notfound", "nosuch"], ErrorType::ResourceNotFound),
    (
        &["conflict", "alreadyexists", "inuse"],
        ErrorType::ResourceConflict,
    ),
    (
        &["unsupported", "notimplemented"],
        ErrorType::UnsupportedOperation,
    ),
    (
        &["serialization", "malformed"],
        ErrorType::SerializationError,
    ),
    (&["timeout", "timedout"], ErrorType::Timeout),
    (&["unavailable"], ErrorType::ServiceUnavailable),
    (&["internal"], ErrorType::InternalError),
    (
        &[
            "validation",
            "invalidparameter",
            "invalidrequest",
            "invalidinput",
        ],
        ErrorType::ValidationError,
    ),
];

// ---------------------------------------------------------------------------
// The startup notice (the emulator prints one line stating the telemetry
// state and exactly how to change it)
// ---------------------------------------------------------------------------

/// The client state the startup notice describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeState {
    /// Collecting: switch on, endpoint configured, worker running.
    Active { source: SwitchSource },
    /// The local master switch resolved to off.
    DisabledBySwitch { source: SwitchSource },
    /// Switch on, but no service endpoint is configured.
    NoEndpoint,
    /// Local state (directory, outbox) could not be initialized.
    InitFailed,
}

/// The `tracing` target the startup notice is emitted under when the
/// emulator runs embedded and stderr may not be visible.
pub const NOTICE_LOG_TARGET: &str = "emulator_telemetry";

/// Longest prefix of an invalid [`TELEMETRY_SWITCH_ENV`] value echoed back in
/// the warning line.
pub const INVALID_SWITCH_VALUE_ECHO_MAX: usize = 32;

/// The single warning line appended to the startup notice when
/// [`TELEMETRY_SWITCH_ENV`] holds an unrecognized value (which fails safe to
/// off).
pub fn invalid_switch_warning_line(raw_value: &str) -> String {
    let mut shown: String = raw_value
        .chars()
        .take(INVALID_SWITCH_VALUE_ECHO_MAX)
        .collect();
    if raw_value.chars().count() > INVALID_SWITCH_VALUE_ECHO_MAX {
        shown.push('…');
    }
    format!(
        "warning: ignoring invalid {TELEMETRY_SWITCH_ENV} value {shown:?} (expected {} or {}); \
         telemetry is off",
        SWITCH_ON_TOKENS.join("|"),
        SWITCH_OFF_TOKENS.join("|"),
    )
}

/// The single warning line appended to the startup notice (or emitted by the
/// worker) when deleting the local telemetry state failed after telemetry
/// became effectively off. Nothing is recorded in that state, but the stale
/// artifact remains on disk until the user removes it.
pub fn state_purge_warning_line(error: &StatePurgeError) -> String {
    format!(
        "warning: could not delete the local telemetry state under \
         {CONFIG_DIR_NAME}/{TELEMETRY_STATE_DIR_NAME}/ ({error}); telemetry is off and \
         records nothing, remove the directory manually"
    )
}

/// The single warning line the worker emits when the local telemetry state
/// cannot be opened for a reason other than another process holding its lock
/// (which only delays the open). The startup notice has already been emitted
/// by then, so this line corrects it.
pub fn state_init_warning_line() -> String {
    format!(
        "warning: could not initialize the local telemetry state under \
         {CONFIG_DIR_NAME}/{TELEMETRY_STATE_DIR_NAME}/; telemetry records nothing until the \
         next start"
    )
}

/// The single warning line the worker emits when the server's last-seen
/// configuration could not be read or persisted. Collection follows the live
/// server decision when one was fetched and stops for the session when none
/// could be — the client never falls back to its shipped defaults over a
/// cached decision it cannot read.
pub fn config_cache_warning_line(error: &ConfigCacheError) -> String {
    format!(
        "warning: could not read or persist the server's telemetry configuration under \
         {CONFIG_DIR_NAME}/{TELEMETRY_STATE_DIR_NAME}/{SERVER_CONFIG_CACHE_FILE} ({error}); \
         telemetry follows the live server decision or stays off until the next start"
    )
}

/// Render the one-line startup notice. Pure: wording derives entirely from
/// the consent mode, the compiled default, the resolved state, and the config
/// file path — so flipping [`TELEMETRY_CONSENT_MODE`] or
/// [`TELEMETRY_DEFAULT_ENABLED`] re-words every affected line automatically.
/// Callers pass the policy constants; tests exercise all combinations.
pub fn startup_notice_line(
    consent: ConsentMode,
    default_enabled: bool,
    state: NoticeState,
    config_path: &str,
) -> String {
    let subject = "This emulator collects anonymous usage and error telemetry";
    let switch_env = TELEMETRY_SWITCH_ENV;
    let off = SWITCH_OFF;
    let on = SWITCH_ON;
    let section = CONFIG_TOML_TELEMETRY_SECTION;
    let key = CONFIG_FILE_ENABLED_KEY;
    let how_to_disable =
        format!("disable with {switch_env}={off} or [{section}] {key} = false in {config_path}");
    let how_to_enable =
        format!("enable with {switch_env}={on} or [{section}] {key} = true in {config_path}");
    let how_to_reenable = "to turn it back on, remove every opt-out setting and restart";
    match state {
        NoticeState::Active { source } => {
            let via = match source {
                SwitchSource::Env => format!("via {switch_env}={on}"),
                SwitchSource::ConfigFile => format!("via {config_path}"),
                SwitchSource::Api => "via the embedding configuration".to_string(),
                _ => match (consent, default_enabled) {
                    (ConsentMode::OptOut, true) => "on by default; opt out any time".to_string(),
                    _ => "via default".to_string(),
                },
            };
            format!("{subject}: enabled ({via}); {how_to_disable}")
        }
        NoticeState::DisabledBySwitch { source } => match source {
            SwitchSource::Env => {
                format!("{subject}: disabled (via {switch_env}={off}) — {how_to_reenable}")
            }
            SwitchSource::ConfigFile => {
                format!("{subject}: disabled (via {config_path}) — {how_to_reenable}")
            }
            SwitchSource::Api => {
                format!("{subject}: disabled (via the embedding configuration) — {how_to_reenable}")
            }
            SwitchSource::Default => format!("{subject}: disabled (default) — {how_to_enable}"),
            SwitchSource::InvalidEnv | SwitchSource::InvalidFile => format!(
                "{subject}: disabled (invalid {switch_env} value or {config_path}; \
                 failing safe to off) — {how_to_reenable}"
            ),
        },
        NoticeState::NoEndpoint => {
            format!(
                "{subject}: inactive (enabled but the scaffold injected no stage \
                 endpoint) — {how_to_disable}"
            )
        }
        NoticeState::InitFailed => {
            format!("{subject}: inactive (local state initialization failed)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCANNED_PRODUCTION_SOURCES: &[(&str, &str)] = &[
        ("client.rs", include_str!("client.rs")),
        ("config.rs", include_str!("config.rs")),
        ("metadata.rs", include_str!("metadata.rs")),
        ("outbox.rs", include_str!("outbox.rs")),
        ("types.rs", include_str!("types.rs")),
        ("wire.rs", include_str!("wire.rs")),
        ("lib.rs", include_str!("lib.rs")),
    ];

    /// Modules that exist only under `#[cfg(test)]` and therefore ship no
    /// production code; tests deliberately pin wire literals, so the guard
    /// does not scan them.
    const TEST_ONLY_MODULES: &[&str] = &["test_support", "backend_contract", "integration_tests"];

    #[test]
    fn the_consent_posture_is_opt_out_and_on_by_default() {
        assert!(TELEMETRY_DEFAULT_ENABLED);
        assert_eq!(TELEMETRY_CONSENT_MODE, ConsentMode::OptOut);
        let notice = startup_notice_line(
            TELEMETRY_CONSENT_MODE,
            TELEMETRY_DEFAULT_ENABLED,
            NoticeState::Active {
                source: SwitchSource::Default,
            },
            "/home/x/.emulator/config.toml",
        );
        assert!(
            notice.contains("enabled (on by default; opt out any time)"),
            "the default-state notice announces on-by-default: {notice:?}"
        );
    }

    #[test]
    fn memory_usage_boundaries_match_the_closed_backend_taxonomy() {
        assert_eq!(memory_usage_bucket(0), MemoryBucket::LessThan256Mib);
        assert_eq!(memory_usage_bucket(255), MemoryBucket::LessThan256Mib);
        assert_eq!(memory_usage_bucket(256), MemoryBucket::From256To512Mib);
        assert_eq!(memory_usage_bucket(511), MemoryBucket::From256To512Mib);
        assert_eq!(memory_usage_bucket(512), MemoryBucket::From512MibTo1Gib);
        assert_eq!(memory_usage_bucket(1024), MemoryBucket::From1To2Gib);
        assert_eq!(memory_usage_bucket(2048), MemoryBucket::From2To4Gib);
        assert_eq!(memory_usage_bucket(4096), MemoryBucket::GreaterThan4Gib);
        for (bucket, token) in [
            (MemoryBucket::LessThan256Mib, MEMORY_LESS_THAN_256_MIB),
            (MemoryBucket::From256To512Mib, MEMORY_256_TO_512_MIB),
            (MemoryBucket::From512MibTo1Gib, MEMORY_512_MIB_TO_1_GIB),
            (MemoryBucket::From1To2Gib, MEMORY_1_TO_2_GIB),
            (MemoryBucket::From2To4Gib, MEMORY_2_TO_4_GIB),
            (MemoryBucket::GreaterThan4Gib, MEMORY_GREATER_THAN_4_GIB),
        ] {
            assert_eq!(memory_bucket_token(bucket), token);
        }
    }

    #[test]
    fn fallbacks_sit_inside_their_own_clamps() {
        let (fmin, fmax) = FLUSH_INTERVAL_CLAMP_SECONDS;
        assert!((fmin..=fmax).contains(&FALLBACK_FLUSH_INTERVAL_SECONDS));
        let (rmin, rmax) = ROWS_PER_BATCH_CLAMP;
        assert!((rmin..=rmax).contains(&FALLBACK_MAX_ROWS_PER_BATCH));
        let (bmin, bmax) = BODY_BYTES_CLAMP;
        assert!((bmin..=bmax).contains(&FALLBACK_MAX_BODY_BYTES));
        let (tmin, tmax) = CONFIG_TTL_CLAMP_SECONDS;
        assert!((tmin..=tmax).contains(&FALLBACK_CONFIG_TTL_SECONDS));
        let (smin, smax) = SAMPLING_RATE_CLAMP;
        assert!((smin..=smax).contains(&SAMPLING_DEFAULT));
        assert!(BACKOFF_BASE <= BACKOFF_CAP);
        assert!(CRASH_FLUSH_MAX_ROWS >= MIN_ROWS_PER_BATCH);
    }

    #[test]
    fn the_allowlist_itself_passes_the_redaction_rules() {
        for key in ALLOWED_WIRE_KEYS {
            let lower = key.to_ascii_lowercase();
            for forbidden in FORBIDDEN_WIRE_KEY_FRAGMENTS {
                assert!(
                    !lower.contains(forbidden),
                    "allowlisted key {key:?} matches forbidden fragment {forbidden:?}"
                );
            }
        }
        assert!(ALLOWED_WIRE_KEYS.contains(&KEY_SENT_AT));
        assert!(ALLOWED_WIRE_KEYS.contains(&KEY_BATCH_ID));
    }

    #[test]
    fn the_rendered_readme_section_covers_the_entire_allowlist() {
        let rendered = render_readme_telemetry_section();
        for key in ALLOWED_WIRE_KEYS {
            assert!(
                rendered.contains(&format!("`{key}`"))
                    || rendered.contains(&format!(".{key}`"))
                    || rendered.contains(&format!(".{key},"))
                    || rendered.contains(&format!("`{key},"))
                    || rendered.contains(&format!(".{key} "))
                    || rendered.contains(&format!("`{key} ")),
                "ALLOWED_WIRE_KEYS entry {key:?} is missing from the rendered README \
                 allowlist section — update README_ALLOWLIST_GROUPS"
            );
        }
        for container in WIRE_KEY_CONTAINERS {
            assert!(
                ALLOWED_WIRE_KEYS.contains(container),
                "container key {container:?} must itself be allowlisted"
            );
            assert!(
                rendered.contains(&format!("`{container}`")),
                "container key {container:?} is missing from the rendered README footer"
            );
        }
        assert!(rendered.starts_with("## Exact telemetry field allowlist\n"));
        assert!(rendered.contains("never sent.\n"));
        assert!(rendered.contains(&outbox_retention_note()));
        assert!(rendered.ends_with(&format!("{README_REDISTRIBUTION_NOTE}\n")));
    }

    #[test]
    fn every_taxonomy_value_folds_to_itself_and_folds_stay_inside_the_taxonomy() {
        for taxonomy in ERROR_TAXONOMY {
            assert_eq!(ErrorType::from_code(taxonomy.as_str()), taxonomy);
            assert_eq!(taxonomy.as_str(), error_type_token(taxonomy));
        }
        for (needles, taxonomy) in ERROR_CODE_FOLDS {
            assert!(ERROR_TAXONOMY.contains(taxonomy));
            assert!(!needles.is_empty());
        }
    }

    #[test]
    fn notice_wording_follows_consent_mode_and_default() {
        let path = "/home/x/.emulator/config.toml";
        let disabled_default = NoticeState::DisabledBySwitch {
            source: SwitchSource::Default,
        };
        let active_default = NoticeState::Active {
            source: SwitchSource::Default,
        };

        let opt_in_off = startup_notice_line(ConsentMode::OptIn, false, disabled_default, path);
        assert!(opt_in_off.contains("disabled"));
        assert!(opt_in_off.contains("enable with"));

        let opt_out_off = startup_notice_line(ConsentMode::OptOut, false, disabled_default, path);
        assert!(opt_out_off.contains("disabled"));

        let opt_out_on = startup_notice_line(ConsentMode::OptOut, true, active_default, path);
        assert!(opt_out_on.contains("enabled"));
        assert!(opt_out_on.contains("opt out"));
        assert!(opt_out_on.contains("disable with"));

        let opt_in_on = startup_notice_line(ConsentMode::OptIn, true, active_default, path);
        assert!(opt_in_on.contains("enabled"));
        assert_ne!(
            opt_out_on, opt_in_on,
            "flipping the consent mode alone must re-word the default-state notice"
        );

        for line in [&opt_in_off, &opt_out_off, &opt_out_on, &opt_in_on] {
            assert!(!line.contains('\n'), "the notice is one line");
            assert!(line.contains(TELEMETRY_SWITCH_ENV));
            assert!(line.contains(path));
        }
    }

    #[test]
    fn every_notice_names_the_published_categories_without_narrowing_them() {
        let path = "/home/x/.emulator/config.toml";
        let sources = [
            SwitchSource::Default,
            SwitchSource::Env,
            SwitchSource::ConfigFile,
            SwitchSource::Api,
            SwitchSource::InvalidEnv,
            SwitchSource::InvalidFile,
        ];
        let states = sources
            .iter()
            .flat_map(|&source| {
                [
                    NoticeState::Active { source },
                    NoticeState::DisabledBySwitch { source },
                ]
            })
            .chain([NoticeState::NoEndpoint, NoticeState::InitFailed]);
        for state in states {
            let line = startup_notice_line(
                TELEMETRY_CONSENT_MODE,
                TELEMETRY_DEFAULT_ENABLED,
                state,
                path,
            );
            assert!(
                line.starts_with("This emulator collects anonymous usage and error telemetry: "),
                "the notice must name the usage and error categories the page lists: {line}"
            );
            assert!(
                !line.contains("only"),
                "the notice must not narrow what is collected: {line}"
            );
        }
    }

    #[test]
    fn explicit_switch_notices_name_the_exact_control_used() {
        let path = "/home/x/.emulator/config.toml";
        let via_env_off = startup_notice_line(
            TELEMETRY_CONSENT_MODE,
            TELEMETRY_DEFAULT_ENABLED,
            NoticeState::DisabledBySwitch {
                source: SwitchSource::Env,
            },
            path,
        );
        assert!(via_env_off.contains(&format!("{TELEMETRY_SWITCH_ENV}={SWITCH_OFF}")));
        let via_file_on = startup_notice_line(
            TELEMETRY_CONSENT_MODE,
            TELEMETRY_DEFAULT_ENABLED,
            NoticeState::Active {
                source: SwitchSource::ConfigFile,
            },
            path,
        );
        assert!(via_file_on.contains(path));
        let no_endpoint = startup_notice_line(
            TELEMETRY_CONSENT_MODE,
            TELEMETRY_DEFAULT_ENABLED,
            NoticeState::NoEndpoint,
            path,
        );
        assert!(no_endpoint.contains("no stage endpoint"));
        assert!(
            !no_endpoint.contains(TELEMETRY_ENDPOINT_ENV),
            "the notice must not advertise the environment override as an activation path"
        );
    }

    #[test]
    fn opted_out_notices_tell_the_user_to_remove_every_opt_out_setting() {
        let path = "/home/x/.emulator/config.toml";
        for source in [
            SwitchSource::Env,
            SwitchSource::ConfigFile,
            SwitchSource::Api,
            SwitchSource::InvalidEnv,
            SwitchSource::InvalidFile,
        ] {
            let line = startup_notice_line(
                TELEMETRY_CONSENT_MODE,
                TELEMETRY_DEFAULT_ENABLED,
                NoticeState::DisabledBySwitch { source },
                path,
            );
            assert!(
                line.contains("remove every opt-out setting and restart"),
                "{source:?}: {line}"
            );
            assert!(
                !line.contains(&format!("{TELEMETRY_SWITCH_ENV}={SWITCH_ON}")),
                "an on setting never overrides an off setting, so the notice must not offer one: {line}"
            );
        }
    }

    #[test]
    fn the_policy_guard_scans_every_production_module() {
        let lib_source = include_str!("lib.rs");
        let scanned: Vec<&str> = SCANNED_PRODUCTION_SOURCES
            .iter()
            .filter_map(|(file, _)| file.strip_suffix(".rs"))
            .collect();
        let mut declared = Vec::new();
        let mut previous_is_cfg_test = false;
        for line in lib_source.lines() {
            let trimmed = line.trim();
            let declaration = trimmed
                .strip_prefix("pub mod ")
                .or_else(|| trimmed.strip_prefix("mod "));
            if let Some(rest) = declaration {
                let name = rest.trim_end_matches(';').trim();
                declared.push(name.to_string());
                if name == "policy" {
                    assert!(
                        !previous_is_cfg_test,
                        "the policy module itself must stay a production module"
                    );
                } else if previous_is_cfg_test {
                    assert!(
                        TEST_ONLY_MODULES.contains(&name),
                        "lib.rs declares test-only module {name:?} the policy guard does not \
                         know; add it to TEST_ONLY_MODULES or scan it"
                    );
                } else {
                    assert!(
                        scanned.contains(&name),
                        "lib.rs declares production module {name:?} the policy guard does \
                         not scan — add it to SCANNED_PRODUCTION_SOURCES"
                    );
                }
            }
            previous_is_cfg_test = trimmed == "#[cfg(test)]";
        }
        for name in scanned {
            assert!(
                name == "lib" || declared.iter().any(|module| module == name),
                "the policy guard scans {name:?} which lib.rs no longer declares"
            );
        }
        for name in TEST_ONLY_MODULES {
            assert!(
                declared.iter().any(|module| module == name),
                "TEST_ONLY_MODULES entry {name:?} is not declared by lib.rs"
            );
        }
    }

    /// The single-source guard: no sibling module's PRODUCTION code may carry
    /// a telemetry policy literal. Comments are stripped; string literals and
    /// numeric tokens are checked against the policy values defined above.
    #[test]
    fn no_policy_literals_outside_this_module() {
        let sources = SCANNED_PRODUCTION_SOURCES;
        let forbidden_fragments = [
            TELEMETRY_SWITCH_ENV,
            EMULATOR_CONFIG_ENV,
            EMULATOR_CONFIG_FILE_NAME,
            CONFIG_DIR_NAME,
            INGEST_CONTENT_TYPE,
            CONFIG_PATH,
            INGEST_PATH,
            OUTBOX_FILE,
            INSTALLATION_ID_FILE,
            SERVER_CONFIG_CACHE_FILE,
            SERVER_CONFIG_LOCK_FILE,
            SERVER_CONFIG_STAGED_FILE,
        ];
        let forbidden_exact: Vec<&str> = [
            SWITCH_ON,
            SWITCH_OFF,
            CONFIG_TOML_TELEMETRY_SECTION,
            CONFIG_FILE_ENABLED_KEY,
            CLIENT_VERSION,
            run_mode_token(RunMode::Standalone),
            run_mode_token(RunMode::Embedded),
            KEY_SENT_AT,
            KEY_BATCH_ID,
            RECORD_TYPE_BATCH,
            RECORD_TYPE_METRIC,
            NOT_EMULATED_SHAPE_NAME,
        ]
        .into_iter()
        .chain(ERROR_TAXONOMY.into_iter().map(error_type_token))
        .chain(
            [
                Outcome::Success,
                Outcome::ClientError,
                Outcome::ServerError,
                Outcome::NotEmulated,
                Outcome::Error,
                Outcome::Crashed,
            ]
            .into_iter()
            .map(outcome_token),
        )
        .chain(
            [CrashType::Panic, CrashType::Oom, CrashType::Sigkill]
                .into_iter()
                .map(crash_type_token),
        )
        .chain(
            [
                MetricType::Operation,
                MetricType::CoverageGap,
                MetricType::Error,
            ]
            .into_iter()
            .map(metric_type_token),
        )
        .collect();
        let forbidden_numbers = [
            "262144", "1048576", "86400", "604800", "100000", "10000", "8192", "3600", "2048",
            "1024", "501", "300", "256", "60", "24", "14",
        ];
        let environment_detection_fragments: Vec<&str> = CI_ENVS
            .iter()
            .copied()
            .chain([DOCKER_MARKER_PATH, PODMAN_MARKER_PATH, KUBERNETES_ENV, ECS_ENV])
            .collect();
        let user_agent_marker_fragments: Vec<&str> = SDK_USER_AGENT_MARKERS
            .iter()
            .flat_map(|(_, markers)| markers.iter().copied())
            .chain(LANGUAGE_VERSION_MARKERS.iter().copied())
            .chain(
                AGENT_CLIENT_MARKERS
                    .iter()
                    .flat_map(|(_, markers)| markers.iter().copied()),
            )
            .chain(AGENT_GENERIC_MARKERS.iter().copied())
            .collect();
        let environment_detection_literals: Vec<&str> =
            [SDK_UNKNOWN, SDK_MIXED, INSTALL_CHANNEL_UNKNOWN]
                .into_iter()
                .chain(
                    [
                        SdkName::AwsSdkRust,
                        SdkName::AwsSdkJava,
                        SdkName::Boto3,
                        SdkName::AwsSdkJsV3,
                        SdkName::AwsCli,
                    ]
                    .map(sdk_name_token),
                )
                .chain(
                    [
                        AgentClient::AmazonQ,
                        AgentClient::ClaudeCode,
                        AgentClient::Kiro,
                        AgentClient::Cursor,
                        AgentClient::Other,
                    ]
                    .map(agent_client_token),
                )
                .chain(
                    [
                        OsFamily::Linux,
                        OsFamily::Macos,
                        OsFamily::Windows,
                        OsFamily::Other,
                    ]
                    .map(os_token),
                )
                .chain([Arch::Amd64, Arch::Aarch64, Arch::Other].map(arch_token))
                .chain(
                    [
                        ContainerRuntime::Docker,
                        ContainerRuntime::Kubernetes,
                        ContainerRuntime::Ecs,
                        ContainerRuntime::Other,
                        ContainerRuntime::None,
                    ]
                    .map(container_runtime_token),
                )
                .chain(
                    [
                        InstallChannel::Cargo,
                        InstallChannel::Npm,
                        InstallChannel::Pip,
                        InstallChannel::Docker,
                        InstallChannel::Binary,
                        InstallChannel::Source,
                        InstallChannel::Other,
                        InstallChannel::Unknown,
                    ]
                    .map(install_channel_token),
                )
                .chain(
                    [
                        MemoryBucket::LessThan256Mib,
                        MemoryBucket::From256To512Mib,
                        MemoryBucket::From512MibTo1Gib,
                        MemoryBucket::From1To2Gib,
                        MemoryBucket::From2To4Gib,
                        MemoryBucket::GreaterThan4Gib,
                    ]
                    .map(memory_bucket_token),
                )
                .chain(
                    [StartupOutcome::Success, StartupOutcome::Failure].map(startup_outcome_token),
                )
                .chain([RunMode::Standalone, RunMode::Embedded].map(run_mode_token))
                .collect();

        for (file, source) in sources {
            let production = production_without_trailing_tests(source);
            let (code, strings) = strip_comments_and_extract_strings(production);
            for literal in &strings {
                for fragment in forbidden_fragments {
                    assert!(
                        !literal.contains(fragment),
                        "{file}: string literal {literal:?} carries policy value \
                         {fragment:?} — move it to policy.rs"
                    );
                }
                for exact in &forbidden_exact {
                    assert_ne!(
                        &literal.as_str(),
                        exact,
                        "{file}: string literal duplicates policy value {exact:?} — \
                         reference policy.rs instead"
                    );
                }
                if matches!(*file, "client.rs" | "metadata.rs") {
                    for &fragment in &environment_detection_fragments {
                        assert!(
                            !literal.contains(fragment),
                            "{file}: environment detection literal containing {fragment:?} must live in policy.rs"
                        );
                    }
                    for fragment in &user_agent_marker_fragments {
                        assert!(
                            !literal.contains(fragment),
                            "{file}: User-Agent marker literal containing {fragment:?} must live in policy.rs"
                        );
                    }
                    for exact in &environment_detection_literals {
                        assert_ne!(
                            &literal.as_str(),
                            exact,
                            "{file}: environment detection literal {exact:?} must live in policy.rs"
                        );
                    }
                }
            }
            for token in numeric_tokens(&code) {
                assert!(
                    !forbidden_numbers.contains(&token.as_str()),
                    "{file}: numeric literal {token} duplicates a policy value — \
                     reference policy.rs instead"
                );
            }
        }
    }

    #[test]
    fn policy_guard_keeps_production_after_inline_test_attributes() {
        let source = "before\n#[cfg(test)]\nfn helper() {}\nafter\n";
        assert_eq!(production_without_trailing_tests(source), source);

        let with_trailing_tests =
            "before\n#[cfg(test)]\nfn helper() {}\nafter\n#[cfg(test)]\nmod tests {}\n";
        let production = production_without_trailing_tests(with_trailing_tests);
        assert!(production.contains("fn helper"));
        assert!(production.contains("after"));
        assert!(!production.contains("mod tests"));
    }

    fn production_without_trailing_tests(source: &str) -> &str {
        let Some(attribute_index) = source.rfind("#[cfg(test)]") else {
            return source;
        };
        let after_attribute = source[attribute_index + "#[cfg(test)]".len()..].trim_start();
        let Some(after_module_name) = after_attribute.strip_prefix("mod tests") else {
            return source;
        };
        let module_delimiter = after_module_name.trim_start();
        if module_delimiter.starts_with('{') || module_delimiter.starts_with(';') {
            &source[..attribute_index]
        } else {
            source
        }
    }

    /// Strip `//` line comments and `/* */` block comments (nested), and pull
    /// out every `"..."` string literal (with escape handling). Char literals
    /// are skipped; lifetimes are left alone.
    fn strip_comments_and_extract_strings(source: &str) -> (String, Vec<String>) {
        let bytes = source.as_bytes();
        let mut code = String::new();
        let mut strings = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    let mut depth = 1;
                    i += 2;
                    while i < bytes.len() && depth > 0 {
                        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                            depth += 1;
                            i += 2;
                        } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                            depth -= 1;
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                }
                b'"' => {
                    let mut literal = String::new();
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        if i < bytes.len() {
                            literal.push(bytes[i] as char);
                            i += 1;
                        }
                    }
                    i += 1;
                    strings.push(literal);
                }
                b'\'' => {
                    let is_char_literal = match (bytes.get(i + 1), bytes.get(i + 2)) {
                        (Some(b'\\'), _) => true,
                        (Some(_), Some(b'\'')) => true,
                        _ => false,
                    };
                    if is_char_literal {
                        i += 1;
                        while i < bytes.len() && bytes[i] != b'\'' {
                            if bytes[i] == b'\\' {
                                i += 1;
                            }
                            i += 1;
                        }
                        i += 1;
                    } else {
                        code.push('\'');
                        i += 1;
                    }
                }
                other => {
                    code.push(other as char);
                    i += 1;
                }
            }
        }
        (code, strings)
    }

    /// Extract identifier-shaped tokens and keep the purely numeric ones,
    /// underscores stripped (`262_144` → `262144`).
    fn numeric_tokens(code: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        for c in code.chars() {
            if c.is_ascii_alphanumeric() || c == '_' {
                current.push(c);
            } else if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        tokens
            .into_iter()
            .filter(|t| t.chars().all(|c| c.is_ascii_digit() || c == '_'))
            .filter(|t| t.chars().any(|c| c.is_ascii_digit()))
            .map(|t| t.replace('_', ""))
            .collect()
    }
}
