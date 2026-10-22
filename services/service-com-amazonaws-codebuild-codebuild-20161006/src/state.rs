// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codebuild-codebuild-20161006 service.
//!
//! Every implemented CodeBuild operation computes its answer from the records
//! here: `CreateProject` writes a [`Project`], `StartBuild` writes a
//! [`StoredBuild`], and the reporting operations (`BatchGetProjects`,
//! `BatchGetBuilds`, `ListProjects`, `ListBuildsForProject`) only read them.
//!
//! The model shapes themselves are what is stored, so a read hands back exactly
//! the structure the write persisted rather than a second, drifting copy of the
//! same resource.

use std::collections::BTreeMap;

use crate::ops::types::{Build, BuildPhase, PhaseContext, Project};

/// The ARN service segment for this service, as `emulator_arn`'s first argument
/// (`arn:aws:codebuild:<region>:<account>:<resource>`). Taken from the model's
/// `aws.api#service` `arnNamespace`.
pub const ARN_NAMESPACE: &str = "codebuild";

/// Region used for ARNs when the request carries no SigV4 credential scope to
/// read one from (an unsigned, host-routed request). Every emulated service
/// defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Build timeout, in minutes, applied when `CreateProject` omits
/// `timeoutInMinutes`. AWS documents the default on that member: "How long, in
/// minutes, from 5 to 2160 (36 hours), for CodeBuild to wait before it times out
/// any build that has not been marked as completed. The default is 60 minutes."
pub const DEFAULT_BUILD_TIMEOUT_MINUTES: i32 = 60;

/// How long after `StartBuild` the clock owes a build's transition out of
/// `IN_PROGRESS`.
///
/// AWS does not document a build duration — it depends on the buildspec — so
/// this span is CHOSEN, inside the 250-750 ms band this project fixes for an
/// emulated transition. It is deliberately not zero: a caller that reads the
/// build straight after starting it still observes `IN_PROGRESS`, and can still
/// stop it. The due instant is stored, never slept on.
pub const BUILD_SETTLE_MILLIS: i64 = 750;

/// How long a `StartBuild` idempotency token stays in force. AWS documents it on
/// `StartBuildInput$idempotencyToken`: "The token is included in the StartBuild
/// request and is valid for 5 minutes."
pub const IDEMPOTENCY_TOKEN_TTL_MILLIS: i64 = 5 * 60 * 1000;

/// Entries one page of a list operation may carry. The model caps both paginated
/// outputs implemented here at 100 entries — `ListBuildsForProject$ids` targets
/// `BuildIds` (`@length(min: 1, max: 100)`) and `ListProjects$projects` targets
/// `ProjectNames` (same bound) — so a page can hold no more than this.
pub const MAX_PAGE_SIZE: usize = 100;

/// Build projects this emulator holds for the account.
///
/// Real CodeBuild enforces an account quota on build projects per region and
/// answers `AccountLimitExceededException` once it is reached; CreateProject does
/// the same here, which is also the ceiling that stops `projects` growing without
/// bound as requests arrive.
pub const MAX_PROJECTS: usize = 5_000;

/// Build records this emulator retains, across every project.
///
/// A build is minted by each `StartBuild` rather than being a resource the caller
/// deletes, so the store needs a ceiling at that growth site: past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_build`] evicts the
/// oldest build (preferring one that has already completed), which also prunes
/// `builds_by_project` and the per-project counters that go with it. Real
/// CodeBuild ages build history out too; the number itself is this emulator's.
pub const MAX_BUILDS: usize = 10_000;

/// `StartBuild` idempotency entries retained at once.
///
/// Every insert first drops the entries whose documented 5-minute lifetime has
/// elapsed, since those can never be matched again; this ceiling is the backstop
/// for a burst that arrives faster than the TTL retires them.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// Environment variables one `StartBuild` may override.
///
/// THIS CEILING IS THE EMULATOR'S, NOT AWS'S: the model puts no `@length` on
/// `EnvironmentVariables`, so the framework's pre-handler constraint walk caps
/// nothing and the count is the caller's alone — yet it is a factor of the
/// per-request merge in `StartBuild`. This emulator therefore declares its own
/// ceiling and answers the modelled `InvalidInputException` past it, the way it
/// does for a modelled bound, so a request real CodeBuild would accept is refused
/// here. AWS documents no count limit (it limits the combined size of a build's
/// environment instead), and this is far above any real buildspec's variable set.
pub const MAX_ENVIRONMENT_VARIABLES: usize = 1_000;

/// Why an emulated build never reaches `SUCCEEDED`.
///
/// Nothing in this emulator executes a CodeBuild build container, so a build
/// cannot report a result for a buildspec that never ran. It ends instead in
/// CodeBuild's own build-system failure status, `FAULT`, carrying this
/// explanation as the failing phase's context.
pub const BUILD_NOT_EXECUTED_MESSAGE: &str = "this emulator does not run CodeBuild build \
    containers, so the build could not be provisioned and no buildspec was executed";

/// Per-service state for com-amazonaws-codebuild-codebuild-20161006. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodebuildCodebuild20161006State {
    /// Build projects, keyed by project name (names are unique per account).
    pub projects: BTreeMap<String, StoredProject>,
    /// Builds, keyed by build id (`<projectName>:<uuid>`).
    pub builds: BTreeMap<String, StoredBuild>,
    /// Build ids per project, in start order — which is build-number order, so a
    /// list operation sorts and pages over this without re-reading every build.
    pub builds_by_project: BTreeMap<String, Vec<String>>,
    /// The next `buildNumber` to hand out per project. A project's first build is
    /// 1 and each subsequent one is incremented by 1; the counter is kept when a
    /// project is deleted, so a number is never reused.
    pub next_build_numbers: BTreeMap<String, i64>,
    /// `StartBuild` idempotency tokens, keyed by (project name, token).
    pub start_build_tokens: BTreeMap<(String, String), StartBuildToken>,
}

/// A persisted build project: the modelled [`Project`] plus the instants its
/// `created` / `lastModified` timestamps report, kept as the integers a list
/// operation sorts on.
#[derive(Debug, Clone)]
pub struct StoredProject {
    /// The record every reader hands back.
    pub project: Project,
    /// Epoch millis the project was created at.
    pub created_millis: i64,
    /// Epoch millis the project was last changed at.
    pub last_modified_millis: i64,
}

/// A persisted build: the modelled [`Build`] plus the instant at which the clock
/// owes its move out of `IN_PROGRESS`.
#[derive(Debug, Clone)]
pub struct StoredBuild {
    /// The record every reader hands back.
    pub build: Build,
    /// Epoch millis the build was started at — the same instant
    /// `build.startTime` reports, kept as the integer the phase arithmetic uses.
    pub start_millis: i64,
    /// Epoch millis at which this build's terminal transition became due.
    pub settle_due_millis: i64,
}

/// What one in-force `StartBuild` idempotency token resolved to.
#[derive(Debug, Clone)]
pub struct StartBuildToken {
    /// The build the first request with this token started.
    pub build_id: String,
    /// The request that token was issued for, as a fixed-size
    /// [`request_fingerprint`], so a later request carrying the same token can be
    /// checked for a parameter mismatch without retaining a request-sized value.
    pub request_fingerprint: String,
    /// Epoch millis the token was issued at; it expires
    /// [`IDEMPOTENCY_TOKEN_TTL_MILLIS`] later.
    pub issued_millis: i64,
}

impl ComAmazonawsCodebuildCodebuild20161006State {
    /// Hand out `project_name`'s next `buildNumber` and advance the counter.
    pub fn allocate_build_number(&mut self, project_name: &str) -> i64 {
        let slot = self
            .next_build_numbers
            .entry(project_name.to_string())
            .or_insert(1);
        let number = *slot;
        *slot += 1;
        number
    }

    /// Persist `stored` under `id` and record it as the newest build of its
    /// project, then bring the store back inside [`MAX_BUILDS`].
    ///
    /// The id is a parameter rather than being read back out of
    /// `stored.build.id`, so no caller can lose a record to an absent id.
    pub fn insert_build(&mut self, id: String, stored: StoredBuild) {
        if let Some(project_name) = stored.build.project_name.clone() {
            self.builds_by_project
                .entry(project_name)
                .or_default()
                .push(id.clone());
        }
        self.builds.insert(id, stored);
        self.enforce_build_ceiling();
    }

    /// Evict builds until at most [`MAX_BUILDS`] are retained.
    ///
    /// The oldest COMPLETED build goes first, so a build a caller may still be
    /// waiting on survives an eviction round; when every retained build is still
    /// running the oldest one goes anyway, because the ceiling has to hold
    /// whatever the traffic looks like. Evicting a build also drops its id from
    /// `builds_by_project` and retires the per-project `buildNumber` counter once
    /// neither a project nor a build of that name is left — nothing can observe a
    /// counter after that, so keeping it would be unbounded growth for no
    /// behaviour.
    fn enforce_build_ceiling(&mut self) {
        let mut evicted_any = false;
        while self.builds.len() > MAX_BUILDS {
            let oldest_completed = self
                .builds
                .iter()
                .filter(|(_, stored)| stored.build.build_complete)
                .min_by_key(|(_, stored)| stored.start_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_completed.or_else(|| {
                self.builds
                    .iter()
                    .min_by_key(|(_, stored)| stored.start_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            evicted_any = true;
            let project_name = self
                .builds
                .remove(&id)
                .and_then(|stored| stored.build.project_name);
            if let Some(project_name) = project_name {
                if let Some(ids) = self.builds_by_project.get_mut(&project_name) {
                    ids.retain(|held| held != &id);
                    if ids.is_empty() {
                        self.builds_by_project.remove(&project_name);
                    }
                }
            }
        }
        // Only an eviction can orphan a counter, so the common path (nothing
        // evicted) does not pay for the scan.
        if !evicted_any {
            return;
        }
        let mut counters = ::std::mem::take(&mut self.next_build_numbers);
        counters.retain(|name, _| {
            self.projects.contains_key(name) || self.builds_by_project.contains_key(name)
        });
        self.next_build_numbers = counters;
    }

    /// Record the build an in-force idempotency token resolves to.
    ///
    /// Expired entries are dropped first — past [`IDEMPOTENCY_TOKEN_TTL_MILLIS`]
    /// an entry can never be matched again — and anything still over
    /// [`MAX_IDEMPOTENCY_TOKENS`] after that is retired oldest-first.
    pub fn record_start_build_token(
        &mut self,
        key: (String, String),
        token: StartBuildToken,
        now_millis: i64,
    ) {
        self.start_build_tokens
            .retain(|_, held| now_millis - held.issued_millis < IDEMPOTENCY_TOKEN_TTL_MILLIS);
        self.start_build_tokens.insert(key, token);
        while self.start_build_tokens.len() > MAX_IDEMPOTENCY_TOKENS {
            let oldest = self
                .start_build_tokens
                .iter()
                .min_by_key(|(_, held)| held.issued_millis)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.start_build_tokens.remove(&oldest);
        }
    }

    /// Read a build, first materialising the transition the clock already owes
    /// it. This is the only write a reporting operation makes: the move out of
    /// `IN_PROGRESS` was due at `settle_due_millis` whether or not anybody
    /// called, and it is computed from the stored instants, so repeating it
    /// changes nothing.
    pub fn settled_build(&mut self, id: &str, now_millis: i64) -> Option<&StoredBuild> {
        let stored = self.builds.get_mut(id)?;
        stored.settle(now_millis);
        Some(stored)
    }
}

impl StoredBuild {
    /// Materialise this build's terminal state once its due instant has passed.
    ///
    /// Idempotent and purely a function of the stored instants: a build that is
    /// already complete, or whose span has not elapsed, is left alone.
    pub fn settle(&mut self, now_millis: i64) {
        if self.build.build_complete || now_millis < self.settle_due_millis {
            return;
        }
        let start = self.start_millis;
        let due = self.settle_due_millis;
        self.build.build_status = Some("FAULT".to_string());
        self.build.current_phase = Some("COMPLETED".to_string());
        self.build.build_complete = true;
        self.build.end_time = Some(datetime(due));
        self.build.phases = Some(terminal_phases(start, due));
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodebuildCodebuild20161006State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodebuildCodebuild20161006State>)
}

/// Now, as epoch milliseconds. The one clock the service reads.
pub fn now_millis() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Epoch milliseconds as the modelled timestamp type. Exact: the stored value is
/// already integer milliseconds, so it is handed over as such rather than through a
/// float.
pub fn datetime(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// A build's unique identifier — the UUID AWS puts after `<projectName>:`.
///
/// DERIVED, not drawn from a generator: an RFC 9562 version-8 (custom,
/// hash-based) UUID over the values that already make a build unique — its
/// project, the `buildNumber` just allocated for it under the state lock, and the
/// instant it started. No two builds of a project can collide, and the result is
/// the 8-4-4-4-12 hex form a client parses. The digest is SHA-256 (`sha2`), the
/// same one [`request_fingerprint`] uses, so this needs no dependency the crate
/// does not already carry.
pub fn build_uuid(project_name: &str, build_number: i64, start_millis: i64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(project_name.as_bytes());
    // A separator, so ("a", 11) and ("a1", 1) cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(build_number.to_be_bytes());
    digest.update(start_millis.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    // RFC 9562: the version goes in the high nibble of octet 6 and the variant
    // (0b10) in the top bits of octet 8.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A fixed-size fingerprint of a rendered request, so one retained idempotency
/// entry can never be request-sized however large the request was.
///
/// The caller renders the whole request with `Debug`, which this crate's
/// generated shapes take from `#[derive(Debug)]`: every member is printed in
/// full, with no redaction, and `StartBuild`'s input closure reaches no
/// `@sensitive` shape in any case. So two requests that differ in ANY member
/// fingerprint differently, which is what a parameter-mismatch check needs.
pub fn request_fingerprint(rendered: &str) -> String {
    use ::sha2::Digest;
    ::hex::encode(::sha2::Sha256::digest(rendered.as_bytes()))
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Who started a build: the access key id the request was signed with, or
/// `"default"` on an unsigned request. CodeBuild reports "the entity that started
/// the build", which is the calling identity and nothing from the request body.
pub fn request_initiator(parts: &::http::request::Parts) -> String {
    ::emulator_server::extract_akid(&parts.headers)
}

/// `arn:aws:codebuild:<region>:<account>:project/<name>`.
pub fn project_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("project/{name}"))
}

/// `arn:aws:codebuild:<region>:<account>:build/<id>`.
pub fn build_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("build/{id}"))
}

/// The phases of a build that is still `IN_PROGRESS`: it was submitted, and it is
/// waiting in the queue.
pub fn in_progress_phases(start_millis: i64) -> Vec<BuildPhase> {
    vec![
        finished_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        open_phase("QUEUED", start_millis),
    ]
}

/// The phases of a build that has reached its terminal state: provisioning is
/// where an emulated build stops, because there is no container to provision.
fn terminal_phases(start_millis: i64, due_millis: i64) -> Vec<BuildPhase> {
    vec![
        finished_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        finished_phase("QUEUED", "SUCCEEDED", start_millis, due_millis, None),
        finished_phase(
            "PROVISIONING",
            "FAULT",
            due_millis,
            due_millis,
            Some(vec![PhaseContext {
                status_code: None,
                message: Some(BUILD_NOT_EXECUTED_MESSAGE.to_string()),
            }]),
        ),
        open_phase("COMPLETED", due_millis),
    ]
}

/// The phases of a build that was stopped while in progress: it leaves the queue
/// with the same status the build took.
pub fn stopped_phases(start_millis: i64, stopped_millis: i64) -> Vec<BuildPhase> {
    vec![
        finished_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        finished_phase("QUEUED", "STOPPED", start_millis, stopped_millis, None),
        open_phase("COMPLETED", stopped_millis),
    ]
}

/// A phase that ran and ended, with its measured duration.
fn finished_phase(
    phase_type: &str,
    phase_status: &str,
    start_millis: i64,
    end_millis: i64,
    contexts: Option<Vec<PhaseContext>>,
) -> BuildPhase {
    BuildPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: Some(phase_status.to_string()),
        start_time: Some(datetime(start_millis)),
        end_time: Some(datetime(end_millis)),
        duration_in_seconds: Some((end_millis - start_millis) / 1000),
        contexts,
    }
}

/// A phase that has started and carries no outcome yet — which is how CodeBuild
/// reports the phase a build currently sits in, including the final `COMPLETED`
/// marker phase.
fn open_phase(phase_type: &str, start_millis: i64) -> BuildPhase {
    BuildPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: None,
        start_time: Some(datetime(start_millis)),
        end_time: None,
        duration_in_seconds: None,
        contexts: None,
    }
}

/// One page of `items`, resuming after the entry `next_token` names.
///
/// A token is the id of the first entry of the page it opens, so paging stays
/// put when entries are added: CodeBuild's own tokens are opaque, and this one is
/// too as far as a caller is concerned. Resolving it is a linear scan rather than
/// an index decode deliberately — an index would silently shift a caller onto a
/// different entry as soon as anything was inserted before it, and the scan is
/// over a list bounded by [`MAX_BUILDS`] / [`MAX_PROJECTS`].
///
/// `None` means the token names nothing in the list, which is a refusable
/// request rather than an empty page.
pub fn page(items: &[String], next_token: Option<&str>) -> Option<(Vec<String>, Option<String>)> {
    let start = match next_token {
        None => 0,
        Some(token) => items.iter().position(|item| item == token)?,
    };
    let end = (start + MAX_PAGE_SIZE).min(items.len());
    let next = items.get(end).cloned();
    Some((items[start..end].to_vec(), next))
}
