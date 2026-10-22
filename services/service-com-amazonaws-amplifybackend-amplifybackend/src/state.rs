// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplifybackend-amplifybackend service.
//!
//! One `BackendApp` per Amplify app id, each holding the backend environments the
//! app owns and every async job recorded against it. Every mutating
//! AmplifyBackend operation answers with a job record, and `GetBackendJob` reads
//! that same record back, so the jobs map is the single home for them.
//!
//! `BTreeMap` rather than `HashMap` everywhere a collection is reported on the
//! wire: `GetBackend`'s `BackendEnvironmentList` is derived from
//! [`BackendApp::environments`], and a hash map would order it differently on
//! every process.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};

use crate::ops::types::{
    BackendAPIResourceConfig, BadRequestException, CreateBackendAuthResourceConfig,
    CreateBackendStorageResourceConfig, GatewayTimeoutException, NotFoundException, ResourceConfig,
    TooManyRequestsException,
};

/// `NotFoundException::ResourceType` for an app no backend was ever created for.
pub const RESOURCE_TYPE_BACKEND: &str = "backend";
/// `NotFoundException::ResourceType` for a backend environment that does not exist.
pub const RESOURCE_TYPE_BACKEND_ENVIRONMENT: &str = "backend-environment";
/// `NotFoundException::ResourceType` for an auth resource that does not exist.
pub const RESOURCE_TYPE_BACKEND_AUTH: &str = "backend-auth";
/// `NotFoundException::ResourceType` for a job id this app never minted.
pub const RESOURCE_TYPE_BACKEND_JOB: &str = "backend-job";

/// The modelled `NotFoundException` payload for a resource that does not exist.
///
/// `resource_type` names the KIND of resource the lookup failed on (the
/// `RESOURCE_TYPE_*` constants above); the message says which one was asked for, built
/// from the request so the caller can see what it addressed.
pub fn not_found(resource_type: &str, message: impl Into<String>) -> NotFoundException {
    NotFoundException {
        message: Some(message.into()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The modelled server-side failure for a request that could not reach this service's
/// store at all. `GatewayTimeoutException` is the only server-side error every
/// AmplifyBackend operation declares, so it is what an unreachable store answers with.
pub fn store_unavailable() -> GatewayTimeoutException {
    GatewayTimeoutException {
        message: Some("the AmplifyBackend store is unavailable".to_string()),
    }
}

/// A job the service has accepted but whose work is not yet due.
pub const JOB_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// A job whose work the clock has made due.
pub const JOB_STATUS_COMPLETED: &str = "COMPLETED";

/// A job that ended with an `Error`.
pub const JOB_STATUS_FAILED: &str = "FAILED";

/// How long a freshly recorded job stays [`JOB_STATUS_IN_PROGRESS`].
///
/// AmplifyBackend mutations are asynchronous — that is why each one answers with a
/// `JobId` and why `GetBackendJob` exists — so a job is never already finished when
/// its own call returns. AWS documents no duration for the work, so this span is
/// CHOSEN: long enough that a caller polling straight after the call still observes
/// the transient status, short enough that a test need not wait long for completion.
pub const JOB_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// How many job records one app retains.
///
/// A job record outlives the environment it ran against (so a lookup after
/// `DeleteBackend` resolves) and nothing in this service deletes one, so the map needs a
/// bound: without it, a caller repeating any mutating call would grow this service's
/// state without limit. AWS publishes no retention count for AmplifyBackend jobs, so the
/// value is CHOSEN — large enough that an ordinary client never loses a job it is still
/// polling, small enough to bound the state one app can hold.
pub const MAX_RETAINED_JOBS_PER_APP: usize = 1000;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller, so each growth site carries a cap and
// nothing caller-sized reaches the store unchecked. AWS publishes no quota for these
// AmplifyBackend collections, so each constant says it is this emulator's resource
// guard and is set well above any realistic use. A request that would grow a store past
// its cap is REFUSED with the modelled `TooManyRequestsException` (429) before the
// insert, never served by growing.
//
// The product is therefore finite: at most MAX_APPS apps, each with at most
// MAX_ENVIRONMENTS_PER_APP environments and MAX_RETAINED_JOBS_PER_APP jobs, each
// environment with at most MAX_AUTH_RESOURCES_PER_ENVIRONMENT auth resources, and every
// identifier that keys one of them at most MAX_IDENTIFIER_CHARS characters.
// ---------------------------------------------------------------------------

/// Emulator resource guard, NOT an AWS quota: the most Amplify apps this emulator holds
/// a backend for. No operation in this slice reclaims an app record, so the map needs a
/// ceiling.
pub const MAX_APPS: usize = 200;

/// Emulator resource guard, NOT an AWS quota: the most backend environments one app may
/// hold at once. `DeleteBackend` reclaims one, so this is a ceiling, not a lifetime
/// total.
pub const MAX_ENVIRONMENTS_PER_APP: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the most auth resources one backend
/// environment may hold at once.
pub const MAX_AUTH_RESOURCES_PER_ENVIRONMENT: usize = 25;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied identifier
/// (`AppId`, `BackendEnvironmentName`, `ResourceName`) this emulator will key a record
/// by, so a bounded COUNT of records cannot hold an unbounded amount of text.
pub const MAX_IDENTIFIER_CHARS: usize = 256;

/// `TooManyRequestsException$LimitType` for the apps bound.
pub const LIMIT_TYPE_APPS: &str = "apps";
/// `TooManyRequestsException$LimitType` for the environments-per-app bound.
pub const LIMIT_TYPE_ENVIRONMENTS: &str = "backend-environments-per-app";
/// `TooManyRequestsException$LimitType` for the auth-resources-per-environment bound.
pub const LIMIT_TYPE_AUTH_RESOURCES: &str = "auth-resources-per-backend-environment";

/// The modelled `TooManyRequestsException` for a request that would grow a store past
/// its cap — "An error that is returned when a limit of a specific type has been
/// exceeded". `limit_type` names WHICH limit, as the modelled member asks; the message is
/// built from the bound so the caller can see what it hit.
pub fn limit_exceeded(limit_type: &str, message: impl Into<String>) -> TooManyRequestsException {
    TooManyRequestsException {
        limit_type: Some(limit_type.to_string()),
        message: Some(message.into()),
    }
}

/// Refuse an identifier longer than [`MAX_IDENTIFIER_CHARS`], naming the member and the
/// bound it broke so the caller can fix the request from the message alone.
///
/// `Err` carries the modelled `BadRequestException` payload: an over-long identifier is a
/// request that is "not formed properly", which is what that error says.
pub fn bound_identifier(member: &str, value: &str) -> Result<(), BadRequestException> {
    let chars = value.chars().count();
    if chars > MAX_IDENTIFIER_CHARS {
        return Err(BadRequestException {
            message: Some(format!(
                "{member} may be at most {MAX_IDENTIFIER_CHARS} characters, got {chars}"
            )),
        });
    }
    Ok(())
}

/// The `Operation` a job records, per the operation that recorded it. The value is the
/// AmplifyBackend operation's own name, which is what `GetBackendJob`/`ListBackendJobs`
/// report.
pub const OPERATION_CREATE_BACKEND: &str = "CreateBackend";
/// See [`OPERATION_CREATE_BACKEND`].
pub const OPERATION_CREATE_BACKEND_AUTH: &str = "CreateBackendAuth";
/// See [`OPERATION_CREATE_BACKEND`].
pub const OPERATION_DELETE_BACKEND: &str = "DeleteBackend";

/// Render an instant the way this service reports `CreateTime` / `UpdateTime`:
/// RFC-3339 in UTC, which is what `restJson1` carries for a timestamp rendered as a
/// string member.
pub fn format_time(at: SystemTime) -> String {
    // `fmt` fails only for an instant outside the representable year range, which no
    // value derived from `SystemTime::now()` is. A clock that far out is reported as
    // the epoch rather than panicking inside a handler.
    DateTime::from(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Per-service state for com-amazonaws-amplifybackend-amplifybackend. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifybackendAmplifybackendState {
    /// Every Amplify app a backend has been created for, keyed by `AppId`.
    pub apps: BTreeMap<String, BackendApp>,
}

impl ComAmazonawsAmplifybackendAmplifybackendState {
    /// The app `app_id` names, or `None` when no backend was ever created for it.
    pub fn app(&self, app_id: &str) -> Option<&BackendApp> {
        self.apps.get(app_id)
    }

    /// As [`Self::app`], for a caller that mutates the app.
    pub fn app_mut(&mut self, app_id: &str) -> Option<&mut BackendApp> {
        self.apps.get_mut(app_id)
    }

    /// Whether a NEW app record may be created for `app_id`: an app that already exists
    /// always may (nothing grows), a new one only while the store is under [`MAX_APPS`].
    /// Call this BEFORE [`Self::app_or_create`].
    pub fn has_room_for_new_app(&self, app_id: &str) -> bool {
        self.apps.contains_key(app_id) || self.apps.len() < MAX_APPS
    }

    /// The app `app_id` names, created with `app_name` when absent. `CreateBackend` is
    /// the only operation that may bring an app record into being — every other one
    /// answers `NotFoundException` for an app that was never created.
    pub fn app_or_create(&mut self, app_id: &str, app_name: &str) -> &mut BackendApp {
        let app = self
            .apps
            .entry(app_id.to_string())
            .or_insert_with(|| BackendApp::new(app_id, app_name));
        // `CreateBackend` carries the app's name as a required member, so a later call
        // for the same app restates it.
        app.app_name = app_name.to_string();
        app
    }
}

/// One Amplify app's backend: its name, the environments created for it, and the jobs
/// recorded against it.
#[derive(Debug, Default)]
pub struct BackendApp {
    /// The app id this record is keyed by.
    pub app_id: String,
    /// The app name, as `CreateBackend` stated it.
    pub app_name: String,
    /// The backend environments that currently exist, keyed by `BackendEnvironmentName`.
    pub environments: BTreeMap<String, BackendEnvironment>,
    /// Every job recorded for this app, keyed by `JobId`. Jobs OUTLIVE the environment
    /// they ran against, so a job lookup after `DeleteBackend` still resolves.
    pub jobs: BTreeMap<String, BackendJob>,
    /// The next job id to hand out, as a decimal epoch-millisecond value. Kept so two
    /// jobs minted in the same millisecond still get distinct, increasing ids.
    pub next_job_seq: u64,
}

impl BackendApp {
    /// A new app record with no environments and no jobs.
    pub fn new(app_id: &str, app_name: &str) -> Self {
        Self {
            app_id: app_id.to_string(),
            app_name: app_name.to_string(),
            environments: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_job_seq: 0,
        }
    }

    /// The environment `name` names, or `None` when it does not exist (never created,
    /// or removed by `DeleteBackend`).
    pub fn environment(&self, name: &str) -> Option<&BackendEnvironment> {
        self.environments.get(name)
    }

    /// As [`Self::environment`], for a caller that mutates the environment.
    pub fn environment_mut(&mut self, name: &str) -> Option<&mut BackendEnvironment> {
        self.environments.get_mut(name)
    }

    /// Whether a NEW environment may be created under `name`: one that already exists
    /// always may (nothing grows), a new one only while the app is under
    /// [`MAX_ENVIRONMENTS_PER_APP`]. Call this BEFORE inserting.
    pub fn has_room_for_new_environment(&self, name: &str) -> bool {
        self.environments.contains_key(name) || self.environments.len() < MAX_ENVIRONMENTS_PER_APP
    }

    /// The environment names this app currently owns, in the order `GetBackend`
    /// reports them.
    pub fn environment_names(&self) -> Vec<String> {
        self.environments.keys().cloned().collect()
    }

    /// Mint a job id: a decimal epoch-millisecond value, bumped past the last one
    /// handed out so ids are unique and increasing even within one millisecond.
    fn mint_job_id(&mut self, now: SystemTime) -> String {
        let millis = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let candidate = u64::try_from(millis).unwrap_or(u64::MAX);
        let id = candidate.max(self.next_job_seq);
        self.next_job_seq = id.saturating_add(1);
        id.to_string()
    }

    /// Apply the job map's bound: reclaim this app's OLDEST job records until there is
    /// room for one more, so the map never holds more than
    /// [`MAX_RETAINED_JOBS_PER_APP`].
    ///
    /// Public and called from each mutating handler's pre-insert guard block, beside the
    /// app and environment count bounds, so every store a request can grow is bounded in
    /// one visible place. [`Self::record_job`] applies it too, so the bound holds however
    /// a job is recorded; a second call is a no-op.
    ///
    /// Reclaiming rather than refusing is deliberate: a job record is a receipt for work
    /// already accepted, so an app that has run many jobs must stay usable. Refusing the
    /// 1001st mutation would wedge the app forever.
    pub fn reclaim_job_capacity(&mut self) {
        self.evict_oldest_jobs(MAX_RETAINED_JOBS_PER_APP.saturating_sub(1));
    }

    /// Record a job for `operation` against `environment_name`, minting and STORING its
    /// id, and hand back the stored record. The job starts
    /// [`JOB_STATUS_IN_PROGRESS`] and becomes [`JOB_STATUS_COMPLETED`] once
    /// [`JOB_COMPLETION_DELAY`] has passed (see [`BackendJob::settle`]).
    ///
    /// The job map is BOUNDED, not growing: recording a job first reclaims the app's
    /// oldest ones through [`Self::reclaim_job_capacity`], so the map holds at most
    /// [`MAX_RETAINED_JOBS_PER_APP`] records however many times this is called.
    pub fn record_job(
        &mut self,
        environment_name: &str,
        operation: &str,
        now: SystemTime,
    ) -> &BackendJob {
        let job_id = self.mint_job_id(now);
        let job = BackendJob {
            job_id: job_id.clone(),
            app_id: self.app_id.clone(),
            backend_environment_name: environment_name.to_string(),
            operation: operation.to_string(),
            status: JOB_STATUS_IN_PROGRESS.to_string(),
            error: None,
            create_time: now,
            update_time: now,
            completes_at: now + JOB_COMPLETION_DELAY,
        };
        // Reclaim BEFORE inserting, leaving room for this one record, so the job being
        // recorded is never itself a candidate for eviction: a wall clock that stepped
        // backwards would otherwise make the newest record the oldest. The insert is the
        // last write, so the record handed back is always present.
        self.reclaim_job_capacity();
        self.jobs.insert(job_id.clone(), job);
        self.jobs
            .get(&job_id)
            .expect("the job inserted on the line above is in the map")
    }

    /// Drop the oldest job records until this app holds at most `keep` of them.
    ///
    /// A job is retained past the environment it ran against so a later lookup still
    /// resolves, but it is retained BOUNDEDLY: no operation in this service reclaims a
    /// job (`DeleteBackend` adds one), so without a cap a caller repeating a mutating
    /// call would grow this map without limit. The oldest record goes first, which keeps
    /// the jobs a caller is still polling.
    fn evict_oldest_jobs(&mut self, keep: usize) {
        while self.jobs.len() > keep {
            // Ordered by creation instant, then by id so two jobs minted in the same
            // millisecond still evict in a defined order (the id increases with age;
            // compared by length first, since a longer decimal id is the larger value).
            // `min_by` over BORROWED keys: only the victim's id is cloned, so an
            // eviction does not allocate once per retained job.
            let oldest = self
                .jobs
                .iter()
                .min_by(|(left_id, left), (right_id, right)| {
                    left.create_time
                        .cmp(&right.create_time)
                        .then_with(|| left_id.len().cmp(&right_id.len()))
                        .then_with(|| left_id.cmp(right_id))
                })
                .map(|(job_id, _)| job_id.clone());
            match oldest {
                Some(job_id) => {
                    self.jobs.remove(&job_id);
                }
                None => break,
            }
        }
    }

    /// The job `job_id` names, when it belongs to `environment_name`. A job is
    /// addressed by (app, environment, job id) on the wire, so a job id that exists
    /// under a DIFFERENT environment is not found.
    pub fn job_mut(&mut self, environment_name: &str, job_id: &str) -> Option<&mut BackendJob> {
        self.jobs
            .get_mut(job_id)
            .filter(|job| job.backend_environment_name == environment_name)
    }
}

/// One backend environment of an app, and the resources created inside it.
#[derive(Debug)]
pub struct BackendEnvironment {
    /// The environment name this record is keyed by.
    pub name: String,
    /// The `ResourceConfig` `CreateBackend` carried, when it carried one.
    pub resource_config: Option<ResourceConfig>,
    /// The `ResourceName` `CreateBackend` carried, when it carried one.
    pub resource_name: Option<String>,
    /// Auth resources, keyed by `ResourceName`; stored verbatim so `GetBackendAuth`
    /// reports exactly what `CreateBackendAuth` was given.
    pub auth: BTreeMap<String, CreateBackendAuthResourceConfig>,
    /// API resources, keyed by `ResourceName`.
    pub api: BTreeMap<String, BackendAPIResourceConfig>,
    /// Storage resources, keyed by `ResourceName`.
    pub storage: BTreeMap<String, CreateBackendStorageResourceConfig>,
    /// When the environment was created.
    pub created_at: SystemTime,
}

impl BackendEnvironment {
    /// Whether a NEW auth resource may be stored under `resource_name`: replacing one
    /// that already exists always may (nothing grows), a new one only while the
    /// environment is under [`MAX_AUTH_RESOURCES_PER_ENVIRONMENT`]. Call this BEFORE
    /// inserting.
    pub fn has_room_for_new_auth_resource(&self, resource_name: &str) -> bool {
        self.auth.contains_key(resource_name)
            || self.auth.len() < MAX_AUTH_RESOURCES_PER_ENVIRONMENT
    }

    /// A new, empty environment created at `created_at`.
    pub fn new(name: &str, created_at: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            resource_config: None,
            resource_name: None,
            auth: BTreeMap::new(),
            api: BTreeMap::new(),
            storage: BTreeMap::new(),
            created_at,
        }
    }
}

/// One async job: what it was, which environment it ran against, and when.
#[derive(Debug)]
pub struct BackendJob {
    /// The id the job was minted with and is addressed by.
    pub job_id: String,
    /// The app the job belongs to.
    pub app_id: String,
    /// The environment the job ran against. Kept on the job because the environment
    /// record itself is removed by `DeleteBackend`.
    pub backend_environment_name: String,
    /// The AmplifyBackend operation that recorded the job.
    pub operation: String,
    /// The job's status: see [`JOB_STATUS_IN_PROGRESS`], [`JOB_STATUS_COMPLETED`] and
    /// [`JOB_STATUS_FAILED`].
    pub status: String,
    /// Why the job failed, absent while it has not.
    pub error: Option<String>,
    /// When the job was recorded.
    pub create_time: SystemTime,
    /// When the job last changed. Never earlier than [`Self::create_time`].
    pub update_time: SystemTime,
    /// The instant the job's work becomes due; until then it reports
    /// [`JOB_STATUS_IN_PROGRESS`].
    pub completes_at: SystemTime,
}

impl BackendJob {
    /// Materialise the completion the clock has ALREADY made due.
    ///
    /// This is the one write a reporting operation performs: the transition was owed at
    /// `completes_at` whether or not anybody read the job, so a reader only records
    /// what the clock decided. Idempotent, and it never moves a failed job.
    /// `update_time` is set to the instant the job became due — not to "now" — so two
    /// reads of a settled job report the same value.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == JOB_STATUS_IN_PROGRESS && now >= self.completes_at {
            self.status = JOB_STATUS_COMPLETED.to_string();
            self.update_time = self.completes_at;
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifybackendAmplifybackendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifybackendAmplifybackendState>)
}
