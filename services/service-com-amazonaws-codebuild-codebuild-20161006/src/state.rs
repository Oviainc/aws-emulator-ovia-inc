// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codebuild-codebuild-20161006 service.
//!
//! Every implemented CodeBuild operation computes its answer from the records
//! here: `CreateProject` writes a [`Project`], `StartBuild` writes a
//! [`StoredBuild`], `StartBuildBatch` writes a [`StoredBuildBatch`],
//! `StartSandbox` writes a [`StoredSandbox`] and `StopSandbox` ends one,
//! `CreateReportGroup` writes a [`StoredReportGroup`],
//! `ImportSourceCredentials` writes a [`StoredSourceCredentials`],
//! `BatchDeleteBuilds` removes a build, and the reporting operations
//! (`BatchGetProjects`, `BatchGetBuilds`, `BatchGetBuildBatches`,
//! `BatchGetSandboxes`, `BatchGetReportGroups`, `ListProjects`,
//! `ListBuildsForProject`, `ListSandboxes`, `ListSandboxesForProject`,
//! `ListSourceCredentials`) only read them.
//!
//! The model shapes themselves are what is stored, so a read hands back exactly
//! the structure the write persisted rather than a second, drifting copy of the
//! same resource.

use std::collections::BTreeMap;

use crate::ops::types::{
    Build, BuildArtifacts, BuildBatch, BuildBatchPhase, BuildPhase, EnvironmentVariable,
    PhaseContext, Project, ProjectArtifacts, ReportGroup, Sandbox, SandboxSession,
    SandboxSessionPhase, SourceCredentialsInfo,
};

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

/// How long after `StartBuild` or `StartBuildBatch` the clock owes the started
/// build's — or batch build's — transition out of `IN_PROGRESS`.
///
/// AWS does not document a build duration — it depends on the buildspec — so
/// this span is CHOSEN, inside the 250-750 ms band this project fixes for an
/// emulated transition. It is deliberately not zero: a caller that reads the
/// build straight after starting it still observes `IN_PROGRESS`, and can still
/// stop it. The due instant is stored, never slept on.
pub const BUILD_SETTLE_MILLIS: i64 = 750;

/// How long after `StartSandbox` the clock owes a sandbox's move out of `QUEUED`
/// into [`SANDBOX_STATUS_RUNNING`] — the span its environment is provisioned in.
///
/// AWS documents no provisioning time for a sandbox, so this span is CHOSEN, inside
/// the 250-750 ms band this project fixes for an emulated transition. It is
/// deliberately not zero: a caller that reads the sandbox straight after starting it
/// still observes `QUEUED`. The due instant is stored, never slept on.
pub const SANDBOX_SETTLE_MILLIS: i64 = 500;

/// How long a `StartBuild` or `StartBuildBatch` idempotency token stays in
/// force. AWS documents the same lifetime on both members — on
/// `StartBuildInput$idempotencyToken`: "The token is included in the StartBuild
/// request and is valid for 5 minutes.", and on
/// `StartBuildBatchInput$idempotencyToken`: "The token is included in the
/// StartBuildBatch request and is valid for five minutes."
pub const IDEMPOTENCY_TOKEN_TTL_MILLIS: i64 = 5 * 60 * 1000;

/// How long a `StartSandbox` idempotency token stays in force.
///
/// AWS documents `StartSandboxInput$idempotencyToken` as only "A unique client
/// token" and states no lifetime for it, so this span is CHOSEN: it is the 5 minutes
/// CodeBuild documents for its nearest documented sibling, `StartBuild`'s token.
pub const SANDBOX_IDEMPOTENCY_TOKEN_TTL_MILLIS: i64 = IDEMPOTENCY_TOKEN_TTL_MILLIS;

/// Entries one page of a list operation may carry. The model caps both paginated
/// outputs implemented here at 100 entries — `ListBuildsForProject$ids` targets
/// `BuildIds` (`@length(min: 1, max: 100)`) and `ListProjects$projects` targets
/// `ProjectNames` (same bound) — so a page can hold no more than this.
pub const MAX_PAGE_SIZE: usize = 100;

/// The `AuthType` values whose `token` is a resource ARN rather than a secret,
/// from `SourceCredentialsInfo$resource`: "The connection ARN if your authType is
/// CODECONNECTIONS or SECRETS_MANAGER."
pub const RESOURCE_ARN_AUTH_TYPES: &[&str] = &["CODECONNECTIONS", "SECRETS_MANAGER"];

/// The one `AuthType` `ImportSourceCredentials` cannot create, from
/// `ImportSourceCredentialsInput$authType`: "An OAUTH connection is not supported
/// by the API and must be created using the CodeBuild console."
pub const CONSOLE_ONLY_AUTH_TYPE: &str = "OAUTH";

/// The one `AuthType` `ImportSourceCredentialsInput$username` is accepted for,
/// from that member's own documentation: "The Bitbucket username when the
/// authType is BASIC_AUTH. This parameter is not valid for other types of source
/// providers or connections."
pub const USERNAME_AUTH_TYPE: &str = "BASIC_AUTH";

/// Build projects this emulator holds for the account.
///
/// Real CodeBuild enforces an account quota on build projects per region and
/// answers `AccountLimitExceededException` once it is reached; CreateProject does
/// the same here, which is also the ceiling that stops `projects` growing without
/// bound as requests arrive.
pub const MAX_PROJECTS: usize = 5_000;

/// Report groups this emulator holds for the account.
///
/// Real CodeBuild caps report groups per region for an account and answers
/// `AccountLimitExceededException` once the cap is reached; `CreateReportGroup`
/// does the same here, which is also the ceiling that stops `report_groups`
/// growing without bound as requests arrive. The NUMBER is this emulator's, not
/// a published AWS quota.
pub const MAX_REPORT_GROUPS: usize = 5_000;

/// The status a report group carries while it exists.
///
/// `ReportGroupStatusType` has exactly two values and the model documents them:
/// `ACTIVE` — "The report group is active" — and `DELETING` — "The report group
/// is in the process of being deleted". A created report group is usable
/// immediately, so it is `ACTIVE` from the call that made it: there is no
/// non-terminal status for the clock to move it out of.
pub const REPORT_GROUP_ACTIVE_STATUS: &str = "ACTIVE";

/// Build records this emulator retains, across every project.
///
/// A build is minted by each `StartBuild` rather than being a resource the caller
/// deletes, so the store needs a ceiling at that growth site: past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_build`] evicts the
/// oldest build (preferring one that has already completed), which also prunes
/// `builds_by_project` and the per-project counters that go with it. Real
/// CodeBuild ages build history out too; the number itself is this emulator's.
pub const MAX_BUILDS: usize = 10_000;

/// Batch-build records this emulator retains, across every project.
///
/// The same growth-site reasoning as [`MAX_BUILDS`]: a batch build is minted by
/// each `StartBuildBatch` rather than being a resource the caller deletes, so
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_build_batch`] evicts the
/// oldest batch (preferring one that has already completed) past this ceiling,
/// pruning `build_batches_by_project` and the per-project counters with it.
pub const MAX_BUILD_BATCHES: usize = 10_000;

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

/// Entries one `BatchGetSandboxes` request may ask for.
///
/// THIS CEILING IS THE EMULATOR'S, NOT AWS'S: `SandboxIds` carries no `@length`, so
/// the framework's pre-handler constraint walk caps nothing and the entry count is
/// the caller's alone — yet each entry copies a whole sandbox record (its source,
/// environment, secondary sources and session phases) into the response, so the count
/// is what the answer's size is a multiple of. This emulator therefore declares its
/// own ceiling and answers the modelled `InvalidInputException` past it, the way it
/// does for a modelled bound. The number is the one the model puts on the sibling
/// `BuildIds` (`@length(min: 1, max: 100)`), which bounds `BatchGetBuilds`.
pub const MAX_BATCH_GET_SANDBOXES_IDS: usize = MAX_PAGE_SIZE;

/// Sandbox records this emulator retains, across every project.
///
/// A sandbox is minted by each `StartSandbox` rather than being a resource the
/// caller deletes, so the store needs a ceiling at that growth site: past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_sandbox`] evicts the oldest
/// sandbox (preferring one that has already reached a terminal status), which also
/// prunes `sandboxes_by_project`. THIS NUMBER IS THE EMULATOR'S, NOT AWS'S — real
/// CodeBuild caps concurrently running sandboxes by account quota instead.
pub const MAX_SANDBOXES: usize = 1_000;

/// `StartSandbox` idempotency entries retained at once — the same backstop
/// [`MAX_IDEMPOTENCY_TOKENS`] is for `StartBuild`'s, for the same reason.
pub const MAX_SANDBOX_IDEMPOTENCY_TOKENS: usize = MAX_IDEMPOTENCY_TOKENS;

/// Why an emulated build never reaches `SUCCEEDED`.
///
/// Nothing in this emulator executes a CodeBuild build container, so a build
/// cannot report a result for a buildspec that never ran. It ends instead in
/// CodeBuild's own build-system failure status, `FAULT`, carrying this
/// explanation as the failing phase's context.
pub const BUILD_NOT_EXECUTED_MESSAGE: &str = "this emulator does not run CodeBuild build \
    containers, so the build could not be provisioned and no buildspec was executed";

/// Why an emulated batch build never reaches `SUCCEEDED`.
///
/// A batch build's first act is `DOWNLOAD_BATCHSPEC`: CodeBuild fetches the
/// project's source inside a build container and reads the batch section of its
/// buildspec to learn which builds the batch is made of. Nothing in this
/// emulator fetches a source or runs a container, so the batch specification is
/// never read, no build of the batch exists, and the batch ends in CodeBuild's
/// build-system failure status, `FAULT`, carrying this explanation as the
/// failing phase's context.
pub const BATCH_SPEC_NOT_DOWNLOADED_MESSAGE: &str = "this emulator does not run CodeBuild \
    build containers, so the batch build specification could not be downloaded and no build \
    of the batch was started";


/// The status a sandbox and its session report between `StartSandbox` and the
/// transition the clock owes them: CodeBuild queues a sandbox request the same way it
/// queues a build.
pub const SANDBOX_STATUS_QUEUED: &str = "QUEUED";

/// The status a provisioned sandbox and its live session report.
///
/// A sandbox is a LIVE, long-lived session rather than a job that runs to an outcome:
/// `Sandbox$timeoutInMinutes` is "How long, in minutes, from 5 to 2160 (36 hours), for
/// CodeBuild to wait before timing out this sandbox if it does not get marked as
/// completed". So once it leaves the queue it reaches this NON-TERMINAL status and
/// stays in it until it is stopped or that timeout elapses — which is what gives the
/// command-execution and connection operations a session to act on.
pub const SANDBOX_STATUS_RUNNING: &str = "RUNNING";

/// The terminal status a sandbox nobody stopped reaches once the
/// `timeoutInMinutes` it carries has elapsed. `TIMED_OUT` is a `StatusType` value,
/// which is the enum the model puts on a sandbox session's phase status.
pub const SANDBOX_STATUS_TIMED_OUT: &str = "TIMED_OUT";

/// The terminal status a sandbox `StopSandbox` ended reaches. `STOPPED` is a
/// `StatusType` value, like [`SANDBOX_STATUS_TIMED_OUT`].
pub const SANDBOX_STATUS_STOPPED: &str = "STOPPED";

/// The outcome of a phase that ran to completion, from `StatusType`.
const PHASE_SUCCEEDED: &str = "SUCCEEDED";

/// The session phase a sandbox request is first recorded in.
const PHASE_SUBMITTED: &str = "SUBMITTED";

/// The session phase a sandbox waits in until it is provisioned.
const PHASE_QUEUED: &str = "QUEUED";

/// The session phase a sandbox's environment is prepared in.
const PHASE_PROVISIONING: &str = "PROVISIONING";

/// The session phase a live sandbox sits in for as long as it is usable, and the
/// `currentSession.currentPhase` CodeBuild reports for a `RUNNING` sandbox (as the
/// recorded responses this emulator is checked against show).
pub const SANDBOX_PHASE_RUN_SANDBOX: &str = "RUN_SANDBOX";

/// The final marker phase of a session that has ended, the way a build reports it.
const PHASE_COMPLETED: &str = "COMPLETED";

/// Milliseconds in one minute — what a documented `timeoutInMinutes` is converted by.
const MILLIS_PER_MINUTE: i64 = 60_000;

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
    /// 1 and each subsequent one is incremented by 1; the counter is kept while the
    /// project exists OR any of its builds is still retained, so deleting a project
    /// (whose builds survive it, as AWS documents) never reuses a number, and
    /// neither does deleting a build of a project that is still there. It is
    /// retired only once neither is left — nothing can observe it then, and keeping
    /// it would be unbounded growth for no behaviour.
    pub next_build_numbers: BTreeMap<String, i64>,
    /// `StartBuild` idempotency tokens, keyed by (project name, token).
    pub start_build_tokens: BTreeMap<(String, String), StartBuildToken>,
    /// Batch builds, keyed by batch build id (`<projectName>:<uuid>`). Batch
    /// builds are their own resource — a batch id is never a build id — so they
    /// are held beside `builds` rather than mixed into it.
    pub build_batches: BTreeMap<String, StoredBuildBatch>,
    /// Batch build ids per project, in start order — which is
    /// `buildBatchNumber` order, so a list operation sorts and pages over this
    /// without re-reading every batch.
    pub build_batches_by_project: BTreeMap<String, Vec<String>>,
    /// The next `buildBatchNumber` to hand out per project. A project's first
    /// batch build is 1 and each subsequent one is incremented by 1; AWS
    /// documents that deleting a batch build leaves the other numbers alone, so
    /// the counter is kept and a number is never reused.
    pub next_build_batch_numbers: BTreeMap<String, i64>,
    /// `StartBuildBatch` idempotency tokens, keyed by (project name, token).
    /// Kept apart from `start_build_tokens` because the two operations issue
    /// their tokens independently: the same token value may be in force for a
    /// build and for a batch build of one project.
    pub start_build_batch_tokens: BTreeMap<(String, String), StartBuildToken>,
    /// Report groups, keyed by report group name (names are unique per account,
    /// the same way project names are).
    pub report_groups: BTreeMap<String, StoredReportGroup>,
    /// Imported source-repository credentials, keyed by the `ServerType` they
    /// were imported for.
    ///
    /// One entry per server type is what the resource's own identity allows: a
    /// credential is addressed only by its token ARN, and that ARN carries
    /// nothing but the server type (see [`source_credential_arn`]), so two
    /// credentials for one server type could not be told apart by
    /// `DeleteSourceCredentials`. It is also what `ImportSourceCredentials`'s
    /// `shouldOverwrite` member and its `ResourceAlreadyExistsException` are
    /// about. Keying by the server type therefore bounds this map at the five
    /// values `ServerType` declares, with no eviction to do.
    pub source_credentials: BTreeMap<String, StoredSourceCredentials>,
    /// Sandboxes, keyed by sandbox id (`<projectName>:<uuid>`).
    pub sandboxes: BTreeMap<String, StoredSandbox>,
    /// Sandbox ids per project, in start order — which is the order
    /// `ListSandboxesForProject` reports them in, so a list operation pages over
    /// this without re-reading every sandbox.
    pub sandboxes_by_project: BTreeMap<String, Vec<String>>,
    /// Every sandbox id of the account, in request order (oldest first).
    ///
    /// `sandboxes` is keyed by id, and an id carries the project name and a
    /// derived uuid, so key order is not request order. `ListSandboxes` reports
    /// the account's sandboxes in request order, which only an explicit sequence
    /// records: two sandboxes requested in the same millisecond are still
    /// ordered, and the order survives an eviction round.
    pub sandbox_order: Vec<String>,
    /// The next per-project sandbox sequence number to hand out. It is not a
    /// modelled member: it is what makes a sandbox's derived id unique when two
    /// sandboxes of one project are requested in the same millisecond.
    pub next_sandbox_numbers: BTreeMap<String, i64>,
    /// `StartSandbox` idempotency tokens, keyed by (project name, token).
    pub start_sandbox_tokens: BTreeMap<(String, String), StartSandboxToken>,
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

/// A persisted batch build: the modelled [`BuildBatch`] plus the instant at
/// which the clock owes its move out of `IN_PROGRESS`.
#[derive(Debug, Clone)]
pub struct StoredBuildBatch {
    /// The record every reader hands back.
    pub build_batch: BuildBatch,
    /// Epoch millis the batch build was started at — the same instant
    /// `build_batch.startTime` reports, kept as the integer the phase
    /// arithmetic uses.
    pub start_millis: i64,
    /// Epoch millis at which this batch build's terminal transition became due.
    pub settle_due_millis: i64,
}

/// A persisted report group: the modelled [`ReportGroup`] plus the instants its
/// `created` / `lastModified` timestamps report, kept as the integers a list
/// operation sorts on — the same shape [`StoredProject`] takes.
#[derive(Debug, Clone)]
pub struct StoredReportGroup {
    /// The record every reader hands back.
    pub report_group: ReportGroup,
    /// Epoch millis the report group was created at.
    pub created_millis: i64,
    /// Epoch millis the report group was last changed at.
    pub last_modified_millis: i64,
}

/// One set of imported source-repository credentials.
///
/// The caller's `token` is NOT kept. Nothing this emulator serves authenticates
/// to a source provider, and no operation reports the token back, so the secret
/// is read and dropped; the only part retained is the connection or secret ARN
/// that `SourceCredentialsInfo$resource` reports ("The connection ARN if your
/// authType is CODECONNECTIONS or SECRETS_MANAGER"), which is a resource
/// identifier rather than a credential.
#[derive(Debug, Clone)]
pub struct StoredSourceCredentials {
    /// The token ARN this credential is addressed by.
    pub arn: String,
    /// The `ServerType` it was imported for — the key this record is held under.
    pub server_type: String,
    /// The `AuthType` it was imported with.
    pub auth_type: String,
    /// The connection or secret ARN, for the auth types whose token IS one.
    pub resource: Option<String>,
}

impl StoredSourceCredentials {
    /// This record as the modelled shape every reader hands back.
    pub fn info(&self) -> SourceCredentialsInfo {
        SourceCredentialsInfo {
            arn: Some(self.arn.clone()),
            server_type: Some(self.server_type.clone()),
            auth_type: Some(self.auth_type.clone()),
            resource: self.resource.clone(),
        }
    }
}

/// A persisted sandbox: the modelled [`Sandbox`] plus the instants its timestamps
/// report and the instant at which the clock owes its move out of `QUEUED`.
#[derive(Debug, Clone)]
pub struct StoredSandbox {
    /// The record every reader hands back.
    pub sandbox: Sandbox,
    /// Epoch millis the sandbox was requested at — the same instant
    /// `sandbox.requestTime` reports, kept as the integer the phase arithmetic uses.
    pub request_millis: i64,
    /// Epoch millis at which this sandbox's transition out of `QUEUED` became due.
    pub settle_due_millis: i64,
}

/// What one in-force `StartSandbox` idempotency token resolved to. The sibling of
/// [`StartBuildToken`], keyed and retired by the same rules.
///
/// It keeps no request fingerprint, because unlike `StartBuild` there is nothing
/// left to mismatch: `StartSandbox`'s only other member is `projectName`, which is
/// already half of the key, so a repeat that reaches an entry carries exactly the
/// request that issued it.
#[derive(Debug, Clone)]
pub struct StartSandboxToken {
    /// The sandbox the first request with this token started.
    pub sandbox_id: String,
    /// Epoch millis the token was issued at; it expires
    /// [`SANDBOX_IDEMPOTENCY_TOKEN_TTL_MILLIS`] later.
    pub issued_millis: i64,
}

/// What one in-force `StartBuild` or `StartBuildBatch` idempotency token
/// resolved to.
#[derive(Debug, Clone)]
pub struct StartBuildToken {
    /// The build — or, for a `StartBuildBatch` token, the batch build — the
    /// first request with this token started.
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
        allocate_number(&mut self.next_build_numbers, project_name)
    }

    /// Hand out `project_name`'s next sandbox sequence number and advance the
    /// counter. Like a `buildNumber`, a number is never reused for a project.
    pub fn allocate_sandbox_number(&mut self, project_name: &str) -> i64 {
        allocate_number(&mut self.next_sandbox_numbers, project_name)
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
            self.remove_build(&id);
        }
    }

    /// Remove the build `id` names, and answer whether a record was held.
    ///
    /// The one place a build leaves the store, so the project index never keeps an
    /// id the build map no longer has: `BatchDeleteBuilds` deletes through here and
    /// so does an eviction. A `buildNumber` counter can only be orphaned by the
    /// removal that empties its project's index, so the scan runs there and nowhere
    /// else — a 100-id delete over one project pays for one scan, and an insert that
    /// evicts nothing pays for none.
    pub fn remove_build(&mut self, id: &str) -> bool {
        let Some(stored) = self.builds.remove(id) else {
            return false;
        };
        if let Some(project_name) = stored.build.project_name {
            if let Some(ids) = self.builds_by_project.get_mut(&project_name) {
                ids.retain(|held| held != id);
                if ids.is_empty() {
                    self.builds_by_project.remove(&project_name);
                    self.retire_orphaned_build_numbers();
                }
            }
        }
        true
    }

    /// Drop the `buildNumber` counters nothing can observe any more.
    ///
    /// A counter is kept while its project exists or any of its builds is still
    /// retained, so a number is never reused for a project a caller can still
    /// reach; once neither is left nothing can observe the counter, and keeping it
    /// would be unbounded growth for no behaviour.
    fn retire_orphaned_build_numbers(&mut self) {
        let mut counters = ::std::mem::take(&mut self.next_build_numbers);
        counters.retain(|name, _| {
            self.projects.contains_key(name) || self.builds_by_project.contains_key(name)
        });
        self.next_build_numbers = counters;
    }

    /// Hand out `project_name`'s next `buildBatchNumber` and advance the
    /// counter. Batch numbers run in their own sequence per project: AWS
    /// documents a project's first batch build as `buildBatchNumber` 1
    /// whatever its builds are numbered.
    pub fn allocate_build_batch_number(&mut self, project_name: &str) -> i64 {
        allocate_number(&mut self.next_build_batch_numbers, project_name)
    }

    /// Persist `stored` under `id` and record it as the newest batch build of
    /// its project, then bring the store back inside [`MAX_BUILD_BATCHES`].
    ///
    /// The id is a parameter rather than being read back out of
    /// `stored.build_batch.id`, so no caller can lose a record to an absent id.
    pub fn insert_build_batch(&mut self, id: String, stored: StoredBuildBatch) {
        if let Some(project_name) = stored.build_batch.project_name.clone() {
            self.build_batches_by_project
                .entry(project_name)
                .or_default()
                .push(id.clone());
        }
        self.build_batches.insert(id, stored);
        self.enforce_build_batch_ceiling();
    }

    /// Evict batch builds until at most [`MAX_BUILD_BATCHES`] are retained —
    /// [`Self::enforce_build_ceiling`]'s rule, applied to the batch records:
    /// the oldest COMPLETED batch goes first so one a caller may still be
    /// waiting on survives a round, and the per-project `buildBatchNumber`
    /// counter is retired only once nothing can observe it again.
    fn enforce_build_batch_ceiling(&mut self) {
        let mut evicted_any = false;
        while self.build_batches.len() > MAX_BUILD_BATCHES {
            let oldest_completed = self
                .build_batches
                .iter()
                .filter(|(_, stored)| stored.build_batch.complete)
                .min_by_key(|(_, stored)| stored.start_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_completed.or_else(|| {
                self.build_batches
                    .iter()
                    .min_by_key(|(_, stored)| stored.start_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            evicted_any = true;
            let project_name = self
                .build_batches
                .remove(&id)
                .and_then(|stored| stored.build_batch.project_name);
            if let Some(project_name) = project_name {
                if let Some(ids) = self.build_batches_by_project.get_mut(&project_name) {
                    ids.retain(|held| held != &id);
                    if ids.is_empty() {
                        self.build_batches_by_project.remove(&project_name);
                    }
                }
            }
        }
        if !evicted_any {
            return;
        }
        let mut counters = ::std::mem::take(&mut self.next_build_batch_numbers);
        counters.retain(|name, _| {
            self.projects.contains_key(name) || self.build_batches_by_project.contains_key(name)
        });
        self.next_build_batch_numbers = counters;
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
        record_token(
            &mut self.start_build_tokens,
            key,
            token,
            now_millis,
            IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_IDEMPOTENCY_TOKENS,
        );
    }

    /// Record the batch build an in-force `StartBuildBatch` idempotency token
    /// resolves to, under [`Self::record_start_build_token`]'s rule.
    pub fn record_start_build_batch_token(
        &mut self,
        key: (String, String),
        token: StartBuildToken,
        now_millis: i64,
    ) {
        record_token(
            &mut self.start_build_batch_tokens,
            key,
            token,
            now_millis,
            IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_IDEMPOTENCY_TOKENS,
        );
    }

    /// The report group `arn` names, if this account holds it.
    ///
    /// Report groups are keyed by name because that is what is unique per
    /// account, while every report-group operation addresses them by ARN. The
    /// name is taken from the ARN's resource segment for the map lookup, and the
    /// candidate is then kept only if its STORED ARN is exactly the one asked
    /// for — so an ARN for another partition, region, account or resource type
    /// can never resolve to a local record, and the read stays logarithmic in
    /// the number of report groups held.
    pub fn report_group_by_arn(&self, arn: &str) -> Option<&StoredReportGroup> {
        let (_, name) = arn.rsplit_once(":report-group/")?;
        let stored = self.report_groups.get(name)?;
        (stored.report_group.arn.as_deref() == Some(arn)).then_some(stored)
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

    /// Read a batch build, first materialising the transition the clock already
    /// owes it, under [`Self::settled_build`]'s rule.
    pub fn settled_build_batch(&mut self, id: &str, now_millis: i64) -> Option<&StoredBuildBatch> {
        let stored = self.build_batches.get_mut(id)?;
        stored.settle(now_millis);
        Some(stored)
    }

    /// Persist `stored` under `id` and record it as the newest sandbox of its
    /// project, then bring the store back inside [`MAX_SANDBOXES`]. The sibling of
    /// [`Self::insert_build`], and the id is a parameter for the same reason.
    pub fn insert_sandbox(&mut self, id: String, stored: StoredSandbox) {
        if let Some(project_name) = stored.sandbox.project_name.clone() {
            self.sandboxes_by_project
                .entry(project_name)
                .or_default()
                .push(id.clone());
        }
        self.sandbox_order.push(id.clone());
        self.sandboxes.insert(id, stored);
        self.enforce_sandbox_ceiling();
    }

    /// Evict sandboxes until at most [`MAX_SANDBOXES`] are retained, oldest
    /// terminal one first — the same rule [`Self::enforce_build_ceiling`] applies to
    /// builds, so a sandbox a caller may still be holding survives an eviction
    /// round. Evicting one also drops its id from `sandboxes_by_project` and retires
    /// the per-project sequence counter once neither a project nor a sandbox of that
    /// name is left.
    fn enforce_sandbox_ceiling(&mut self) {
        let mut evicted_any = false;
        while self.sandboxes.len() > MAX_SANDBOXES {
            let oldest_terminal = self
                .sandboxes
                .iter()
                .filter(|(_, stored)| stored.is_terminal())
                .min_by_key(|(_, stored)| stored.request_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_terminal.or_else(|| {
                self.sandboxes
                    .iter()
                    .min_by_key(|(_, stored)| stored.request_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            evicted_any = true;
            let project_name = self
                .sandboxes
                .remove(&id)
                .and_then(|stored| stored.sandbox.project_name);
            self.sandbox_order.retain(|held| held != &id);
            if let Some(project_name) = project_name {
                if let Some(ids) = self.sandboxes_by_project.get_mut(&project_name) {
                    ids.retain(|held| held != &id);
                    if ids.is_empty() {
                        self.sandboxes_by_project.remove(&project_name);
                    }
                }
            }
        }
        if !evicted_any {
            return;
        }
        let mut counters = ::std::mem::take(&mut self.next_sandbox_numbers);
        counters.retain(|name, _| {
            self.projects.contains_key(name) || self.sandboxes_by_project.contains_key(name)
        });
        self.next_sandbox_numbers = counters;
    }

    /// Record the sandbox an in-force idempotency token resolves to, under the same
    /// retention rules [`Self::record_start_build_token`] applies.
    pub fn record_start_sandbox_token(
        &mut self,
        key: (String, String),
        token: StartSandboxToken,
        now_millis: i64,
    ) {
        record_token(
            &mut self.start_sandbox_tokens,
            key,
            token,
            now_millis,
            SANDBOX_IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_SANDBOX_IDEMPOTENCY_TOKENS,
        );
    }

    /// Read a sandbox, first materialising the transitions the clock already owes
    /// it — the sandbox counterpart of [`Self::settled_build`], and the only write a
    /// reporting operation makes: the move out of `QUEUED` was due at
    /// `settle_due_millis`, and the timeout at
    /// [`StoredSandbox::timeout_due_millis`], whether or not anybody called, and both
    /// are computed from the stored instants, so repeating them changes nothing.
    pub fn settled_sandbox(&mut self, id: &str, now_millis: i64) -> Option<&StoredSandbox> {
        let stored = self.sandboxes.get_mut(id)?;
        stored.settle(now_millis);
        Some(stored)
    }
}

/// Hand out `name`'s next number from `counters` and advance it. Shared by the
/// per-project `buildNumber` and sandbox sequence counters, which allocate alike.
fn allocate_number(counters: &mut BTreeMap<String, i64>, name: &str) -> i64 {
    let slot = counters.entry(name.to_string()).or_insert(1);
    let number = *slot;
    *slot += 1;
    number
}

/// An idempotency entry, as the shared retention pass below reads it.
trait IssuedToken {
    /// Epoch millis the entry was issued at.
    fn issued_millis(&self) -> i64;
}

impl IssuedToken for StartBuildToken {
    fn issued_millis(&self) -> i64 {
        self.issued_millis
    }
}

impl IssuedToken for StartSandboxToken {
    fn issued_millis(&self) -> i64 {
        self.issued_millis
    }
}

/// Record `token` under `key`, dropping the entries whose `ttl_millis` lifetime has
/// elapsed first — past it an entry can never be matched again — and retiring
/// anything still over `ceiling` oldest-first.
fn record_token<T: IssuedToken>(
    tokens: &mut BTreeMap<(String, String), T>,
    key: (String, String),
    token: T,
    now_millis: i64,
    ttl_millis: i64,
    ceiling: usize,
) {
    tokens.retain(|_, held| now_millis - held.issued_millis() < ttl_millis);
    tokens.insert(key, token);
    while tokens.len() > ceiling {
        let oldest = tokens
            .iter()
            .min_by_key(|(_, held)| held.issued_millis())
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else { break };
        tokens.remove(&oldest);
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

impl StoredBuildBatch {
    /// Materialise this batch build's terminal state once its due instant has
    /// passed, under [`StoredBuild::settle`]'s rule: idempotent, and purely a
    /// function of the stored instants.
    pub fn settle(&mut self, now_millis: i64) {
        if self.build_batch.complete || now_millis < self.settle_due_millis {
            return;
        }
        let start = self.start_millis;
        let due = self.settle_due_millis;
        self.build_batch.build_batch_status = Some("FAULT".to_string());
        self.build_batch.current_phase = Some("FAILED".to_string());
        self.build_batch.complete = true;
        self.build_batch.end_time = Some(datetime(due));
        self.build_batch.phases = Some(terminal_batch_phases(start, due));
    }
}

impl StoredSandbox {
    /// Whether this sandbox has reached a status it can never leave on its own.
    ///
    /// Read from the instant the record carries rather than from the status string:
    /// `endTime` is set exactly when the sandbox stopped, so nothing has to keep a
    /// list of terminal status names in step with it.
    pub fn is_terminal(&self) -> bool {
        self.sandbox.end_time.is_some()
    }

    /// The minutes this sandbox waits before timing out, from the modelled member it
    /// carries: `Sandbox$timeoutInMinutes` is "How long, in minutes, from 5 to 2160 (36
    /// hours), for CodeBuild to wait before timing out this sandbox if it does not get
    /// marked as completed". A record that carries none falls back to the build default
    /// AWS documents for a project, [`DEFAULT_BUILD_TIMEOUT_MINUTES`], which is the
    /// value `CreateProject` stores when the caller omits one.
    pub fn timeout_minutes(&self) -> i32 {
        self.sandbox
            .timeout_in_minutes
            .unwrap_or(DEFAULT_BUILD_TIMEOUT_MINUTES)
    }

    /// Epoch millis at which an un-stopped sandbox times out: its documented
    /// `timeoutInMinutes` measured from the instant the session went live, since that
    /// is the wait "before timing out this sandbox if it does not get marked as
    /// completed".
    pub fn timeout_due_millis(&self) -> i64 {
        self.settle_due_millis
            .saturating_add(i64::from(self.timeout_minutes()).saturating_mul(MILLIS_PER_MINUTE))
    }

    /// Materialise the transitions the clock already owes this sandbox.
    ///
    /// Idempotent and purely a function of the stored instants, so repeating it (or
    /// skipping a read entirely) changes nothing:
    ///
    /// * before `settle_due_millis` the sandbox is still `QUEUED`;
    /// * from `settle_due_millis` it has been provisioned and its session is live —
    ///   [`SANDBOX_STATUS_RUNNING`], sitting in the [`SANDBOX_PHASE_RUN_SANDBOX`]
    ///   phase, which is where it STAYS, because a sandbox is a live session rather
    ///   than a job with an outcome;
    /// * from [`Self::timeout_due_millis`] it has run past its own
    ///   `timeoutInMinutes` without being marked completed, so it times out.
    ///
    /// A sandbox a caller already stopped is terminal and is left alone.
    pub fn settle(&mut self, now_millis: i64) {
        if self.is_terminal() || now_millis < self.settle_due_millis {
            return;
        }
        self.run();
        let timed_out_at = self.timeout_due_millis();
        if now_millis >= timed_out_at {
            self.end(SANDBOX_STATUS_TIMED_OUT, timed_out_at);
        }
    }

    /// Move this sandbox to the live state its provisioning span ended in.
    fn run(&mut self) {
        let requested = self.request_millis;
        let started = self.settle_due_millis;
        self.sandbox.status = Some(SANDBOX_STATUS_RUNNING.to_string());
        self.sandbox.start_time = Some(datetime(started));
        if let Some(session) = self.sandbox.current_session.as_mut() {
            session.status = Some(SANDBOX_STATUS_RUNNING.to_string());
            session.current_phase = Some(SANDBOX_PHASE_RUN_SANDBOX.to_string());
            session.end_time = None;
            session.phases = Some(running_session_phases(requested, started));
        }
    }

    /// End this sandbox at `ended_millis` with the terminal `status` the session's
    /// open phase took — `TIMED_OUT` for the timeout the clock owed it, `STOPPED` for
    /// a `StopSandbox`. The phase that takes the status is whichever one the sandbox
    /// was sitting in: the queue if it never got provisioned, the live session if it
    /// did.
    pub fn end(&mut self, status: &str, ended_millis: i64) {
        if self.is_terminal() {
            return;
        }
        let requested = self.request_millis;
        let started = self.settle_due_millis;
        let was_running = ended_millis >= started;
        self.sandbox.status = Some(status.to_string());
        self.sandbox.end_time = Some(datetime(ended_millis));
        if let Some(session) = self.sandbox.current_session.as_mut() {
            session.status = Some(status.to_string());
            session.current_phase = Some(PHASE_COMPLETED.to_string());
            session.end_time = Some(datetime(ended_millis));
            session.phases = Some(if was_running {
                ended_session_phases(requested, started, ended_millis, status)
            } else {
                left_the_queue_session_phases(requested, ended_millis, status)
            });
        }
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
/// Derived by [`derived_uuid`] from the project, the `buildNumber` just allocated
/// for it under the state lock, and the instant it started, so no two builds of a
/// project can collide.
pub fn build_uuid(project_name: &str, build_number: i64, start_millis: i64) -> String {
    derived_uuid("build", project_name, build_number, start_millis)
}

/// A batch build's unique identifier — the UUID AWS puts after `<projectName>:`.
///
/// [`build_uuid`]'s derivation, in its own namespace: the digest is taken over a
/// `build-batch` tag as well as the project, the `buildBatchNumber` just allocated
/// and the start instant, so a batch build and a build of the same project that were
/// numbered alike in the same millisecond still carry different ids.
pub fn build_batch_uuid(project_name: &str, build_batch_number: i64, start_millis: i64) -> String {
    derived_uuid("build-batch", project_name, build_batch_number, start_millis)
}

/// A sandbox's unique identifier — the UUID that follows `<projectName>:`, derived
/// exactly as [`build_uuid`] is, from the project, the sandbox sequence number just
/// allocated for it under the state lock, and the instant it was requested.
pub fn sandbox_uuid(project_name: &str, sandbox_number: i64, request_millis: i64) -> String {
    derived_uuid("sandbox", project_name, sandbox_number, request_millis)
}

/// A sandbox session's unique identifier, derived from the sandbox it belongs to,
/// the session's sequence number within that sandbox and the instant it started.
pub fn sandbox_session_uuid(sandbox_id: &str, session_number: i64, start_millis: i64) -> String {
    derived_uuid("sandbox-session", sandbox_id, session_number, start_millis)
}

/// An RFC 9562 version-8 (custom, hash-based) UUID over the values that already make
/// a resource unique: what KIND of resource it is, the name it belongs to, the
/// sequence number allocated for it under the state lock, and the instant it was
/// requested at.
///
/// DERIVED, not drawn from a generator, so no two resources of one kind and name can
/// collide, and the result is the 8-4-4-4-12 hex form a client parses. The digest is
/// SHA-256 (`sha2`), the same one [`request_fingerprint`] uses, so this needs no
/// dependency the crate does not already carry. `namespace` keeps two kinds of
/// resource with the same name and sequence number apart.
fn derived_uuid(namespace: &str, name: &str, sequence: i64, millis: i64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(namespace.as_bytes());
    // A separator, so ("a", 11) and ("a1", 1) cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(name.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    digest.update(millis.to_be_bytes());
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

/// `arn:aws:codebuild:<region>:<account>:build-batch/<id>`.
///
/// `build-batch` is CodeBuild's own resource type for a batch build, which is
/// how its ARNs are formed and how an IAM policy names one.
pub fn build_batch_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("build-batch/{id}"))
}

/// `arn:aws:codebuild:<region>:<account>:report-group/<name>`.
pub fn report_group_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("report-group/{name}"))
}

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id, a request path and a resource name are all
/// caller-controlled, so an unbounded echo would let one request emit an
/// arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 256;

/// What the `resource` field reads as when the call resolved no resource.
pub const AUDIT_NO_RESOURCE: &str = "-";

/// A request-derived value as it may appear in an audit record: control
/// characters (above all CR and LF, which would forge a second record) replaced,
/// and the whole value bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|character| if character.is_control() { '.' } else { character })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A report group was minted and stored.
    ReportGroupCreated,
    /// The request was refused as invalid (400 `InvalidInputException`).
    ValidationRefused,
    /// A resource of that name already exists (400
    /// `ResourceAlreadyExistsException`).
    AlreadyExists,
    /// An account quota — the service's, or this emulator's own ceiling — was met
    /// (400 `AccountLimitExceededException`).
    QuotaExceeded,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReportGroupCreated => "report-group-created",
            Self::ValidationRefused => "validation-refused",
            Self::AlreadyExists => "already-exists",
            Self::QuotaExceeded => "quota-exceeded",
        }
    }
}

/// One structured audit record for a state-changing call: which operation, the
/// identity the request presented, what it was addressed to, the resource it
/// resolved and the outcome. The `tracing` subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given
/// one: the request's parts carry its method, URI, headers and extensions, and
/// nothing in this framework puts a peer address in them, so the presented
/// credential is what identifies the caller. Never the credential itself — only
/// the access key id — and deliberately metadata only: no tag, buildspec or
/// export destination is logged, so a caller's own data never reaches the log.
/// Every interpolated value is request-derived, so [`audit_field`] bounds it and
/// replaces the control characters that would otherwise let one request forge a
/// second record.
pub fn audit(
    operation: &str,
    parts: &::http::request::Parts,
    outcome: AuditOutcome,
    resource: Option<&str>,
) {
    ::tracing::info!(
        target: "emulator::audit",
        service = ARN_NAMESPACE,
        operation = %audit_field(operation),
        access_key_id = %audit_field(&request_initiator(parts)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        resource = %audit_field(resource.unwrap_or(AUDIT_NO_RESOURCE)),
        outcome = outcome.as_str(),
        "codebuild api call"
    );
}

/// `arn:aws:codebuild:<region>:<account>:token/<server type, lowercased>` — the
/// token ARN `ImportSourceCredentials` returns and `DeleteSourceCredentials`
/// takes, as AWS documents it for the CLI
/// (`--arn arn:aws:codebuild:us-west-2:<account>:token/github`).
///
/// The server type is the only request value in it, which is why a credential is
/// held per server type.
pub fn source_credential_arn(region: &str, server_type: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("token/{}", server_type.to_ascii_lowercase()),
    )
}

/// `arn:aws:codebuild:<region>:<account>:sandbox/<id>`.
pub fn sandbox_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("sandbox/{id}"))
}

/// The session a just-requested sandbox carries: it was submitted and is waiting in
/// the queue, so it has no outcome, no log location (a log stream is created with the
/// container) and no network interface (one is attached when the container joins the
/// project's VPC) yet.
///
/// `resolvedSourceVersion` stays absent for the same reason it does on a build:
/// nothing clones a repository here, so there is no revision to report.
pub fn queued_sandbox_session(id: String, requested_millis: i64) -> SandboxSession {
    SandboxSession {
        id: Some(id),
        status: Some(SANDBOX_STATUS_QUEUED.to_string()),
        start_time: Some(datetime(requested_millis)),
        end_time: None,
        current_phase: Some(PHASE_QUEUED.to_string()),
        phases: Some(queued_session_phases(requested_millis)),
        resolved_source_version: None,
        logs: None,
        network_interface: None,
    }
}

/// The phases of a sandbox session that is still queued: it was submitted, and it is
/// waiting in the queue — the sandbox counterpart of [`in_progress_phases`].
fn queued_session_phases(requested_millis: i64) -> Vec<SandboxSessionPhase> {
    vec![
        finished_session_phase(
            PHASE_SUBMITTED,
            PHASE_SUCCEEDED,
            requested_millis,
            requested_millis,
            None,
        ),
        open_session_phase(PHASE_QUEUED, requested_millis),
    ]
}

/// The phases of a sandbox session that is LIVE: it was submitted, it waited in the
/// queue, its environment was provisioned, and it now sits in the `RUN_SANDBOX` phase
/// — the open phase it stays in until the sandbox is stopped or times out.
fn running_session_phases(requested_millis: i64, started_millis: i64) -> Vec<SandboxSessionPhase> {
    vec![
        finished_session_phase(
            PHASE_SUBMITTED,
            PHASE_SUCCEEDED,
            requested_millis,
            requested_millis,
            None,
        ),
        finished_session_phase(
            PHASE_QUEUED,
            PHASE_SUCCEEDED,
            requested_millis,
            started_millis,
            None,
        ),
        finished_session_phase(
            PHASE_PROVISIONING,
            PHASE_SUCCEEDED,
            started_millis,
            started_millis,
            None,
        ),
        open_session_phase(SANDBOX_PHASE_RUN_SANDBOX, started_millis),
    ]
}

/// The phases of a sandbox session that was live and has ended: the `RUN_SANDBOX`
/// phase closes with the status the session ended in (`TIMED_OUT` or `STOPPED`), and
/// the terminal `COMPLETED` marker opens at that instant.
fn ended_session_phases(
    requested_millis: i64,
    started_millis: i64,
    ended_millis: i64,
    status: &str,
) -> Vec<SandboxSessionPhase> {
    let mut phases = running_session_phases(requested_millis, started_millis);
    phases.pop();
    phases.push(finished_session_phase(
        SANDBOX_PHASE_RUN_SANDBOX,
        status,
        started_millis,
        ended_millis,
        None,
    ));
    phases.push(open_session_phase(PHASE_COMPLETED, ended_millis));
    phases
}

/// The phases of a sandbox session that ended before it was ever provisioned: the
/// queue is the phase that takes the ending status, exactly as [`stopped_phases`]
/// reports it for a build stopped while queued.
fn left_the_queue_session_phases(
    requested_millis: i64,
    ended_millis: i64,
    status: &str,
) -> Vec<SandboxSessionPhase> {
    vec![
        finished_session_phase(
            PHASE_SUBMITTED,
            PHASE_SUCCEEDED,
            requested_millis,
            requested_millis,
            None,
        ),
        finished_session_phase(PHASE_QUEUED, status, requested_millis, ended_millis, None),
        open_session_phase(PHASE_COMPLETED, ended_millis),
    ]
}

/// A sandbox session phase that ran and ended, with its measured duration.
fn finished_session_phase(
    phase_type: &str,
    phase_status: &str,
    start_millis: i64,
    end_millis: i64,
    contexts: Option<Vec<PhaseContext>>,
) -> SandboxSessionPhase {
    SandboxSessionPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: Some(phase_status.to_string()),
        start_time: Some(datetime(start_millis)),
        end_time: Some(datetime(end_millis)),
        duration_in_seconds: Some((end_millis - start_millis) / 1000),
        contexts,
    }
}

/// A sandbox session phase that has started and carries no outcome yet — including
/// the final `COMPLETED` marker phase, the way a build reports it.
fn open_session_phase(phase_type: &str, start_millis: i64) -> SandboxSessionPhase {
    SandboxSessionPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: None,
        start_time: Some(datetime(start_millis)),
        end_time: None,
        duration_in_seconds: None,
        contexts: None,
    }
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

/// The phases of a batch build that is still `IN_PROGRESS`: it was submitted,
/// and CodeBuild is fetching the batch build specification.
///
/// `BuildBatchPhaseType` is its own enum — `SUBMITTED`, `DOWNLOAD_BATCHSPEC`,
/// `IN_PROGRESS`, `COMBINE_ARTIFACTS`, `SUCCEEDED`, `FAILED`, `STOPPED` — so a
/// batch's phases are built from these rather than from a build's.
pub fn in_progress_batch_phases(start_millis: i64) -> Vec<BuildBatchPhase> {
    vec![
        finished_batch_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        open_batch_phase("DOWNLOAD_BATCHSPEC", start_millis),
    ]
}

/// The phases of a batch build that has reached its terminal state: downloading
/// the batch build specification is where an emulated batch stops, because
/// nothing fetches the project's source (see
/// [`BATCH_SPEC_NOT_DOWNLOADED_MESSAGE`]).
fn terminal_batch_phases(start_millis: i64, due_millis: i64) -> Vec<BuildBatchPhase> {
    vec![
        finished_batch_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        finished_batch_phase(
            "DOWNLOAD_BATCHSPEC",
            "FAULT",
            start_millis,
            due_millis,
            Some(vec![PhaseContext {
                status_code: None,
                message: Some(BATCH_SPEC_NOT_DOWNLOADED_MESSAGE.to_string()),
            }]),
        ),
        open_batch_phase("FAILED", due_millis),
    ]
}

/// [`finished_phase`], for a batch build's phase list.
fn finished_batch_phase(
    phase_type: &str,
    phase_status: &str,
    start_millis: i64,
    end_millis: i64,
    contexts: Option<Vec<PhaseContext>>,
) -> BuildBatchPhase {
    BuildBatchPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: Some(phase_status.to_string()),
        start_time: Some(datetime(start_millis)),
        end_time: Some(datetime(end_millis)),
        duration_in_seconds: Some((end_millis - start_millis) / 1000),
        contexts,
    }
}

/// [`open_phase`], for a batch build's phase list.
fn open_batch_phase(phase_type: &str, start_millis: i64) -> BuildBatchPhase {
    BuildBatchPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: None,
        start_time: Some(datetime(start_millis)),
        end_time: None,
        duration_in_seconds: None,
        contexts: None,
    }
}

/// A started build's view of its output artifacts: the configured destination, with
/// the checksums and the resolved location left absent because nothing was uploaded.
///
/// Shared by `StartBuild` and `StartBuildBatch`, which project the same
/// `ProjectArtifacts` onto the `BuildArtifacts` their records carry.
pub fn build_artifacts_of(artifacts: ProjectArtifacts) -> BuildArtifacts {
    BuildArtifacts {
        location: artifacts.location,
        sha256sum: None,
        md5sum: None,
        override_artifact_name: artifacts.override_artifact_name,
        encryption_disabled: artifacts.encryption_disabled,
        artifact_identifier: artifacts.artifact_identifier,
        bucket_owner_access: artifacts.bucket_owner_access,
    }
}

/// `defined` with `overrides` applied: an override replaces the variable of the same
/// name IN PLACE and a new name is appended in the order the request listed it, which
/// is what both `StartBuild` and `StartBuildBatch` document for
/// `environmentVariablesOverride` ("override, or add to, the environment variables
/// defined in the ... project").
///
/// The merge is keyed by a name -> position index rather than a scan per entry, so it
/// stays linearithmic in the number of variables; a handler has already refused a
/// request past [`MAX_ENVIRONMENT_VARIABLES`] before calling this.
pub fn merge_environment_variables(
    mut defined: Vec<EnvironmentVariable>,
    overrides: Vec<EnvironmentVariable>,
) -> Vec<EnvironmentVariable> {
    let mut position_of: BTreeMap<String, usize> = defined
        .iter()
        .enumerate()
        .map(|(position, held)| (held.name.clone(), position))
        .collect();
    for variable in overrides {
        match position_of.get(&variable.name) {
            Some(&position) => defined[position] = variable,
            None => {
                position_of.insert(variable.name.clone(), defined.len());
                defined.push(variable);
            }
        }
    }
    defined
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
    page_limited(items, next_token, None)
}

/// One page of `items` of at most `max_results` entries, resuming after the entry
/// `next_token` names.
///
/// [`page`] with the page size a caller may ask for, for the list operations whose
/// input models a `maxResults`. `None` leaves the page at this emulator's
/// [`MAX_PAGE_SIZE`], which is also the ceiling a requested size is held to — the
/// model's `PageSize` is `@range(min: 1, max: 100)`, so the boundary walk has
/// already refused anything outside it and the clamp is only belt and braces.
pub fn page_limited(
    items: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<(Vec<String>, Option<String>)> {
    let start = match next_token {
        None => 0,
        Some(token) => items.iter().position(|item| item == token)?,
    };
    let size = max_results.map_or(MAX_PAGE_SIZE, |requested| {
        (requested.max(0) as usize).clamp(1, MAX_PAGE_SIZE)
    });
    let end = (start + size).min(items.len());
    let next = items.get(end).cloned();
    Some((items[start..end].to_vec(), next))
}
