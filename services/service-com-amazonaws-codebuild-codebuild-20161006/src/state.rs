// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codebuild-codebuild-20161006 service.
//!
//! Every implemented CodeBuild operation computes its answer from the records
//! here: `CreateProject` writes a [`Project`] and `UpdateProjectVisibility`
//! rewrites that project's visibility, `CreateFleet` writes a
//! [`StoredFleet`], `CreateWebhook` writes that
//! project's [`crate::ops::types::Webhook`], `StartBuild` writes a
//! [`StoredBuild`], `StartBuildBatch` writes a [`StoredBuildBatch`],
//! `StartSandbox` writes a [`StoredSandbox`] and `StopSandbox` ends one,
//! `StartCommandExecution` writes a [`StoredCommandExecution`],
//! `CreateReportGroup` writes a [`StoredReportGroup`],
//! `ImportSourceCredentials` writes a [`StoredSourceCredentials`],
//! `BatchDeleteBuilds` removes a build, and the reporting operations
//! (`BatchGetProjects`, `BatchGetBuilds`, `BatchGetBuildBatches`,
//! `BatchGetFleets`, `BatchGetSandboxes`, `BatchGetCommandExecutions`,
//! `BatchGetReportGroups`, `ListProjects`, `ListBuildsForProject`,
//! `ListSandboxes`, `ListSandboxesForProject`, `ListSourceCredentials`) only
//! read them.
//!
//! The model shapes themselves are what is stored, so a read hands back exactly
//! the structure the write persisted rather than a second, drifting copy of the
//! same resource.

use std::collections::BTreeMap;

use crate::ops::types::{
    Build, BuildArtifacts, BuildBatch, BuildBatchPhase, BuildPhase, CommandExecution,
    EnvironmentVariable, Fleet, FleetStatus, PhaseContext, Project, ProjectArtifacts, ProjectCache,
    ProjectEnvironment, ProjectSource, ReportGroup, Sandbox, SandboxSession, SandboxSessionPhase,
    SourceCredentialsInfo,
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

/// How long an idempotency token stays in force. AWS documents the same lifetime
/// on every build operation that takes one: `StartBuildInput$idempotencyToken` —
/// "The token is included in the StartBuild request and is valid for 5 minutes",
/// `RetryBuildInput$idempotencyToken` — "The token is included in the RetryBuild
/// request and is valid for five minutes" — and
/// `StartBuildBatchInput$idempotencyToken` — "The token is included in the
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

/// Compute fleets this emulator holds for the account.
///
/// Real CodeBuild enforces an account quota on compute fleets per region and
/// answers `AccountLimitExceededException` once it is reached; `CreateFleet` does
/// the same here, which is also the ceiling that stops `fleets` growing without
/// bound as requests arrive. THE NUMBER IS THIS EMULATOR'S: AWS publishes a
/// per-region fleet quota that is adjustable per account, so there is no single
/// documented value to copy, and this one is far above any emulated workload.
pub const MAX_FLEETS: usize = 1_000;

/// The smallest value a `FleetCapacity` member may take.
///
/// Every member of that shape counts machines — `baseCapacity` is "the initial
/// number of machines allocated to the fleet" and `maxCapacity` "the maximum
/// number of instances in the fleet when auto-scaling" — so a fleet of fewer
/// than one machine cannot run a build. The model puts no `@range` on
/// `FleetCapacity`, so the boundary validator passes a zero or negative count
/// through and the service refuses it with the modelled
/// `InvalidInputException`.
pub const MIN_FLEET_CAPACITY: i32 = 1;

/// How long after `CreateFleet` the clock owes a fleet's transition out of
/// `CREATING`.
///
/// AWS does not document how long provisioning reserved capacity takes, so this
/// span is CHOSEN, inside the 250-750 ms band this project fixes for an emulated
/// transition. It is deliberately not zero: a caller that reads the fleet
/// straight after creating it still observes `CREATING`. The due instant is
/// stored, never slept on.
pub const FLEET_SETTLE_MILLIS: i64 = 500;

/// `FleetStatusCode` for a fleet whose capacity is still being provisioned.
pub const FLEET_STATUS_CREATING: &str = "CREATING";

/// `FleetStatusCode` for a fleet that finished provisioning and can take builds.
pub const FLEET_STATUS_ACTIVE: &str = "ACTIVE";

/// The `ComputeType` values AWS documents as requiring `computeConfiguration`:
/// "This is only required if `computeType` is set to `ATTRIBUTE_BASED_COMPUTE` or
/// `CUSTOM_INSTANCE_TYPE`" (`Fleet$computeConfiguration`).
pub const COMPUTE_TYPES_NEEDING_CONFIGURATION: [&str; 2] =
    ["ATTRIBUTE_BASED_COMPUTE", "CUSTOM_INSTANCE_TYPE"];

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

/// Idempotency entries retained at once, across every operation that takes a token.
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

/// The source types a project must have for `CreateWebhook` to connect repository
/// events to it.
///
/// CodeBuild documents the operation as enabling a rebuild "every time a code change
/// is pushed to the repository" for "an existing CodeBuild build project that has its
/// source code stored in a GitHub or Bitbucket repository", and `ScopeConfiguration`
/// extends the same surface to GitLab and GitLab Self Managed. A project whose source
/// is none of these (`CODECOMMIT`, `CODEPIPELINE`, `S3`, `NO_SOURCE`) has no provider
/// that sends repository events, so there is nothing to connect — the sole exception
/// being a runner webhook, see [`RUNNER_WEBHOOK_BUILD_TYPE`].
pub const WEBHOOK_SOURCE_TYPES: &[&str] = &[
    "BITBUCKET",
    "GITHUB",
    "GITHUB_ENTERPRISE",
    "GITLAB",
    "GITLAB_SELF_MANAGED",
];

/// The one `WebhookBuildType` that is NOT a repository-event webhook.
///
/// AWS documents it on `CreateWebhookInput$buildType`: "RUNNER_BUILDKITE_BUILD is only
/// available for NO_SOURCE source type projects configured for Buildkite runner
/// builds" — so this build type requires [`RUNNER_WEBHOOK_SOURCE_TYPE`] and is the only
/// way a `NO_SOURCE` project takes a webhook.
pub const RUNNER_WEBHOOK_BUILD_TYPE: &str = "RUNNER_BUILDKITE_BUILD";

/// The source type a runner webhook's project has — see [`RUNNER_WEBHOOK_BUILD_TYPE`].
pub const RUNNER_WEBHOOK_SOURCE_TYPE: &str = "NO_SOURCE";

/// The source type whose webhooks CodeBuild creates for the caller rather than handing
/// back values to register by hand.
///
/// AWS documents `manualCreation` as "only available for GitHub webhooks".
pub const MANUAL_CREATION_SOURCE_TYPE: &str = "GITHUB";

/// The source type that carries no webhook `secret`.
///
/// AWS documents it on `Webhook$secret`: "A Bitbucket webhook does not support
/// `secret`."
pub const SECRETLESS_WEBHOOK_SOURCE_TYPE: &str = "BITBUCKET";

/// The `WebhookFilter$type` every filter group must carry at least one of. AWS
/// documents it on `filterGroups`: "At least one WebhookFilter in the array must
/// specify EVENT as its type."
pub const EVENT_WEBHOOK_FILTER_TYPE: &str = "EVENT";

/// Characters in a webhook secret this emulator mints.
///
/// THE LENGTH IS THIS EMULATOR'S: AWS documents no length for `Webhook$secret` beyond
/// `@length(min: 1)`, only that it is "the secret token of the associated repository",
/// so a fixed, comfortably long value is chosen rather than derived from anything.
const WEBHOOK_SECRET_LEN: usize = 40;

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

/// Command-execution records this emulator retains, across every sandbox.
///
/// A command execution is minted by each `StartCommandExecution` rather than being a
/// resource the caller deletes, so the store needs a ceiling at that growth site:
/// past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_command_execution`] evicts
/// the oldest execution (preferring one that has already reached a terminal status),
/// which also prunes `command_executions_by_sandbox`. THIS NUMBER IS THE EMULATOR'S,
/// NOT AWS'S — it is the one [`MAX_SANDBOXES`] puts on the resource these belong to.
pub const MAX_COMMAND_EXECUTIONS: usize = MAX_SANDBOXES;

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

/// How long after `StartCommandExecution` the clock owes a command execution its move
/// out of [`COMMAND_EXECUTION_STATUS_IN_PROGRESS`].
///
/// AWS documents no duration for a command execution — it is however long the command
/// itself takes — so this span is CHOSEN, inside the 250-750 ms band this project
/// fixes for an emulated transition. It is deliberately not zero: a caller that reads
/// an execution straight after starting it still observes it in flight. The due
/// instant is stored, never slept on.
pub const COMMAND_EXECUTION_SETTLE_MILLIS: i64 = 500;

/// Why an emulated command execution never reports a result.
///
/// A command execution runs a shell command inside the sandbox's environment; nothing
/// in this emulator executes one — there is no shell, container or process here to
/// hand the command to — so the execution cannot report an exit code or any output. It
/// ends instead in CodeBuild's own build-system failure status,
/// [`COMMAND_EXECUTION_STATUS_FAULT`], carrying this explanation — the emulator's own,
/// never output attributed to the command — as the execution's error text.
pub const COMMAND_NOT_EXECUTED_MESSAGE: &str = "this emulator does not execute sandbox \
    commands, so nothing ran this command: it produced no output and no exit code";

/// The status a command execution reports between `StartCommandExecution` and the
/// transition the clock owes it.
///
/// The model types `CommandExecution$status` as a free-form `NonEmptyString`, so the
/// values reported for it here are the service's own `StatusType` enum values — the
/// same pair a build's `buildStatus` moves through in this emulator.
pub const COMMAND_EXECUTION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status an emulated command execution reaches, for the reason
/// [`COMMAND_NOT_EXECUTED_MESSAGE`] gives. `FAULT` is the `StatusType` value CodeBuild
/// reports when the build system itself could not run the work.
pub const COMMAND_EXECUTION_STATUS_FAULT: &str = "FAULT";

/// The `CommandType` a `StartCommandExecution` that names none can only mean.
///
/// The model declares exactly one value for `CommandType` — `SHELL`, the single entry
/// of [`crate::ops::types::COMMAND_TYPE_VALUES`] — so an omitted `type` is a request
/// for that one.
pub const DEFAULT_COMMAND_TYPE: &str = "SHELL";

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
    /// Compute fleets, keyed by fleet name (names are unique per account).
    pub fleets: BTreeMap<String, StoredFleet>,
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
    /// Command executions, keyed by command execution id.
    ///
    /// Keyed globally rather than per sandbox because that is how
    /// `BatchGetCommandExecutions` addresses one (an id, with the sandbox as the scope
    /// it must belong to), and the id is derived under the sandbox it was started in,
    /// so two sandboxes cannot mint the same one.
    pub command_executions: BTreeMap<String, StoredCommandExecution>,
    /// Command execution ids per sandbox id, in submit order — which is the order
    /// `ListCommandExecutionsForSandbox` pages over, so a list operation reads this
    /// without re-reading every execution.
    pub command_executions_by_sandbox: BTreeMap<String, Vec<String>>,
    /// The next per-sandbox command execution sequence number to hand out. It is not a
    /// modelled member: it is what makes an execution's derived id unique when two
    /// commands of one sandbox are submitted in the same millisecond.
    pub next_command_execution_numbers: BTreeMap<String, i64>,
    /// `RetryBuild` idempotency tokens, keyed by (project name, token).
    ///
    /// Kept apart from `start_build_tokens` for the reason
    /// `start_build_batch_tokens` is: the two operations issue their tokens
    /// independently, so the same token value may be in force for a build and
    /// for a retry of a build of one project, and neither may resolve the
    /// other's build.
    pub retry_build_tokens: BTreeMap<(String, String), StartBuildToken>,
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

/// A persisted compute fleet: the modelled [`Fleet`] plus the instants its
/// `created` / `lastModified` timestamps report and the instant at which the
/// clock owes its move out of `CREATING`.
#[derive(Debug, Clone)]
pub struct StoredFleet {
    /// The record every reader hands back.
    pub fleet: Fleet,
    /// Epoch millis the fleet was created at. Kept as the integer a list or sort
    /// operation works on, the way [`StoredProject`]'s is; `ListFleets` and
    /// `UpdateFleet`, which read and move it, are still scaffolded.
    pub created_millis: i64,
    /// Epoch millis the fleet was last changed at — written by `UpdateFleet`,
    /// which is still scaffolded, so for now it is the creation instant.
    pub last_modified_millis: i64,
    /// Epoch millis at which this fleet's move to `ACTIVE` became due.
    pub active_due_millis: i64,
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

/// A persisted command execution: the modelled [`CommandExecution`] plus the instant
/// its `submitTime` reports and the instant at which the clock owes its move out of
/// [`COMMAND_EXECUTION_STATUS_IN_PROGRESS`].
#[derive(Debug, Clone)]
pub struct StoredCommandExecution {
    /// The record every reader hands back.
    pub command_execution: CommandExecution,
    /// Epoch millis the command was submitted at — the same instant
    /// `commandExecution.submitTime` reports, kept as the integer the arithmetic uses.
    pub submit_millis: i64,
    /// Epoch millis at which this execution's terminal transition became due.
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

/// What one in-force `StartBuild`, `RetryBuild` or `StartBuildBatch` idempotency
/// token resolved to.
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

/// The same token, in force, carrying a DIFFERENT request than the one it was
/// issued for — the parameter mismatch AWS documents on every
/// `idempotencyToken` member. Each operation answers it with its own modelled
/// `InvalidInputException`, so this carries no message of its own.
#[derive(Debug, Clone, Copy)]
pub struct IdempotencyMismatch;

/// Which operation's idempotency store a lookup reads.
///
/// Each operation keeps its own store (the crate's convention, as
/// `start_build_batch_tokens` and `start_sandbox_tokens` do), so a token is only
/// ever resolved against the operation that issued it;
/// [`ComAmazonawsCodebuildCodebuild20161006State::resolve_idempotency_token`]
/// takes this so the documented five-minute / parameter-mismatch rule is written
/// once for the operations that mint a [`Build`].
#[derive(Debug, Clone, Copy)]
pub enum BuildTokenStore {
    /// `StartBuild`'s store (`start_build_tokens`).
    StartBuild,
    /// `RetryBuild`'s store (`retry_build_tokens`).
    RetryBuild,
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

    /// What an idempotency token already in force resolves to.
    ///
    /// AWS documents the token as valid for five minutes: a repeat of the SAME
    /// request inside that window resolves to the build the first one started
    /// (`Ok(Some(build))`), and the same token with a changed request is a
    /// parameter mismatch ([`IdempotencyMismatch`]), which AWS documents as a
    /// refusal rather than a second build. `Ok(None)` means this request has to
    /// start one — the token is absent, expired, or the build it named is no
    /// longer retained.
    ///
    /// `store` is the operation's own token store, so the rule is written once
    /// for the operations that mint a [`Build`] and still resolves a token only
    /// against the operation that issued it.
    ///
    /// Reads the build through [`Self::settled_build`], so a replay reports the
    /// build as of now rather than as of the first call.
    pub fn resolve_idempotency_token(
        &mut self,
        store: BuildTokenStore,
        key: Option<&(String, String)>,
        fingerprint: &str,
        now_millis: i64,
    ) -> Result<Option<Build>, IdempotencyMismatch> {
        let Some(key) = key else {
            return Ok(None);
        };
        let tokens = match store {
            BuildTokenStore::StartBuild => &self.start_build_tokens,
            BuildTokenStore::RetryBuild => &self.retry_build_tokens,
        };
        let in_force = tokens
            .get(key)
            .filter(|issued| now_millis - issued.issued_millis < IDEMPOTENCY_TOKEN_TTL_MILLIS)
            .cloned();
        let Some(issued) = in_force else {
            return Ok(None);
        };
        if issued.request_fingerprint != fingerprint {
            return Err(IdempotencyMismatch);
        }
        Ok(self
            .settled_build(&issued.build_id, now_millis)
            .map(|stored| stored.build.clone()))
    }

    /// Record the build an in-force `RetryBuild` idempotency token resolves to,
    /// under [`Self::record_start_build_token`]'s rule and in that operation's
    /// own store.
    pub fn record_retry_build_token(
        &mut self,
        key: (String, String),
        token: StartBuildToken,
        now_millis: i64,
    ) {
        record_token(
            &mut self.retry_build_tokens,
            key,
            token,
            now_millis,
            IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_IDEMPOTENCY_TOKENS,
        );
    }

    /// Link `retried_id`'s record forward to the build a retry of it started.
    ///
    /// AWS documents `AutoRetryConfig$nextAutoRetry` as "The build ARN of the
    /// auto-retried build triggered by the current build", so the record of the
    /// build that was retried is what carries it. A build that is no longer
    /// retained (evicted by [`MAX_BUILDS`]) is simply not linked.
    ///
    /// The member is singular, so a build retried more than once reports its
    /// LATEST retry here: each call replaces the link, and the chain is still
    /// walkable from the other end, where every retry keeps its own
    /// `previousAutoRetry`.
    pub fn link_next_auto_retry(&mut self, retried_id: &str, retry_arn: String) {
        let Some(stored) = self.builds.get_mut(retried_id) else {
            return;
        };
        let held = stored.build.auto_retry_config.take().unwrap_or_default();
        stored.build.auto_retry_config = Some(crate::ops::types::AutoRetryConfig {
            next_auto_retry: Some(retry_arn),
            ..held
        });
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

    /// Read a fleet, first materialising the transition the clock already owes
    /// it. Like [`Self::settled_build`] this is the only write a reporting
    /// operation makes: the move out of `CREATING` was due at
    /// `active_due_millis` whether or not anybody called, and it is computed
    /// from the stored instants, so repeating it changes nothing.
    ///
    /// `BatchGetFleets$names` takes "the names or ARNs of the compute fleets",
    /// so both forms resolve here. An identifier that is not an ARN is a fleet
    /// name; an ARN is resolved the way [`Self::report_group_by_arn`] resolves a
    /// report group's — the name comes out of the ARN's resource segment for the
    /// map lookup, and the candidate is kept only if its STORED ARN is exactly
    /// the one asked for, so an ARN for another partition, region, account,
    /// resource type or fleet id can never resolve to a local record.
    pub fn settled_fleet(&mut self, identifier: &str, now_millis: i64) -> Option<&StoredFleet> {
        let by_arn = identifier.starts_with("arn:");
        let name = if by_arn {
            // `fleet/<name>:<uuid>`: the name is what the store is keyed by.
            identifier.rsplit_once(":fleet/")?.1.split_once(':')?.0
        } else {
            identifier
        }
        .to_string();
        let stored = self.fleets.get_mut(&name)?;
        if by_arn && stored.fleet.arn.as_deref() != Some(identifier) {
            return None;
        }
        stored.settle(now_millis);
        Some(stored)
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

    /// Hand out `sandbox_id`'s next command execution sequence number and advance the
    /// counter. Like a `buildNumber`, a number is never reused for a sandbox.
    pub fn allocate_command_execution_number(&mut self, sandbox_id: &str) -> i64 {
        allocate_number(&mut self.next_command_execution_numbers, sandbox_id)
    }

    /// Persist `stored` under `id` and record it as the newest command execution of its
    /// sandbox, then bring the store back inside [`MAX_COMMAND_EXECUTIONS`]. The
    /// sibling of [`Self::insert_sandbox`], and the id is a parameter for the same
    /// reason: no caller can lose a record to an absent id.
    pub fn insert_command_execution(&mut self, id: String, stored: StoredCommandExecution) {
        if let Some(sandbox_id) = stored.command_execution.sandbox_id.clone() {
            self.command_executions_by_sandbox
                .entry(sandbox_id)
                .or_default()
                .push(id.clone());
        }
        self.command_executions.insert(id, stored);
        self.enforce_command_execution_ceiling();
    }

    /// Evict command executions until at most [`MAX_COMMAND_EXECUTIONS`] are retained,
    /// oldest terminal one first — the same rule [`Self::enforce_sandbox_ceiling`]
    /// applies to sandboxes, so an execution a caller may still be polling survives an
    /// eviction round. Evicting one also drops its id from
    /// `command_executions_by_sandbox` and retires the per-sandbox sequence counter
    /// once no execution of that sandbox is left.
    fn enforce_command_execution_ceiling(&mut self) {
        let mut evicted_any = false;
        while self.command_executions.len() > MAX_COMMAND_EXECUTIONS {
            let oldest_terminal = self
                .command_executions
                .iter()
                .filter(|(_, stored)| stored.is_terminal())
                .min_by_key(|(_, stored)| stored.submit_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_terminal.or_else(|| {
                self.command_executions
                    .iter()
                    .min_by_key(|(_, stored)| stored.submit_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            evicted_any = true;
            let sandbox_id = self
                .command_executions
                .remove(&id)
                .and_then(|stored| stored.command_execution.sandbox_id);
            if let Some(sandbox_id) = sandbox_id {
                if let Some(ids) = self.command_executions_by_sandbox.get_mut(&sandbox_id) {
                    ids.retain(|held| held != &id);
                    if ids.is_empty() {
                        self.command_executions_by_sandbox.remove(&sandbox_id);
                    }
                }
            }
        }
        if !evicted_any {
            return;
        }
        let mut counters = ::std::mem::take(&mut self.next_command_execution_numbers);
        counters.retain(|sandbox_id, _| {
            self.sandboxes.contains_key(sandbox_id)
                || self.command_executions_by_sandbox.contains_key(sandbox_id)
        });
        self.next_command_execution_numbers = counters;
    }

    /// Read a command execution of `sandbox_id`, first materialising the transition the
    /// clock already owes it — the command-execution counterpart of
    /// [`Self::settled_build`], and the only write a reporting operation makes: the
    /// move out of [`COMMAND_EXECUTION_STATUS_IN_PROGRESS`] was due at
    /// `settle_due_millis` whether or not anybody called, and it is computed from the
    /// stored instants, so repeating it changes nothing.
    ///
    /// `None` for an id this sandbox did not start, even when another sandbox did: a
    /// command execution is addressed within the sandbox it belongs to, so one
    /// sandbox's id never resolves against another's executions.
    pub fn settled_command_execution(
        &mut self,
        sandbox_id: &str,
        id: &str,
        now_millis: i64,
    ) -> Option<&StoredCommandExecution> {
        let stored = self.command_executions.get_mut(id)?;
        if stored.command_execution.sandbox_id.as_deref() != Some(sandbox_id) {
            return None;
        }
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

impl StoredFleet {
    /// Materialise this fleet's `ACTIVE` status once its due instant has passed.
    ///
    /// Idempotent and purely a function of the stored instants: a fleet that is
    /// not provisioning, or whose span has not elapsed, is left alone.
    pub fn settle(&mut self, now_millis: i64) {
        if now_millis < self.active_due_millis {
            return;
        }
        let provisioning = self
            .fleet
            .status
            .as_ref()
            .and_then(|status| status.status_code.as_deref())
            == Some(FLEET_STATUS_CREATING);
        if !provisioning {
            return;
        }
        self.fleet.status = Some(FleetStatus {
            status_code: Some(FLEET_STATUS_ACTIVE.to_string()),
            // An `ACTIVE` fleet carries no `FleetContextCode`: every value of
            // that enum explains a failure or a pending deletion.
            context: None,
            message: None,
        });
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

    /// Whether this sandbox's session is LIVE — provisioned, not ended — which is what
    /// a command execution or a connection needs to act on.
    ///
    /// Read off the record's own status, so it is true exactly for the sandbox
    /// [`Self::settle`] has moved to [`SANDBOX_STATUS_RUNNING`]: one still in the queue
    /// has no session yet, and one that stopped or timed out no longer has one. Call it
    /// through [`ComAmazonawsCodebuildCodebuild20161006State::settled_sandbox`], so the
    /// transitions the clock owes the sandbox are materialised before it is judged.
    pub fn is_running(&self) -> bool {
        !self.is_terminal() && self.sandbox.status.as_deref() == Some(SANDBOX_STATUS_RUNNING)
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

impl StoredCommandExecution {
    /// Whether this command execution has reached a status it can never leave.
    ///
    /// Read from the instant the record carries rather than from the status string,
    /// the way [`StoredSandbox::is_terminal`] is: `endTime` is set exactly when the
    /// execution stopped, so nothing has to keep a list of terminal status names in
    /// step with it.
    pub fn is_terminal(&self) -> bool {
        self.command_execution.end_time.is_some()
    }

    /// Materialise this command execution's terminal state once its due instant has
    /// passed.
    ///
    /// Idempotent and purely a function of the stored instants: an execution that has
    /// already ended, or whose span has not elapsed, is left alone. It ends in
    /// [`COMMAND_EXECUTION_STATUS_FAULT`] because nothing here ran the command — see
    /// [`COMMAND_NOT_EXECUTED_MESSAGE`] — and therefore reports neither an
    /// `exitCode` nor any `standardOutputContent`: no process exited and no command
    /// wrote anything.
    ///
    /// `startTime` and `endTime` are both the due instant: the attempt was made and
    /// given up there, so the record carries no run duration for work nothing
    /// performed.
    pub fn settle(&mut self, now_millis: i64) {
        if self.is_terminal() || now_millis < self.settle_due_millis {
            return;
        }
        let due = self.settle_due_millis;
        self.command_execution.status = Some(COMMAND_EXECUTION_STATUS_FAULT.to_string());
        self.command_execution.start_time = Some(datetime(due));
        self.command_execution.end_time = Some(datetime(due));
        self.command_execution.standard_err_content =
            Some(COMMAND_NOT_EXECUTED_MESSAGE.to_string());
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

/// A build's unique identifier — the UUID AWS puts after `<projectName>:`, and
/// (through [`webhook_payload_url`]) the opaque token in a webhook's payload URL.
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

/// A command execution's unique identifier, derived from the sandbox it was submitted
/// to, the per-sandbox sequence number allocated for it under the state lock, and the
/// instant it was submitted — exactly as [`sandbox_uuid`] is derived for a sandbox.
pub fn command_execution_uuid(sandbox_id: &str, command_number: i64, submit_millis: i64) -> String {
    derived_uuid(
        "command-execution",
        sandbox_id,
        command_number,
        submit_millis,
    )
}

/// The store key for one sandbox a request names.
///
/// Every operation that takes a sandbox takes "a sandboxId or sandboxArn"
/// (`StartCommandExecution$sandboxId`, `BatchGetSandboxes$ids`), so an ARN names the
/// same sandbox its id does: everything up to and including the `sandbox/` resource
/// segment is dropped. Only a value in ARN form is rewritten, so a project literally
/// named `sandbox` (whose sandbox ids read `sandbox:<uuid>`) is still looked up as it
/// stands.
pub fn sandbox_store_id(entry: &str) -> &str {
    if !entry.starts_with("arn:") {
        return entry;
    }
    entry.rsplit_once(":sandbox/").map_or(entry, |(_, id)| id)
}

/// A compute fleet's unique identifier — the UUID AWS puts after `<fleetName>:`.
///
/// [`build_uuid`]'s derivation in its own namespace, over the values that already
/// make a fleet unique: its name, which identifies one fleet per account at any
/// instant, and the instant it was created. A fleet is addressed by that name
/// rather than by a per-name sequence number, so it has none to derive from —
/// which also means a name deleted and recreated inside the SAME millisecond
/// derives the same id again; once `DeleteFleet` exists it needs a per-name
/// counter here, the way `allocate_build_number` feeds [`build_uuid`].
pub fn fleet_uuid(name: &str, created_millis: i64) -> String {
    derived_uuid("fleet", name, NO_SEQUENCE_NUMBER, created_millis)
}

/// The `sequence` [`derived_uuid`] takes for a resource that has no per-name
/// sequence number of its own, because its name already identifies it.
const NO_SEQUENCE_NUMBER: i64 = 0;

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

/// Whether `value` has the six colon-separated fields of an ARN,
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — the syntactic
/// check an operation that takes an ARN applies before it looks anything up.
///
/// The region and account fields may be empty (a global or account-less
/// resource); the partition, service and resource fields may not.
pub fn is_arn(value: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next().is_some_and(|partition| !partition.is_empty())
        && fields.next().is_some_and(|service| !service.is_empty())
        && fields.next().is_some()
        && fields.next().is_some()
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

/// Whether an auth type's `token` is another resource's ARN rather than a secret:
/// the two [`RESOURCE_ARN_AUTH_TYPES`], read as a predicate.
///
/// The model says so twice: `ImportSourceCredentialsInput$token` — "For the
/// authType CODECONNECTIONS, this is the connectionArn. For the authType
/// SECRETS_MANAGER, this is the secretArn." — and
/// `SourceCredentialsInfo$resource` — "The connection ARN if your authType is
/// CODECONNECTIONS or SECRETS_MANAGER." For every other auth type the token is a
/// password or an access token, which no operation reports.
pub fn token_is_resource_arn(auth_type: &str) -> bool {
    RESOURCE_ARN_AUTH_TYPES.contains(&auth_type)
}

/// `arn:aws:codebuild:<region>:<account>:build/<id>`.
pub fn build_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("build/{id}"))
}

/// The project name an `arn:<partition>:codebuild:<region>:<account>:project/<name>`
/// ARN identifies, or `None` when the value is not a CodeBuild project ARN.
///
/// The inverse of [`project_arn`]'s resource segment, for an operation that addresses
/// a project by ARN rather than by name: the name is what this store is keyed by. It
/// answers the SYNTAX question only — whether the value is a project ARN of this
/// service at all — so a caller that resolves a record still keeps it only if its
/// STORED ARN is exactly the one asked for, the way
/// [`ComAmazonawsCodebuildCodebuild20161006State::report_group_by_arn`] does; that is
/// what keeps an ARN for another partition, region or account from resolving to a
/// local record. A project name cannot contain `:` (the model's `@pattern` on
/// `CreateProjectInput$name`), so the six ARN fields split cleanly.
pub fn project_name_from_arn(arn: &str) -> Option<&str> {
    // Walked field by field rather than collected, so resolving an ARN allocates
    // nothing.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" || fields.next()?.is_empty() || fields.next()? != ARN_NAMESPACE {
        return None;
    }
    // The region and the account are not checked here; see above.
    fields.next()?;
    fields.next()?;
    let name = fields.next()?.strip_prefix("project/")?;
    (!name.is_empty()).then_some(name)
}

/// A public project's `publicProjectAlias` — "the project identifier used with the
/// public build APIs".
///
/// DERIVED, not drawn from a generator, for the same reason [`build_uuid`] is: AWS
/// documents no format for the alias (the model types it only as `NonEmptyString`), so
/// this emulator mints it as the first 16 bytes of the SHA-256 of the project's ARN,
/// hex-encoded. It is therefore a function of the project alone — stable across reads,
/// the same again when a project is re-published, and distinct for distinct projects —
/// rather than a value that changes per run. The digest is SHA-256 (`sha2`), already a
/// dependency of this crate.
pub fn public_project_alias(project_arn: &str) -> String {
    use ::sha2::Digest;
    ::hex::encode(&::sha2::Sha256::digest(project_arn.as_bytes())[..16])
}

/// `arn:aws:codebuild:<region>:<account>:fleet/<id>`, where `id` is a fleet's
/// `<name>:<uuid>` identifier — the form `UpdateFleet` and `DeleteFleet` take.
pub fn fleet_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("fleet/{id}"))
}

/// The CodeBuild endpoint a project's webhook events are sent to, as
/// `Webhook$payloadUrl` reports it: the regional CodeBuild webhook endpoint carrying
/// the opaque token that identifies this webhook, in AWS's `?t=<token>&v=1` form.
///
/// The token is DERIVED, not drawn from a generator: an RFC 9562 version-8 UUID over
/// the values that already make a webhook unique — the account, the region, the project
/// it belongs to and the instant it was created — through the same
/// [`build_uuid`] construction a build id uses. So two webhooks never share a payload
/// URL, re-reading the stored webhook reports the same URL, and nothing about the URL
/// is read back out of the request.
pub fn webhook_payload_url(region: &str, project_name: &str, created_millis: i64) -> String {
    // The project name goes through the digest rather than into the URL, so the token
    // stays opaque the way AWS's does.
    let token = build_uuid(
        &format!(
            "{}:{region}:webhook:{project_name}",
            ::emulator_server::emulator_account_id()
        ),
        0,
        created_millis,
    );
    format!("https://codebuild.{region}.amazonaws.com/webhooks?t={token}&v=1")
}

/// A fresh webhook secret, for a webhook whose provider supports one.
///
/// The value is a secret handed to a caller — the repository provider signs its
/// payloads with it — but it is NOT a credential of this emulator and must never be
/// accepted as one. Nothing in a service may construct a secret itself, so the value is
/// cut from a secret minted by the one component that generates secrets, the emulator's
/// credential store, which is then removed from that store again: the caller gets the
/// secret, the emulator keeps no credential it would accept, and nothing accumulates
/// per call. Only the alphanumeric characters of that secret are kept, the way a
/// provider's webhook secret reads.
///
/// `None` when the store minted no secret, or one with fewer than
/// [`WEBHOOK_SECRET_LEN`] usable characters, which leaves the operation to refuse
/// rather than report a value it made up.
pub fn mint_webhook_secret(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let secret: String = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(WEBHOOK_SECRET_LEN)
        .collect();
    // The webhook secret is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    (secret.chars().count() == WEBHOOK_SECRET_LEN).then_some(secret)
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
    /// A compute fleet was minted and stored.
    FleetCreated,
    /// The request was refused as invalid (400 `InvalidInputException`).
    ValidationRefused,
    /// A resource of that name already exists (400
    /// `ResourceAlreadyExistsException`).
    AlreadyExists,
    /// An account quota — the service's, or this emulator's own ceiling — was met
    /// (400 `AccountLimitExceededException`).
    QuotaExceeded,
    /// A build project was minted and stored.
    ProjectCreated,
    /// A build project's settings were changed and stored.
    ProjectUpdated,
    /// No resource of that name exists (400 `ResourceNotFoundException`).
    NotFound,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReportGroupCreated => "report-group-created",
            Self::FleetCreated => "fleet-created",
            Self::ValidationRefused => "validation-refused",
            Self::AlreadyExists => "already-exists",
            Self::QuotaExceeded => "quota-exceeded",
            Self::ProjectCreated => "project-created",
            Self::ProjectUpdated => "project-updated",
            Self::NotFound => "not-found",
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

/// The `CacheType` a run whose project configures no cache reports.
///
/// `ProjectCache$type` documents this value as the one that means no cache is
/// used at all: "NO_CACHE: The build project does not use any cache." A started
/// build or batch build reports the configuration it RUNS WITH, so the absence of
/// a cache is reported as this cache type rather than as an absent member.
pub const CACHE_TYPE_NO_CACHE: &str = "NO_CACHE";

/// The `ImagePullCredentialsType` a run whose environment names none pulls its
/// image with.
///
/// `ProjectEnvironment$imagePullCredentialsType` documents the choice between its
/// two values by what the image requires: "When you use a cross-account or private
/// registry image, you must use SERVICE_ROLE credentials. When you use an CodeBuild
/// curated image, you must use CODEBUILD credentials." An environment that names
/// no `registryCredential` is pulling a curated image, which is this value.
pub const DEFAULT_IMAGE_PULL_CREDENTIALS_TYPE: &str = "CODEBUILD";

/// Whether an unconfigured `ProjectSource$insecureSsl` flag is on.
///
/// The member is the opt-in "Enable this flag to ignore SSL warnings while
/// connecting to the project source code", so a source that does not set it runs
/// with it off. A run reports its effective value rather than omitting the member.
pub const DEFAULT_INSECURE_SSL: bool = false;

/// Whether an unconfigured `ProjectEnvironment$privilegedMode` flag is on.
///
/// AWS documents the default on that member: "Enables running the Docker daemon
/// inside a Docker container. ... The default setting is `false`."
pub const DEFAULT_PRIVILEGED_MODE: bool = false;

/// The KMS key a run encrypts its output artifacts with when nothing named one:
/// `arn:aws:kms:<region>:<account>:alias/aws/s3`.
///
/// AWS documents the default on `Project$encryptionKey`: "If you don't specify a
/// value, CodeBuild uses the managed CMK for Amazon Simple Storage Service (Amazon
/// S3)", which is that service's managed alias.
pub fn default_encryption_key(region: &str) -> String {
    ::emulator_server::emulator_arn("kms", region, "alias/aws/s3")
}

/// `cache` as a started run reports it: the configured cache, or the explicit
/// [`CACHE_TYPE_NO_CACHE`] cache of a project that configures none.
pub fn effective_cache(cache: Option<ProjectCache>) -> ProjectCache {
    cache.unwrap_or_else(|| ProjectCache {
        r#type: CACHE_TYPE_NO_CACHE.to_string(),
        location: None,
        modes: None,
        cache_namespace: None,
    })
}

/// `source` as a started run reports it: every flag resolved to the value the run
/// actually uses.
///
/// Shared by `StartBuild` and `StartBuildBatch` (and so by every read of the
/// records they persist), which resolve the same `ProjectSource` with their own
/// `*Override` members already applied.
pub fn effective_source(mut source: ProjectSource) -> ProjectSource {
    source.insecure_ssl = Some(source.insecure_ssl.unwrap_or(DEFAULT_INSECURE_SSL));
    source
}

/// `environment` as a started run reports it: the documented defaults resolved,
/// and the variable list present even when the project defines none.
pub fn effective_environment(mut environment: ProjectEnvironment) -> ProjectEnvironment {
    environment.privileged_mode =
        Some(environment.privileged_mode.unwrap_or(DEFAULT_PRIVILEGED_MODE));
    environment.image_pull_credentials_type = Some(
        environment
            .image_pull_credentials_type
            .unwrap_or_else(|| DEFAULT_IMAGE_PULL_CREDENTIALS_TYPE.to_string()),
    );
    environment.environment_variables =
        Some(environment.environment_variables.unwrap_or_default());
    environment
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

/// The `ProjectSource$type` value of a project that has no input source code. One of
/// `SourceType`'s modelled values (see `crate::ops::types::SOURCE_TYPE_VALUES`); the
/// same value [`RUNNER_WEBHOOK_SOURCE_TYPE`] names for the unrelated webhook rule.
pub const NO_SOURCE_TYPE: &str = "NO_SOURCE";

/// Why `source` cannot be a build project's source configuration, as the message the
/// modelled `InvalidInputException` carries — or `None` when it can.
///
/// The model documents `NO_SOURCE` as "The project does not have input source code",
/// and `ProjectSource$buildspec` as: "If this value is not provided or is set to an
/// empty string, the source code must contain a buildspec file in its root directory."
/// A project with no input source code has no root directory for that file to live in,
/// so its buildspec has to be declared inline: the combination is refused wherever a
/// project's source is SET — `CreateProject` and `UpdateProject` alike.
///
/// A buildspec of nothing but whitespace counts as empty here. That is this emulator's
/// reading, not the model's wording (which names "not provided or [...] an empty
/// string"): such a value declares no build commands either, so treating it as provided
/// would store a project nothing could be built from.
///
/// Only a PRIMARY source is checked. A secondary source is checked out beside the
/// primary one and never supplies the project's buildspec, so `NO_SOURCE` there is not
/// this configuration.
pub fn inline_buildspec_violation(
    source: &crate::ops::types::ProjectSource,
) -> Option<String> {
    let no_inline_buildspec = source
        .buildspec
        .as_ref()
        .is_none_or(|buildspec| buildspec.trim().is_empty());
    (source.r#type == NO_SOURCE_TYPE && no_inline_buildspec).then(|| {
        format!(
            "A buildspec must be provided when the project source type is {NO_SOURCE_TYPE}, \
             which has no source code to hold a buildspec file"
        )
    })
}
