// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amp-amazonprometheusservice service.
//!
//! Besides the state struct itself this module holds the few derivations every
//! workspace handler needs (the ARN, the Prometheus endpoint, the request's region,
//! a fresh workspace id), so `src/ops/*.rs` share ONE copy of each rule.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `WorkspaceStatusCode` values a workspace moves through here. Each is an entry of
/// the modelled `com.amazonaws.amp#WorkspaceStatusCode` enum, whose own
/// documentation states the lifecycle rules this crate enforces:
/// `CREATING` — "Workspace is being created. Deletion is disallowed until status is
/// ACTIVE."; `ACTIVE` — "Workspace has been created and is usable."; `DELETING` —
/// "Workspace is being deleted. Deletions are allowed only when status is ACTIVE."
/// (`crate::ops::types::WORKSPACE_STATUS_CODE_VALUES` carries the full list.)
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";

/// `WorkspaceConfigurationStatusCode` values a workspace CONFIGURATION moves through
/// here. Each is an entry of the modelled
/// `com.amazonaws.amp#WorkspaceConfigurationStatusCode` enum, whose own documentation
/// states the rule this crate enforces: `UPDATING` — "Workspace configuration is being
/// updated. Update is disallowed until workspace configuration is ACTIVE and workspace
/// status is ACTIVE."; `ACTIVE` — "Workspace configuration has been updated. Update is
/// disallowed until workspace configuration is ACTIVE and workspace status is ACTIVE."
/// The enum's third entry, `UPDATE_FAILED`, is never reported: nothing in this emulator
/// fails a configuration update, so claiming that status would be a status for work that
/// never ran. (`crate::ops::types::WORKSPACE_CONFIGURATION_STATUS_CODE_VALUES` carries
/// the full list.)
pub const CONFIG_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`CONFIG_STATUS_ACTIVE`].
pub const CONFIG_STATUS_UPDATING: &str = "UPDATING";

/// The retention period a workspace's configuration carries before any
/// `UpdateWorkspaceConfiguration` sets one.
///
/// DOCUMENTED: Amazon Managed Service for Prometheus publishes "Data retention period"
/// as a workspace quota with a default value of 150 days, so a workspace that nobody has
/// configured still retains metrics for that long and `DescribeWorkspaceConfiguration`
/// reports it. `WorkspaceConfigurationDescription$retentionPeriodInDays` is modelled
/// `@range(min: 1)`, which this value satisfies.
pub const DEFAULT_RETENTION_PERIOD_IN_DAYS: i32 = 150;

/// The out-of-order sample window a workspace's configuration carries before any
/// `UpdateWorkspaceConfiguration` sets one.
///
/// Amazon Managed Service for Prometheus rejects out-of-order samples unless a window
/// is configured for them, so an unconfigured workspace's window is zero seconds —
/// which is also the published lower bound of the modelled `@range(min: 0)` on
/// `WorkspaceConfigurationDescription$outOfOrderTimeWindowInSeconds`. The member is
/// reported rather than omitted because the workspace HAS a window from the moment it
/// exists; a caller comparing it before and after an update reads a number either way.
pub const DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS: i32 = 0;

/// The rule-evaluation query offset a workspace's configuration carries before any
/// `UpdateWorkspaceConfiguration` sets one.
///
/// `UpdateWorkspaceConfigurationRequest$ruleQueryOffsetInSeconds` documents what the
/// member buys — "Specifies the duration in seconds to offset rule evaluation queries
/// into the past. This allows ingested samples to be available before rule evaluation."
/// — so a workspace nobody has configured offsets them by nothing, the published lower
/// bound of the modelled `@range(min: 0)`. Reported for the same reason as
/// [`DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS`].
pub const DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS: i32 = 0;

/// How many label sets one workspace's configuration holds before
/// `UpdateWorkspaceConfiguration` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// This emulator models neither Service Quotas nor an account's adjustments, so — like
/// [`WORKSPACE_QUOTA`] — this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is what stops one request's `limitsPerLabelSet`
/// list growing a workspace's stored configuration without end; an update REPLACES the
/// stored list, so nothing accumulates beyond this bound.
pub const LABEL_SET_LIMITS_PER_WORKSPACE: usize = 1_000;

/// The Prometheus metric-name label. `LimitsPerLabelSet$labelSet` documents the rule
/// that goes with it: "If the label name is metric name label `__name__`, then the
/// metric part of the name must conform to the following pattern:
/// `[a-zA-Z_:][a-zA-Z0-9_:]*`" — so for this label name the VALUE is a metric name and
/// is held to that pattern. Validated by `UpdateWorkspaceConfiguration`, because
/// `LabelValue` is modelled with a length bound only.
pub const METRIC_NAME_LABEL: &str = "__name__";

/// `LoggingConfigurationStatusCode` values a rules-and-alerting logging configuration
/// moves through here. Each is an entry of the modelled
/// `com.amazonaws.amp#LoggingConfigurationStatusCode` enum, whose own documentation
/// states the lifecycle rule this crate enforces: `CREATING` — "Logging configuration
/// is being created. Update/Deletion is disallowed until logging configuration is
/// ACTIVE and workspace status is ACTIVE."; `ACTIVE` — "Logging configuration has been
/// created/updated." (`crate::ops::types::LOGGING_CONFIGURATION_STATUS_CODE_VALUES`
/// carries the full list.)
pub const LOGGING_STATUS_CREATING: &str = "CREATING";
/// See [`LOGGING_STATUS_CREATING`].
pub const LOGGING_STATUS_ACTIVE: &str = "ACTIVE";

/// The `resourceType` the modelled `ResourceNotFoundException` reports for a workspace's
/// rules-and-alerting logging configuration. The member is a free-form `String`
/// documented as "Type of the resource affected", so the emulator reports this
/// resource's name in the service's `AWS::APS::` namespace, the way
/// [`WORKSPACE_RESOURCE_TYPE`] does for the workspace itself.
pub const LOGGING_CONFIGURATION_RESOURCE_TYPE: &str = "AWS::APS::LoggingConfiguration";

/// How long a workspace stays in a transient status before the clock owes the next
/// transition (`CREATING` -> `ACTIVE`, `DELETING` -> removed, a workspace
/// configuration's `UPDATING` -> `ACTIVE`).
///
/// CHOSEN, not documented: Amazon Managed Service for Prometheus publishes no
/// duration for any of these transitions, so this is the emulator's own span — long
/// enough that a caller acting straight after
/// `CreateWorkspace`/`DeleteWorkspace`/`UpdateWorkspaceConfiguration` still observes
/// the transient status and the state errors that go with it, short enough
/// that a workflow does not wait on it. Nothing sleeps for it: the due instant is
/// stored and applied by [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`].
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a workspace. The member is a free-form `String` documented as "Type of
/// the resource affected", so the emulator reports the resource's AWS type name.
pub const WORKSPACE_RESOURCE_TYPE: &str = "AWS::APS::Workspace";

/// `QueryLoggingConfigurationStatusCode` values a query logging configuration moves
/// through here. Each is an entry of the modelled
/// `com.amazonaws.amp#QueryLoggingConfigurationStatusCode` enum, whose own
/// documentation names them: `CREATING` — "Query logging configuration is being
/// created."; `ACTIVE` — "Query logging configuration is active."; `UPDATING` — "Query
/// logging configuration is being updated."; `DELETING` — "Query logging configuration
/// is being deleted." (`crate::ops::types::QUERY_LOGGING_CONFIGURATION_STATUS_CODE_VALUES`
/// carries the full list, including the two failure codes nothing here reaches: this
/// emulator runs no vended-log delivery that could fail.)
pub const QUERY_LOGGING_STATUS_CREATING: &str = "CREATING";
/// See [`QUERY_LOGGING_STATUS_CREATING`].
pub const QUERY_LOGGING_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`QUERY_LOGGING_STATUS_CREATING`].
pub const QUERY_LOGGING_STATUS_UPDATING: &str = "UPDATING";
/// See [`QUERY_LOGGING_STATUS_CREATING`].
pub const QUERY_LOGGING_STATUS_DELETING: &str = "DELETING";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a query logging configuration — the resource's AWS type name, as
/// [`WORKSPACE_RESOURCE_TYPE`] is for a workspace.
pub const QUERY_LOGGING_RESOURCE_TYPE: &str = "AWS::APS::QueryLoggingConfiguration";

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace that
/// holds no query logging configuration.
///
/// One CONSTANT sentence, as Amazon Managed Service for Prometheus answers it: it names
/// the kind of resource that is missing and carries nothing from the request, which
/// `resourceId` reports instead. The three operations that look a configuration up share
/// it, so they cannot drift apart.
pub const QUERY_LOGGING_NOT_FOUND_MESSAGE: &str = "Workspace logging configuration not found.";

/// How many workspaces the store holds before `CreateWorkspace` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amazon Managed Service for Prometheus publishes an adjustable quota on workspaces
/// per account per Region, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on the store, not a
/// claim about any account's limit. It sits far above any workflow an emulator serves,
/// and it is what stops the state growing without end: every record it bounds is
/// released by `DeleteWorkspace`, which also drops that workspace's ARN index entry
/// and forgets its client tokens, so neither of those maps can outgrow it either.
pub const WORKSPACE_QUOTA: usize = 1_000;

/// The largest page `ListWorkspaces` serves, and the page size it uses when the request
/// names no `maxResults`: `ListWorkspacesRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)` and documented "The maximum number of workspaces to
/// return per request. The default is 100." The handler clamps into `1..=MAX` itself
/// rather than trusting the boundary validator, so a caller cannot choose the amount of
/// work one request does.
pub const WORKSPACE_PAGE_LIMIT: i32 = 1_000;
/// See [`WORKSPACE_PAGE_LIMIT`].
pub const WORKSPACE_PAGE_DEFAULT: i32 = 100;

/// This service's ARN namespace (`OPERATIONS.md`: "ARN namespace: `aps`").
const ARN_SERVICE: &str = "aps";

/// Region used when a request carries no SigV4 credential scope to read one from —
/// the region the emulator's own generated client config defaults to.
const DEFAULT_REGION: &str = "us-east-1";

/// One stored workspace: every member `WorkspaceDescription` / `WorkspaceSummary`
/// report, plus the bookkeeping the lifecycle needs.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: String,
    pub arn: String,
    /// `None` when the workspace carries no alias (none given, or one that was all
    /// blank space — see [`normalize_alias`]).
    pub alias: Option<String>,
    /// One of [`STATUS_CREATING`], [`STATUS_ACTIVE`], [`STATUS_DELETING`].
    pub status_code: String,
    pub prometheus_endpoint: String,
    pub created_at: SystemTime,
    /// Sorted so a listing and a describe report the tags in one stable order.
    pub tags: BTreeMap<String, String>,
    pub kms_key_arn: Option<String>,
    /// When the clock owes this workspace's next lifecycle transition, if one is
    /// pending: `CREATING` becomes `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the `DeleteWorkspace` call that started a pending
    /// deletion, so replaying that exact call stays idempotent rather than
    /// conflicting with the deletion it already started.
    pub delete_token: Option<String>,
    /// What `DescribeWorkspaceConfiguration` reports and `UpdateWorkspaceConfiguration`
    /// writes: the workspace's label set limits, retention period and sample windows.
    pub configuration: WorkspaceConfiguration,
    /// This workspace's rules-and-alerting logging configuration, when it has one.
    ///
    /// `/workspaces/{workspaceId}/logging` names at most ONE such configuration per
    /// workspace — `LoggingConfigurationMetadata` is a single structure keyed by nothing
    /// but the workspace — so it lives ON the workspace record: a workspace that goes
    /// away takes its logging configuration with it.
    pub logging_configuration: Option<LoggingConfiguration>,
}

/// One entry of a configuration's `limitsPerLabelSet`: a label set and the active time
/// series limit enforced for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSetLimit {
    /// The label set, `labelName -> labelValue`. Sorted, so a describe reports one
    /// stable order whatever order the request's `LabelSet` map was iterated in.
    pub label_set: BTreeMap<String, String>,
    /// `LimitsPerLabelSetEntry$maxSeries`, absent when the request set none.
    pub max_series: Option<i64>,
}

/// One workspace's stored configuration — the record
/// `WorkspaceConfigurationDescription` reports.
#[derive(Debug, Clone)]
pub struct WorkspaceConfiguration {
    /// One of [`CONFIG_STATUS_ACTIVE`], [`CONFIG_STATUS_UPDATING`].
    pub status_code: String,
    /// `WorkspaceConfigurationStatus$statusReason` — "The reason for the current
    /// status, if a reason is available". No status this emulator reports has one.
    pub status_reason: Option<String>,
    /// The label sets with an enforced active time series limit, in the order the
    /// update that set them listed them.
    pub limits_per_label_set: Vec<LabelSetLimit>,
    /// Always present: a workspace retains metrics for some period from the moment it
    /// exists ([`DEFAULT_RETENTION_PERIOD_IN_DAYS`] until an update changes it).
    pub retention_period_in_days: i32,
    /// Always present, for the same reason:
    /// [`DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS`] until an update changes it.
    pub out_of_order_time_window_in_seconds: i32,
    /// Always present, for the same reason:
    /// [`DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS`] until an update changes it.
    pub rule_query_offset_in_seconds: i32,
    /// When the clock owes this configuration's move out of `UPDATING` back to
    /// `ACTIVE`, if an update is pending.
    pub status_due: Option<SystemTime>,
    /// The `clientToken` of the `UpdateWorkspaceConfiguration` call that started the
    /// update now pending, so replaying that exact call answers with the update it
    /// already started instead of starting a second one. Forgotten when that update
    /// settles, so it never speaks for a configuration that is no longer updating.
    pub update_token: Option<String>,
}

impl Default for WorkspaceConfiguration {
    /// The configuration a freshly created workspace carries: nothing is updating, no
    /// label set limit is enforced, metrics are retained for the service's default
    /// period, and neither sample window is offset.
    fn default() -> Self {
        Self {
            status_code: CONFIG_STATUS_ACTIVE.to_string(),
            status_reason: None,
            limits_per_label_set: Vec::new(),
            retention_period_in_days: DEFAULT_RETENTION_PERIOD_IN_DAYS,
            out_of_order_time_window_in_seconds: DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS,
            rule_query_offset_in_seconds: DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS,
            status_due: None,
            update_token: None,
        }
    }
}

/// One stored rules-and-alerting logging configuration: every member
/// `LoggingConfigurationMetadata` reports, plus the bookkeeping its lifecycle needs.
#[derive(Debug, Clone)]
pub struct LoggingConfiguration {
    /// The CloudWatch log group the vended log data is published to.
    pub log_group_arn: String,
    /// One of [`LOGGING_STATUS_CREATING`], [`LOGGING_STATUS_ACTIVE`].
    pub status_code: String,
    /// `LoggingConfigurationStatus$statusReason` — "If failed, the reason for the
    /// failure", so `None` while nothing has failed.
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    /// When the configuration was most recently CHANGED, i.e. by the call that created
    /// it or last updated what it points at — not by the clock moving its status on.
    pub modified_at: SystemTime,
    /// When the clock owes this configuration's move out of a transient status
    /// (`CREATING` -> `ACTIVE`), if one is pending.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the call that created it, so replaying that exact call is
    /// idempotent rather than a second create.
    pub client_token: Option<String>,
}

impl LoggingConfiguration {
    /// Apply the transition the clock already owes this configuration at `now`.
    ///
    /// The due instant was recorded by the call that started the transition, so it is
    /// owed whether or not anybody calls again. Every transient status this emulator
    /// puts a configuration in resolves to `ACTIVE`.
    fn materialize_due(&mut self, now: SystemTime) {
        if self.transition_due.is_some_and(|due| now >= due) {
            self.transition_due = None;
            self.status_code = LOGGING_STATUS_ACTIVE.to_string();
        }
    }
}

/// One destination of a query logging configuration: the `LoggingDestination` members
/// the service persists (`cloudWatchLogs.logGroupArn` and `filters.qspThreshold`), held
/// as plain data so the wire shapes are built from the STORED record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryLoggingDestination {
    /// `CloudWatchLogDestination$logGroupArn` — the log group the vended log data is
    /// published to.
    pub log_group_arn: String,
    /// `LoggingFilter$qspThreshold` — the Query Samples Processed threshold above which
    /// a query is logged (`@range(min: 0)`).
    pub qsp_threshold: i64,
}

/// One workspace's query logging configuration: every member
/// `QueryLoggingConfigurationMetadata` reports, plus the lifecycle bookkeeping.
///
/// A workspace holds at most one, which is why the store keys them by `workspaceId`
/// (`DescribeQueryLoggingConfiguration` takes nothing else to find one by).
#[derive(Debug, Clone)]
pub struct QueryLoggingConfiguration {
    pub workspace_id: String,
    /// One of [`QUERY_LOGGING_STATUS_CREATING`], [`QUERY_LOGGING_STATUS_ACTIVE`],
    /// [`QUERY_LOGGING_STATUS_UPDATING`], [`QUERY_LOGGING_STATUS_DELETING`].
    pub status_code: String,
    /// `QueryLoggingConfigurationStatus$statusReason` — "If there is a failure, the
    /// reason for the failure", so it stays `None` while nothing failed.
    pub status_reason: Option<String>,
    /// `LoggingDestinations` is modelled `len 1..1`, so this holds exactly one entry.
    pub destinations: Vec<QueryLoggingDestination>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// When the clock owes this configuration's next transition, if one is pending:
    /// `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the `CreateQueryLoggingConfiguration` call that created
    /// this configuration, never overwritten — so replaying that exact create still
    /// answers from the stored record after later updates, which is what
    /// `@idempotencyToken` asks for.
    pub create_token: Option<String>,
    /// The `clientToken` of the call that last CHANGED this configuration (an update or
    /// the start of a deletion). Replaying that exact token is the SAME request, so a
    /// handler answers from the stored record instead of applying the change twice.
    pub client_token: Option<String>,
}

/// Per-service state for com-amazonaws-amp-amazonprometheusservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmpAmazonprometheusserviceState {
    /// Every workspace this emulator holds, keyed by `workspaceId`.
    pub workspaces: HashMap<String, Workspace>,
    /// Each workspace's query logging configuration, keyed by `workspaceId`. A
    /// workspace holds at most one, and it is dropped with the workspace.
    pub query_logging: HashMap<String, QueryLoggingConfiguration>,
    /// `workspaceArn -> workspaceId`, so the `/tags/{resourceArn}` operations
    /// resolve an ARN without scanning every workspace.
    pub arn_index: HashMap<String, String>,
    /// `clientToken -> workspaceId`, which is what makes `CreateWorkspace`
    /// idempotent per its `@idempotencyToken`.
    pub client_tokens: HashMap<String, String>,
    /// Counts the ids this store has minted; see [`mint_workspace_id`].
    pub next_id: u64,
}

impl ComAmazonawsAmpAmazonprometheusserviceState {
    /// Apply every lifecycle transition the clock already owes at `now`.
    ///
    /// Each pending transition was due at an instant recorded when the call that
    /// started it ran, so it is owed whether or not anybody calls again; applying it
    /// here is materialising a change the clock made, not a side effect of the read.
    /// Call it FIRST in every handler, so each one sees the same state a caller
    /// would after any other.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let mut deleted: Vec<String> = Vec::new();
        for workspace in self.workspaces.values_mut() {
            if workspace.configuration.status_due.is_some_and(|due| now >= due) {
                workspace.configuration.status_due = None;
                workspace.configuration.status_code = CONFIG_STATUS_ACTIVE.to_string();
                workspace.configuration.status_reason = None;
                // The token identified the update that just settled, so it stops
                // standing for one: a later call carrying it is a new request.
                workspace.configuration.update_token = None;
            }
            if let Some(logging) = workspace.logging_configuration.as_mut() {
                logging.materialize_due(now);
            }
            let Some(due) = workspace.transition_due else {
                continue;
            };
            if now < due {
                continue;
            }
            workspace.transition_due = None;
            if workspace.status_code == STATUS_DELETING {
                // Dropping the workspace drops its logging configuration with it.
                deleted.push(workspace.workspace_id.clone());
            } else {
                workspace.status_code = STATUS_ACTIVE.to_string();
            }
        }
        for workspace_id in deleted {
            if let Some(workspace) = self.workspaces.remove(&workspace_id) {
                self.arn_index.remove(&workspace.arn);
            }
            self.client_tokens.retain(|_, id| id != &workspace_id);
            // A query logging configuration belongs to its workspace, so it goes when
            // the workspace does.
            self.query_logging.remove(&workspace_id);
        }

        let mut dropped: Vec<String> = Vec::new();
        for configuration in self.query_logging.values_mut() {
            let Some(due) = configuration.transition_due else {
                continue;
            };
            if now < due {
                continue;
            }
            configuration.transition_due = None;
            if configuration.status_code == QUERY_LOGGING_STATUS_DELETING {
                dropped.push(configuration.workspace_id.clone());
            } else {
                configuration.status_code = QUERY_LOGGING_STATUS_ACTIVE.to_string();
            }
        }
        for workspace_id in dropped {
            self.query_logging.remove(&workspace_id);
        }
    }

    /// The `workspaceId` the stored ARN `resource_arn` belongs to, if any.
    pub fn workspace_id_of_arn(&self, resource_arn: &str) -> Option<String> {
        self.arn_index.get(resource_arn).cloned()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmpAmazonprometheusserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmpAmazonprometheusserviceState>)
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope (an anonymous or host-routed request).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A workspace ARN: `arn:aws:aps:<region>:<account>:workspace/<workspaceId>`, as
/// `WorkspaceArn`'s pattern `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:workspace/.+$`
/// requires. The account segment comes from the engine, never from a literal.
pub fn workspace_arn(region: &str, workspace_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("workspace/{workspace_id}"))
}

/// The workspace's Prometheus endpoint, in the form
/// `WorkspaceDescription$prometheusEndpoint` documents: "For example,
/// `https://aps-workspaces.<region>.amazonaws.com/workspaces/ws-.../api/v1/`".
pub fn prometheus_endpoint(region: &str, workspace_id: &str) -> String {
    format!("https://aps-workspaces.{region}.amazonaws.com/workspaces/{workspace_id}/api/v1/")
}

/// A fresh workspace id of the shape the service mints: `ws-` followed by a UUID,
/// which `WorkspaceId`'s pattern (`[0-9A-Za-z][-.0-9A-Z_a-z]*`, length 1..64)
/// accepts and `WorkspaceDescription$workspaceId` shows ("For example,
/// `ws-example1-1234-abcd-5678-ef90abcd1234`").
///
/// The UUID is a digest of this store's id counter (unique within the process) folded
/// with a per-process seed (so two emulator processes do not mint the same ids), hashed
/// so the id discloses neither. It takes the counter rather than `&mut self` so a
/// handler holding a stored record borrowed can still mint — which is what lets it mint
/// only AFTER every refusal has been answered.
pub fn mint_workspace_id(counter: &mut u64) -> String {
    *counter += 1;
    let mut hasher = Sha256::new();
    hasher.update(b"workspace");
    hasher.update(counter.to_be_bytes());
    hasher.update(process_seed().to_be_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = hex::encode(digest);
    format!(
        "ws-{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A seed drawn once per process, so ids minted by two emulator processes from the same
/// counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    })
}

/// The alias to STORE for a given `alias` member.
///
/// `CreateWorkspace$alias` and `UpdateWorkspaceAlias$alias` both document that
/// "blank spaces at the beginning or end of the alias that you specify will be
/// trimmed from the value used", so the stored value is the trimmed one. An alias
/// that is nothing but blank space trims to the empty string, which `WorkspaceAlias`
/// (length 1..100) cannot hold, so it leaves the workspace with no alias at all.
pub fn normalize_alias(alias: Option<&str>) -> Option<String> {
    alias
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(ToString::to_string)
}

/// How many tags one resource holds. `TagMap` is modelled `len ..50` — which bounds a
/// single request — and 50 is also the service's documented per-resource tag limit, so
/// `TagResource` enforces it on the ACCUMULATED map. That is both the fidelity rule and
/// what stops a stored tag map growing without end as a caller repeats the operation
/// with fresh keys.
pub const TAGS_PER_RESOURCE_LIMIT: usize = 50;

/// The tag-key prefix the service reserves: `TagResource$tags` documents "Keys must
/// not begin with aws:".
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The reserved tag key a request carries, if any — the lowest one, so the refusal
/// names the same key however the request's map was iterated.
pub fn reserved_tag_key<'a, I: IntoIterator<Item = &'a String>>(keys: I) -> Option<&'a str> {
    keys.into_iter()
        .map(String::as_str)
        .filter(|key| key.starts_with(RESERVED_TAG_KEY_PREFIX))
        .min()
}

/// The label set limits to STORE for a request's `limitsPerLabelSet`.
///
/// `LabelSet` is a map on the wire, so each entry's labels are kept sorted; the list
/// itself keeps the request's order, which is the order a later describe reports.
pub fn store_label_set_limits(limits: &[crate::ops::types::LimitsPerLabelSet]) -> Vec<LabelSetLimit> {
    limits
        .iter()
        .map(|entry| LabelSetLimit {
            label_set: entry
                .label_set
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            max_series: entry.limits.max_series,
        })
        .collect()
}

/// The stored label set limits as the `LimitsPerLabelSetList` the wire shapes carry.
/// Always a list, empty when no label set limit is enforced, so a reader sees `[]`
/// rather than an absent member — the same choice [`tag_map`] makes for `TagMap`.
pub fn label_set_limit_list(
    limits: &[LabelSetLimit],
) -> Vec<crate::ops::types::LimitsPerLabelSet> {
    limits
        .iter()
        .map(|entry| crate::ops::types::LimitsPerLabelSet {
            label_set: entry
                .label_set
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            limits: crate::ops::types::LimitsPerLabelSetEntry {
                max_series: entry.max_series,
            },
        })
        .collect()
}

/// The stored configuration's status, as the modelled `LoggingConfigurationStatus` the
/// logging operations answer with — so every one of them reports it the same way.
pub fn logging_configuration_status(
    logging: &LoggingConfiguration,
) -> crate::ops::types::LoggingConfigurationStatus {
    crate::ops::types::LoggingConfigurationStatus {
        status_code: logging.status_code.clone(),
        status_reason: logging.status_reason.clone(),
    }
}

/// The stored tags as the `TagMap` the wire shapes carry. Always a map, empty when
/// the resource has no tags, so a reader sees `{}` rather than an absent member.
pub fn tag_map(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The query logging destinations to STORE for a request's `destinations` member: the
/// two members `LoggingDestination` carries, kept as plain data.
pub fn stored_query_logging_destinations(
    destinations: &[crate::ops::types::LoggingDestination],
) -> Vec<QueryLoggingDestination> {
    destinations
        .iter()
        .map(|destination| QueryLoggingDestination {
            log_group_arn: destination.cloud_watch_logs.log_group_arn.clone(),
            qsp_threshold: destination.filters.qsp_threshold,
        })
        .collect()
}

/// The `LoggingDestinations` a query logging reader reports, rebuilt from the STORED
/// record rather than from whatever the request carried.
pub fn wire_query_logging_destinations(
    destinations: &[QueryLoggingDestination],
) -> Vec<crate::ops::types::LoggingDestination> {
    destinations
        .iter()
        .map(|destination| crate::ops::types::LoggingDestination {
            cloud_watch_logs: crate::ops::types::CloudWatchLogDestination {
                log_group_arn: destination.log_group_arn.clone(),
            },
            filters: crate::ops::types::LoggingFilter {
                qsp_threshold: destination.qsp_threshold,
            },
        })
        .collect()
}

/// The `QueryLoggingConfigurationStatus` a stored configuration reports — the one
/// derivation `CreateQueryLoggingConfiguration`, `UpdateQueryLoggingConfiguration` and
/// `DescribeQueryLoggingConfiguration` all answer with.
pub fn query_logging_status(
    configuration: &QueryLoggingConfiguration,
) -> crate::ops::types::QueryLoggingConfigurationStatus {
    crate::ops::types::QueryLoggingConfigurationStatus {
        status_code: configuration.status_code.clone(),
        status_reason: configuration.status_reason.clone(),
    }
}

/// The `QueryLoggingConfigurationMetadata` a stored configuration reports.
pub fn query_logging_metadata(
    configuration: &QueryLoggingConfiguration,
) -> crate::ops::types::QueryLoggingConfigurationMetadata {
    crate::ops::types::QueryLoggingConfigurationMetadata {
        status: query_logging_status(configuration),
        workspace: configuration.workspace_id.clone(),
        destinations: wire_query_logging_destinations(&configuration.destinations),
        created_at: ::aws_smithy_types::DateTime::from(configuration.created_at),
        modified_at: ::aws_smithy_types::DateTime::from(configuration.modified_at),
    }
}
