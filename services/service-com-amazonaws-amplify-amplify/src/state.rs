// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplify-amplify service.
//!
//! The persisted record for a resource IS the modelled structure this service hands
//! back for it: [`App`] for an app, [`Branch`] for a branch, [`Webhook`] for a
//! webhook, [`Job`] for a job. Keeping one copy of each resource — rather than a
//! parallel struct with the same ~30 members plus a conversion — is what makes
//! `GetApp` after `CreateApp` return the created record field-for-field: a reader
//! clones what the writer stored, so the two cannot drift.
//!
//! Branches and webhooks live INSIDE their owning app, so `DeleteApp` removes them
//! transitively and `ListBranches` / `ListWebhooks` can never reach another app's
//! records. Jobs live INSIDE their owning branch for the same reason: `DeleteBranch`
//! takes that branch's jobs with it, and `GetJob` can never reach a job of another
//! branch.

use std::collections::BTreeMap;

use crate::ops::types::{App, Branch, Job, Step, Webhook};

/// One Amplify app as this service persists it: the modelled record it hands back,
/// plus the branches and webhooks created on it.
///
/// The maps are `BTreeMap`s, not `HashMap`s, because the list operations are
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `nextToken` a page returns is the key of the first record it did NOT return, so
/// the next call resumes at that key.
#[derive(Debug, Clone)]
pub struct AmplifyApp {
    /// The app record, exactly as every reader returns it.
    pub app: App,
    /// The branches of this app, keyed by `branchName`.
    pub branches: BTreeMap<String, AmplifyBranch>,
    /// The webhooks of this app, keyed by `webhookId`.
    ///
    /// A webhook is created under an app (`POST /apps/{appId}/webhooks`) and is
    /// addressed afterwards by its own id alone (`GET /webhooks/{webhookId}`), which
    /// [`ComAmazonawsAmplifyAmplifyState::webhook`] resolves across the apps.
    pub webhooks: BTreeMap<String, Webhook>,
}

impl AmplifyApp {
    /// A newly created app with no branches and no webhooks.
    pub fn new(app: App) -> Self {
        Self {
            app,
            branches: BTreeMap::new(),
            webhooks: BTreeMap::new(),
        }
    }
}

/// One branch as this service persists it: the modelled record every reader returns,
/// plus the jobs started on it, keyed by the `jobId` this branch minted for each.
#[derive(Debug, Clone)]
pub struct AmplifyBranch {
    /// The branch record, exactly as every reader returns it.
    pub branch: Branch,
    /// The jobs started on this branch, keyed by `jobId`.
    ///
    /// BOUNDED, with an explicit retention policy: the map holds at most
    /// [`MAX_JOBS_PER_BRANCH`] jobs, and starting one more RETIRES the oldest job
    /// that has reached a terminal status (see
    /// [`AmplifyBranch::make_room_for_a_job`]). A branch's job history is therefore
    /// finite however many jobs are started on it, and a job that was retired is
    /// `NotFoundException` to every reader. `Branch$totalNumberOfJobs` counts the
    /// jobs the branch has STARTED ([`AmplifyBranch::jobs_started`]), not the
    /// retained ones, so retiring history does not rewrite that count.
    pub jobs: BTreeMap<String, BranchJob>,
    /// Monotonic counter feeding [`AmplifyBranch::mint_job_id`], and so the number of
    /// jobs this branch has started.
    pub next_job_seq: u64,
}

impl AmplifyBranch {
    /// A newly created branch, which has never been built and so holds no jobs.
    pub fn new(branch: Branch) -> Self {
        Self {
            branch,
            jobs: BTreeMap::new(),
            next_job_seq: 0,
        }
    }

    /// Mint a `jobId` no job of THIS branch is using.
    ///
    /// `JobId` is modelled `length 0..255, pattern ^[0-9]+$`
    /// (`smithy_query(query="shape",shape="JobSummary")`), so a job id is decimal
    /// digits — not an opaque token — and a job is addressed within its branch
    /// (`/apps/{appId}/branches/{branchName}/jobs/{jobId}`). The ids are therefore a
    /// per-branch count: the first job a branch runs is `1`, the next `2`, which is
    /// also what `Branch$activeJobId` and `Branch$totalNumberOfJobs` report about.
    pub fn mint_job_id(&mut self) -> String {
        loop {
            self.next_job_seq = self.next_job_seq.wrapping_add(1);
            let candidate = self.next_job_seq.to_string();
            if !self.jobs.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// How many jobs this branch has STARTED, which is what
    /// `Branch$totalNumberOfJobs` reports.
    ///
    /// Not the number of jobs the branch still holds: the history is bounded (see
    /// [`AmplifyBranch::jobs`]), and a branch that has run a thousand jobs has run a
    /// thousand jobs whether or not the oldest are still readable.
    pub fn jobs_started(&self) -> u64 {
        self.next_job_seq
    }

    /// Retire old jobs until this branch can hold one more, and report whether it can.
    ///
    /// The retention policy of [`AmplifyBranch::jobs`], applied: the map never exceeds
    /// [`MAX_JOBS_PER_BRANCH`] entries, and the entry that makes room is the oldest
    /// (lowest-numbered, i.e. first-started) job that has reached a TERMINAL status — a
    /// job that is still running is never dropped out from under the caller watching
    /// it. The clock-owed settling is materialised first, because a job the clock has
    /// already finished is retirable even if nobody has read it yet.
    ///
    /// `false` means the branch is at its cap and every job it holds is still running,
    /// so there is nothing to retire: the caller refuses the start with the modelled
    /// `LimitExceededException` rather than growing the map.
    pub fn make_room_for_a_job(&mut self) -> bool {
        if self.jobs.len() < MAX_JOBS_PER_BRANCH {
            return true;
        }
        for stored in self.jobs.values_mut() {
            stored.settle();
        }
        while self.jobs.len() >= MAX_JOBS_PER_BRANCH {
            // A minted id is decimal, so numeric order IS the order the jobs started;
            // an id that does not parse sorts first and is retired first.
            let Some(oldest) = self
                .jobs
                .iter()
                .filter(|(_, stored)| stored.settles_at.is_none())
                .min_by_key(|(job_id, _)| job_id.parse::<u128>().unwrap_or(0))
                .map(|(job_id, _)| job_id.clone())
            else {
                return false;
            };
            self.jobs.remove(&oldest);
        }
        true
    }
}

/// How many jobs one branch's history holds at once.
///
/// CHOSEN, not documented: AWS publishes no per-branch job-history quota, so this is
/// the emulator's own retention bound — a branch's job map cannot grow without limit
/// however many jobs are started on it. It is the page maximum `ListJobs` models
/// (`maxResults` `range 0..50`, `smithy_query(query="operation",operation="ListJobs")`),
/// so a branch's whole retained history is reportable in one page. Reaching it retires
/// the oldest finished job rather than refusing the caller; see
/// [`AmplifyBranch::make_room_for_a_job`].
pub const MAX_JOBS_PER_BRANCH: usize = 50;

/// One job as this service persists it: the modelled [`Job`] record, plus the instant
/// at which the clock owes that job its next status change.
///
/// Starting a job is ASYNCHRONOUS — `StartJob` answers with a non-terminal status and
/// the job settles later — so the record carries when that settling is due instead of
/// blocking the call that started it. [`BranchJob::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct BranchJob {
    /// The job record, exactly as every reader returns it.
    pub job: Job,
    /// Epoch milliseconds at which this job's non-terminal status is due to settle,
    /// or `None` once it has settled and the status is terminal.
    pub settles_at: Option<i64>,
}

impl BranchJob {
    /// A job that has just been started: [`JOB_STATUS_PENDING`] now, settled by the
    /// clock [`JOB_SETTLE_MILLIS`] from now.
    pub fn started(job: Job) -> Self {
        Self {
            job,
            settles_at: Some(now_millis().saturating_add(JOB_SETTLE_MILLIS)),
        }
    }

    /// Materialise the status change the clock has ALREADY made due, and nothing else.
    ///
    /// The change is owed at `settles_at` whether or not anybody calls, so a reporting
    /// operation applies it before answering rather than reporting a `PENDING` job that
    /// the clock settled seconds ago. Before that instant this is a no-op, so a caller
    /// that reads straight after `StartJob` still observes the non-terminal status.
    ///
    /// The status it settles to is [`JOB_STATUS_FAILED`]: a job builds a branch from
    /// its repository, this emulator has no build environment to run that in, and a
    /// build nothing ran cannot be reported as having succeeded. The reason is carried
    /// on the one modelled member that can hold it — `Step$statusReason` — because
    /// `JobSummary` models no reason of its own.
    pub fn settle(&mut self) {
        let Some(due) = self.settles_at else {
            return;
        };
        if now_millis() < due {
            return;
        }
        self.settles_at = None;
        let ended = ::aws_smithy_types::DateTime::from_millis(due);
        self.job.summary.status = JOB_STATUS_FAILED.to_string();
        self.job.summary.end_time = Some(ended);
        self.job.steps = vec![Step {
            step_name: BUILD_STEP_NAME.to_string(),
            start_time: self.job.summary.start_time,
            status: JOB_STATUS_FAILED.to_string(),
            end_time: ended,
            status_reason: Some(NO_BUILD_ENVIRONMENT_REASON.to_string()),
            // Every URL below addresses an artifact a build produced, and no build
            // ran; `screenshots` and `context` likewise only exist for a step that
            // executed.
            log_url: None,
            artifacts_url: None,
            test_artifacts_url: None,
            test_config_url: None,
            screenshots: None,
            context: None,
        }];
    }
}

/// How long after `StartJob` the clock settles a job out of its non-terminal status.
///
/// CHOSEN, not documented: a real Amplify build takes minutes, which no caller of an
/// emulator can wait for. The span is non-zero so a caller that reads the job straight
/// after starting it still sees the `PENDING` status a real client has to poll through.
pub const JOB_SETTLE_MILLIS: i64 = 500;

/// The status a job carries from `StartJob` until the clock settles it.
///
/// `JobStatus` is modelled `CREATED | PENDING | PROVISIONING | RUNNING | FAILED |
/// SUCCEED | CANCELLING | CANCELLED` (`smithy_query(query="shape",shape="JobStatus")`)
/// and `StartJob` "starts a new job": the job exists and has not run yet, which is
/// what `PENDING` says.
pub const JOB_STATUS_PENDING: &str = "PENDING";

/// The terminal status a job settles to; see [`BranchJob::settle`].
pub const JOB_STATUS_FAILED: &str = "FAILED";

/// The name of the execution step a settled job carries its failure reason on.
///
/// `Step$stepName` is modelled as free text (`smithy_query(query="shape",shape="Step")`)
/// and the phase a job fails in here is the build, which Amplify names `BUILD`.
pub const BUILD_STEP_NAME: &str = "BUILD";

/// Why a job this emulator started ends in [`JOB_STATUS_FAILED`].
pub const NO_BUILD_ENVIRONMENT_REASON: &str =
    "This emulator has no build environment, so the job's build was never executed.";

/// Per-service state for com-amazonaws-amplify-amplify. A handler reaches it through [`service_state`].
///
/// The apps map is NOT partitioned by region: an app's ARN records the region of the
/// request that created it, but any later call reaches every app in the store
/// whatever region it is signed for. Partitioning the control plane by region is a
/// separate change; nothing here claims it.
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifyAmplifyState {
    /// Every app that exists, keyed by its minted `appId`.
    pub apps: BTreeMap<String, AmplifyApp>,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_app_id`].
    pub next_app_id_seq: u64,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_webhook_id`].
    pub next_webhook_id_seq: u64,
}

impl ComAmazonawsAmplifyAmplifyState {
    /// Mint an `appId` no app in state is using.
    ///
    /// `AppId` is modelled `length 1..20, pattern ^d[a-z0-9]+$`
    /// (`smithy_query(query="shape",shape="App")`), so the id is a literal `d`
    /// followed by 13 base-36 digits — 14 characters, inside the bound, and inside
    /// the pattern's alphabet.
    ///
    /// The 13 digits come from a monotonic counter mixed with the wall clock through
    /// the SplitMix64 finaliser, so ids do not expose a guessable sequence, and the
    /// loop re-mints on the (astronomically unlikely) collision, so two `CreateApp`
    /// calls can never be handed the same id.
    pub fn mint_app_id(&mut self) -> String {
        loop {
            self.next_app_id_seq = self.next_app_id_seq.wrapping_add(1);
            let candidate = format!("d{}", base36_digits(mixed(self.next_app_id_seq)));
            if !self.apps.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `webhookId` no webhook in state is using, and the opaque token that
    /// authorises the webhook's own URL.
    ///
    /// `WebhookId` is modelled `length 0..255, pattern ^(?s)`
    /// (`smithy_query(query="shape",shape="Webhook")`), so the format is the
    /// service's rather than the model's: Amplify hands back a UUID, which is what
    /// [`uuid_v4`] renders — 36 characters, inside the modelled bound.
    ///
    /// The token is minted in the same call because it is part of the same secret:
    /// knowing a webhook's id is not enough to trigger it, so the URL carries a
    /// second unguessable value. Both come from the same mixer that mints an app id,
    /// so neither exposes the counter behind it, and the loop re-mints on a
    /// collision, so two creates can never be handed the same id.
    pub fn mint_webhook_id(&mut self) -> (String, String) {
        loop {
            self.next_webhook_id_seq = self.next_webhook_id_seq.wrapping_add(1);
            let (high, low) = mixed_pair(self.next_webhook_id_seq);
            let candidate = uuid_v4(high, low);
            if self.webhook(&candidate).is_none() {
                let (token_high, token_low) = mixed_pair(low ^ high);
                return (candidate, format!("{token_high:016x}{token_low:016x}"));
            }
        }
    }

    /// The webhook `webhook_id` names, whichever app holds it.
    ///
    /// `GetWebhook`, `UpdateWebhook` and `DeleteWebhook` address a webhook by its id
    /// alone, and an id is unique across the store (see [`Self::mint_webhook_id`]), so
    /// the lookup is a scan of the apps for the one holding that id.
    pub fn webhook(&self, webhook_id: &str) -> Option<&Webhook> {
        self.apps
            .values()
            .find_map(|app| app.webhooks.get(webhook_id))
    }
}

/// A counter value mixed with the wall clock through the SplitMix64 finaliser: the
/// entropy every minted identifier in this service is built from, so an id does not
/// expose a guessable sequence.
fn mixed(seq: u64) -> u64 {
    let clock = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    splitmix64(seq ^ clock.rotate_left(17))
}

/// 128 bits of [`mixed`] entropy, for an identifier wider than a `u64`.
fn mixed_pair(seq: u64) -> (u64, u64) {
    let high = mixed(seq);
    let low = splitmix64(high.rotate_left(32) ^ seq);
    (high, low)
}

/// `high`/`low` rendered as a version-4 UUID: 32 hexadecimal digits in the 8-4-4-4-12
/// grouping, with RFC 4122's version nibble (`4`) and variant bits (`10`) set.
fn uuid_v4(high: u64, low: u64) -> String {
    let high = (high & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let low = (low & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xFFFF,
        high & 0xFFFF,
        low >> 48,
        low & 0xFFFF_FFFF_FFFF,
    )
}

/// The SplitMix64 finaliser: an invertible bit mixer, so distinct inputs give
/// distinct outputs and a counter does not show through the minted id.
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly 13 base-36 digits over `[0-9a-z]`, zero-padded on the left.
///
/// 36^13 > `u64::MAX`, so 13 digits represent every `u64` without truncation.
fn base36_digits(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = [b'0'; 13];
    for slot in out.iter_mut().rev() {
        let digit = usize::try_from(value % 36).unwrap_or(0);
        *slot = DIGITS[digit];
        value /= 36;
    }
    out.iter().map(|byte| char::from(*byte)).collect()
}

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own shared
/// AWS config names, so an unsigned call and a default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the one
/// in its SigV4 credential scope, so that is what an ARN minted while serving it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Now, as epoch milliseconds.
///
/// The one clock this service reads, so a stored timestamp and a stored due instant
/// (see [`BranchJob::settles_at`]) are measured against the same thing.
pub fn now_millis() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Now, truncated to milliseconds.
///
/// restJson1 writes a timestamp as epoch seconds with a fractional part, i.e. as a
/// JSON number: an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond, so a nanosecond-precision instant would not survive the round trip
/// and `createTime` would differ between the create response and a later read.
/// Milliseconds round-trip exactly, which keeps a stored timestamp stable across reads.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(now_millis())
}

/// The page size to serve for a `@paginated` operation whose `maxResults` member is
/// `max_results` and whose modelled `range` maximum is `modelled_max`.
///
/// `maxResults` is modelled with `default: 0`, so an absent member arrives as `0`:
/// that is "the caller set no bound", which the operation serves with its modelled
/// maximum rather than with an empty page. A caller-supplied bound is already inside
/// the modelled range by the time a handler runs (the boundary validator refuses
/// anything else); clamping again costs nothing and keeps this total.
pub fn page_size(max_results: i32, modelled_max: usize) -> usize {
    match usize::try_from(max_results) {
        Ok(0) | Err(_) => modelled_max,
        Ok(bound) => bound.min(modelled_max),
    }
}

/// One page of an ordered `(key, record)` sequence, plus the `nextToken` to continue
/// from — `None` when the page reached the end.
///
/// The token is the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts
/// at the first key at or after the token. Resuming is therefore well defined even
/// when the record the token names was deleted between the two calls: the scan simply
/// continues at the next key.
pub fn page<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if next_token.is_some_and(|token| key.as_str() < token) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(key));
        }
        records.push(record);
    }
    (records, None)
}

/// Replace a stored @required member when the request supplied one.
///
/// A partial update is "every member the request carries, and nothing else": an
/// absent member is not a value, so it leaves the stored one as it was. Shared by
/// `UpdateApp` and `UpdateBranch` so the two cannot drift on that rule.
pub fn apply<T>(stored: &mut T, supplied: Option<T>) {
    if let Some(value) = supplied {
        *stored = value;
    }
}

/// Replace a stored OPTIONAL member when the request supplied one.
///
/// The same rule as [`apply`], for a member that may legitimately be unset: an
/// omitted member leaves the stored value alone rather than clearing it. Clearing a
/// member is therefore NOT expressible by omission — a caller that wants a member
/// emptied sends its empty value, which [`apply`] and this function store as given.
pub fn apply_optional<T>(stored: &mut Option<T>, supplied: Option<T>) {
    if supplied.is_some() {
        *stored = supplied;
    }
}

/// The `message` an Amplify `NotFoundException` carries for a resource the request
/// addressed and this store does not hold.
///
/// The text is built FROM THE REQUEST and is a complete sentence: the kind of entity,
/// the identifier the request used for it, and `not found` closed by the
/// sentence-terminating period. Every 404 in this crate is formatted through this one
/// function, so `GetApp`, `DeleteApp`, `GetBranch`, `DeleteBranch`, `CreateBranch`,
/// `ListBranches`, `UpdateApp`, `UpdateBranch`, the webhook operations, `StartJob` and
/// `GetJob` cannot drift from one another — and
/// an identifier is NEVER inspected here, so the message is the same construction
/// whatever the request named.
pub fn not_found_message(entity: &str, identifier: &str) -> String {
    format!("{entity} {identifier} not found.")
}

/// The `message` an Amplify `BadRequestException` carries when a create is refused
/// because the owning app ALREADY HOLDS a resource of that kind under the name the
/// request asked for.
///
/// Like [`not_found_message`], the text is built FROM THE REQUEST and from nothing
/// else: Amplify names the kind of resource it failed to create, then the name the
/// request used for it, then the app it already exists under —
/// `Failed to create <entity>. The <entity> <name> already exists for the app <appId>`,
/// with the entity in lower case, as recorded behaviour shows (the model does not
/// spell the message out). Every duplicate-name refusal in this crate is formatted
/// through this one function, so the creates cannot drift from one another, and an
/// identifier is NEVER inspected here: the message is the same construction whatever
/// the request named.
pub fn already_exists_message(entity: &str, identifier: &str, app_id: &str) -> String {
    let entity = entity.to_lowercase();
    format!(
        "Failed to create {entity}. The {entity} {identifier} already exists for the app {app_id}"
    )
}

/// The entity name [`not_found_message`] uses for an app addressed by `appId`.
pub const APP_ENTITY: &str = "App";

/// The entity name [`not_found_message`] and [`already_exists_message`] use for a
/// branch addressed by `branchName`.
pub const BRANCH_ENTITY: &str = "Branch";

/// The entity name [`not_found_message`] uses for a job addressed by `jobId`.
pub const JOB_ENTITY: &str = "Job";

/// The entity name [`not_found_message`] uses for a webhook addressed by `webhookId`.
pub const WEBHOOK_ENTITY: &str = "Webhook";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifyAmplifyState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifyAmplifyState>)
}
