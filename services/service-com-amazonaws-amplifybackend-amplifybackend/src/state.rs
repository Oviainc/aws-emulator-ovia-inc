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

use std::collections::btree_map::Entry as BTreeEntry;
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
/// `NotFoundException::ResourceType` for a challenge token (app id + session id) this
/// service does not hold: one that was never created, already deleted, or expired.
pub const RESOURCE_TYPE_CHALLENGE_TOKEN: &str = "challenge-token";

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
// MAX_ENVIRONMENTS_PER_APP environments, MAX_RETAINED_JOBS_PER_APP jobs and
// MAX_CHALLENGE_TOKENS_PER_APP live challenge tokens, each environment with at most
// MAX_AUTH_RESOURCES_PER_ENVIRONMENT auth resources,
// MAX_API_RESOURCES_PER_ENVIRONMENT API resources and
// MAX_STORAGE_RESOURCES_PER_ENVIRONMENT storage resources, and every identifier that
// keys one of them at most MAX_IDENTIFIER_CHARS characters.
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

/// Emulator resource guard, NOT an AWS quota: the most API resources one backend
/// environment may hold at once. The `ResourceName` a caller supplies keys the map, so
/// the COUNT needs a ceiling as well as the key length.
pub const MAX_API_RESOURCES_PER_ENVIRONMENT: usize = 25;

/// Emulator resource guard, NOT an AWS quota: the most storage resources one backend
/// environment may hold at once. See [`MAX_API_RESOURCES_PER_ENVIRONMENT`].
pub const MAX_STORAGE_RESOURCES_PER_ENVIRONMENT: usize = 25;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied identifier
/// (`AppId`, `BackendEnvironmentName`, `ResourceName`) this emulator will key a record
/// by, so a bounded COUNT of records cannot hold an unbounded amount of text.
pub const MAX_IDENTIFIER_CHARS: usize = 256;

/// Emulator resource guard, NOT an AWS quota: the most LIVE challenge tokens one app may
/// hold at once. A token is reclaimed by `DeleteToken` or by its own expiry, so this is a
/// ceiling on what is live, not a lifetime total.
pub const MAX_CHALLENGE_TOKENS_PER_APP: usize = 100;

/// `TooManyRequestsException$LimitType` for the apps bound.
pub const LIMIT_TYPE_APPS: &str = "apps";
/// `TooManyRequestsException$LimitType` for the environments-per-app bound.
pub const LIMIT_TYPE_ENVIRONMENTS: &str = "backend-environments-per-app";
/// `TooManyRequestsException$LimitType` for the auth-resources-per-environment bound.
pub const LIMIT_TYPE_AUTH_RESOURCES: &str = "auth-resources-per-backend-environment";
/// `TooManyRequestsException$LimitType` for the api-resources-per-environment bound.
pub const LIMIT_TYPE_API_RESOURCES: &str = "api-resources-per-backend-environment";
/// `TooManyRequestsException$LimitType` for the storage-resources-per-environment bound.
pub const LIMIT_TYPE_STORAGE_RESOURCES: &str = "storage-resources-per-backend-environment";
/// `TooManyRequestsException$LimitType` for the live-challenge-tokens-per-app bound.
pub const LIMIT_TYPE_CHALLENGE_TOKENS: &str = "challenge-tokens-per-app";

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

// ---------------------------------------------------------------------------
// Admin UI challenge tokens
//
// `CreateToken` mints a one-time challenge code for signing in to an app's Amplify
// Admin UI, `GetToken` reports the minted token back, and `DeleteToken` reclaims it.
// A token is NOT part of a backend environment — it addresses an app directly
// (`/backend/{AppId}/challenge`) and AmplifyBackend mints one whether or not a backend
// was ever created for that app — so the tokens live in their own map rather than on
// [`BackendApp`], which only exists for an app `CreateBackend` was called for.
// ---------------------------------------------------------------------------

/// How long a challenge code minted by `CreateToken` stays usable, and the value
/// reported as `Ttl`.
///
/// CHOSEN, not documented: AWS publishes no lifetime for an Amplify Admin UI challenge
/// code. The span is short, the way a one-time sign-in code's is, and long enough that a
/// client that mints a code, opens the Admin UI and polls `GetToken` for it still finds
/// it.
pub const CHALLENGE_TOKEN_TTL: Duration = Duration::from_secs(300);

/// How many characters of store-minted secret material a challenge code carries.
///
/// CHOSEN: AWS publishes no length for the code. 20 characters of the credential store's
/// 64-symbol alphabet is ~120 bits, so a code cannot be guessed inside its lifetime.
pub const CHALLENGE_CODE_CHARS: usize = 20;

/// One live Admin UI challenge token: the code itself and when it stops being usable.
#[derive(Debug, Clone)]
pub struct ChallengeToken {
    /// The app the token authenticates into.
    pub app_id: String,
    /// The unique id `CreateToken` handed back, which addresses the token afterwards.
    pub session_id: String,
    /// The one-time challenge code. Minted by the emulator's credential store (see
    /// [`mint_challenge_token`]), never constructed here.
    pub challenge_code: String,
    /// When the token was minted.
    pub created_at: SystemTime,
    /// When the code stops being usable: [`CHALLENGE_TOKEN_TTL`] after
    /// [`Self::created_at`].
    pub expires_at: SystemTime,
}

impl ChallengeToken {
    /// A token minted at `now`, expiring [`CHALLENGE_TOKEN_TTL`] later.
    pub fn new(
        app_id: &str,
        session_id: String,
        challenge_code: String,
        now: SystemTime,
    ) -> Self {
        Self {
            app_id: app_id.to_string(),
            session_id,
            challenge_code,
            created_at: now,
            expires_at: now + CHALLENGE_TOKEN_TTL,
        }
    }

    /// Whether the code is still usable at `now`.
    pub fn is_live(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }

    /// The `Ttl` member: this code's LIFETIME — the whole seconds between the instant it
    /// was minted and the instant it expires — as a decimal string.
    ///
    /// A lifetime, not a countdown. The model does NOT fix this: it documents the member
    /// only as "The expiry time for the one-time generated token code" and gives it no
    /// format, no pattern and no units, so the member's NAME — `Ttl`, a time to live — is
    /// the ground for reporting a span in seconds, and this emulator's choice is recorded
    /// here rather than claimed from the model. The value is read from the token's own two
    /// instants, so every report of one token carries the same value however long after
    /// the mint it is read. What this service enforces is the EXPIRY INSTANT
    /// ([`Self::expires_at`], applied by
    /// [`ComAmazonawsAmplifybackendAmplifybackendState::expire_challenge_tokens`]) —
    /// `created_at + ttl` — so a caller reading a token late is told how long the code
    /// lives in total, not how much of that is left.
    pub fn ttl(&self) -> String {
        self.expires_at
            .duration_since(self.created_at)
            .unwrap_or(CHALLENGE_TOKEN_TTL)
            .as_secs()
            .to_string()
    }
}

/// A fresh challenge code, and the store-minted random text a `SessionId` is built from.
///
/// The challenge code is a secret handed to a caller — it signs the holder in to the
/// Amplify Admin UI — and nothing in a service may construct secret material itself, so
/// it is minted by the one component that generates secrets, the emulator's credential
/// store. It authenticates to the Admin UI and NOT to this emulator, so the credential is
/// removed from the store again straight away: the caller gets the code, the emulator
/// keeps no credential it would accept as SigV4, and nothing accumulates per call.
///
/// The second half is the credential's own ID — the store-generated, NON-secret part of a
/// minted credential. It is what
/// [`ComAmazonawsAmplifybackendAmplifybackendState::mint_session_id`] builds the session
/// id from, because a session id must not be guessable: `GetToken` resolves a token by
/// `(AppId, SessionId)`, so a predictable session id would be a predictable address for
/// somebody else's challenge code.
///
/// `None` when the store minted no session-token secret, which leaves `CreateToken` to
/// refuse rather than report a code it made up.
pub fn mint_challenge_token(
    store: &::emulator_server::CredentialStore,
) -> Option<(String, String)> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let challenge_code: Option<String> = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(|secret| secret.chars().take(CHALLENGE_CODE_CHARS).collect());
    // Reclaimed on EVERY path, before the `?` below: the code is not a credential of this
    // emulator (see above), so a mint that yielded no secret must not leave one behind
    // either.
    let _ = store.delete(&minted.id);
    Some((minted.id, challenge_code?))
}

/// Per-service state for com-amazonaws-amplifybackend-amplifybackend. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifybackendAmplifybackendState {
    /// Every Amplify app a backend has been created for, keyed by `AppId`.
    pub apps: BTreeMap<String, BackendApp>,
    /// The LIVE Admin UI challenge tokens, keyed by `AppId` and then by `SessionId`.
    /// Separate from [`Self::apps`] because a challenge token addresses an app, not a
    /// backend environment, and is minted whether or not the app has a backend.
    pub challenge_tokens: BTreeMap<String, BTreeMap<String, ChallengeToken>>,
    /// The next sequence number [`Self::mint_session_id`] hands out, so two session ids
    /// are distinct even if the credential store repeats the random text they carry.
    pub next_challenge_session_seq: u64,
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

    /// Mint a `SessionId` for a challenge token: the store-minted random text from
    /// [`mint_challenge_token`], lower-cased, plus this store's own sequence number.
    ///
    /// Both halves are needed. The random half is what makes a session id UNGUESSABLE,
    /// which matters because `GetToken` resolves a challenge code by `(AppId, SessionId)`.
    /// The sequence half makes uniqueness EXACT rather than probabilistic: the credential
    /// store only avoids ids it currently holds, and `mint_challenge_token` releases the
    /// credential as soon as it has the code, so the random text alone could repeat.
    pub fn mint_session_id(&mut self, minted_credential_id: &str) -> String {
        let sequence = self.next_challenge_session_seq;
        self.next_challenge_session_seq = sequence.saturating_add(1);
        format!(
            "{}-{sequence:016x}",
            minted_credential_id.to_ascii_lowercase()
        )
    }

    /// Materialise the expiry the clock has ALREADY made due: forget every challenge
    /// token whose [`CHALLENGE_TOKEN_TTL`] has run out, and the app entries that leaves
    /// empty.
    ///
    /// Owed at each token's `expires_at` whether or not anybody called, so every handler
    /// of a token operation runs this first and then reads only live tokens. Idempotent.
    pub fn expire_challenge_tokens(&mut self, now: SystemTime) {
        for sessions in self.challenge_tokens.values_mut() {
            sessions.retain(|_, token| token.is_live(now));
        }
        self.challenge_tokens.retain(|_, sessions| !sessions.is_empty());
    }

    /// The live challenge token `(app_id, session_id)` addresses, or `None` when this
    /// service holds none. Call [`Self::expire_challenge_tokens`] first so an expired
    /// token is not reported.
    pub fn challenge_token(&self, app_id: &str, session_id: &str) -> Option<&ChallengeToken> {
        self.challenge_tokens
            .get(app_id)
            .and_then(|sessions| sessions.get(session_id))
    }

    /// Whether a token may be minted for an app the token map does not hold yet: one it
    /// already holds always may (nothing grows), a new one only while the map is under
    /// [`MAX_APPS`]. Call this BEFORE [`Self::store_challenge_token`].
    pub fn has_room_for_new_challenge_token_app(&self, app_id: &str) -> bool {
        self.challenge_tokens.contains_key(app_id) || self.challenge_tokens.len() < MAX_APPS
    }

    /// Whether one more LIVE token may be minted for `app_id`, i.e. whether the app is
    /// under [`MAX_CHALLENGE_TOKENS_PER_APP`]. Call this BEFORE
    /// [`Self::store_challenge_token`], on a store whose expired tokens are already
    /// forgotten, so a caller is never refused over tokens the clock has retired.
    pub fn has_room_for_new_challenge_token(&self, app_id: &str) -> bool {
        self.challenge_tokens
            .get(app_id)
            .is_none_or(|sessions| sessions.len() < MAX_CHALLENGE_TOKENS_PER_APP)
    }

    /// Store `token` and hand the stored record back, so the response reports exactly
    /// what a later `GetToken` will.
    pub fn store_challenge_token(&mut self, token: ChallengeToken) -> &ChallengeToken {
        let sessions = self.challenge_tokens.entry(token.app_id.clone()).or_default();
        match sessions.entry(token.session_id.clone()) {
            BTreeEntry::Vacant(slot) => slot.insert(token),
            // A session id is a fresh UUID per mint, so this arm is not reached by
            // `CreateToken`; it keeps the insert total rather than leaving a panic path.
            BTreeEntry::Occupied(mut slot) => {
                slot.insert(token);
                slot.into_mut()
            }
        }
    }

    /// Forget the token `(app_id, session_id)` addresses, answering whether one was
    /// held. `DeleteToken` is what reclaims a token before its expiry.
    pub fn remove_challenge_token(&mut self, app_id: &str, session_id: &str) -> bool {
        let Some(sessions) = self.challenge_tokens.get_mut(app_id) else {
            return false;
        };
        let removed = sessions.remove(session_id).is_some();
        if sessions.is_empty() {
            self.challenge_tokens.remove(app_id);
        }
        removed
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
    /// API resources, keyed by `ResourceName`. Bounded by
    /// [`MAX_API_RESOURCES_PER_ENVIRONMENT`]: an inserting handler asks
    /// [`Self::has_room_for_new_api_resource`] first.
    pub api: BTreeMap<String, BackendAPIResourceConfig>,
    /// Storage resources, keyed by `ResourceName`. Bounded by
    /// [`MAX_STORAGE_RESOURCES_PER_ENVIRONMENT`]: an inserting handler asks
    /// [`Self::has_room_for_new_storage_resource`] first.
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

    /// Whether a NEW API resource may be stored under `resource_name`; as
    /// [`Self::has_room_for_new_auth_resource`], for [`Self::api`] and
    /// [`MAX_API_RESOURCES_PER_ENVIRONMENT`]. Call this BEFORE inserting, and refuse an
    /// over-cap insert with the modelled `TooManyRequestsException`
    /// ([`LIMIT_TYPE_API_RESOURCES`]).
    pub fn has_room_for_new_api_resource(&self, resource_name: &str) -> bool {
        self.api.contains_key(resource_name) || self.api.len() < MAX_API_RESOURCES_PER_ENVIRONMENT
    }

    /// Whether a NEW storage resource may be stored under `resource_name`; as
    /// [`Self::has_room_for_new_auth_resource`], for [`Self::storage`] and
    /// [`MAX_STORAGE_RESOURCES_PER_ENVIRONMENT`]. Call this BEFORE inserting, and refuse
    /// an over-cap insert with the modelled `TooManyRequestsException`
    /// ([`LIMIT_TYPE_STORAGE_RESOURCES`]).
    pub fn has_room_for_new_storage_resource(&self, resource_name: &str) -> bool {
        self.storage.contains_key(resource_name)
            || self.storage.len() < MAX_STORAGE_RESOURCES_PER_ENVIRONMENT
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
