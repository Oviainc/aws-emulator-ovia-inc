// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-synthetics-synthetics service.
//!
//! One record per canary, keyed by canary name — the identity CloudWatch Synthetics
//! itself uses on the wire (`GET /canary/{Name}`), so a name is unique per account and
//! region. A [`BTreeMap`] rather than a hash map because `DescribeCanaries` pages over
//! this collection and a tokenised page boundary needs a total, stable order.
//!
//! Everything a handler returns is projected from one of these records
//! ([`StoredCanary::to_canary`]); no handler builds a `Canary` from its own request.

use std::collections::hash_map::RandomState;
use std::collections::BTreeMap;
use std::hash::{BuildHasher as _, Hasher as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ArtifactConfigOutput, BrowserConfig, Canary, CanaryCodeOutput, CanaryRun,
    CanaryRunConfigOutput, CanaryRunStatus, CanaryRunTimeline, CanaryScheduleOutput, CanaryStatus,
    CanaryTimeline, Dependency, RetryConfigOutput, S3EncryptionConfig, VpcConfigOutput,
};

/// Mint a fresh identifier for a canary or a canary run.
///
/// `Canary$Id` and `CanaryRun$Id` are modelled as `UUID`
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), which is exactly a
/// lowercase-hyphenated version-4 UUID, so that is what this mints: sixteen bytes of
/// SHA-256 (the `sha2` crate, not a hand-written hash) over a seed that is unique per
/// call, laid out with the version-4 and RFC 4122 variant nibbles and hex-encoded (the
/// `hex` crate, not a hand-written codec) into the five modelled groups.
///
/// The seed is a process-unique counter, the current instant and one operating-system
/// seeded value per process, so two ids minted by one process differ by construction and
/// ids minted by different processes differ too. NOTHING about the request enters the
/// seed: an id is not derived from client input.
pub fn new_uuid() -> String {
    use sha2::Digest as _;

    /// Distinguishes two ids minted in the same instant by the same process.
    static MINTED: AtomicU64 = AtomicU64::new(0);
    /// One value per process, drawn from the operating system: `RandomState`'s keys are
    /// OS-seeded the first time one is built, which is the entropy std exposes.
    static PROCESS_SEED: OnceLock<u64> = OnceLock::new();

    let process_seed = *PROCESS_SEED.get_or_init(|| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u32(::std::process::id());
        hasher.finish()
    });
    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = sha2::Sha256::new();
    digest.update(process_seed.to_le_bytes());
    digest.update(minted.to_le_bytes());
    digest.update(nanos.to_le_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    // Version 4 in the high nibble of byte 6 and the RFC 4122 variant in the top bits of
    // byte 8, the layout a random UUID carries on the wire.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let digits = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// Whether `value` has the shape the model gives `Canary$Id` and `CanaryRun$Id`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`. Group lengths and
/// lowercase hex only — the modelled pattern says nothing about the version or variant
/// nibbles — which is stricter than a general UUID parse (that accepts uppercase and
/// braced or unhyphenated forms). Used by the token validation that has to recognise an
/// id this service minted; this crate's tests read the modelled pattern themselves rather
/// than calling this, so a wrong predicate here cannot make an assertion pass vacuously.
pub fn is_uuid(value: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = value.split('-');
    for len in groups {
        match parts.next() {
            Some(part)
                if part.len() == len
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) => {}
            _ => return false,
        }
    }
    parts.next().is_none()
}

/// The retention default both retention members document: "If you omit this field, the
/// default of 31 days is used." (`CreateCanaryRequest$SuccessRetentionPeriodInDays` /
/// `$FailureRetentionPeriodInDays`, via
/// `smithy_query(query="doc",shape="CreateCanaryRequest",member="SuccessRetentionPeriodInDays")`).
pub const DEFAULT_RETENTION_PERIOD_IN_DAYS: i32 = 31;

/// `CanaryScheduleInput$DurationInSeconds`: "If you omit this field, the default of 0 is
/// used."
pub const DEFAULT_SCHEDULE_DURATION_IN_SECONDS: i64 = 0;

/// `CreateCanaryRequest$ProvisionedResourceCleanup`: "If you omit this parameter, the
/// default of AUTOMATIC is used".
pub const DEFAULT_PROVISIONED_RESOURCE_CLEANUP: &str = "AUTOMATIC";

/// `CanaryRunConfigInput$TimeoutInSeconds`: "If you omit this field, the frequency of the
/// canary is used as this value, up to a maximum of 14 minutes." 14 minutes = 840 s, which
/// is also the member's modelled maximum.
pub const MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS: i32 = 840;

/// The member's modelled minimum (`range 3..840`), the floor for a derived default.
pub const MIN_RUN_TIMEOUT_IN_SECONDS: i32 = 3;

/// How long a provisioning transition this emulator owes stays pending.
///
/// `CreateCanary` answers with `CREATING`, a non-terminal state, so the move to `READY`
/// is owed by the clock rather than by any later call: the due instant is recorded and
/// materialised by whichever request next looks at the record. The span is deliberately
/// non-zero, so a caller that acts immediately still observes `CREATING` and the
/// conflict errors that state implies.
pub const PROVISIONING_SETTLE: Duration = Duration::from_millis(500);

/// How long a canary run this emulator started stays `RUNNING` before the clock owes its
/// outcome. Same contract as [`PROVISIONING_SETTLE`].
pub const RUN_SETTLE: Duration = Duration::from_millis(500);

/// Why a run this emulator started cannot pass.
///
/// A canary run executes customer Node.js/Python browser automation against an endpoint.
/// This emulator has no canary runtime and makes no outbound connection, so nothing runs:
/// the run therefore ends in the service's own modelled failure state with a reason that
/// says why, rather than reporting a result for work that never happened.
pub const RUN_NOT_EXECUTED_REASON: &str =
    "This emulator has no canary runtime, so the canary script was not executed and the run \
     produced no result.";

/// `CanaryRunStateReasonCode` member used for [`RUN_NOT_EXECUTED_REASON`]: the run failed
/// before the script produced a verdict, which is an execution failure rather than a
/// canary (assertion) failure.
pub const RUN_NOT_EXECUTED_REASON_CODE: &str = "EXECUTION_FAILURE";

/// `CanaryStateReasonCode` reported while a freshly created canary is still `CREATING`.
pub const CREATE_IN_PROGRESS_REASON_CODE: &str = "CREATE_IN_PROGRESS";

/// The message every handler answers with when the shared store cannot be reached. One
/// constant so the six handlers that can hit it word it identically.
pub const STORE_UNAVAILABLE: &str = "the synthetics canary store is unavailable";

/// `ResourceNotFoundException$Message` for a canary that does not exist.
///
/// The model says of the error only "One of the specified resources was not found." and
/// documents no text, but the recorded service answers every canary lookup that misses —
/// whichever operation asked — with one fixed sentence that never names the requested
/// canary. The message is therefore request-independent: it is a constant here, not
/// built from the lookup key, so the identifier the real service withholds is not handed
/// back to the caller.
pub const CANARY_NOT_FOUND: &str = "Canary not found.";

/// `ResourceNotFoundException$Message` for a dry run that does not exist, by the same
/// rule as [`CANARY_NOT_FOUND`]: a missing-resource message names the resource KIND that
/// was not found and never the identifier the request asked for.
pub const DRY_RUN_NOT_FOUND: &str = "Dry run not found.";

/// The most canaries this store holds, so a request-driven create loop cannot grow it
/// without limit.
///
/// CloudWatch Synthetics' own default quota is 100 canaries per Region ("Canaries" in the
/// CloudWatch Synthetics service quotas), so a caller past this bound is past the real
/// service's bound too and is refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_CANARIES: usize = 100;

/// The most runs this store keeps per canary, so a request-driven start/stop loop cannot
/// grow one canary's history without limit.
///
/// Emulator-chosen: the real service bounds the history by the canary's retention period
/// (enforced in [`StoredCanary::settle`]) rather than by a count, but a wall-clock
/// retention of days is no bound at all on a loop that runs for seconds, so the oldest
/// runs beyond this many are dropped as well.
pub const MAX_RETAINED_RUNS_PER_CANARY: usize = 100;

/// Seconds in the day the retention periods are counted in.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The lifecycle states a canary can be in.
///
/// The wire spellings are the members of the model's `CanaryState` enum (see
/// `ops::types::CANARY_STATE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryState {
    Creating,
    Ready,
    Starting,
    Running,
    Updating,
    Stopping,
    Stopped,
    Error,
    Deleting,
}

impl CanaryState {
    /// The `CanaryState` member name this state serialises as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Ready => "READY",
            Self::Starting => "STARTING",
            Self::Running => "RUNNING",
            Self::Updating => "UPDATING",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
            Self::Error => "ERROR",
            Self::Deleting => "DELETING",
        }
    }

    /// Whether an operation on the canary is already in flight, so a second one
    /// conflicts with it. `ConflictException`'s documented meaning is "a conflicting
    /// operation is already in progress", which is exactly this set.
    pub fn operation_in_progress(self) -> bool {
        matches!(
            self,
            Self::Creating | Self::Starting | Self::Stopping | Self::Updating | Self::Deleting
        )
    }
}

/// The states a single canary run can be in (model enum `CanaryRunState`, see
/// `ops::types::CANARY_RUN_STATE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryRunState {
    Running,
    Passed,
    Failed,
}

impl CanaryRunState {
    /// The `CanaryRunState` member name this state serialises as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Passed => "PASSED",
            Self::Failed => "FAILED",
        }
    }
}

/// Where a canary's script came from, plus the layer ARN Synthetics reports for it.
#[derive(Debug, Clone)]
pub struct StoredCode {
    pub handler: Option<String>,
    pub blueprint_types: Vec<String>,
    pub dependencies: Vec<StoredDependency>,
    pub source_location_arn: String,
    pub s3_bucket: Option<String>,
    pub s3_key: Option<String>,
    pub s3_version: Option<String>,
}

/// One entry of `CanaryCodeInput$Dependencies`, kept so the code structure reports back
/// what was supplied.
#[derive(Debug, Clone)]
pub struct StoredDependency {
    pub r#type: Option<String>,
    pub reference: String,
}

/// How often the canary runs, and for how long it keeps doing so.
#[derive(Debug, Clone)]
pub struct StoredSchedule {
    pub expression: String,
    pub duration_in_seconds: i64,
    pub max_retries: Option<i32>,
}

/// Per-run configuration. `environment_variables` is stored but never reported:
/// `CanaryRunConfigOutput` models no member for it, because the real service does not
/// echo canary environment variables back.
#[derive(Debug, Clone, Default)]
pub struct StoredRunConfig {
    pub timeout_in_seconds: i32,
    pub memory_in_mb: Option<i32>,
    pub active_tracing: Option<bool>,
    pub ephemeral_storage: Option<i32>,
    pub environment_variables: BTreeMap<String, String>,
}

/// Artifact encryption settings, as supplied.
#[derive(Debug, Clone)]
pub struct StoredArtifactConfig {
    pub encryption_mode: Option<String>,
    pub kms_key_arn: Option<String>,
}

/// VPC placement, as supplied. `vpc_id` is not stored: the real service derives it from
/// the subnets, and this emulator has no subnet inventory to derive it from, so it is
/// reported absent rather than invented.
#[derive(Debug, Clone)]
pub struct StoredVpcConfig {
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub ipv6_allowed_for_dual_stack: Option<bool>,
}

/// One recorded run of a canary.
#[derive(Debug, Clone)]
pub struct StoredRun {
    pub id: String,
    pub name: String,
    pub state: CanaryRunState,
    pub state_reason: Option<String>,
    pub state_reason_code: Option<String>,
    pub started: SystemTime,
    pub completed: Option<SystemTime>,
    /// The instant at which the clock owes this run's outcome, while it is `RUNNING`.
    pub outcome_due: Option<SystemTime>,
    pub artifact_s3_location: String,
}

impl StoredRun {
    /// Project this run onto the modelled `CanaryRun` shape.
    pub fn to_canary_run(&self) -> CanaryRun {
        CanaryRun {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(CanaryRunStatus {
                state: Some(self.state.as_wire().to_string()),
                state_reason: self.state_reason.clone(),
                state_reason_code: self.state_reason_code.clone(),
                test_result: None,
            }),
            timeline: Some(CanaryRunTimeline {
                started: Some(self.started.into()),
                completed: self.completed.map(Into::into),
                metric_timestamp_for_run_and_retries: None,
            }),
            artifact_s3_location: Some(self.artifact_s3_location.clone()),
            ..Default::default()
        }
    }
}

/// One canary, as this emulator persists it. Every reported field is read from here.
#[derive(Debug, Clone)]
pub struct StoredCanary {
    pub id: String,
    pub name: String,
    pub region: String,
    pub code: StoredCode,
    pub execution_role_arn: String,
    pub schedule: StoredSchedule,
    pub run_config: StoredRunConfig,
    pub success_retention_period_in_days: i32,
    pub failure_retention_period_in_days: i32,
    pub artifact_s3_location: String,
    pub artifact_config: Option<StoredArtifactConfig>,
    pub runtime_version: String,
    pub vpc_config: Option<StoredVpcConfig>,
    pub engine_arn: String,
    pub provisioned_resource_cleanup: String,
    pub kms_key_arn: Option<String>,
    pub state: CanaryState,
    pub state_reason: Option<String>,
    pub state_reason_code: Option<String>,
    /// The instant at which the clock owes the move out of [`CanaryState::Creating`].
    pub ready_due: Option<SystemTime>,
    pub created: SystemTime,
    pub last_modified: SystemTime,
    pub last_started: Option<SystemTime>,
    pub last_stopped: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// The browser types the canary was created for, in the order supplied.
    pub browser_types: Vec<String>,
    /// Newest run first, the order `GetCanaryRuns` reports.
    pub runs: Vec<StoredRun>,
}

impl StoredCanary {
    /// This canary's ARN: `arn:aws:synthetics:<region>:<account>:canary:<name>`.
    pub fn arn(&self) -> String {
        canary_arn(&self.region, &self.name)
    }

    /// Materialise every transition the clock already owes at `now`, and nothing else.
    ///
    /// Called by reporting and mutating operations alike: these changes were due at this
    /// instant whether or not anybody called, so applying them on a read does not change
    /// what a later caller sees. That includes retention expiry: a run whose data the
    /// canary's retention period no longer covers was already out of range at this
    /// instant, so it is dropped here rather than when somebody happens to look.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == CanaryState::Creating
            && self.ready_due.is_some_and(|due| now >= due)
        {
            self.state = CanaryState::Ready;
            self.state_reason = None;
            self.state_reason_code = None;
            self.ready_due = None;
        }
        for run in &mut self.runs {
            if run.state == CanaryRunState::Running
                && run.outcome_due.is_some_and(|due| now >= due)
            {
                // Nothing executed the script, so the run cannot have a verdict: it ends
                // in the modelled failure state with the reason why.
                run.state = CanaryRunState::Failed;
                run.state_reason = Some(RUN_NOT_EXECUTED_REASON.to_string());
                run.state_reason_code = Some(RUN_NOT_EXECUTED_REASON_CODE.to_string());
                run.completed = Some(run.outcome_due.unwrap_or(now));
                run.outcome_due = None;
            }
        }
        self.expire_runs(now);
    }

    /// Drop every finished run the canary's retention period no longer covers.
    ///
    /// `CreateCanaryRequest$SuccessRetentionPeriodInDays` /
    /// `$FailureRetentionPeriodInDays`: "The number of days to retain data about
    /// successful [failed] runs of this canary. … This setting affects the range of
    /// information returned by GetCanaryRuns".
    fn expire_runs(&mut self, now: SystemTime) {
        // Copied out first: the closure below cannot borrow `self` while `self.runs` is
        // borrowed mutably.
        let success_days = self.success_retention_period_in_days.max(0) as u64;
        let failure_days = self.failure_retention_period_in_days.max(0) as u64;
        self.runs.retain(|run| {
            let Some(completed) = run.completed else {
                // Still open — a `RUNNING` run never carries `completed` — so there is
                // nothing retained yet to expire.
                return true;
            };
            let days = if run.state == CanaryRunState::Passed {
                success_days
            } else {
                failure_days
            };
            let ttl = Duration::from_secs(days.saturating_mul(SECONDS_PER_DAY));
            match now.duration_since(completed) {
                Ok(age) => age < ttl,
                // Completed in this instant's future (a clock that stepped back): keep it.
                Err(_) => true,
            }
        });
    }

    /// Record the start of a run, keeping the history within
    /// [`MAX_RETAINED_RUNS_PER_CANARY`] so a start/stop loop cannot grow it without limit.
    ///
    /// The history is newest-first, so the entries dropped are the oldest.
    pub fn begin_run(&mut self, run: StoredRun) {
        self.runs.insert(0, run);
        self.runs.truncate(MAX_RETAINED_RUNS_PER_CANARY);
    }

    /// Finish whichever run is still open, because the canary was stopped.
    pub fn close_open_run(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            if run.state == CanaryRunState::Running {
                run.state = CanaryRunState::Failed;
                run.state_reason = Some(RUN_NOT_EXECUTED_REASON.to_string());
                run.state_reason_code = Some(RUN_NOT_EXECUTED_REASON_CODE.to_string());
                run.completed = Some(now);
                run.outcome_due = None;
            }
        }
    }

    /// Project this record onto the modelled `Canary` shape. The ONLY way a handler
    /// produces a `Canary`, so `CreateCanary`, `GetCanary` and `DescribeCanaries` cannot
    /// disagree about the same record.
    pub fn to_canary(&self) -> Canary {
        Canary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            code: Some(CanaryCodeOutput {
                source_location_arn: Some(self.code.source_location_arn.clone()),
                handler: self.code.handler.clone(),
                blueprint_types: (!self.code.blueprint_types.is_empty())
                    .then(|| self.code.blueprint_types.clone()),
                dependencies: (!self.code.dependencies.is_empty()).then(|| {
                    self.code
                        .dependencies
                        .iter()
                        .map(|dep| Dependency {
                            r#type: dep.r#type.clone(),
                            reference: dep.reference.clone(),
                        })
                        .collect()
                }),
            }),
            execution_role_arn: Some(self.execution_role_arn.clone()),
            schedule: Some(CanaryScheduleOutput {
                expression: Some(self.schedule.expression.clone()),
                duration_in_seconds: Some(self.schedule.duration_in_seconds),
                retry_config: self.schedule.max_retries.map(|max_retries| RetryConfigOutput {
                    max_retries: Some(max_retries),
                }),
            }),
            run_config: Some(CanaryRunConfigOutput {
                timeout_in_seconds: Some(self.run_config.timeout_in_seconds),
                memory_in_mb: self.run_config.memory_in_mb,
                active_tracing: self.run_config.active_tracing,
                ephemeral_storage: self.run_config.ephemeral_storage,
            }),
            success_retention_period_in_days: Some(self.success_retention_period_in_days),
            failure_retention_period_in_days: Some(self.failure_retention_period_in_days),
            status: Some(CanaryStatus {
                state: Some(self.state.as_wire().to_string()),
                state_reason: self.state_reason.clone(),
                state_reason_code: self.state_reason_code.clone(),
            }),
            timeline: Some(CanaryTimeline {
                created: Some(self.created.into()),
                last_modified: Some(self.last_modified.into()),
                last_started: self.last_started.map(Into::into),
                last_stopped: self.last_stopped.map(Into::into),
            }),
            artifact_s3_location: Some(self.artifact_s3_location.clone()),
            engine_arn: Some(self.engine_arn.clone()),
            runtime_version: Some(self.runtime_version.clone()),
            vpc_config: self.vpc_config.as_ref().map(|vpc| VpcConfigOutput {
                // Absent on purpose: the real service resolves the VPC from the subnets,
                // and this emulator has no subnet inventory to resolve it from.
                vpc_id: None,
                subnet_ids: Some(vpc.subnet_ids.clone()),
                security_group_ids: Some(vpc.security_group_ids.clone()),
                ipv6_allowed_for_dual_stack: vpc.ipv6_allowed_for_dual_stack,
            }),
            provisioned_resource_cleanup: Some(self.provisioned_resource_cleanup.clone()),
            browser_configs: (!self.browser_types.is_empty()).then(|| {
                self.browser_types
                    .iter()
                    .map(|browser_type| BrowserConfig {
                        browser_type: Some(browser_type.clone()),
                    })
                    .collect()
            }),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            }),
            artifact_config: self.artifact_config.as_ref().map(|cfg| ArtifactConfigOutput {
                s3_encryption: Some(S3EncryptionConfig {
                    encryption_mode: cfg.encryption_mode.clone(),
                    kms_key_arn: cfg.kms_key_arn.clone(),
                }),
            }),
            kms_key_arn: self.kms_key_arn.clone(),
            ..Default::default()
        }
    }
}

/// Per-service state for com-amazonaws-synthetics-synthetics. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSyntheticsSyntheticsState {
    /// Canaries by name — the identity the API addresses them by.
    pub canaries: BTreeMap<String, StoredCanary>,
}

impl ComAmazonawsSyntheticsSyntheticsState {
    /// Apply [`StoredCanary::settle`] to every record, so any operation starts from a
    /// store in which the clock's own transitions have already happened.
    pub fn settle(&mut self, now: SystemTime) {
        for canary in self.canaries.values_mut() {
            canary.settle(now);
        }
    }
}

/// A canary's ARN: `arn:aws:synthetics:<region>:<account>:canary:<name>`.
///
/// The shape of the `ResourceArn` the group and tag operations accept
/// (`^arn:(aws[a-zA-Z-]*)?:synthetics:…:\d{12}:canary:[0-9a-z_\-]{1,255}$`), so those
/// operations can key off these same records once they are implemented.
pub fn canary_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("synthetics", region, &format!("canary:{name}"))
}

/// The ARN of the Lambda function Synthetics provisions for a canary, which the service
/// reports as `Canary$EngineArn`. Synthetics names these `cwsyn-<canary name>-<canary id>`.
pub fn engine_arn(region: &str, name: &str, id: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("function:cwsyn-{name}-{id}"))
}

/// The ARN of the Lambda layer Synthetics stores the canary script in, which the service
/// reports as `CanaryCodeOutput$SourceLocationArn`.
pub fn source_location_arn(region: &str, name: &str, id: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("layer:cwsyn-{name}-{id}:1"))
}

/// The region this request was signed for, which is the region the resources it creates
/// live in. Falls back to the emulator's default region when the request carries no SigV4
/// credential scope (an unsigned request routed by its host).
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The scheme every Amazon S3 location is written with.
pub const S3_LOCATION_SCHEME: &str = "s3://";

/// Amazon S3's bucket naming rules, as the S3 User Guide's "General purpose bucket naming
/// rules" states them: a name is 3–63 characters long, uses only lowercase letters,
/// numbers, periods and hyphens, begins and ends with a letter or a number, must not be
/// formatted as an IP address, must not contain two adjacent periods, must not start with
/// `xn--`, `sthree-` or `amzn-s3-demo-`, and must not end with `-s3alias` or `--ol-s3`.
const S3_BUCKET_MIN_LEN: usize = 3;
const S3_BUCKET_MAX_LEN: usize = 63;
const S3_BUCKET_RESERVED_PREFIXES: [&str; 3] = ["xn--", "sthree-", "amzn-s3-demo-"];
const S3_BUCKET_RESERVED_SUFFIXES: [&str; 2] = ["-s3alias", "--ol-s3"];

/// Whether `location` is a well-formed Amazon S3 location for canary artifacts.
///
/// `CreateCanaryRequest$ArtifactS3Location` — and the same member on
/// `UpdateCanaryRequest` and `StartCanaryDryRunRequest` — documents itself as "The
/// location in Amazon S3 where Synthetics stores artifacts from the test runs of this
/// canary … The name of the Amazon S3 bucket can't include a period (.)"
/// (`smithy_query(query="doc",shape="CreateCanaryRequest",member="ArtifactS3Location")`).
/// So the value names an S3 bucket and an optional key prefix inside it: `s3://<bucket>`
/// or `s3://<bucket>/<key prefix>`, with the bucket component held to Amazon S3's own
/// bucket naming rules (see the constants above) plus that member's extra no-period rule.
/// A value that is not such a location is refused with the modelled
/// `ValidationException`; nothing else about the value is checked here, because the
/// member's length is already a modelled `@length` constraint the request boundary
/// enforces.
pub fn is_s3_artifact_location(location: &str) -> bool {
    let Some(bucket_and_key) = location.strip_prefix(S3_LOCATION_SCHEME) else {
        return false;
    };
    // The bucket is everything up to the first `/`; the rest is the key prefix the
    // artifacts are written under, which may be absent and is otherwise opaque (any
    // sequence of characters is a legal S3 key).
    let bucket = match bucket_and_key.split_once('/') {
        Some((bucket, _key_prefix)) => bucket,
        None => bucket_and_key,
    };
    is_s3_bucket_name(bucket)
}

/// Whether `bucket` satisfies Amazon S3's bucket naming rules AND the no-period rule
/// `ArtifactS3Location` adds to them. See [`is_s3_artifact_location`].
fn is_s3_bucket_name(bucket: &str) -> bool {
    if !(S3_BUCKET_MIN_LEN..=S3_BUCKET_MAX_LEN).contains(&bucket.len()) {
        return false;
    }
    // "The name of the Amazon S3 bucket can't include a period (.)" — the member's own
    // rule, stricter than S3's, which also settles S3's "no two adjacent periods" and
    // "must not be formatted as an IP address" rules: neither form survives it.
    if bucket.contains('.') {
        return false;
    }
    if !bucket
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return false;
    }
    let alphanumeric = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !bucket.bytes().next().is_some_and(alphanumeric)
        || !bucket.bytes().next_back().is_some_and(alphanumeric)
    {
        return false;
    }
    if S3_BUCKET_RESERVED_PREFIXES
        .iter()
        .any(|prefix| bucket.starts_with(prefix))
    {
        return false;
    }
    !S3_BUCKET_RESERVED_SUFFIXES
        .iter()
        .any(|suffix| bucket.ends_with(suffix))
}

/// How Synthetics words the refusal of a location that is not a well-formed S3 location.
///
/// One sentence naming the member and the defect class, the way the service states it;
/// it carries nothing from the request, so a rejected value is never echoed back.
pub const ARTIFACT_S3_LOCATION_MALFORMED: &str = "ArtifactS3Location: The location is malformed.";

/// How often `expression` runs the canary, in seconds, when that is determinable.
///
/// `CanaryScheduleInput$Expression` documents the rate form as `rate(<number> <unit>)`
/// with `unit` one of `minute`, `minutes` or `hour`, and `rate(0 minute)` / `rate(0 hour)`
/// as the special "run once" value — which has no frequency, hence `None`. A `cron(...)`
/// expression also yields `None` here: its frequency is not a single interval.
pub fn schedule_frequency_in_seconds(expression: &str) -> Option<i64> {
    let body = expression
        .trim()
        .strip_prefix("rate(")
        .and_then(|rest| rest.strip_suffix(')'))?;
    let mut fields = body.split_whitespace();
    let count: i64 = fields.next()?.parse().ok()?;
    let unit = fields.next()?;
    if fields.next().is_some() || count == 0 {
        return None;
    }
    let seconds_per_unit = match unit {
        "minute" | "minutes" => 60,
        "hour" | "hours" => 3600,
        _ => return None,
    };
    count.checked_mul(seconds_per_unit)
}

/// The `TimeoutInSeconds` a canary gets when `RunConfig.TimeoutInSeconds` is omitted.
///
/// `CanaryRunConfigInput$TimeoutInSeconds`: "If you omit this field, the frequency of the
/// canary is used as this value, up to a maximum of 14 minutes." A schedule with no single
/// frequency (`cron(...)`, or the run-once `rate(0 ...)`) therefore takes that maximum.
pub fn default_run_timeout_in_seconds(expression: &str) -> i32 {
    match schedule_frequency_in_seconds(expression) {
        Some(frequency) => i32::try_from(frequency)
            .unwrap_or(MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS)
            .clamp(
                MIN_RUN_TIMEOUT_IN_SECONDS,
                MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS,
            ),
        None => MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSyntheticsSyntheticsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSyntheticsSyntheticsState>)
}
