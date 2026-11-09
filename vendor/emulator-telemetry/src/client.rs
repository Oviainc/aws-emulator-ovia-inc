// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The telemetry client: a fail-silent, non-blocking background worker.
//!
//! Recording is a bounded channel send that never blocks the caller; all I/O
//! (config fetch, SQLite, ingest POSTs) happens on one dedicated worker
//! thread. Every failure path is swallowed (logged at debug level at most):
//! telemetry must never affect emulator behavior, output, or shutdown beyond
//! the bounded final flush the caller explicitly requests.

use crate::config::{
    build_http_client, current_config, load_cached_config, resolve_config_file, resolve_state_dir,
    resolve_switch, switch_env_value, CollectionDenial, ConfigRefresh, Endpoint, ServerConfig,
    ServerConfigCache, Switch, SwitchSource,
};
use crate::metadata::{Merged, SanitizedRequestMetadata};
use crate::outbox::{backoff_delay, purge_outbox, purge_state, Outbox};
use crate::policy;
use crate::types::{
    AgentClient, Arch, BatchId, ConfigCacheError, ContainerRuntime, CrashType, ErrorType,
    InstallChannel, InstallationId, LanguageVersion, MetricType, Namespace, OperationName,
    OsFamily, Outcome, ProductVersion, ProjectId, Region, RunMode, SdkDimension, SdkName,
    SdkVersion, SessionId, StartupOutcome, StatePurgeError, TelemetryFeatureFlags,
};
use crate::wire::{
    build_batches, epoch_seconds_now, patch_sent_at, split_batch_body, BatchLimits, BatchPayload,
    CrashBlock, EnvironmentBlock, MetricKey, MetricValue, PerformanceBlock, ProductVersionFields,
    SessionContext,
};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{
    sync_channel, Receiver, RecvError, RecvTimeoutError, SyncSender, TrySendError,
};
use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

/// Options for [`TelemetryClient::start`].
#[derive(Debug, Clone)]
pub struct TelemetryOptions {
    /// The canonical project id this emulator was generated for.
    pub project_id: ProjectId,
    /// The product version selected by the scaffold's version source.
    pub product_version: ProductVersion,
    /// Boolean-only capabilities injected by the generated project.
    pub feature_flags: TelemetryFeatureFlags,
    /// The embedding API's switch source, DISABLE-ONLY by design. One of the
    /// three equal off-wins sources ([`crate::policy::TELEMETRY_SWITCH_ENV`],
    /// the emulator config file, and this field): `true` disables telemetry
    /// regardless of the other sources; `false` stays silent. An embedding
    /// application can never force telemetry on over the end user's
    /// environment variable or config file — force-on is unrepresentable.
    pub embedding_opt_out: bool,
    /// Override the state directory (default: `telemetry/` in the
    /// `emulator_paths::emulator_home` folder, independent of
    /// [`crate::policy::EMULATOR_CONFIG_ENV`]).
    pub state_dir: Option<PathBuf>,
    /// How the emulator runs. [`RunMode::Embedded`] writes the startup notice
    /// to the [`crate::policy::NOTICE_LOG_TARGET`] log instead of stderr.
    pub run_mode: RunMode,
    /// Stage-owned telemetry configuration endpoint injected by the scaffold,
    /// already validated as an [`Endpoint`]. [`policy::TELEMETRY_ENDPOINT_ENV`]
    /// may override it at runtime, but only with an endpoint on the same host
    /// — a cross-host override is ignored so a compromised environment cannot
    /// redirect telemetry away from the stage's backend. Without an injected
    /// endpoint the environment variable is ignored entirely: the environment
    /// is never an endpoint authority on its own.
    pub config_endpoint: Option<Endpoint>,
    /// Optional stage-owned ingest endpoint. Must share the configuration
    /// endpoint's host; when absent, the server config or the configuration
    /// endpoint's service base determines the ingest URL.
    pub ingest_endpoint: Option<Endpoint>,
    /// Override the server-directed flush interval (integration tests).
    pub flush_interval_override: Option<Duration>,
    retry_backoff_override: Option<Duration>,
    config_file_override: Option<PathBuf>,
    #[cfg(test)]
    switch_env_override: Option<String>,
    #[cfg(test)]
    notice_sink_override: Option<NoticeSink>,
}

/// A test-injectable sink for the startup notice, so integration tests can
/// observe emission order without capturing the process's stderr.
#[derive(Clone)]
pub(crate) struct NoticeSink(pub(crate) Arc<dyn Fn(&str) + Send + Sync>);

impl std::fmt::Debug for NoticeSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NoticeSink")
    }
}

impl TelemetryOptions {
    /// Create options from values injected by the project scaffold and stage.
    pub fn new(
        project_id: ProjectId,
        product_version: ProductVersion,
        config_endpoint: Endpoint,
    ) -> Self {
        Self {
            project_id,
            product_version,
            feature_flags: TelemetryFeatureFlags::default(),
            embedding_opt_out: false,
            state_dir: None,
            run_mode: RunMode::Standalone,
            config_endpoint: Some(config_endpoint),
            ingest_endpoint: None,
            flush_interval_override: None,
            retry_backoff_override: None,
            config_file_override: None,
            #[cfg(test)]
            switch_env_override: None,
            #[cfg(test)]
            notice_sink_override: None,
        }
    }

    /// Create options with no injected configuration endpoint. The client
    /// starts inert (`NoEndpoint`) and performs no network activity;
    /// [`policy::TELEMETRY_ENDPOINT_ENV`] is ignored because the environment
    /// alone is never trusted as an endpoint authority.
    pub fn without_endpoint(project_id: ProjectId, product_version: ProductVersion) -> Self {
        Self {
            project_id,
            product_version,
            feature_flags: TelemetryFeatureFlags::default(),
            embedding_opt_out: false,
            state_dir: None,
            run_mode: RunMode::Standalone,
            config_endpoint: None,
            ingest_endpoint: None,
            flush_interval_override: None,
            retry_backoff_override: None,
            config_file_override: None,
            #[cfg(test)]
            switch_env_override: None,
            #[cfg(test)]
            notice_sink_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_retry_backoff_override(&mut self, value: Duration) {
        self.retry_backoff_override = Some(value);
    }

    #[cfg(test)]
    pub(crate) fn set_config_file_override(&mut self, path: PathBuf) {
        self.config_file_override = Some(path);
    }

    #[cfg(test)]
    pub(crate) fn set_switch_env_override(&mut self, value: &str) {
        self.switch_env_override = Some(value.to_string());
    }

    #[cfg(test)]
    pub(crate) fn set_notice_sink_override(&mut self, sink: Arc<dyn Fn(&str) + Send + Sync>) {
        self.notice_sink_override = Some(NoticeSink(sink));
    }
}

enum Command {
    Metric {
        key: MetricKey,
        latency_millis: Option<u64>,
        request_metadata: Option<SanitizedRequestMetadata>,
    },
    Flush {
        is_final: bool,
        ack: SyncSender<()>,
    },
    Shutdown {
        deadline: Instant,
        ack: SyncSender<()>,
    },
}

/// Why the client is (or is not) collecting, for the startup notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    DisabledBySwitch(SwitchSource),
    NoEndpoint,
    InitFailed,
}

/// The server-side collection decision for this session. `Pending` until the
/// worker's startup config fetch has been answered: observations made in that
/// window wait in the bounded in-memory queue and are dropped unread if the
/// answer is no — nothing is aggregated, persisted, or sent before the
/// decision. `Denied` is sticky for the rest of the session (re-evaluated at
/// the next start) and means nothing is recorded, queued, or kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Collection {
    Pending = 0,
    Allowed = 1,
    Denied = 2,
}

/// The collection decision shared between the caller side
/// ([`TelemetryClient::recording_active`]), the worker, and the panic hook.
/// Reads are lock-free; the one mutual exclusion is between the worker's
/// denial purge and a crash write, so the panic hook can never recreate state
/// the purge is deleting.
#[derive(Clone)]
struct CollectionGate(Arc<GateCell>);

struct GateCell {
    decision: AtomicU8,
    exclusive: Mutex<()>,
}

impl CollectionGate {
    fn new() -> Self {
        Self(Arc::new(GateCell {
            decision: AtomicU8::new(Collection::Pending as u8),
            exclusive: Mutex::new(()),
        }))
    }

    /// Decode fail-closed: any byte that is not a known decision reads as
    /// `Denied`.
    fn load(&self) -> Collection {
        match self.0.decision.load(Ordering::Acquire) {
            raw if raw == Collection::Pending as u8 => Collection::Pending,
            raw if raw == Collection::Allowed as u8 => Collection::Allowed,
            _ => Collection::Denied,
        }
    }

    /// Let collection begin: `Pending` becomes `Allowed`. A closed gate never
    /// reopens — a worker whose decision lands after [`Self::close`] (a
    /// shutdown that outlived a slow startup) learns so from the `false`.
    fn open(&self) -> bool {
        let _ = self.0.decision.compare_exchange(
            Collection::Pending as u8,
            Collection::Allowed as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.allows()
    }

    /// Close the gate for good and run `purge` while no crash write can be
    /// in flight or start.
    fn close(&self, purge: impl FnOnce()) {
        let _exclusive = self.hold_exclusive();
        self.0
            .decision
            .store(Collection::Denied as u8, Ordering::Release);
        purge();
    }

    /// Run `write` only if collection is allowed and no purge is in progress.
    /// Never blocks (it runs inside the panic hook): a contended or poisoned
    /// lock means the write is skipped, which is the privacy-safe outcome.
    fn while_open(&self, write: impl FnOnce()) {
        let Ok(_exclusive) = self.0.exclusive.try_lock() else {
            return;
        };
        if self.allows() {
            write();
        }
    }

    fn hold_exclusive(&self) -> MutexGuard<'_, ()> {
        self.0
            .exclusive
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn allows(&self) -> bool {
        self.load() == Collection::Allowed
    }

    fn denies(&self) -> bool {
        self.load() == Collection::Denied
    }
}

/// The first failure of one kind, shared between the start path and the
/// worker so the client can report it and the notice carries it exactly once.
struct FailureSlot<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for FailureSlot<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for FailureSlot<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T: Copy> FailureSlot<T> {
    /// Keep the first failure; returns whether `error` was it.
    fn record(&self, error: T) -> bool {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return false;
        }
        *slot = Some(error);
        true
    }

    fn get(&self) -> Option<T> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether an inert client deletes the local telemetry state on its way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cleanup {
    /// Telemetry is effectively off, or this start created state it can no
    /// longer use: delete the installation id and the outbox.
    PurgeLocalState,
    /// Nothing was created and the state may belong to a concurrent starter
    /// of the same installation (a busy database, an unusable transport):
    /// record nothing, delete nothing.
    KeepLocalState,
}

/// Where notice lines go: the test-injected sink when present, otherwise the
/// surface the run mode demands.
#[derive(Clone, Debug)]
struct Notice {
    sink: Option<NoticeSink>,
    run_mode: RunMode,
}

impl Notice {
    fn emit(&self, line: &str) {
        match &self.sink {
            Some(sink) => (sink.0)(line),
            None => emit_notice_line_for(self.run_mode, line),
        }
    }
}

struct RuntimePerformance {
    started: Instant,
    current_concurrency: AtomicU64,
    peak_concurrency: AtomicU64,
    cold_start_millis: AtomicU64,
    ready: AtomicBool,
    startup_failed: AtomicBool,
}

impl RuntimePerformance {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            current_concurrency: AtomicU64::new(0),
            peak_concurrency: AtomicU64::new(0),
            cold_start_millis: AtomicU64::new(0),
            ready: AtomicBool::new(false),
            startup_failed: AtomicBool::new(false),
        }
    }

    fn begin_request(&self) {
        let current = self.current_concurrency.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_concurrency.fetch_max(current, Ordering::Relaxed);
    }

    fn mark_ready(&self) {
        let millis = duration_millis(self.started.elapsed()).saturating_add(1);
        if self
            .cold_start_millis
            .compare_exchange(0, millis, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.ready.store(true, Ordering::Release);
        }
    }

    fn mark_startup_failed(&self) {
        if !self.ready.load(Ordering::Acquire) {
            self.startup_failed.store(true, Ordering::Release);
        }
    }

    fn startup_outcome(&self) -> Option<StartupOutcome> {
        if self.ready.load(Ordering::Acquire) {
            Some(StartupOutcome::Success)
        } else if self.startup_failed.load(Ordering::Acquire) {
            Some(StartupOutcome::Failure)
        } else {
            None
        }
    }
}

/// RAII guard used by a generated router to track concurrent requests.
pub struct TelemetryRequestGuard {
    performance: Option<Arc<RuntimePerformance>>,
}

impl Drop for TelemetryRequestGuard {
    fn drop(&mut self) {
        if let Some(performance) = &self.performance {
            performance
                .current_concurrency
                .fetch_sub(1, Ordering::Relaxed);
        }
    }
}

struct Inner {
    state: State,
    switch: Switch,
    config_path: Option<PathBuf>,
    invalid_switch_value: Option<String>,
    gate: CollectionGate,
    purge_failure: FailureSlot<StatePurgeError>,
    cache_failure: FailureSlot<ConfigCacheError>,
    live: Option<Live>,
}

/// What only an active client has: the worker, its command channel, the
/// runtime performance counters, and the crash reporter.
struct Live {
    performance: Arc<RuntimePerformance>,
    sender: SyncSender<Command>,
    crash_reporter: Arc<CrashReporter>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Everything [`TelemetryClient::start`] resolves before it knows whether a
/// worker can run. Every inert outcome goes through [`Startup::inert`] with
/// an explicit [`Cleanup`] decision: an effectively-off client (switched off
/// from any source, or without an endpoint it could ever send to) and a start
/// that created state it cannot use delete the local state and keep nothing;
/// an initialization failure that created nothing and may be shared with a
/// concurrent starter of the same installation records nothing but deletes
/// nothing.
struct Startup {
    switch: Switch,
    config_path: Option<PathBuf>,
    invalid_switch_value: Option<String>,
    state_dir: Option<PathBuf>,
    notice: Notice,
}

impl Startup {
    fn inert(self, state: State, cleanup: Cleanup) -> TelemetryClient {
        let purge_failure = FailureSlot::default();
        if let Some(error) = self
            .state_dir
            .as_deref()
            .filter(|_| cleanup == Cleanup::PurgeLocalState)
            .and_then(|dir| purge_state(dir).err())
        {
            purge_failure.record(error);
        }
        self.finish(
            state,
            CollectionGate::new(),
            purge_failure,
            FailureSlot::default(),
            None,
        )
    }

    fn finish(
        self,
        state: State,
        gate: CollectionGate,
        purge_failure: FailureSlot<StatePurgeError>,
        cache_failure: FailureSlot<ConfigCacheError>,
        live: Option<Live>,
    ) -> TelemetryClient {
        let client = TelemetryClient {
            inner: Arc::new(Inner {
                state,
                switch: self.switch,
                config_path: self.config_path,
                invalid_switch_value: self.invalid_switch_value,
                gate,
                purge_failure,
                cache_failure,
                live,
            }),
        };
        self.notice.emit(&client.startup_notice());
        client
    }
}

/// Handle to the telemetry worker. Cheap to clone; all clones share one
/// worker. Dropping the last handle without calling
/// [`TelemetryClient::shutdown`] triggers a short bounded final flush on the
/// worker before it exits — the outbox still holds anything not yet
/// delivered.
#[derive(Clone)]
pub struct TelemetryClient {
    inner: Arc<Inner>,
}

/// Resolve the configuration endpoint from the stage-injected anchor and the
/// [`policy::TELEMETRY_ENDPOINT_ENV`] override. The injected endpoint is the
/// only authority: without it the override is ignored (the environment can
/// never introduce an endpoint), and with it the override applies only when
/// it targets the same host.
pub(crate) fn resolve_config_endpoint(
    env_value: Option<&str>,
    injected: Option<&Endpoint>,
) -> Option<Endpoint> {
    let injected = injected?;
    match env_value.and_then(|raw| Endpoint::parse(raw).ok()) {
        Some(from_env) if from_env.same_host(injected) => Some(from_env),
        _ => Some(injected.clone()),
    }
}

/// Write the startup notice to the sink the run mode demands: standalone
/// emulators own their stderr, while an embedded emulator's stderr may not be
/// visible, so the notice goes to the host application's log via the
/// [`policy::NOTICE_LOG_TARGET`] tracing target.
fn emit_notice_line_for(run_mode: RunMode, line: &str) {
    match run_mode {
        RunMode::Standalone => eprintln!("{line}"),
        RunMode::Embedded => tracing::info!(target: policy::NOTICE_LOG_TARGET, "{line}"),
    }
}

impl TelemetryClient {
    /// Start the client. Never fails and never panics: on any initialization
    /// problem the returned handle is inert and [`startup_notice`] explains
    /// why.
    ///
    /// The startup notice is emitted exactly once, synchronously, before the
    /// worker thread is released — nothing can be transmitted before the
    /// notice exists (standalone: stderr; embedded: the
    /// [`policy::NOTICE_LOG_TARGET`] tracing target, because stderr may not
    /// be visible inside a host application).
    ///
    /// Never blocks on local state: the outbox and installation id are opened
    /// by the worker, which buffers recordings in memory while another process
    /// holds the database locked. A state that cannot be opened at all stops
    /// collection for the session with one warning line.
    ///
    /// [`startup_notice`]: TelemetryClient::startup_notice
    pub fn start(options: TelemetryOptions) -> TelemetryClient {
        let config_path = options.config_file_override.clone().or_else(|| {
            resolve_config_file(std::env::var(policy::EMULATOR_CONFIG_ENV).ok().as_deref())
        });
        let state_dir = options.state_dir.clone().or_else(resolve_state_dir);

        let switch_env_raw = {
            let from_env = switch_env_value(std::env::var_os(policy::TELEMETRY_SWITCH_ENV));
            #[cfg(test)]
            let from_env = options.switch_env_override.clone().or(from_env);
            from_env
        };
        #[cfg(test)]
        let sink = options.notice_sink_override.clone();
        #[cfg(not(test))]
        let sink = None;
        let notice = Notice {
            sink,
            run_mode: options.run_mode,
        };

        let config_file = config_path.as_ref().map(std::fs::read_to_string).filter(
            |read| !matches!(read, Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        );
        let switch = resolve_switch(
            switch_env_raw.as_deref(),
            config_file.as_ref().map(Result::as_deref),
            options.embedding_opt_out,
        );
        let invalid_switch_value = (switch.source == SwitchSource::InvalidEnv)
            .then(|| switch_env_raw.clone().unwrap_or_default());
        let startup = Startup {
            switch,
            config_path,
            invalid_switch_value,
            state_dir,
            notice,
        };

        if !startup.switch.enabled {
            let source = startup.switch.source;
            return startup.inert(State::DisabledBySwitch(source), Cleanup::PurgeLocalState);
        }

        let endpoint = resolve_config_endpoint(
            std::env::var(policy::TELEMETRY_ENDPOINT_ENV)
                .ok()
                .as_deref(),
            options.config_endpoint.as_ref(),
        );
        let Some(endpoint) = endpoint else {
            return startup.inert(State::NoEndpoint, Cleanup::PurgeLocalState);
        };
        let ingest_endpoint = match &options.ingest_endpoint {
            Some(ingest) if ingest.same_host(&endpoint) => Some(ingest.clone()),
            Some(_) => return startup.inert(State::InitFailed, Cleanup::KeepLocalState),
            None => None,
        };
        let Some(http) = build_http_client() else {
            return startup.inert(State::InitFailed, Cleanup::KeepLocalState);
        };
        let Some(state_dir) = startup.state_dir.clone() else {
            return startup.inert(State::InitFailed, Cleanup::KeepLocalState);
        };
        let durable_state = DurableStateSpec {
            state_dir: state_dir.clone(),
            project_id: options.project_id.clone(),
            product_version: options.product_version.clone(),
            environment: detect_environment(options.feature_flags, options.run_mode),
        };

        let gate = CollectionGate::new();
        let purge_failure = FailureSlot::default();
        let cache_failure = FailureSlot::default();
        let performance = Arc::new(RuntimePerformance::new());
        let session_start = Instant::now();
        let crash_reporter = Arc::new(CrashReporter {
            context: OnceLock::new(),
            performance: performance.clone(),
            gate: gate.clone(),
            state_dir: state_dir.clone(),
            flush_sequence: AtomicU64::new(0),
            session_start,
        });

        let (sender, receiver) = sync_channel(policy::RECORD_QUEUE_BOUND);
        let (release_worker, notice_gate) = sync_channel::<()>(1);
        let seed = WorkerSeed {
            durable_state,
            config_cache: ServerConfigCache::open(&state_dir),
            http,
            endpoint,
            ingest_endpoint,
            gate: gate.clone(),
            purge_failure: purge_failure.clone(),
            cache_failure: cache_failure.clone(),
            notice: startup.notice.clone(),
            performance: performance.clone(),
            reporter: crash_reporter.clone(),
            flush_interval_override: options.flush_interval_override,
            retry_backoff_override: options.retry_backoff_override,
            session_start,
        };
        let worker = std::thread::Builder::new()
            .name(policy::WORKER_THREAD_NAME.to_string())
            .spawn(move || {
                let _ = notice_gate.recv();
                worker_main(Inbox::new(receiver), seed);
            });
        let Ok(worker) = worker else {
            return startup.inert(State::InitFailed, Cleanup::KeepLocalState);
        };

        let live = Live {
            performance,
            sender,
            crash_reporter,
            worker: Mutex::new(Some(worker)),
        };
        let client = startup.finish(
            State::Active,
            gate,
            purge_failure,
            cache_failure,
            Some(live),
        );
        let _ = release_worker.send(());
        client
    }

    /// Whether the client is actively collecting (switch on, endpoint
    /// configured, worker running, and the server has not denied collection
    /// for this session).
    pub fn is_active(&self) -> bool {
        self.inner.state == State::Active && !self.inner.gate.denies()
    }

    /// Whether recording calls can reach a live worker that will keep them.
    /// Cheap enough to gate per-request work: when this returns `false` the
    /// router should skip header sanitizing and every other telemetry-only
    /// allocation. `true` while the server decision is still pending (those
    /// observations are held in memory and dropped unread on a denial) and
    /// `false` for the rest of the session once the server config disables
    /// collection, does not sample this installation, or no longer supports
    /// this client version.
    pub fn recording_active(&self) -> bool {
        self.inner.live.is_some() && !self.inner.gate.denies()
    }

    /// The first failure to delete the local telemetry state after telemetry
    /// became effectively off, if any. Also appended to the startup notice
    /// (or emitted by the worker) as a warning line: a stale installation id
    /// or outbox is never silently left behind.
    pub fn state_purge_error(&self) -> Option<StatePurgeError> {
        self.inner.purge_failure.get()
    }

    /// The first failure to persist the server's last-seen configuration, if
    /// any. Also emitted by the worker as a warning line: while it stands a
    /// server-side denial holds for this session but cannot be remembered
    /// across an offline restart.
    pub fn config_cache_error(&self) -> Option<ConfigCacheError> {
        self.inner.cache_failure.get()
    }

    /// Mark the emulator ready to serve, for cold-start telemetry. The first
    /// call sets the startup time; later calls change nothing.
    pub fn mark_ready(&self) {
        if let Some(live) = &self.inner.live {
            live.performance.mark_ready();
        }
    }

    /// Record that the emulator stopped before it was ready to serve, so the
    /// session reports a failed startup. No effect once [`Self::mark_ready`]
    /// has run: an error after that is not a startup failure.
    pub fn mark_startup_failed(&self) {
        if let Some(live) = &self.inner.live {
            live.performance.mark_startup_failed();
        }
    }

    /// Start tracking one generated-router request. Dropping the returned guard
    /// decrements current concurrency even on early returns or unwinding.
    pub fn begin_request(&self) -> TelemetryRequestGuard {
        let performance = self
            .inner
            .live
            .as_ref()
            .map(|live| live.performance.clone());
        if let Some(performance) = &performance {
            performance.begin_request();
        }
        TelemetryRequestGuard { performance }
    }

    /// One line describing the telemetry state and exactly how to change it
    /// (plus a warning line when the switch env var held an invalid value, and
    /// another when deleting the local state failed), emitted once by
    /// [`TelemetryClient::start`] and also available here for the emulator's
    /// own surfaces. Wording derives from the [`crate::policy`] consent mode
    /// and default via [`policy::startup_notice_line`].
    pub fn startup_notice(&self) -> String {
        let config_path = match &self.inner.config_path {
            Some(path) => path.display().to_string(),
            None => format!(
                "~/{}/{}",
                policy::CONFIG_DIR_NAME,
                policy::EMULATOR_CONFIG_FILE_NAME
            ),
        };
        let state = match self.inner.state {
            State::Active => policy::NoticeState::Active {
                source: self.inner.switch.source,
            },
            State::DisabledBySwitch(source) => policy::NoticeState::DisabledBySwitch { source },
            State::NoEndpoint => policy::NoticeState::NoEndpoint,
            State::InitFailed => policy::NoticeState::InitFailed,
        };
        let mut notice = policy::startup_notice_line(
            policy::TELEMETRY_CONSENT_MODE,
            policy::TELEMETRY_DEFAULT_ENABLED,
            state,
            &config_path,
        );
        if let Some(raw) = &self.inner.invalid_switch_value {
            notice.push('\n');
            notice.push_str(&policy::invalid_switch_warning_line(raw));
        }
        if let Some(error) = self.state_purge_error() {
            notice.push('\n');
            notice.push_str(&policy::state_purge_warning_line(&error));
        }
        notice
    }

    /// Record one emulated-operation outcome. Non-blocking; drops (never
    /// blocks) when the queue is full or the client is inactive. Crate-private:
    /// [`record_generated_operation_with_handle`] is the sole public recording
    /// path, and it accepts only model-derived handles and classified
    /// failures.
    pub(crate) fn record_operation(
        &self,
        operation: &OperationHandle,
        outcome: Outcome,
        error_type: Option<ErrorType>,
        latency: Duration,
    ) {
        self.send_operation_metric(operation, outcome, error_type, latency, None);
    }

    /// Record an operation while attaching only the already-sanitized request
    /// context produced at the router boundary.
    pub(crate) fn record_operation_with_metadata(
        &self,
        operation: &OperationHandle,
        outcome: Outcome,
        error_type: Option<ErrorType>,
        latency: Duration,
        request_metadata: SanitizedRequestMetadata,
    ) {
        self.send_operation_metric(
            operation,
            outcome,
            error_type,
            latency,
            Some(request_metadata),
        );
    }

    fn send_operation_metric(
        &self,
        operation: &OperationHandle,
        outcome: Outcome,
        error_type: Option<ErrorType>,
        latency: Duration,
        request_metadata: Option<SanitizedRequestMetadata>,
    ) {
        self.send_metric(
            operation.metric_key(
                MetricType::Operation,
                outcome,
                error_type.filter(|_| outcome != Outcome::Success),
            ),
            Some(duration_millis(latency)),
            request_metadata,
        );
    }

    /// Record a request for an operation the emulator does not implement.
    pub(crate) fn record_coverage_gap(&self, operation: &OperationHandle) {
        self.send_metric(
            operation.metric_key(MetricType::CoverageGap, Outcome::NotEmulated, None),
            None,
            None,
        );
    }

    /// Record a coverage gap with sanitized request context.
    pub(crate) fn record_coverage_gap_with_metadata(
        &self,
        operation: &OperationHandle,
        request_metadata: SanitizedRequestMetadata,
    ) {
        self.send_metric(
            operation.metric_key(MetricType::CoverageGap, Outcome::NotEmulated, None),
            None,
            Some(request_metadata),
        );
    }

    fn send_metric(
        &self,
        key: MetricKey,
        latency_millis: Option<u64>,
        request_metadata: Option<SanitizedRequestMetadata>,
    ) {
        if !self.recording_active() {
            return;
        }
        if let Some(live) = &self.inner.live {
            match live.sender.try_send(Command::Metric {
                key,
                latency_millis,
                request_metadata,
            }) {
                Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    /// Install the process panic hook that durably enqueues a final session
    /// record with `crash: {crashType: "panic"}` — type only, never a
    /// location, message, or backtrace. The previous hook (default panic
    /// output included) still runs. The hook itself installs at most once per
    /// process and reports through whichever active client asked last; an
    /// effectively-off client asking retires the reporter, so a process whose
    /// telemetry is off records no crash at all, and a client whose collection
    /// the server denied (or that already shut down) can only retire its own
    /// reporter, never re-arm it.
    pub fn install_panic_hook(&self) {
        match &self.inner.live {
            Some(live) if !self.inner.gate.denies() => CrashHook::install(&live.crash_reporter),
            Some(live) => CrashHook::retire_if(&live.crash_reporter),
            None => CrashHook::retire(),
        }
    }

    /// Flush aggregated metrics to the outbox and attempt delivery, waiting
    /// up to `timeout`. Returns whether the flush completed in time. Mainly
    /// for tests; production relies on the interval and [`shutdown`].
    ///
    /// [`shutdown`]: TelemetryClient::shutdown
    pub fn flush_blocking(&self, timeout: Duration) -> bool {
        let Some(live) = &self.inner.live else {
            return true;
        };
        let (ack, done) = sync_channel(1);
        if live
            .sender
            .try_send(Command::Flush {
                is_final: false,
                ack,
            })
            .is_err()
        {
            return false;
        }
        done.recv_timeout(timeout).is_ok()
    }

    /// Bounded final flush: sends `isFinal: true`, attempts delivery until
    /// `timeout`, and joins the worker only after the worker acknowledged
    /// completion — a worker wedged mid-delivery (e.g. on a stalled server)
    /// is abandoned rather than awaited past the budget, so telemetry can
    /// never stall emulator shutdown. Retires this client's crash reporter and
    /// closes its collection gate for good: after shutdown nothing is written
    /// on a panic, not even through a reporter handle a panic hook upgraded a
    /// moment earlier, and an abandoned worker finishes at most the request
    /// already in flight before it stops delivering. Idempotent across
    /// clones; later calls return immediately.
    pub fn shutdown(&self, timeout: Duration) {
        let Some(live) = &self.inner.live else {
            return;
        };
        CrashHook::retire_if(&live.crash_reporter);
        let deadline = Instant::now() + timeout;
        let mut acknowledged = false;
        let (ack, done) = sync_channel(1);
        let mut command = Command::Shutdown { deadline, ack };
        loop {
            match live.sender.try_send(command) {
                Ok(()) => {
                    acknowledged = done
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .is_ok();
                    break;
                }
                Err(TrySendError::Full(returned)) if Instant::now() < deadline => {
                    command = returned;
                    std::thread::sleep(policy::SHUTDOWN_QUEUE_RETRY_PAUSE);
                }
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => break,
            }
        }
        self.inner.gate.close(|| {});
        if let Ok(mut worker) = live.worker.lock() {
            if let Some(handle) = worker.take() {
                if acknowledged {
                    let _ = handle.join();
                }
            }
        }
    }

    /// Opt out after the client started: stop collecting for the rest of the
    /// process and delete the stored installation id and outbox, as an opt-out
    /// at start does. Call it when an embedding application switches telemetry
    /// off while a client is running; [`TelemetryClient::shutdown`] alone keeps
    /// the local state, and the next process would send what it queued.
    ///
    /// Closes the collection gate first, so the final flush sends nothing,
    /// then shuts the worker down within [`policy::SHUTDOWN_FLUSH_BUDGET`] and
    /// deletes the state. A worker still stalled in a request when the budget
    /// runs out is abandoned, not awaited, and may still hold the outbox open;
    /// where the platform refuses to delete an open file, the deletion fails.
    /// Any deletion failure is reported by
    /// [`TelemetryClient::state_purge_error`]. A client that never started a
    /// worker returns at once: its start already deleted the state when it
    /// was off.
    pub fn opt_out(&self) {
        let Some(live) = &self.inner.live else {
            return;
        };
        self.inner.gate.close(|| {});
        self.shutdown(policy::SHUTDOWN_FLUSH_BUDGET);
        self.inner.gate.close(|| {
            if let Err(error) = purge_state(&live.crash_reporter.state_dir) {
                self.inner.purge_failure.record(error);
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn report_crash_for_test(&self) {
        if let Some(live) = &self.inner.live {
            live.crash_reporter.enqueue_crash(CrashType::Panic);
        }
    }

    #[cfg(test)]
    pub(crate) fn hold_purge_lock_for_test(&self) -> MutexGuard<'_, ()> {
        self.inner.gate.hold_exclusive()
    }
}

impl std::fmt::Debug for TelemetryClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelemetryClient")
            .field("state", &self.inner.state)
            .finish()
    }
}

/// The process-wide panic hook and the reporter it currently writes through.
/// The hook is installed once; the reporter slot is replaceable and holds a
/// weak reference, so a client that shut down, was dropped, or went
/// effectively off can never be written through by a later panic.
pub(crate) struct CrashHook;

static CRASH_REPORTER: Mutex<Option<Weak<CrashReporter>>> = Mutex::new(None);

impl CrashHook {
    fn install(reporter: &Arc<CrashReporter>) {
        Self::set(Some(Arc::downgrade(reporter)));
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                static FIRED: Once = Once::new();
                FIRED.call_once(|| Self::report(CrashType::Panic));
                previous(info);
            }));
        });
    }

    fn retire() {
        Self::set(None);
    }

    /// Retire the slot only when it still points at `reporter`, so one
    /// client's shutdown never silences a different active client.
    fn retire_if(reporter: &Arc<CrashReporter>) {
        let mut slot = CRASH_REPORTER
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot
            .as_ref()
            .is_some_and(|current| Weak::ptr_eq(current, &Arc::downgrade(reporter)))
        {
            *slot = None;
        }
    }

    fn set(reporter: Option<Weak<CrashReporter>>) {
        *CRASH_REPORTER
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = reporter;
    }

    /// Write the crash record through the current reporter, if any. Runs
    /// inside the panic hook, so it never blocks: a contended slot means no
    /// record.
    fn report(crash_type: CrashType) {
        let reporter = CRASH_REPORTER
            .try_lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade));
        if let Some(reporter) = reporter {
            reporter.enqueue_crash(crash_type);
        }
    }

    #[cfg(test)]
    pub(crate) fn report_for_test() {
        Self::report(CrashType::Panic);
    }
}

/// Shared with the panic hook: everything needed to durably enqueue a
/// crash-annotated session record from inside the hook, without touching the
/// (possibly panicking) worker.
struct CrashReporter {
    /// Set by the worker, with the persisted installation id, before it first
    /// opens the gate.
    context: OnceLock<SessionContext>,
    performance: Arc<RuntimePerformance>,
    gate: CollectionGate,
    state_dir: PathBuf,
    /// The worker's last used flush sequence, mirrored so the crash record
    /// extends the sequence instead of colliding with it.
    flush_sequence: AtomicU64,
    session_start: Instant,
}

impl CrashReporter {
    /// Durably enqueue the crash record — only while the server has allowed
    /// collection for this session. Before the decision or after a denial the
    /// hook records nothing, and the write is mutually exclusive with the
    /// worker's denial purge, so it can never recreate purged state.
    fn enqueue_crash(&self, crash_type: CrashType) {
        self.gate.while_open(|| {
            let Some(context) = self.context.get() else {
                return;
            };
            let Ok(outbox) = Outbox::open_for_crash(&self.state_dir) else {
                return;
            };
            let sequence = self.flush_sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let batches = build_batches(
                &BatchPayload {
                    context,
                    metrics: &BTreeMap::new(),
                    flush_sequence: sequence,
                    is_final: false,
                    session_duration_ms: duration_millis(self.session_start.elapsed()),
                    performance: performance_block(&self.performance, None),
                    crash: Some(CrashBlock { crash_type }),
                },
                BatchLimits {
                    max_rows_per_batch: policy::CRASH_FLUSH_MAX_ROWS,
                    max_body_bytes: policy::FALLBACK_MAX_BODY_BYTES,
                },
                &mut new_batch_id,
            );
            let now = epoch_seconds_now();
            for batch in &batches {
                let _ = outbox.enqueue(batch, now);
            }
        });
    }
}

fn new_batch_id() -> BatchId {
    BatchId::random()
}

fn duration_millis(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

fn performance_block(
    performance: &RuntimePerformance,
    throughput: Option<u64>,
) -> PerformanceBlock {
    let ready = performance.ready.load(Ordering::Acquire);
    PerformanceBlock {
        memory_usage: memory_usage_bucket(),
        throughput,
        peak_concurrency: Some(performance.peak_concurrency.load(Ordering::Relaxed)),
        cold_start_time: ready.then(|| {
            performance
                .cold_start_millis
                .load(Ordering::Relaxed)
                .saturating_sub(1)
        }),
        startup_outcome: performance.startup_outcome(),
    }
}

/// The session's peak memory use, bucketed. Left out on an OS with no peak
/// reading.
fn memory_usage_bucket() -> Option<crate::types::MemoryBucket> {
    peak_memory_mebibytes().map(policy::memory_usage_bucket)
}

/// Linux: the kernel's resident-set high-water mark (`VmHWM`). Any other OS
/// without `/proc/self/status` reads nothing.
#[cfg(not(any(windows, target_vendor = "apple")))]
fn peak_memory_mebibytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    Some(peak_resident_kibibytes(&status)? / policy::KIBIBYTES_PER_MIB)
}

/// macOS: the peak resident set from `getrusage`, which macOS reports in bytes.
#[cfg(target_vendor = "apple")]
fn peak_memory_mebibytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` fully writes `usage` when it returns 0.
    let usage = unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
            return None;
        }
        usage.assume_init()
    };
    Some(u64::try_from(usage.ru_maxrss).ok()? / policy::BYTES_PER_MIB)
}

/// Windows: the process's peak working set.
#[cfg(windows)]
fn peak_memory_mebibytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    let mut counters = std::mem::MaybeUninit::<PROCESS_MEMORY_COUNTERS>::uninit();
    // SAFETY: the pseudo-handle needs no closing, and `GetProcessMemoryInfo`
    // fully writes `counters` (of `size` bytes) when it returns nonzero.
    let counters = unsafe {
        if GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), size) == 0 {
            return None;
        }
        counters.assume_init()
    };
    Some(u64::try_from(counters.PeakWorkingSetSize).ok()? / policy::BYTES_PER_MIB)
}

#[cfg_attr(any(windows, target_vendor = "apple"), allow(dead_code))]
fn peak_resident_kibibytes(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmHWM:")?.trim();
        value.split_ascii_whitespace().next()?.parse::<u64>().ok()
    })
}

/// Whether any CI marker variable is set, `is_set` reporting one variable.
fn detect_ci(is_set: impl Fn(&str) -> bool) -> bool {
    policy::CI_ENVS.iter().any(|name| is_set(name))
}

/// Detect coarse, non-identifying environment dimensions.
fn detect_environment(feature_flags: TelemetryFeatureFlags, run_mode: RunMode) -> EnvironmentBlock {
    let is_ci = detect_ci(|name| std::env::var_os(name).is_some());
    let container_runtime = if std::env::var_os(policy::ECS_ENV).is_some() {
        ContainerRuntime::Ecs
    } else if std::env::var_os(policy::KUBERNETES_ENV).is_some() {
        ContainerRuntime::Kubernetes
    } else if std::path::Path::new(policy::DOCKER_MARKER_PATH).exists() {
        ContainerRuntime::Docker
    } else if std::path::Path::new(policy::PODMAN_MARKER_PATH).exists() {
        // Podman has no dedicated wire token; it folds to `other`.
        ContainerRuntime::Other
    } else {
        ContainerRuntime::None
    };
    let os = match std::env::consts::OS {
        value if value == policy::OS_LINUX => OsFamily::Linux,
        value if value == policy::OS_MACOS => OsFamily::Macos,
        value if value == policy::OS_WINDOWS => OsFamily::Windows,
        _ => OsFamily::Other,
    };
    let arch = match std::env::consts::ARCH {
        value if value == policy::ARCH_X86_64 => Arch::Amd64,
        value if value == policy::ARCH_AARCH64 => Arch::Aarch64,
        _ => Arch::Other,
    };
    EnvironmentBlock {
        os,
        arch,
        sdk_name: SdkDimension::Unknown,
        sdk_version: None,
        language_version: None,
        agent_client: None,
        region: None,
        run_mode,
        install_channel: install_channel(run_mode, policy::RELEASE_BUILD, container_runtime),
        is_ci,
        container_runtime,
        feature_flags,
    }
}

/// How this emulator was installed. Embedded, it is a crate the host builds.
/// Standalone, the release build is the downloadable binary, and that binary
/// in a container is the Docker image; any other build is from source.
fn install_channel(
    run_mode: RunMode,
    release_build: bool,
    container_runtime: ContainerRuntime,
) -> InstallChannel {
    match (run_mode, release_build, container_runtime) {
        (RunMode::Embedded, _, _) => InstallChannel::Cargo,
        (RunMode::Standalone, false, _) => InstallChannel::Source,
        (RunMode::Standalone, true, ContainerRuntime::None) => InstallChannel::Binary,
        (RunMode::Standalone, true, _) => InstallChannel::Docker,
    }
}

/// Session-sticky caller metadata, merged one sanitized observation at a
/// time. Every dimension is tri-state ([`Merged`]): once two observations
/// conflict it stays `Mixed` for the rest of the session — a later agreeing
/// observation can never un-mix it.
struct CallerMetadata {
    sdk: Merged<SdkName>,
    sdk_version: Merged<SdkVersion>,
    language_version: Merged<LanguageVersion>,
    region: Merged<Region>,
    agent: Merged<AgentClient>,
}

impl CallerMetadata {
    fn new() -> Self {
        Self {
            sdk: Merged::Unset,
            sdk_version: Merged::Unset,
            language_version: Merged::Unset,
            region: Merged::Unset,
            agent: Merged::Unset,
        }
    }

    fn absorb(&mut self, metadata: &SanitizedRequestMetadata) {
        match &metadata.sdk {
            Merged::Unset => {}
            Merged::Mixed => {
                self.sdk.absorb(Merged::Mixed);
                self.sdk_version.absorb(Merged::Mixed);
            }
            Merged::Value(name) => {
                self.sdk.absorb(Merged::Value(*name));
                if self.sdk.is_mixed() {
                    self.sdk_version.absorb(Merged::Mixed);
                } else {
                    self.sdk_version.observe(metadata.sdk_version.clone());
                }
            }
        }
        self.language_version
            .observe(metadata.language_version.clone());
        self.region.observe(metadata.region.clone());
        self.agent.observe(metadata.agent_client);
    }

    /// Render the sticky caller dimensions onto the statically detected
    /// environment: `Mixed` omits the field (`sdkName` alone has a wire
    /// token for it; a mixed agent renders as `other`).
    fn render(&self, base: &EnvironmentBlock) -> EnvironmentBlock {
        let mut environment = base.clone();
        environment.sdk_name = match &self.sdk {
            Merged::Unset => SdkDimension::Unknown,
            Merged::Value(name) => SdkDimension::Sdk(*name),
            Merged::Mixed => SdkDimension::Mixed,
        };
        environment.sdk_version = match self.sdk.value() {
            Some(_) => self.sdk_version.value().cloned(),
            None => None,
        };
        environment.language_version = self.language_version.value().cloned();
        environment.region = self.region.value().cloned();
        environment.agent_client = match &self.agent {
            Merged::Unset => None,
            Merged::Value(agent) => Some(*agent),
            Merged::Mixed => Some(AgentClient::Other),
        };
        environment
    }
}

struct WorkerState {
    outbox: Outbox,
    config_cache: ServerConfigCache,
    http: reqwest::blocking::Client,
    endpoint: Endpoint,
    ingest_endpoint: Option<Endpoint>,
    context: SessionContext,
    installation_id_storage: InstallationIdStorage,
    callers: CallerMetadata,
    gate: CollectionGate,
    purge_failure: FailureSlot<StatePurgeError>,
    cache_failure: FailureSlot<ConfigCacheError>,
    notice: Notice,
    performance: Arc<RuntimePerformance>,
    reporter: Arc<CrashReporter>,
    flush_interval_override: Option<Duration>,
    retry_backoff_override: Option<Duration>,
    metrics: BTreeMap<MetricKey, MetricValue>,
    total_observations: u64,
    session_start: Instant,
    session_decision: Option<SessionDecision>,
}

/// The most recent trustworthy configuration this session applied (fetched
/// or read back from the cache). When the on-disk cache later cannot be read
/// it stands in, but only within its own TTL — exactly as long as the cached
/// copy would have.
struct SessionDecision {
    config: ServerConfig,
    decided_at: u64,
}

impl SessionDecision {
    fn current(&self, now: u64) -> Option<ServerConfig> {
        let expires_at = self
            .decided_at
            .saturating_add(self.config.config_ttl_seconds);
        (now < expires_at).then(|| self.config.clone())
    }
}

/// The outbox and session context the worker opened.
struct DurableState {
    outbox: Outbox,
    context: SessionContext,
    installation_id_storage: InstallationIdStorage,
}

/// Whether the session's installation id is on disk yet. An unwritten id
/// lives only in memory until the server config allows collection for it, so
/// a denial never leaves a new identifier behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallationIdStorage {
    Persisted,
    Unwritten,
}

/// Everything the worker thread owns from birth, grouped so the spawn site
/// hands over a single value.
struct WorkerSeed {
    durable_state: DurableStateSpec,
    config_cache: ServerConfigCache,
    http: reqwest::blocking::Client,
    endpoint: Endpoint,
    ingest_endpoint: Option<Endpoint>,
    gate: CollectionGate,
    purge_failure: FailureSlot<StatePurgeError>,
    cache_failure: FailureSlot<ConfigCacheError>,
    notice: Notice,
    performance: Arc<RuntimePerformance>,
    reporter: Arc<CrashReporter>,
    flush_interval_override: Option<Duration>,
    retry_backoff_override: Option<Duration>,
    session_start: Instant,
}

/// What the worker needs to open the on-disk state off the caller's thread.
struct DurableStateSpec {
    state_dir: PathBuf,
    project_id: ProjectId,
    product_version: ProductVersion,
    environment: EnvironmentBlock,
}

/// Why the durable state could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateOpenError {
    /// Another process holds the database locked past the busy timeout.
    Busy,
    /// The state cannot be used at all (corrupt, unwritable, invalid id).
    Unusable,
}

impl From<rusqlite::Error> for StateOpenError {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy
            }
            _ => Self::Unusable,
        }
    }
}

impl DurableStateSpec {
    /// Open the outbox and read the installation id, assembling the session
    /// context. Without a persisted id a fresh one is held in memory only; it
    /// is written once the server config allows collection for it.
    fn open(&self) -> Result<DurableState, StateOpenError> {
        let outbox = Outbox::open(&self.state_dir)?;
        let (installation_id, installation_id_storage) = match outbox
            .load_installation_id()
            .map_err(|_| StateOpenError::Unusable)?
        {
            Some(id) => (id, InstallationIdStorage::Persisted),
            None => (InstallationId::random(), InstallationIdStorage::Unwritten),
        };

        let previous_version_key = format!("previous_product_version:{}", self.project_id.as_str());
        let previous_version = outbox
            .kv_get(&previous_version_key)
            .filter(|value| value != self.product_version.as_str())
            .and_then(|value| ProductVersion::new(value).ok());
        let _ = outbox.kv_put(&previous_version_key, self.product_version.as_str());

        let context = SessionContext {
            installation_id,
            session_id: SessionId::random(),
            project_id: self.project_id.clone(),
            version: ProductVersionFields {
                product_version: self.product_version.clone(),
                previous_version,
                first_run: installation_id_storage == InstallationIdStorage::Unwritten,
            },
            environment: self.environment.clone(),
        };
        Ok(DurableState {
            outbox,
            context,
            installation_id_storage,
        })
    }
}

/// The worker's command source: commands buffered while the durable state
/// was still opening, then the live channel.
struct Inbox {
    buffered: VecDeque<Command>,
    receiver: Receiver<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parked {
    Resume,
    Stop,
}

impl Inbox {
    fn new(receiver: Receiver<Command>) -> Self {
        Self {
            buffered: VecDeque::new(),
            receiver,
        }
    }

    fn recv(&mut self) -> Result<Command, RecvError> {
        match self.buffered.pop_front() {
            Some(command) => Ok(command),
            None => self.receiver.recv(),
        }
    }

    fn recv_timeout(&mut self, timeout: Duration) -> Result<Command, RecvTimeoutError> {
        match self.buffered.pop_front() {
            Some(command) => Ok(command),
            None => self.receiver.recv_timeout(timeout),
        }
    }

    /// Buffer incoming commands for `pause`, bounded like the channel: past
    /// [`policy::RECORD_QUEUE_BOUND`] metrics are dropped and flushes go
    /// unacknowledged. `Stop` once a shutdown is buffered or every sender is
    /// gone. Returns by `pause` even under a sustained stream of commands.
    fn park(&mut self, pause: Duration) -> Parked {
        let until = Instant::now() + pause;
        loop {
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Parked::Resume;
            }
            match self.receiver.recv_timeout(remaining) {
                Ok(command @ Command::Shutdown { .. }) => {
                    self.buffered.push_back(command);
                    return Parked::Stop;
                }
                Ok(command) if self.buffered.len() < policy::RECORD_QUEUE_BOUND => {
                    self.buffered.push_back(command);
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Parked::Resume,
                Err(RecvTimeoutError::Disconnected) => return Parked::Stop,
            }
        }
    }
}

/// The worker thread: open the durable state — retrying, with commands
/// buffered, while another process holds it locked — then run the command
/// loop. Unusable state stops collection for the session; a shutdown that
/// closed the gate without reaching the worker cancels further attempts.
fn worker_main(mut inbox: Inbox, seed: WorkerSeed) {
    loop {
        if seed.gate.denies() {
            return serve_denied(inbox);
        }
        match seed.durable_state.open() {
            Ok(_) if seed.gate.denies() => return serve_denied(inbox),
            Ok(durable) => return worker_loop(inbox, seed, durable),
            Err(StateOpenError::Busy) => {
                if inbox.park(policy::STATE_OPEN_RETRY_PAUSE) == Parked::Stop {
                    return serve_denied(inbox);
                }
            }
            Err(StateOpenError::Unusable) => {
                seed.notice.emit(&policy::state_init_warning_line());
                seed.gate.close(|| {});
                return serve_denied(inbox);
            }
        }
    }
}

fn worker_loop(mut inbox: Inbox, seed: WorkerSeed, durable: DurableState) {
    let mut state = WorkerState {
        outbox: durable.outbox,
        config_cache: seed.config_cache,
        http: seed.http,
        endpoint: seed.endpoint,
        ingest_endpoint: seed.ingest_endpoint,
        context: durable.context,
        installation_id_storage: durable.installation_id_storage,
        callers: CallerMetadata::new(),
        gate: seed.gate,
        purge_failure: seed.purge_failure,
        cache_failure: seed.cache_failure,
        notice: seed.notice,
        performance: seed.performance,
        reporter: seed.reporter,
        flush_interval_override: seed.flush_interval_override,
        retry_backoff_override: seed.retry_backoff_override,
        metrics: BTreeMap::new(),
        total_observations: 0,
        session_start: seed.session_start,
        session_decision: None,
    };

    let startup_config = match state.startup() {
        Ok(config) => config,
        Err(halt) => {
            state.halt(halt);
            return serve_denied(inbox);
        }
    };
    let mut next_flush = Instant::now() + state.flush_interval(Some(&startup_config));
    loop {
        let timeout = next_flush.saturating_duration_since(Instant::now());
        match inbox.recv_timeout(timeout) {
            Ok(Command::Metric {
                key,
                latency_millis,
                request_metadata,
            }) => {
                state.observe(key, latency_millis, request_metadata);
            }
            Ok(Command::Flush { is_final, ack }) => match state.flush(is_final, None) {
                Ok(config) => {
                    next_flush = Instant::now() + state.flush_interval(config.as_ref());
                    let _ = ack.try_send(());
                }
                Err(halt) => {
                    state.halt(halt);
                    let _ = ack.try_send(());
                    return serve_denied(inbox);
                }
            },
            Ok(Command::Shutdown { deadline, ack }) => {
                if let Err(halt) = state.flush(true, Some(deadline)) {
                    state.halt(halt);
                }
                let _ = ack.try_send(());
                return;
            }
            Err(RecvTimeoutError::Timeout) => match state.flush(false, None) {
                Ok(config) => {
                    next_flush = Instant::now() + state.flush_interval(config.as_ref());
                }
                Err(halt) => {
                    state.halt(halt);
                    return serve_denied(inbox);
                }
            },
            Err(RecvTimeoutError::Disconnected) => {
                if let Err(halt) = state.flush(
                    true,
                    Some(Instant::now() + policy::DISCONNECT_FINAL_FLUSH_GRACE),
                ) {
                    state.halt(halt);
                }
                return;
            }
        }
    }
}

/// The command loop after the worker stopped collecting for the session:
/// every observation is dropped, flushes and shutdown are acknowledged
/// immediately so callers never block, and no I/O of any kind happens for
/// the rest of the session.
fn serve_denied(mut inbox: Inbox) {
    loop {
        match inbox.recv() {
            Ok(Command::Metric { .. }) => {}
            Ok(Command::Flush { ack, .. }) => {
                let _ = ack.try_send(());
            }
            Ok(Command::Shutdown { ack, .. }) => {
                let _ = ack.try_send(());
                return;
            }
            Err(_) => return,
        }
    }
}

/// Why the worker stops collecting for the rest of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Halt {
    /// The server config denies collection (the caller purges accordingly).
    Denied(CollectionDenial),
    /// The gate was already closed — a shutdown outlived this startup or
    /// flush — so nothing may be stored, drained, or sent any more.
    Closed,
    /// Neither the server nor a readable cached decision is available (first
    /// run offline, or an unreadable cache): the client may not fall back to
    /// its shipped defaults over a decision the server never made or it cannot
    /// read, so it collects nothing until the next start.
    NoDecision,
    /// The server allowed collection but the installation id could not be
    /// persisted, so the session collects nothing and keeps every artifact.
    InstallationIdUnwritable,
}

impl WorkerState {
    /// The startup pass, run once before the command loop: fetch the server
    /// config immediately — so the flush interval, kill switch, sampling and
    /// minimum client version take effect now rather than one interval from
    /// now — decide whether this session may collect at all, and drain
    /// batches persisted by earlier sessions when it may.
    fn startup(&mut self) -> Result<ServerConfig, Halt> {
        let config = self.refresh_config(epoch_seconds_now())?;
        self.drain(&config, None);
        Ok(config)
    }

    /// Fetch (or reuse) the server config and apply the collection decision:
    /// `Ok` opens the gate, `Err` reports why the worker must stop. When the
    /// config cannot be fetched the cached copy decides — so a server that was
    /// last seen disabled stays denied even offline. When there is no cached
    /// copy or it cannot be read, the decision this session last applied
    /// stands in within its TTL; past that, or before the server ever
    /// answered, there is no decision and nothing — not even the installation
    /// id — is written. The first cache read or write
    /// failure is surfaced once through the notice and
    /// [`TelemetryClient::config_cache_error`].
    fn refresh_config(&mut self, now: u64) -> Result<ServerConfig, Halt> {
        let ConfigRefresh {
            config,
            cache_error,
        } = current_config(
            &self.http,
            &self.config_cache,
            &self.endpoint,
            &self.context.project_id,
            now,
        );
        if let Some(error) = cache_error {
            self.report_cache_error(error);
        }
        let config = match config {
            Some(config) => self.remember_decision(config, now),
            None => self.session_config(now).ok_or(Halt::NoDecision)?,
        };
        self.decide(config)
    }

    fn remember_decision(&mut self, config: ServerConfig, now: u64) -> ServerConfig {
        self.session_decision = Some(SessionDecision {
            config: config.clone(),
            decided_at: now,
        });
        config
    }

    fn session_config(&self, now: u64) -> Option<ServerConfig> {
        self.session_decision
            .as_ref()
            .and_then(|decision| decision.current(now))
    }

    fn report_cache_error(&self, error: ConfigCacheError) {
        if self.cache_failure.record(error) {
            self.notice.emit(&policy::config_cache_warning_line(&error));
        }
    }

    fn decide(&mut self, config: ServerConfig) -> Result<ServerConfig, Halt> {
        if let Some(denial) = config.collection_denial(&self.context.installation_id) {
            return Err(Halt::Denied(denial));
        }
        if self.installation_id_storage == InstallationIdStorage::Unwritten {
            self.persist_installation_id()?;
            return self.decide(config);
        }
        self.reporter.context.get_or_init(|| self.context.clone());
        if !self.gate.open() {
            return Err(Halt::Closed);
        }
        Ok(config)
    }

    /// Write the in-memory installation id now that the server allows
    /// collection for it. A concurrent starter may have persisted its own id
    /// first; that one is adopted, and the caller re-applies the decision to
    /// it since sampling is derived from the id.
    fn persist_installation_id(&mut self) -> Result<(), Halt> {
        let (installation_id, created) = self
            .outbox
            .claim_installation_id(&self.context.installation_id)
            .map_err(|_| Halt::InstallationIdUnwritable)?;
        self.context.installation_id = installation_id;
        self.context.version.first_run = created;
        self.installation_id_storage = InstallationIdStorage::Persisted;
        Ok(())
    }

    /// Stop collecting for the rest of the session. A server denial purges
    /// (see [`Self::deny`]); a gate already closed by shutdown needs nothing
    /// more; a missing decision or an unwritable installation id closes the
    /// gate and keeps every artifact, since nothing is known to be denied.
    fn halt(self, halt: Halt) {
        match halt {
            Halt::Denied(denial) => self.deny(denial),
            Halt::Closed => {}
            Halt::NoDecision | Halt::InstallationIdUnwritable => self.gate.close(|| {}),
        }
    }

    /// Close the gate for the rest of the session and delete everything this
    /// session or an earlier one queued: nothing recorded before the decision
    /// may be sent later. A server that disabled collection or no longer
    /// supports this client leaves no local state at all; an unsampled
    /// installation keeps only an installation id it had already persisted,
    /// so its deterministic sampling decision stays stable across restarts,
    /// and a new one is never written. The server's own
    /// cached configuration is kept in both cases so the denial holds while
    /// offline. A deletion failure is surfaced through the notice and
    /// [`TelemetryClient::state_purge_error`], never ignored.
    fn deny(self, denial: CollectionDenial) {
        let WorkerState {
            outbox,
            gate,
            purge_failure,
            notice,
            ..
        } = self;
        let state_dir = outbox.state_dir().to_path_buf();
        gate.close(move || {
            drop(outbox);
            let purged = match denial {
                CollectionDenial::NotSampled => purge_outbox(&state_dir),
                CollectionDenial::RemoteDisabled | CollectionDenial::UnsupportedVersion => {
                    purge_state(&state_dir)
                }
            };
            if let Err(error) = purged {
                if purge_failure.record(error) {
                    notice.emit(&policy::state_purge_warning_line(&error));
                }
            }
        });
    }

    fn flush_interval(&self, config: Option<&ServerConfig>) -> Duration {
        if let Some(interval) = self.flush_interval_override {
            return interval;
        }
        let seconds = config
            .map(|c| c.flush_interval_seconds)
            .unwrap_or(policy::FALLBACK_FLUSH_INTERVAL_SECONDS);
        Duration::from_secs(seconds)
    }

    /// Aggregate one observation — only once the server has allowed
    /// collection for this session. Observations that arrive before the
    /// decision or after a denial are dropped, never stored.
    fn observe(
        &mut self,
        key: MetricKey,
        latency_millis: Option<u64>,
        request_metadata: Option<SanitizedRequestMetadata>,
    ) {
        if !self.gate.allows() {
            return;
        }
        if let Some(metadata) = request_metadata {
            self.callers.absorb(&metadata);
        }
        self.total_observations = self.total_observations.saturating_add(1);
        if self.metrics.len() >= policy::MAX_AGGREGATED_KEYS && !self.metrics.contains_key(&key) {
            return;
        }
        self.metrics.entry(key).or_default().observe(latency_millis);
    }

    /// One flush cycle: refresh config, re-apply the collection decision,
    /// move aggregates into the outbox as NDJSON batches, then drain due
    /// batches. `Ok(None)` when the deadline had already passed; `Err` when
    /// the worker must stop (the caller purges on a server denial).
    fn flush(
        &mut self,
        is_final: bool,
        deadline: Option<Instant>,
    ) -> Result<Option<ServerConfig>, Halt> {
        if deadline.is_some_and(|value| Instant::now() >= value) {
            return Ok(None);
        }
        let now = epoch_seconds_now();
        let _ = self.outbox.enforce_bounds(now);
        let config = if deadline.is_some() {
            // A missing cache falls back to this session's decision and never to the
            // shipped defaults, so the final flush never sends on terms the server did
            // not set.
            let config = match load_cached_config(&self.config_cache, &self.context.project_id) {
                Ok(Some(cached)) => Some(cached.config),
                Ok(None) => self.session_config(now),
                Err(error) => {
                    self.report_cache_error(error);
                    self.session_config(now)
                }
            };
            self.decide(config.ok_or(Halt::NoDecision)?)?
        } else {
            self.refresh_config(now)?
        };

        let metrics = std::mem::take(&mut self.metrics);
        let sequence = self.reporter.flush_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let elapsed = self.session_start.elapsed();
        let throughput = if elapsed.as_secs() > 0 {
            Some(self.total_observations / elapsed.as_secs())
        } else {
            Some(self.total_observations)
        };
        let context = SessionContext {
            environment: self.callers.render(&self.context.environment),
            ..self.context.clone()
        };
        let batches = build_batches(
            &BatchPayload {
                context: &context,
                metrics: &metrics,
                flush_sequence: sequence,
                is_final,
                session_duration_ms: duration_millis(elapsed),
                performance: performance_block(&self.performance, throughput),
                crash: None,
            },
            BatchLimits {
                max_rows_per_batch: config.max_rows_per_batch,
                max_body_bytes: config.max_body_bytes,
            },
            &mut new_batch_id,
        );
        for batch in &batches {
            let _ = self.outbox.enqueue(batch, now);
        }

        self.drain(&config, deadline);
        Ok(Some(config))
    }

    /// Attempt delivery of every due batch: 2xx acknowledges, 400 splits (or
    /// drops an unsplittable batch), 408/429 and 5xx/transport failures retry
    /// with backoff, and other 4xx responses drop as undeliverable. A stored
    /// row whose body is not a batch this crate built is deleted unsent.
    /// Delivery stops at the next batch boundary once the gate has closed
    /// (a shutdown that gave up on this worker), so an abandoned worker sends
    /// at most the request already in flight; the rest stays queued.
    fn drain(&mut self, config: &ServerConfig, deadline: Option<Instant>) {
        let ingest_url = self
            .ingest_endpoint
            .as_ref()
            .map(|endpoint| endpoint.as_str().to_string())
            .or_else(|| {
                config
                    .ingest_endpoint
                    .as_ref()
                    .filter(|endpoint| endpoint.same_host(&self.endpoint))
                    .map(|endpoint| endpoint.as_str().to_string())
            })
            .unwrap_or_else(|| self.endpoint.ingest_url());

        let now = epoch_seconds_now();
        let _ = self.outbox.enforce_bounds(now);
        let Ok(due) = self.outbox.due_batches(now, policy::DRAIN_BATCH_LIMIT) else {
            return;
        };
        for batch in due {
            if deadline.is_some_and(|d| Instant::now() >= d) || !self.gate.allows() {
                return;
            }
            let Some(body) = patch_sent_at(&batch.body, &batch.batch_id, epoch_seconds_now())
            else {
                let _ = self.outbox.delete(&batch.batch_id);
                continue;
            };
            let response = self
                .http
                .post(&ingest_url)
                .header(reqwest::header::CONTENT_TYPE, policy::INGEST_CONTENT_TYPE)
                .body(body)
                .send();
            match response {
                Ok(resp) if resp.status().is_success() => {
                    let _ = self.outbox.delete(&batch.batch_id);
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::BAD_REQUEST => {
                    match split_batch_body(&batch.body, &mut new_batch_id) {
                        Some(halves) => {
                            let _ = self.outbox.replace_with_split(
                                &batch.batch_id,
                                &halves,
                                epoch_seconds_now(),
                            );
                        }
                        None => {
                            let _ = self.outbox.delete(&batch.batch_id);
                        }
                    }
                }
                Ok(resp)
                    if resp.status() == reqwest::StatusCode::REQUEST_TIMEOUT
                        || resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS =>
                {
                    let delay = self
                        .retry_backoff_override
                        .unwrap_or_else(|| backoff_delay(&batch.batch_id, batch.attempts));
                    let _ = self
                        .outbox
                        .record_failure(&batch.batch_id, epoch_seconds_now(), delay);
                }
                Ok(resp) if resp.status().is_client_error() => {
                    let _ = self.outbox.delete(&batch.batch_id);
                }
                _ => {
                    let delay = self
                        .retry_backoff_override
                        .unwrap_or_else(|| backoff_delay(&batch.batch_id, batch.attempts));
                    let _ = self
                        .outbox
                        .record_failure(&batch.batch_id, epoch_seconds_now(), delay);
                }
            }
        }
    }
}

/// Global handle for emulator binaries that install the panic hook before the
/// client exists, or need one shared client across generated modules.
static GLOBAL_CLIENT: OnceLock<TelemetryClient> = OnceLock::new();

/// Store the process-wide client (first call wins) and return it.
pub fn init_global(client: TelemetryClient) -> &'static TelemetryClient {
    GLOBAL_CLIENT.get_or_init(|| client)
}

/// The process-wide client, when [`init_global`] has run.
pub fn global() -> Option<&'static TelemetryClient> {
    GLOBAL_CLIENT.get()
}

/// Whether the process-wide client exists and can record. Generated routers
/// gate ALL per-request telemetry work on this so a disabled client costs
/// nothing per request.
pub fn recording_active() -> bool {
    global().is_some_and(TelemetryClient::recording_active)
}

/// Run a bounded final flush of the [`global`] client when the process exits normally, for an
/// embedder whose process never calls [`TelemetryClient::shutdown`] (a test binary). A signal
/// or an abort skips it. Registers once, however often it is called.
pub fn flush_at_process_exit() {
    extern "C" fn flush() {
        if let Some(client) = global() {
            client.shutdown(policy::EXIT_FLUSH_BUDGET);
        }
    }
    extern "C" {
        fn atexit(callback: extern "C" fn()) -> std::os::raw::c_int;
    }
    static REGISTER: Once = Once::new();
    // SAFETY: hands the C runtime a plain function to call once at exit; nothing is shared.
    REGISTER.call_once(|| unsafe {
        atexit(flush);
    });
}

/// A model-derived operation identity. Generated code implements this on one
/// marker type per operation stub, with both constants lifted straight from
/// the Smithy model at generation time — the recording API accepts nothing
/// else, so no request-time or free-form string can become a dimension.
/// Values outside their grammar (an operation must be a PascalCase AWS
/// operation name such as `CreateDBInstance`) fold to
/// [`policy::DIMENSION_UNKNOWN`] when the handle is built.
pub trait ModelOperation {
    /// The emulated service namespace (e.g. `sqs`).
    const NAMESPACE: &'static str;
    /// The emulated operation name (e.g. `SendMessage`).
    const OPERATION: &'static str;
}

/// Namespace + operation identity for one generated operation stub, built
/// once from a [`ModelOperation`] (e.g. in a `OnceLock`) and passed by
/// reference so per-request recording never re-parses or re-allocates the
/// dimensions. The only way to name an operation to the recording API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationHandle {
    namespace: Namespace,
    operation: OperationName,
}

impl OperationHandle {
    /// Build the handle for one model operation, folding either dimension to
    /// [`policy::DIMENSION_UNKNOWN`] when it falls outside the closed
    /// dimension grammar. Infallible: an unrecognised model value is recorded
    /// as unknown, never as its raw text.
    pub fn of<T: ModelOperation>() -> Self {
        Self {
            namespace: Namespace::fold(T::NAMESPACE),
            operation: OperationName::fold(T::OPERATION),
        }
    }

    /// Build the handle from the two model-derived names directly, folding
    /// either dimension to [`policy::DIMENSION_UNKNOWN`] when it falls outside
    /// the closed dimension grammar — the same fold [`of`](Self::of) applies to
    /// a [`ModelOperation`]'s constants.
    ///
    /// For a framework caller that holds the namespace and operation as
    /// model-derived `&str` (e.g. the service's signing name and the
    /// operation's wire name) rather than a per-operation `ModelOperation`
    /// marker type. Infallible and lossy in exactly the same way as `of`: an
    /// unrecognised value is recorded as unknown, never as its raw text, so no
    /// free-form or request-time string can become a dimension.
    pub fn folded(namespace: &str, operation: &str) -> Self {
        Self {
            namespace: Namespace::fold(namespace),
            operation: OperationName::fold(operation),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_dimensions(namespace: Namespace, operation: OperationName) -> Self {
        Self {
            namespace,
            operation,
        }
    }

    /// The validated namespace dimension.
    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// The validated operation dimension.
    pub fn operation(&self) -> &OperationName {
        &self.operation
    }

    fn metric_key(
        &self,
        metric_type: MetricType,
        outcome: Outcome,
        error_type: Option<ErrorType>,
    ) -> MetricKey {
        MetricKey {
            metric_type,
            namespace: self.namespace.clone(),
            operation: self.operation.clone(),
            outcome,
            error_type,
        }
    }
}

/// A failed generated operation, classified from protocol-level facts alone
/// (the response HTTP status and the error's wire shape name) into the closed
/// outcome and error-type vocabularies. The raw shape name is discarded
/// here, at the fold boundary, exactly like [`ErrorType::from_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationFailure {
    outcome: Outcome,
    error_type: Option<ErrorType>,
}

impl OperationFailure {
    /// Classify one failure. [`policy::NOT_EMULATED_HTTP_STATUS`] together
    /// with [`policy::NOT_EMULATED_SHAPE_NAME`] is the scaffold's
    /// not-implemented signal and becomes a coverage gap
    /// ([`Outcome::NotEmulated`], no error type); every other pair becomes
    /// [`Outcome::from_http_status`] plus the shape name folded through
    /// [`ErrorType::from_code`].
    pub fn classify(http_status: u16, error_shape_name: &str) -> Self {
        if http_status == policy::NOT_EMULATED_HTTP_STATUS
            && error_shape_name == policy::NOT_EMULATED_SHAPE_NAME
        {
            return Self {
                outcome: Outcome::NotEmulated,
                error_type: None,
            };
        }
        Self {
            outcome: Outcome::from_http_status(http_status),
            error_type: Some(ErrorType::from_code(error_shape_name)),
        }
    }

    /// The classified outcome.
    pub fn outcome(self) -> Outcome {
        self.outcome
    }

    /// The folded error type; `None` only for a coverage gap.
    pub fn error_type(self) -> Option<ErrorType> {
        self.error_type
    }

    /// Whether this failure is the not-implemented signal.
    pub fn is_coverage_gap(self) -> bool {
        self.outcome == Outcome::NotEmulated
    }
}

/// Record one generated-emulator operation outcome against the process-wide
/// client ([`global`]) through a pre-built [`OperationHandle`], skipping all
/// per-request parsing:
///
/// * `failure: None` — a success operation row, with latency.
/// * a coverage-gap [`OperationFailure`] — a coverage-gap row
///   ([`Outcome::NotEmulated`]).
/// * any other [`OperationFailure`] — an operation row carrying its
///   classified outcome and folded error type.
///
/// Fail-silent: with no global client, or once the server has denied
/// collection for this session, nothing is recorded.
pub fn record_generated_operation_with_handle(
    handle: &OperationHandle,
    latency: Duration,
    failure: Option<OperationFailure>,
    request_metadata: Option<SanitizedRequestMetadata>,
) {
    let Some(client) = global() else {
        return;
    };
    if !client.recording_active() {
        return;
    }
    dispatch_generated_operation(client, handle, latency, failure, request_metadata);
}

pub(crate) fn dispatch_generated_operation(
    client: &TelemetryClient,
    handle: &OperationHandle,
    latency: Duration,
    failure: Option<OperationFailure>,
    request_metadata: Option<SanitizedRequestMetadata>,
) {
    let (outcome, error_type) = match failure {
        None => (Outcome::Success, None),
        Some(failure) if failure.is_coverage_gap() => {
            return match request_metadata {
                Some(metadata) => client.record_coverage_gap_with_metadata(handle, metadata),
                None => client.record_coverage_gap(handle),
            };
        }
        Some(failure) => (failure.outcome(), failure.error_type()),
    };
    match request_metadata {
        Some(metadata) => {
            client.record_operation_with_metadata(handle, outcome, error_type, latency, metadata)
        }
        None => client.record_operation(handle, outcome, error_type, latency),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_ci_marker_counts_as_ci_not_only_the_generic_one() {
        assert!(!detect_ci(|_| false));
        // Each marker alone is enough, including the CI systems that never
        // set the generic one.
        for &marker in policy::CI_ENVS {
            assert!(detect_ci(|name| name == marker), "{marker} alone must mean CI");
        }
    }

    #[test]
    fn install_channel_follows_run_mode_release_build_and_container() {
        use ContainerRuntime::{Docker, Kubernetes, None as Bare};
        use InstallChannel::{Binary, Cargo, Docker as Image, Source};
        for (run_mode, release_build, container, expected) in [
            (RunMode::Embedded, true, Bare, Cargo),
            (RunMode::Embedded, false, Docker, Cargo),
            (RunMode::Standalone, false, Bare, Source),
            (RunMode::Standalone, false, Docker, Source),
            (RunMode::Standalone, true, Bare, Binary),
            (RunMode::Standalone, true, Docker, Image),
            (RunMode::Standalone, true, Kubernetes, Image),
        ] {
            assert_eq!(
                install_channel(run_mode, release_build, container),
                expected
            );
        }
    }

    #[test]
    fn startup_reports_failure_only_when_it_ends_before_ready() {
        let pending = RuntimePerformance::new();
        assert_eq!(performance_block(&pending, None).startup_outcome, None);

        let failed = RuntimePerformance::new();
        failed.mark_startup_failed();
        let block = performance_block(&failed, None);
        assert_eq!(block.startup_outcome, Some(StartupOutcome::Failure));
        assert_eq!(block.cold_start_time, None);

        let ready = RuntimePerformance::new();
        ready.mark_ready();
        ready.mark_startup_failed();
        let block = performance_block(&ready, None);
        assert_eq!(block.startup_outcome, Some(StartupOutcome::Success));
        assert!(block.cold_start_time.is_some());
    }

    #[test]
    fn memory_usage_reads_the_peak_not_the_current_resident_set() {
        let status = "Name:\temulator\nVmHWM:\t  812345 kB\nVmRSS:\t  204800 kB\n";
        assert_eq!(peak_resident_kibibytes(status), Some(812_345));
        assert_eq!(peak_resident_kibibytes("VmRSS:\t204800 kB\n"), None);
    }

    #[test]
    fn park_resumes_on_time_under_a_sustained_stream_of_commands() {
        let (sender, receiver) = sync_channel(policy::RECORD_QUEUE_BOUND);
        let producing = Arc::new(AtomicBool::new(true));
        let producer = std::thread::spawn({
            let producing = producing.clone();
            move || {
                while producing.load(Ordering::Relaxed) {
                    let (ack, _) = sync_channel(1);
                    let _ = sender.try_send(Command::Flush {
                        is_final: false,
                        ack,
                    });
                }
            }
        });

        let pause = Duration::from_millis(50);
        let parked_at = Instant::now();
        let parked = Inbox::new(receiver).park(pause);
        let parked_for = parked_at.elapsed();
        producing.store(false, Ordering::Relaxed);
        producer.join().unwrap();

        assert_eq!(parked, Parked::Resume);
        assert!(
            parked_for < pause + Duration::from_secs(1),
            "park returned after {parked_for:?}"
        );
    }
}
