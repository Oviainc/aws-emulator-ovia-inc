// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amp-amazonprometheusservice service.
//!
//! Besides the state struct itself this module holds the few derivations every
//! workspace handler needs (the ARN, the Prometheus endpoint, the request's region,
//! a fresh workspace id), so `src/ops/*.rs` share ONE copy of each rule.

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
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
///
/// The same three literals are entries of `com.amazonaws.amp#AnomalyDetectorStatusCode`
/// too (`crate::ops::types::ANOMALY_DETECTOR_STATUS_CODE_VALUES`), so an anomaly
/// detector's lifecycle reports them from these same constants rather than from a
/// second copy of the strings.
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
/// created/updated."; `UPDATING` — "Logging configuration is being updated.
/// Update/Deletion is disallowed until logging configuration is ACTIVE and workspace
/// status is ACTIVE." (`crate::ops::types::LOGGING_CONFIGURATION_STATUS_CODE_VALUES`
/// carries the full list, including the two failure codes nothing here reaches: this
/// emulator runs no vended-log delivery that could fail.)
pub const LOGGING_STATUS_CREATING: &str = "CREATING";
/// See [`LOGGING_STATUS_CREATING`].
pub const LOGGING_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`LOGGING_STATUS_CREATING`].
pub const LOGGING_STATUS_UPDATING: &str = "UPDATING";

/// The `resourceType` the modelled `ResourceNotFoundException` reports for a workspace's
/// rules-and-alerting logging configuration. The member is a free-form `String`
/// documented as "Type of the resource affected", so the emulator reports this
/// resource's name in the service's `AWS::APS::` namespace, the way
/// [`WORKSPACE_RESOURCE_TYPE`] does for the workspace itself.
pub const LOGGING_CONFIGURATION_RESOURCE_TYPE: &str = "AWS::APS::LoggingConfiguration";

/// `WorkspacePolicyStatusCode` values a workspace's resource-based policy moves
/// through here. Each is an entry of the modelled
/// `com.amazonaws.amp#WorkspacePolicyStatusCode` enum, whose own documentation states
/// the lifecycle rule this crate enforces — `CREATING`: "Resource-based Policy is
/// being created. Update/Deletion is disallowed until Resource-based Policy is ACTIVE
/// and Workspace is ACTIVE.", and `ACTIVE` / `UPDATING` / `DELETING` repeat that same
/// sentence. (`crate::ops::types::WORKSPACE_POLICY_STATUS_CODE_VALUES` carries the
/// full list.)
pub const POLICY_STATUS_CREATING: &str = "CREATING";
/// See [`POLICY_STATUS_CREATING`].
pub const POLICY_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`POLICY_STATUS_CREATING`].
pub const POLICY_STATUS_UPDATING: &str = "UPDATING";
/// See [`POLICY_STATUS_CREATING`].
pub const POLICY_STATUS_DELETING: &str = "DELETING";

/// The revision id a caller may name on the first put for a workspace that holds no
/// resource-based policy: `PutResourcePolicyRequest$revisionId` documents "For the
/// first PUT request on a workspace that doesn't have an existing resource policy, you
/// can specify `NO_POLICY` as the revision ID."
pub const NO_POLICY_REVISION_ID: &str = "NO_POLICY";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a workspace's resource-based policy. The member is a free-form `String`
/// documented as "Type of the resource affected", so the emulator reports the
/// resource's AWS type name.
pub const POLICY_RESOURCE_TYPE: &str = "AWS::APS::ResourcePolicy";

/// How long a workspace stays in a transient status before the clock owes the next
/// transition (`CREATING` -> `ACTIVE`, `DELETING` -> removed, a workspace
/// configuration's `UPDATING` -> `ACTIVE`), and the same for a resource-based policy
/// (`CREATING`/`UPDATING` -> `ACTIVE`, `DELETING` -> removed).
///
/// CHOSEN, not documented: Amazon Managed Service for Prometheus publishes no
/// duration for any of these transitions, so this is the emulator's own span — long
/// enough that a caller acting straight after
/// `CreateWorkspace`/`DeleteWorkspace`/`UpdateWorkspaceConfiguration` still observes
/// the transient status and the state errors that go with it, short enough
/// that a workflow does not wait on it. Nothing sleeps for it: the due instant is
/// stored and applied by [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`].
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// `RuleGroupsNamespaceStatusCode` values a rule groups namespace moves through here.
/// Each is an entry of the modelled `com.amazonaws.amp#RuleGroupsNamespaceStatusCode`
/// enum, whose own documentation states the lifecycle rules this crate enforces:
/// `CREATING` — "Namespace is being created. Update/Deletion is disallowed until
/// namespace is ACTIVE and workspace status is ACTIVE."; `ACTIVE` — "Namespace has been
/// created/updated. …"; `UPDATING` — "Namespace is being updated. …"; `DELETING` —
/// "Namespace is being deleting. …". (`crate::ops::types::RULE_GROUPS_NAMESPACE_STATUS_CODE_VALUES`
/// carries the full list.) They are spelled separately from the `WorkspaceStatusCode`
/// constants above because they belong to a different modelled enum.
pub const NAMESPACE_STATUS_CREATING: &str = "CREATING";
/// See [`NAMESPACE_STATUS_CREATING`].
pub const NAMESPACE_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`NAMESPACE_STATUS_CREATING`].
pub const NAMESPACE_STATUS_UPDATING: &str = "UPDATING";
/// See [`NAMESPACE_STATUS_CREATING`].
pub const NAMESPACE_STATUS_DELETING: &str = "DELETING";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a workspace. The member is a free-form `String` documented as "Type of
/// the resource affected", so the emulator reports the resource's AWS type name.
pub const WORKSPACE_RESOURCE_TYPE: &str = "AWS::APS::Workspace";

/// The `resourceType` those same exceptions report for a rule groups namespace. The
/// name is the one the model itself gives the resource: `RuleGroupsNamespaceArn`
/// carries `@arnReference(type: "AWS::APS::RuleGroupsNamespace")`.
pub const RULE_GROUPS_NAMESPACE_RESOURCE_TYPE: &str = "AWS::APS::RuleGroupsNamespace";

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

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace this
/// account does not hold.
///
/// One CONSTANT sentence, exactly as [`QUERY_LOGGING_NOT_FOUND_MESSAGE`] is for a
/// configuration: `ResourceNotFoundException` models `resourceId` (@required) beside
/// `message`, so the identifier of the missing resource has its own home and the
/// sentence carries nothing from the request. Every per-workspace operation shares this
/// one constant, so a caller comparing the not-found message across calls — or across
/// operations — reads the same value every time.
pub const WORKSPACE_NOT_FOUND_MESSAGE: &str = "Workspace not found.";

/// The `message` the ARN-addressed `/tags/{resourceArn}` operations report for a
/// `resourceArn` that resolves to nothing — RECORDED BEHAVIOUR, like
/// [`RESOURCE_NOT_FOUND_CODE`] beside it. Constant and ARN-free for the same reason
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] is: nothing of the request is echoed back into it.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code the ARN-addressed `/tags/{resourceArn}` operations answer for a
/// `resourceArn` that resolves to no live resource.
///
/// RECORDED BEHAVIOUR: those three operations model `ResourceNotFoundException`
/// (@httpError 404) — `smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
/// — but the recorded real-aps response for an ARN that names nothing carries the code
/// `NotFoundException` at that same 404, whatever ARN type it names. A client branches on
/// the CODE, so that is what this service answers; the code is not a shape the model
/// declares, which is why it is raised through [`arn_not_found`] and the synthetic
/// [`crate::scaffolded::NotFoundException`] rather than through an operation's modelled
/// error enum.
///
/// The operations addressed by RESOURCE ID rather than by ARN keep the modelled
/// `ResourceNotFoundException`, which is what they are recorded answering (a
/// `DescribeAnomalyDetector` for a detector that is gone, for one).
pub const RESOURCE_NOT_FOUND_CODE: &str = "NotFoundException";

/// The `message` the modelled `ResourceNotFoundException` reports for a rule groups
/// namespace a workspace does not hold. Constant and id-free for the same reason
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] is; the namespace name is reported in `resourceId`.
pub const RULE_GROUPS_NAMESPACE_NOT_FOUND_MESSAGE: &str = "Rule groups namespace not found.";

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace that
/// holds no rules-and-alerting logging configuration — constant and id-free for the same
/// reason [`QUERY_LOGGING_NOT_FOUND_MESSAGE`] is.
pub const LOGGING_NOT_FOUND_MESSAGE: &str = "Logging configuration not found.";

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace that
/// holds no resource-based policy — constant and id-free for the same reason
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] is; the workspace is reported in `resourceId`.
pub const POLICY_NOT_FOUND_MESSAGE: &str = "Resource-based policy not found.";

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

/// How many rule groups namespaces ONE workspace holds before
/// `CreateRuleGroupsNamespace` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// Amazon Managed Service for Prometheus publishes an adjustable quota on rule groups
/// namespaces per workspace, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on the store, not a
/// claim about any account's limit. It is what stops one workspace's namespace map
/// growing without end; the map itself is owned by the workspace record, so deleting
/// the workspace releases every namespace in it.
pub const RULE_GROUPS_NAMESPACES_PER_WORKSPACE: usize = 1_000;

/// The largest page `ListWorkspaces` serves, and the page size it uses when the request
/// names no `maxResults`: `ListWorkspacesRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)` and documented "The maximum number of workspaces to
/// return per request. The default is 100." The handler clamps into `1..=MAX` itself
/// rather than trusting the boundary validator, so a caller cannot choose the amount of
/// work one request does.
pub const WORKSPACE_PAGE_LIMIT: i32 = 1_000;
/// See [`WORKSPACE_PAGE_LIMIT`].
pub const WORKSPACE_PAGE_DEFAULT: i32 = 100;

/// The largest page `ListScrapers` serves, and the page size it uses when the request
/// names no `maxResults`: `ListScrapersRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)` and documented "The maximum number of scrapers to return
/// in one ListScrapers operation. The range is 1-1000. If you omit this parameter, the
/// default of 100 is used." Clamped by the handler rather than trusted from the request,
/// exactly as [`WORKSPACE_PAGE_LIMIT`] is.
pub const SCRAPER_PAGE_LIMIT: i32 = 1_000;
/// See [`SCRAPER_PAGE_LIMIT`].
pub const SCRAPER_PAGE_DEFAULT: i32 = 100;

/// The largest page `ListRuleGroupsNamespaces` serves, and the page size it uses when
/// the request names no `maxResults`: `ListRuleGroupsNamespacesRequest$maxResults` is
/// modelled `@range(min: 1, max: 1000)` and documented "The maximum number of results to
/// return. The default is 100." Clamped by the handler rather than trusted from the
/// request, exactly as [`WORKSPACE_PAGE_LIMIT`] is.
pub const RULE_GROUPS_NAMESPACE_PAGE_LIMIT: i32 = 1_000;
/// See [`RULE_GROUPS_NAMESPACE_PAGE_LIMIT`].
pub const RULE_GROUPS_NAMESPACE_PAGE_DEFAULT: i32 = 100;

/// The filter keys `ListScrapers` honours, as `ListScrapersRequest$filters` documents
/// them: "Keys include `status`, `sourceArn`, `destinationArn`, and `alias`."
///
/// A key that is not one of these four is not a filter this service defines, and it is
/// IGNORED rather than refused: the member documents the keys it includes without saying
/// that any other is rejected, so refusing one would invent a refusal. The `@httpQuery`
/// members (`nextToken`, `maxResults`) never reach the map at all — the
/// `@httpQueryParams` binding in `crate::ops::list_scrapers` leaves them out, as the
/// trait requires.
pub const SCRAPER_FILTER_STATUS: &str = "status";
/// See [`SCRAPER_FILTER_STATUS`].
pub const SCRAPER_FILTER_SOURCE_ARN: &str = "sourceArn";
/// See [`SCRAPER_FILTER_STATUS`].
pub const SCRAPER_FILTER_DESTINATION_ARN: &str = "destinationArn";
/// See [`SCRAPER_FILTER_STATUS`].
pub const SCRAPER_FILTER_ALIAS: &str = "alias";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException` /
/// `ServiceQuotaExceededException` report for an anomaly detector — the resource's AWS
/// type name, as [`WORKSPACE_RESOURCE_TYPE`] is for a workspace.
pub const ANOMALY_DETECTOR_RESOURCE_TYPE: &str = "AWS::APS::AnomalyDetector";

/// The `message` the modelled `ResourceNotFoundException` carries for an anomaly
/// detector that does not resolve.
///
/// The id itself goes in that exception's `resourceId` member — which is what the
/// member is for — so the message is the fixed sentence the service answers with,
/// rather than a second copy of the id.
pub const ANOMALY_DETECTOR_NOT_FOUND_MESSAGE: &str = "Anomaly detector not found.";

/// How many anomaly detectors one workspace holds before `CreateAnomalyDetector`
/// refuses with the modelled `ServiceQuotaExceededException`.
///
/// Amazon Managed Service for Prometheus publishes no fixed number here, and this
/// emulator models neither Service Quotas nor an account's adjustments, so this is the
/// emulator's OWN bound on the store — the role [`WORKSPACE_QUOTA`] plays for
/// workspaces. It is what stops a workspace's detector map (and the client-token map
/// the same deletion releases) growing without end.
pub const ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA: usize = 1_000;

/// The PromQL aggregation operators a Random Cut Forest query may be wrapped by.
///
/// Source: `RandomCutForestConfiguration$query`, which states "Random Cut Forest
/// queries must be wrapped by a supported PromQL aggregation operator" and then lists
/// them — "Supported PromQL aggregation operators: `avg`, `count`, `group`, `max`,
/// `min`, `quantile`, `stddev`, `stdvar`, and `sum`". Sorted, so a refusal names them
/// in one stable order.
pub const RANDOM_CUT_FOREST_AGGREGATION_OPERATORS: &[&str] = &[
    "avg", "count", "group", "max", "min", "quantile", "stddev", "stdvar", "sum",
];

/// This service's ARN namespace (`OPERATIONS.md`: "ARN namespace: `aps`").
const ARN_SERVICE: &str = "aps";

/// Region used when a request carries no SigV4 credential scope to read one from —
/// the region the emulator's own generated client config defaults to.
const DEFAULT_REGION: &str = "us-east-1";

/// One stored resource-based policy: the members `DescribeResourcePolicy` reports,
/// plus the bookkeeping its lifecycle and its optimistic lock need. It lives ON the
/// workspace it is attached to (`/workspaces/{workspaceId}/policy`), so deleting the
/// workspace releases the policy with it.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// The JSON policy document exactly as it was put — the service stores the
    /// caller's document and reports it back.
    pub policy_document: String,
    /// One of [`POLICY_STATUS_CREATING`], [`POLICY_STATUS_ACTIVE`],
    /// [`POLICY_STATUS_UPDATING`], [`POLICY_STATUS_DELETING`].
    pub status_code: String,
    /// The revision this document is at: minted by [`mint_revision_id`] on every put,
    /// and what a later put or delete names to take the optimistic lock.
    pub revision_id: String,
    /// When the clock owes this policy's next lifecycle transition, if one is pending:
    /// `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the `PutResourcePolicy` call that produced this revision,
    /// which is what makes that call idempotent per its `@idempotencyToken`: replaying
    /// it answers with this revision instead of minting a second one.
    pub client_token: Option<String>,
    /// The `clientToken` of the `DeleteResourcePolicy` call that started a pending
    /// deletion, so replaying that exact call stays idempotent rather than conflicting
    /// with the deletion it already started.
    pub delete_token: Option<String>,
}

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
    /// The anomaly detectors this workspace holds, keyed by `anomalyDetectorId`. A
    /// detector is a CHILD of its workspace
    /// (`/workspaces/{workspaceId}/anomalydetectors`), so it lives in the workspace's
    /// own record: removing the workspace removes its detectors with it, and no second
    /// map can disagree about which workspace a detector belongs to.
    pub anomaly_detectors: HashMap<String, AnomalyDetector>,
    /// `clientToken -> anomalyDetectorId`, which is what makes `CreateAnomalyDetector`
    /// idempotent per its `@idempotencyToken`.
    pub anomaly_detector_tokens: HashMap<String, String>,
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
    /// The resource-based policy attached to this workspace, if one is attached:
    /// `PutResourcePolicy` creates or replaces it, `DescribeResourcePolicy` reads it
    /// and `DeleteResourcePolicy` removes it.
    pub policy: Option<ResourcePolicy>,
    /// This workspace's rule groups namespaces, keyed by `RuleGroupsNamespaceName`
    /// (unique within a workspace, which is what `CreateRuleGroupsNamespace` refuses a
    /// second time). They hang off the workspace record because the namespace is a
    /// child of the workspace: dropping the workspace releases every one of them.
    /// Sorted, so a listing reports one stable order.
    pub rule_groups_namespaces: BTreeMap<String, RuleGroupsNamespace>,
}

/// One stored rule groups namespace: every member `RuleGroupsNamespaceDescription` /
/// `RuleGroupsNamespaceSummary` report, plus the bookkeeping the lifecycle needs.
#[derive(Debug, Clone)]
pub struct RuleGroupsNamespace {
    pub name: String,
    pub arn: String,
    /// The rules file the namespace holds, exactly as the request carried it:
    /// `RuleGroupsNamespaceData` is a blob, so the bytes are stored unaltered and read
    /// back unaltered.
    pub data: Vec<u8>,
    /// One of [`NAMESPACE_STATUS_CREATING`], [`NAMESPACE_STATUS_ACTIVE`],
    /// [`NAMESPACE_STATUS_UPDATING`], [`NAMESPACE_STATUS_DELETING`].
    pub status_code: String,
    /// `RuleGroupsNamespaceStatus$statusReason` — "The reason for the failure, if any",
    /// so it is absent while nothing failed.
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    /// When the namespace was "most recently changed"
    /// (`RuleGroupsNamespaceDescription$modifiedAt`): the creation instant until a
    /// later call changes the record.
    pub modified_at: SystemTime,
    /// Sorted so a listing and a describe report the tags in one stable order.
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this namespace's next lifecycle transition, if one is
    /// pending: `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the call that created this namespace, which is what makes
    /// `CreateRuleGroupsNamespace` idempotent per its `@idempotencyToken`.
    pub create_token: Option<String>,
    /// The `clientToken` of the `PutRuleGroupsNamespace` update still in flight, which
    /// is what makes that `@idempotent` operation idempotent per its
    /// `@idempotencyToken`: replaying it answers with the update it already started
    /// instead of starting a second one. Forgotten the moment the update settles (see
    /// [`materialize_namespaces_due`]), so a later call reusing the token is a new
    /// request.
    pub update_token: Option<String>,
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
    /// One of [`LOGGING_STATUS_CREATING`], [`LOGGING_STATUS_ACTIVE`],
    /// [`LOGGING_STATUS_UPDATING`].
    pub status_code: String,
    /// `LoggingConfigurationStatus$statusReason` — "If failed, the reason for the
    /// failure", so `None` while nothing has failed.
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    /// When the configuration was most recently CHANGED, i.e. by the call that created
    /// it or last updated what it points at — not by the clock moving its status on.
    pub modified_at: SystemTime,
    /// When the clock owes this configuration's move out of a transient status
    /// (`CREATING`/`UPDATING` -> `ACTIVE`), if one is pending.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the call that created it, never overwritten — so replaying
    /// that exact create still answers from the stored record after later updates, which
    /// is what `@idempotencyToken` asks for.
    pub client_token: Option<String>,
    /// The `clientToken` of the call that last UPDATED it. Replaying that exact token is
    /// the SAME request, so `UpdateLoggingConfiguration` answers from the stored record
    /// instead of applying the change twice. Kept apart from [`Self::client_token`] for
    /// the same reason `QueryLoggingConfiguration` keeps its create token apart.
    pub update_token: Option<String>,
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

/// One stored scraper: every member `ScraperSummary` reports.
///
/// `ListScrapers` reports the scrapers this store HOLDS, so the listing is computed from
/// state rather than announced. Nothing writes this map yet — `CreateScraper` is still
/// scaffolded and answers 501, which is its own stub issue — so an account that has never
/// created one lists the empty page AWS lists for it; when `CreateScraper` lands it
/// inserts here and the listing needs no change.
#[derive(Debug, Clone)]
pub struct Scraper {
    pub scraper_id: String,
    pub arn: String,
    /// `None` when the scraper carries no alias; `ScraperSummary$alias` is optional.
    pub alias: Option<String>,
    pub role_arn: String,
    /// One entry of the modelled `ScraperStatusCode` enum
    /// (`crate::ops::types::SCRAPER_STATUS_CODE_VALUES`).
    pub status_code: String,
    /// "If there is a failure, the reason for the failure"
    /// (`ScraperSummary$statusReason`), so `None` while nothing has failed.
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// "The Amazon EKS cluster from which the scraper collects metrics."
    pub source: crate::ops::types::Source,
    /// "The destination where the scraper sends metrics."
    pub destination: crate::ops::types::Destination,
    pub role_configuration: Option<crate::ops::types::RoleConfiguration>,
    /// `ScraperSummary$exporters` is modelled `len ..1`; empty when none is configured.
    pub exporters: Vec<crate::ops::types::ExporterConfiguration>,
}

/// One stored anomaly detector: every member `AnomalyDetectorDescription` /
/// `AnomalyDetectorSummary` report, plus the bookkeeping its lifecycle needs.
#[derive(Debug, Clone)]
pub struct AnomalyDetector {
    pub anomaly_detector_id: String,
    /// The workspace this detector belongs to — the `ws-` segment of its ARN.
    pub workspace_id: String,
    pub arn: String,
    /// `AnomalyDetectorDescription$alias` is **required**, so a detector always has one.
    pub alias: String,
    pub evaluation_interval_in_seconds: i32,
    pub missing_data_action: Option<crate::ops::types::AnomalyDetectorMissingDataAction>,
    pub configuration: crate::ops::types::AnomalyDetectorConfiguration,
    /// Sorted, so a describe reports the labels in one stable order.
    pub labels: BTreeMap<String, String>,
    /// One of [`STATUS_CREATING`], [`STATUS_ACTIVE`], [`STATUS_DELETING`].
    pub status_code: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// Sorted so a describe reports the tags in one stable order.
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this detector's next lifecycle transition, if one is
    /// pending: `CREATING` becomes `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
}

/// Per-service state for com-amazonaws-amp-amazonprometheusservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmpAmazonprometheusserviceState {
    /// Every workspace this emulator holds, keyed by `workspaceId`.
    pub workspaces: HashMap<String, Workspace>,
    /// Every scraper this emulator holds, keyed by `scraperId` — what `ListScrapers`
    /// reports. Ordered, so a page boundary means the same thing on every call.
    pub scrapers: BTreeMap<String, Scraper>,
    /// Each workspace's query logging configuration, keyed by `workspaceId`. A
    /// workspace holds at most one, and it is dropped with the workspace.
    pub query_logging: HashMap<String, QueryLoggingConfiguration>,
    /// `workspaceArn -> workspaceId`, so the `/tags/{resourceArn}` operations
    /// resolve an ARN without scanning every workspace.
    pub arn_index: HashMap<String, String>,
    /// `clientToken -> workspaceId`, which is what makes `CreateWorkspace`
    /// idempotent per its `@idempotencyToken`.
    pub client_tokens: HashMap<String, String>,
    /// Counts the identifiers this store has minted — workspace ids and resource-policy
    /// revision ids both; see [`mint_workspace_id`] and [`mint_revision_id`].
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
            // A detector's own pending transitions, owed on the same terms. A workspace
            // removed below takes its detectors with it, so this only has to settle the
            // detectors of a workspace that stays.
            let mut deleted_detectors: Vec<String> = Vec::new();
            for detector in workspace.anomaly_detectors.values_mut() {
                let Some(detector_due) = detector.transition_due else {
                    continue;
                };
                if now < detector_due {
                    continue;
                }
                detector.transition_due = None;
                if detector.status_code == STATUS_DELETING {
                    deleted_detectors.push(detector.anomaly_detector_id.clone());
                } else {
                    // Only the lifecycle status moves here — which `status` reports —
                    // so `modifiedAt` keeps the instant the record itself last changed.
                    detector.status_code = STATUS_ACTIVE.to_string();
                }
            }
            for detector_id in deleted_detectors {
                workspace.anomaly_detectors.remove(&detector_id);
                workspace
                    .anomaly_detector_tokens
                    .retain(|_, id| id != &detector_id);
            }

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
            // The attached policy's transition is owed on its own clock, independently
            // of the workspace's: a policy put on an active workspace becomes ACTIVE
            // without the workspace moving at all.
            let policy_removed = match workspace.policy.as_mut() {
                Some(policy) if policy.transition_due.is_some_and(|due| now >= due) => {
                    policy.transition_due = None;
                    if policy.status_code == POLICY_STATUS_DELETING {
                        true
                    } else {
                        policy.status_code = POLICY_STATUS_ACTIVE.to_string();
                        false
                    }
                }
                _ => false,
            };
            if policy_removed {
                workspace.policy = None;
            }
            materialize_namespaces_due(&mut workspace.rule_groups_namespaces, now);
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

/// Whether a workspace and its rules-and-alerting logging configuration are in the state
/// the modelled `LoggingConfigurationStatusCode` requires before an update or a deletion
/// may act on that configuration.
///
/// Every entry of that enum documents the same rule: "Update/Deletion is disallowed until
/// logging configuration is ACTIVE and workspace status is ACTIVE." Takes the two statuses
/// rather than the workspace, so a caller that has already answered the missing-resource
/// case reports the pair it refused on.
pub fn logging_mutation_allowed(workspace_status: &str, logging_status: &str) -> bool {
    workspace_status == STATUS_ACTIVE && logging_status == LOGGING_STATUS_ACTIVE
}

/// Whether a workspace and its attached policy are in the state the modelled
/// `WorkspacePolicyStatusCode` requires before a resource-policy put or delete may act.
///
/// Every entry of that enum documents the same rule: "Update/Deletion is disallowed
/// until Resource-based Policy is ACTIVE and Workspace is ACTIVE." A workspace with no
/// policy attached yet has nothing to wait for, so only the workspace's own status
/// gates the first put.
pub fn policy_mutation_allowed(workspace: &Workspace) -> bool {
    workspace.status_code == STATUS_ACTIVE
        && workspace
            .policy
            .as_ref()
            .is_none_or(|policy| policy.status_code == POLICY_STATUS_ACTIVE)
}

/// Whether a request naming `revision_id` may act on the policy a workspace currently
/// holds — the optimistic lock `PutResourcePolicyRequest$revisionId` and
/// `DeleteResourcePolicyRequest$revisionId` describe.
///
/// "If you don't specify a revision ID, the policy is updated regardless of its current
/// revision", so an absent id always matches. Otherwise the id must name the policy's
/// current revision, and on a workspace that holds no policy the only id that matches
/// is [`NO_POLICY_REVISION_ID`]: "For the first PUT request on a workspace that doesn't
/// have an existing resource policy, you can specify `NO_POLICY` as the revision ID."
///
/// A request whose `revisionId` does not match is refused through
/// [`stale_revision_rejection`].
pub fn revision_matches(policy: Option<&ResourcePolicy>, revision_id: Option<&str>) -> bool {
    match (policy, revision_id) {
        (_, None) => true,
        (None, Some(revision_id)) => revision_id == NO_POLICY_REVISION_ID,
        (Some(policy), Some(revision_id)) => revision_id == policy.revision_id,
    }
}

/// Apply every rule-groups-namespace transition the clock owes at `now` inside one
/// workspace: a namespace whose due instant has passed becomes `ACTIVE`, or is removed
/// when it was being deleted. Called from
/// [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`], for the same
/// reason — the transition was owed at an instant recorded when the call that started
/// it ran, whether or not anybody calls again.
fn materialize_namespaces_due(
    namespaces: &mut BTreeMap<String, RuleGroupsNamespace>,
    now: SystemTime,
) {
    namespaces.retain(|_, namespace| match namespace.transition_due {
        Some(due) if now >= due => {
            namespace.transition_due = None;
            if namespace.status_code == NAMESPACE_STATUS_DELETING {
                false
            } else {
                namespace.status_code = NAMESPACE_STATUS_ACTIVE.to_string();
                // The token identified the update that just settled, so it stops
                // standing for one: a later call carrying it is a new request.
                namespace.update_token = None;
                true
            }
        }
        _ => true,
    });
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

/// An anomaly detector ARN:
/// `arn:aws:aps:<region>:<account>:anomalydetector/<workspaceId>/<anomalyDetectorId>`,
/// as `AnomalyDetectorArn`'s pattern
/// `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:anomalydetector/ws-.+/ad-.+$` requires.
/// The account segment comes from the engine, never from a literal.
pub fn anomaly_detector_arn(region: &str, workspace_id: &str, anomaly_detector_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("anomalydetector/{workspace_id}/{anomaly_detector_id}"),
    )
}

/// A rule groups namespace ARN, in the form
/// `RuleGroupsNamespaceDescription$arn` documents: "For example,
/// `arn:aws:aps:<region>:123456789012:rulegroupsnamespace/ws-example1-1234-abcd-5678-ef90abcd1234/rulesfile1`".
/// The account segment comes from the engine, never from a literal.
pub fn rule_groups_namespace_arn(region: &str, workspace_id: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("rulegroupsnamespace/{workspace_id}/{name}"),
    )
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
pub fn mint_workspace_id(counter: &mut u64) -> String {
    mint_id(b"workspace", "ws", counter)
}

/// A fresh anomaly detector id of the shape the service mints: `ad-` followed by a
/// UUID, which `AnomalyDetectorId`'s pattern (`^ad-[0-9A-Za-z][-.0-9A-Z_a-z]*$`,
/// length 1..64) requires. Minted exactly as [`mint_workspace_id`] is, off the same
/// store counter, so two resources never share an id's input.
pub fn mint_anomaly_detector_id(counter: &mut u64) -> String {
    mint_id(b"anomalydetector", "ad", counter)
}

/// A fresh resource-policy revision id: the opaque, server-minted value
/// `PutResourcePolicyResponse$revisionId` carries and a later put or delete names to
/// take the optimistic lock. Every put mints a new one, so no two revisions of a
/// policy — or of two policies — share an id. A revision names no resource kind, so it
/// is the bare UUID [`mint_uuid`] mints rather than a prefixed id.
pub fn mint_revision_id(counter: &mut u64) -> String {
    mint_uuid(b"resource-policy-revision", counter)
}

/// `<prefix>-<uuid>` for one resource `domain`: the shared body of
/// [`mint_workspace_id`] and [`mint_anomaly_detector_id`], so both kinds of id are
/// minted by ONE rule.
fn mint_id(domain: &[u8], prefix: &str, counter: &mut u64) -> String {
    format!("{prefix}-{}", mint_uuid(domain, counter))
}

/// A fresh UUID for one of this store's minted identifiers, in the 8-4-4-4-12 hex form.
///
/// It is a digest of `domain` (so identifiers of different kinds never collide) and
/// this store's id counter (unique within the process), folded with a per-process seed
/// (so two emulator processes do not mint the same ids) and hashed so the identifier
/// discloses neither. It takes the counter rather than `&mut self` so a handler holding
/// a stored record borrowed can still mint — which is what lets it mint only AFTER
/// every refusal has been answered.
fn mint_uuid(domain: &[u8], counter: &mut u64) -> String {
    *counter += 1;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(counter.to_be_bytes());
    hasher.update(process_seed().to_be_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = hex::encode(digest);
    format!(
        "{}-{}-{}-{}-{}",
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

/// The requirement a `tags` member fails when it carries a reserved key, worded as
/// [`constraint_requirement`] words a modelled constraint, so a rule the service
/// enforces itself reads like one the model declares.
pub fn reserved_tag_key_requirement() -> String {
    format!("Member must not contain a key beginning with {RESERVED_TAG_KEY_PREFIX}")
}

/// The refusal the three ARN-addressed `/tags/{resourceArn}` operations share for a
/// `resourceArn` that resolves to no live resource: [`RESOURCE_NOT_FOUND_CODE`] at 404,
/// carrying [`RESOURCE_NOT_FOUND_MESSAGE`].
///
/// ONE composition for all three, so `TagResource`, `UntagResource` and
/// `ListTagsForResource` cannot drift apart, and generic over the operation's modelled
/// error enum so each returns it through its own error channel.
pub fn arn_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::not_found(RESOURCE_NOT_FOUND_MESSAGE)
}

/// How a `revisionId` that does not name the policy's current revision is refused: the
/// modelled `ValidationException` (400) naming the `revisionId` member and the
/// requirement it failed.
///
/// RECORDED BEHAVIOUR: `PutResourcePolicy` and `DeleteResourcePolicy` model BOTH
/// `ValidationException` (@httpError 400) and `ConflictException` (@httpError 409), and a
/// recorded real-aps put carrying a revision the policy is no longer at is refused
/// `ValidationException`. The two codes are not interchangeable to a caller: a 400
/// `ValidationException` says "the value you sent is wrong — re-read the revision and
/// send the fresh one", while a 409 `ConflictException` reads as a transient
/// resource-state conflict that SDK and hand-written retry wrappers replay unchanged. The
/// optimistic lock is a MEMBER the caller got wrong, so it is refused as one.
///
/// `ConflictException` stays what it is for: the resource being in a status that forbids
/// the call at all ("Update/Deletion is disallowed until Resource-based Policy is ACTIVE
/// and Workspace is ACTIVE"), which no value in the request can fix.
pub fn stale_revision_rejection() -> crate::ops::types::ValidationException {
    member_validation_failed(REVISION_ID_MEMBER, &stale_revision_requirement())
}

/// The wire name of the optimistic-lock member, as `fieldList` reports it.
const REVISION_ID_MEMBER: &str = "revisionId";

/// The requirement a `revisionId` fails when it does not name the revision the policy is
/// currently at, worded as [`constraint_requirement`] words a modelled constraint — the
/// same reason [`reserved_tag_key_requirement`] is worded that way. It names the member's
/// own documented purpose ("Use this parameter to ensure that you are updating the correct
/// version of the policy") and echoes no value from the request.
pub fn stale_revision_requirement() -> String {
    format!(
        "Member must name the current revision of the resource-based policy, or {NO_POLICY_REVISION_ID} \
         while the workspace holds none"
    )
}

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
    string_map(tags, false)
}

/// The stored metric labels as the `PrometheusMetricLabelMap` the wire shapes carry.
/// ABSENT when the resource carries no labels — the member is optional and a detector
/// created without labels has none, so there is nothing to report.
pub fn label_map(labels: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    string_map(labels, true)
}

/// One stored string map as the wire map shape, so [`tag_map`] and [`label_map`] share
/// ONE projection and differ only in `empty_is_absent`: whether an empty map is
/// reported as `{}` or left out of the response altogether.
fn string_map(
    map: &BTreeMap<String, String>,
    empty_is_absent: bool,
) -> Option<HashMap<String, String>> {
    if empty_is_absent && map.is_empty() {
        return None;
    }
    Some(
        map.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The operator an expression APPLIES outermost, when it leads with one: the identifier
/// the expression STARTS with, immediately followed by its argument list (`sum(x)`) or by
/// an aggregation modifier (`sum by (job) (x)`, `sum without (job) (x)`) — the form
/// PromQL's aggregation syntax takes, `<aggr-op> [by|without (<labels>)] (<expression>)`.
///
/// This is how [`RANDOM_CUT_FOREST_AGGREGATION_OPERATORS`] is checked: a Random Cut
/// Forest query must be WRAPPED by one of those aggregation operators, so the whole
/// expression has to BE a call to one. `None` when the expression does not lead with a
/// call at all (a bare selector, a parenthesised or arithmetic expression), which is
/// exactly the shape that is not wrapped.
pub fn leading_call_operator(expression: &str) -> Option<&str> {
    let trimmed = expression.trim_start();
    let end = trimmed
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(trimmed.len());
    let (identifier, rest) = trimmed.split_at(end);
    // An identifier starts with a letter or `_`, so a leading digit is a number and not
    // a call at all.
    if identifier.is_empty() || identifier.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let rest = rest.trim_start();
    let applies = rest.starts_with('(')
        || ["by", "without"].iter().any(|modifier| {
            rest.strip_prefix(modifier)
                .is_some_and(|tail| tail.starts_with('(') || tail.starts_with(char::is_whitespace))
        });
    applies.then_some(identifier)
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

// =============================================================================
// The media type this service answers in
// =============================================================================

/// The media type every aps response announces.
///
/// RECORDED BEHAVIOUR: the model declares `restJson1`, whose serializer stamps
/// [`JSON_CONTENT_TYPE`] on a JSON body, but every recorded real-aps response — a
/// success and a failure alike, on 200s, 400s and 404s — carries
/// `application/x-amz-json-1.1`, which is what a client content-negotiating or
/// protocol-asserting on the response reads. The body is the same JSON either way, so
/// nothing but the announced type changes. Applied in
/// [`crate::service::EmulatedService`], which is the one place this service's responses
/// pass through.
pub const RESPONSE_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the `restJson1` serializer stamps on a JSON body — the one value
/// [`RESPONSE_CONTENT_TYPE`] replaces. A response whose body is NOT that JSON envelope
/// (a `@httpPayload` blob's `application/octet-stream`, a string payload's
/// `text/plain`) announces its own type and is left alone.
pub const JSON_CONTENT_TYPE: &str = "application/json";

// =============================================================================
// Validation: how this service states a refused request
// =============================================================================

/// The `reason` every aps `ValidationException` carries.
///
/// RECORDED BEHAVIOUR, like [`RESPONSE_CONTENT_TYPE`]: `ValidationException$reason` is
/// @required and targets `ValidationExceptionReason`, whose declared value set is
/// `UNKNOWN_OPERATION` / `CANNOT_PARSE` / `FIELD_VALIDATION_FAILED` / `OTHER`
/// (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`) — but every recorded
/// real-aps 400, across four operations in four fixtures, reports `Validation error`,
/// and `reason` is the member a client branches on. The shape is a STRING with a
/// declared value set rather than a closed enum type — which is why the member is a
/// `String` here and why a value outside that set still deserializes — so answering the
/// value the service actually answers costs a client nothing.
///
/// One value for every refusal this service raises, whether a member is at fault or the
/// request is wrong as a whole: that is the one value observed, and a client cannot read
/// two vocabularies for one error shape. WHICH member is at fault is reported in
/// `fieldList`, which is absent exactly when no single member is.
pub const VALIDATION_REASON: &str = "Validation error";

/// `ValidationException`'s own documentation, which is what a refusal naming no member
/// can say about itself: "The input fails to satisfy the constraints specified by an
/// Amazon Web Services service."
const VALIDATION_EXCEPTION_DOC: &str =
    "The input fails to satisfy the constraints specified by an Amazon Web Services service.";

/// This service's modelled 400 for the members a request got wrong.
///
/// ONE composition, shared by the request boundary and by every handler that refuses a
/// rule the model cannot express, so a caller reads one form for both: `fieldList`
/// carries a `{name, message}` entry per refused member, `reason` is
/// [`VALIDATION_REASON`], and `message` names the first refused member and the
/// requirement it failed — `Invalid <member>: <requirement>`. The submitted VALUE is
/// never echoed: the caller named the member, and aps keeps its error text free of
/// caller input.
pub fn validation_failed(
    fields: Vec<crate::ops::types::ValidationExceptionField>,
) -> crate::ops::types::ValidationException {
    let message = fields.first().map_or_else(
        || VALIDATION_EXCEPTION_DOC.to_string(),
        |field| format!("Invalid {}: {}", field.name, field.message),
    );
    crate::ops::types::ValidationException {
        message,
        reason: VALIDATION_REASON.to_string(),
        field_list: Some(fields),
    }
}

/// [`validation_failed`] for a single refused member: the member's name as the wire
/// names it, and the requirement it failed.
pub fn member_validation_failed(
    name: &str,
    requirement: &str,
) -> crate::ops::types::ValidationException {
    validation_failed(vec![crate::ops::types::ValidationExceptionField {
        name: name.to_string(),
        message: requirement.to_string(),
    }])
}

/// This service's modelled 400 for a request that is wrong as a WHOLE rather than at one
/// member: there is no per-field breakdown to report, so `fieldList` is absent, and
/// `message` is one constant sentence carrying nothing from the request.
pub fn request_validation_failed(message: &str) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: message.to_string(),
        reason: VALIDATION_REASON.to_string(),
        field_list: None,
    }
}

/// [`validation_failed`] for the constraint violations the framework's own walk found at
/// the request boundary — one `fieldList` entry per refused MEMBER, in the order the
/// walk visits them, so a member that violates two constraints is reported once (by the
/// first requirement it failed) rather than twice.
pub fn constraint_validation_failed(
    violations: &[Violation],
) -> crate::ops::types::ValidationException {
    let mut fields: Vec<crate::ops::types::ValidationExceptionField> = Vec::new();
    for violation in violations {
        if fields.iter().any(|field| field.name == violation.path) {
            continue;
        }
        fields.push(crate::ops::types::ValidationExceptionField {
            name: violation.path.clone(),
            message: constraint_requirement(&violation.kind),
        });
    }
    validation_failed(fields)
}

/// The requirement a violated constraint states, as this service states it.
///
/// The wording is the framework's own vocabulary for each constraint class, with ONE
/// difference: a `@pattern` is stated [`unanchored`], because the anchors are how the
/// model says "the whole value", not part of what the caller has to match.
///
/// MIGRATION NOTE, as `services/common/src/validation.rs` asks of anything that inlines
/// these rules again: the requirement half of the sentence and the `@pattern`
/// anchor-stripping are the SAME for every service, and that module stages them for one
/// migration of its own (`grep -rln 'fn unanchored' services/*/src` lists the crates
/// carrying a copy — this crate is now one of them). What stays this service's own is the
/// DIALECT around it: `Invalid <member>: <requirement>` with one `fieldList` entry per
/// member ([`validation_failed`]). When that migration lands, these two functions are to
/// be deleted and called from the common module, not copied further.
pub fn constraint_requirement(kind: &ViolationKind) -> String {
    match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class the framework adds
        // later is still reported as a refused member rather than going unworded.
        _ => VALIDATION_EXCEPTION_DOC.to_string(),
    }
}

/// A `@pattern` as a message states it: without the `^` and `$` anchors.
///
/// Rendering only — never a decision about whether a value is acceptable, which is the
/// framework's constraint walk over the model's own pattern. A trailing `$` is kept when
/// it is escaped (`\$`, the literal dollar sign), since that one is a character the
/// pattern admits rather than an anchor.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(head) if !head.ends_with('\\') => head,
        _ => body,
    }
}

// =============================================================================
// Scrapers
// =============================================================================

/// The stored scraper as the `ScraperSummary` `ListScrapers` reports — every member read
/// back off the persisted record.
pub fn scraper_summary(scraper: &Scraper) -> crate::ops::types::ScraperSummary {
    crate::ops::types::ScraperSummary {
        scraper_id: scraper.scraper_id.clone(),
        arn: scraper.arn.clone(),
        alias: scraper.alias.clone(),
        role_arn: scraper.role_arn.clone(),
        status: crate::ops::types::ScraperStatus {
            status_code: scraper.status_code.clone(),
        },
        status_reason: scraper.status_reason.clone(),
        created_at: ::aws_smithy_types::DateTime::from(scraper.created_at),
        last_modified_at: ::aws_smithy_types::DateTime::from(scraper.last_modified_at),
        tags: tag_map(&scraper.tags),
        source: scraper.source.clone(),
        destination: scraper.destination.clone(),
        role_configuration: scraper.role_configuration.clone(),
        exporters: Some(scraper.exporters.clone()),
    }
}

/// The ARN of the resource a scraper reads from, for the `sourceArn` filter.
///
/// `Source` is a union: `eksConfiguration` names the EKS cluster by ARN, and
/// `vpcConfiguration` models no ARN at all — it names security groups and subnets — so a
/// VPC source has no `sourceArn` to match and never satisfies that filter.
pub fn scraper_source_arn(source: &crate::ops::types::Source) -> Option<&str> {
    match source {
        crate::ops::types::Source::eksConfiguration(eks) => Some(eks.cluster_arn.as_str()),
        crate::ops::types::Source::vpcConfiguration(_) => None,
    }
}

/// The ARN of the resource a scraper writes to, for the `destinationArn` filter: the
/// workspace of an `ampConfiguration`, or the dataset of a `cloudWatchConfiguration`.
pub fn scraper_destination_arn(destination: &crate::ops::types::Destination) -> Option<&str> {
    match destination {
        crate::ops::types::Destination::ampConfiguration(amp) => {
            Some(amp.workspace_arn.as_str())
        }
        crate::ops::types::Destination::cloudWatchConfiguration(cloud_watch) => {
            Some(cloud_watch.dataset_arn.as_str())
        }
    }
}

/// Whether `scraper` satisfies every filter in `filters`.
///
/// `ListScrapersRequest$filters` documents the rule: "Filters on the same key are `OR`'d
/// together, and filters on different keys are `AND`'d together." The keys honoured are
/// the four the member documents ([`SCRAPER_FILTER_STATUS`] and friends); any other key
/// is not a filter this service defines and is ignored.
pub fn scraper_matches_filters(
    scraper: &Scraper,
    filters: &HashMap<String, Vec<String>>,
) -> bool {
    filters.iter().all(|(key, wanted)| {
        let held: Option<&str> = match key.as_str() {
            SCRAPER_FILTER_STATUS => Some(scraper.status_code.as_str()),
            SCRAPER_FILTER_ALIAS => scraper.alias.as_deref(),
            SCRAPER_FILTER_SOURCE_ARN => scraper_source_arn(&scraper.source),
            SCRAPER_FILTER_DESTINATION_ARN => scraper_destination_arn(&scraper.destination),
            // Not one of the documented filter keys: nothing to filter on.
            _ => return true,
        };
        held.is_some_and(|held| wanted.iter().any(|value| value == held))
    })
}

