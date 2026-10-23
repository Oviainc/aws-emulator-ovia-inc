// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatchlogs-logs-20140328 service.
//!
//! The log-group / log-stream / log-event core loop is stored here, together with the
//! small helpers every handler of that loop shares: ARN construction, the wall clock,
//! log-group identifier resolution and the opaque pagination tokens
//! `GetLogEvents` / `DescribeLogStreams` / `DescribeLogGroups` hand back.
//!
//! A log group's subscription filters live on the group they are "associated with"
//! ([`LogGroupRecord::subscription_filters`]), so deleting the group removes them too.
//!
//! A log group's log transformer lives on the group too
//! ([`LogGroupRecord::transformer`]), because `PutTransformer` creates one "for a single
//! log group" and nothing else identifies it.
//!
//! The account's resource policies live here too: an account-scoped one in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::account_resource_policies`], a
//! resource-scoped one on the log group it is attached to
//! ([`LogGroupRecord::resource_policy`]).
//!
//! The account's import tasks — the CloudTrail Lake imports `CreateImportTask` starts and
//! `DescribeImportTasks` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::import_tasks`], each one an
//! [`ImportTaskRecord`] carrying the instant the clock owes it a terminal status.
//!
//! A log group's data protection policy lives on that group as well
//! ([`LogGroupRecord::data_protection_policy`]), because `PutDataProtectionPolicy`
//! "applies to only the specified log group": one group holds at most one, and deleting
//! the group takes its policy with it.
//!
//! The account's saved CloudWatch Logs Insights query definitions live here
//! ([`ComAmazonawsCloudwatchlogsLogs20140328State::query_definitions`]) rather than on a
//! log group: a definition is identified only by its own `queryDefinitionId`, and the log
//! groups it names are a value it carries, not its owner.
//!
//! A log group's syslog configurations live on the group as well
//! ([`LogGroupRecord::syslog_configurations`]), because `PutSyslogConfiguration` "creates
//! or updates a syslog configuration for a log group" and the only other thing that names
//! one is the VPC endpoint it ingests through — so the group keys them by that endpoint and
//! deleting the group takes them with it.
//!
//! The account's deliveries — each "a connection between a logical delivery source and a
//! logical delivery destination", which `CreateDelivery` creates and `DescribeDeliveries`
//! reports — live in [`ComAmazonawsCloudwatchlogsLogs20140328State::deliveries`], each one
//! a [`DeliveryRecord`].
//!
//! The account's log anomaly detectors — "one anomaly detector in the account", which
//! `CreateLogAnomalyDetector` creates and `GetLogAnomalyDetector` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::anomaly_detectors`], each one an
//! [`AnomalyDetectorRecord`] carrying the instant the clock owes it its next status. A
//! detector is an account-level resource rather than a log group's: it watches log groups
//! named by their ARNs and outlives none of them, so it lives beside the groups and not on
//! [`LogGroupRecord`].

use std::collections::BTreeMap;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Generated emulators default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The log group class `CreateLogGroup` applies when the request omits
/// `logGroupClass` ("If you omit this parameter, the default of STANDARD is used").
pub const DEFAULT_LOG_GROUP_CLASS: &str = "STANDARD";

/// The one log group class a log-group-level transformer may be created on, from
/// `PutTransformer`: "You can create transformers only for the log groups in the
/// Standard log class." Spelled as the model's `LogGroupClass` spells it
/// (`LOG_GROUP_CLASS_VALUES`). Named apart from [`DEFAULT_LOG_GROUP_CLASS`] because the
/// two state different rules — the class a group gets when the request names none, and
/// the class this feature requires — even where the model spells them the same.
pub const TRANSFORMER_LOG_GROUP_CLASS: &str = "STANDARD";

/// Per-event storage overhead CloudWatch Logs charges a log group's `storedBytes`.
/// From the PutLogEvents quota: a batch's size "is calculated as the sum of all event
/// messages in UTF-8, plus 26 bytes for each log event".
pub const EVENT_OVERHEAD_BYTES: i64 = 26;

/// The number of items `DescribeLogGroups` / `DescribeLogStreams` return when the
/// request omits `limit` ("If you don't specify a value, the default is up to 50 items").
pub const DEFAULT_DESCRIBE_LIMIT: i32 = 50;

/// The number of events `GetLogEvents` returns when the request omits `limit`: the
/// documented ceiling of "up to 10,000 log events" per page.
pub const DEFAULT_EVENTS_LIMIT: i32 = 10_000;

/// The number of items the name-pattern list operations return when the request omits
/// `limit`: `ListLogGroups` ("If you omit this parameter, the default is up to 50 log
/// groups") and `ListAggregateLogGroupSummaries` ("If you omit this parameter, the
/// default is up to 50 aggregated summaries").
pub const DEFAULT_LIST_LIMIT: i32 = 50;

/// The shortest `logGroupNamePattern` the model admits: `LogGroupNameRegexPattern`
/// carries `@length(min: 3, max: 129)`.
pub const LOG_GROUP_NAME_PATTERN_MIN_LEN: usize = 3;

/// The longest `logGroupNamePattern` the model admits. See
/// [`LOG_GROUP_NAME_PATTERN_MIN_LEN`].
pub const LOG_GROUP_NAME_PATTERN_MAX_LEN: usize = 129;

/// How many `|`-separated alternatives one `logGroupNamePattern` carries, from the
/// member's own documentation: "You can specify as many as five different regular
/// expression patterns in this field".
pub const MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES: usize = 5;

/// The shortest one alternative of a `logGroupNamePattern` can be, from the same
/// sentence: "each of which must be between 3 and 24 characters".
///
/// The MAXIMUM half of that sentence — and of the `@pattern`'s `{3,24}` segments — is
/// deliberately NOT a constant here: the service accepts an alternative longer than 24
/// characters (log group names routinely exceed it), so refusing one would refuse a
/// request the service answers. See [`check_log_group_name_pattern`].
pub const MIN_LOG_GROUP_NAME_PATTERN_ALTERNATIVE_LEN: usize = 3;

/// The prefix anchor a `logGroupNamePattern` alternative may carry: "Use ^ to match log
/// group names by prefix."
pub const LOG_GROUP_NAME_PATTERN_ANCHOR: char = '^';

/// The separator between a `logGroupNamePattern`'s alternatives: "To match multiple
/// patterns, separate them with a | as in the example ^/aws/lambda|discovery".
pub const LOG_GROUP_NAME_PATTERN_SEPARATOR: char = '|';

/// AWS's documented log group quota, from `CreateLogGroup`: "You can create up to
/// 1,000,000 log groups per Region per account."
pub const MAX_LOG_GROUPS: usize = 1_000_000;

/// AWS's documented resource-policy quota, from `PutResourcePolicy`: "An account can
/// have a maximum of 10 policies without resourceARN and one per LogGroup resourceARN".
/// The per-resource half of that quota is structural here: a resource-scoped policy is
/// stored ON its log group ([`LogGroupRecord::resource_policy`]), so a group can hold
/// exactly one.
pub const MAX_ACCOUNT_RESOURCE_POLICIES: usize = 10;

/// AWS's documented per-resource tag quota, from `TagResource`: "You can associate as many
/// as 50 tags with a CloudWatch Logs resource" — the quota `TooManyTagsException` states
/// ("A resource can have no more than 50 tags").
///
/// The modelled `Tags` map is `@length(max: 50)`, so the boundary validator already refuses
/// a single request carrying more than this; the quota this constant holds a resource to is
/// on the tag set the resource ends up with, which a request inside that length can still
/// push past when the resource already carries tags.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest `policyDocument` `PutDataProtectionPolicy` admits, from that member's own
/// documentation: "The JSON specified in policyDocument can be up to 30,720 characters."
/// The model puts no `@length` on `DataProtectionPolicyDocument`, so nothing refuses an
/// over-long document before the handler runs.
pub const MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS: usize = 30_720;

/// The account-level policy type that protects sensitive data, from the model's
/// `PolicyType` enum (`DATA_PROTECTION_POLICY` | `SUBSCRIPTION_FILTER_POLICY` |
/// `FIELD_INDEX_POLICY` | `TRANSFORMER_POLICY` | `METRIC_EXTRACTION_POLICY`, the same
/// five listed as [`crate::ops::types::POLICY_TYPE_VALUES`]). `policyType` targets that
/// enum, so the boundary validator has already held it to those five by the time a
/// handler runs.
pub const POLICY_TYPE_DATA_PROTECTION: &str = "DATA_PROTECTION_POLICY";

/// The account-level policy type that feeds log events to another service. See
/// [`POLICY_TYPE_DATA_PROTECTION`] for where these five values come from.
pub const POLICY_TYPE_SUBSCRIPTION_FILTER: &str = "SUBSCRIPTION_FILTER_POLICY";

/// The account-level policy type that indexes fields. See
/// [`POLICY_TYPE_DATA_PROTECTION`].
pub const POLICY_TYPE_FIELD_INDEX: &str = "FIELD_INDEX_POLICY";

/// The account-level policy type that transforms ingested log events. See
/// [`POLICY_TYPE_DATA_PROTECTION`].
pub const POLICY_TYPE_TRANSFORMER: &str = "TRANSFORMER_POLICY";

/// The account-level policy type that controls EMF metric creation. See
/// [`POLICY_TYPE_DATA_PROTECTION`].
pub const POLICY_TYPE_METRIC_EXTRACTION: &str = "METRIC_EXTRACTION_POLICY";

/// The scope an account-level policy carries when the put naming it specified none, from
/// `PutAccountPolicy$scope`: "Currently the only valid value for this parameter is ALL,
/// which specifies that the data protection policy applies to all log groups in the
/// account. If you omit this parameter, the default of ALL is used." It is the only value
/// of the model's `Scope` enum, so every stored account policy reports it.
pub const DEFAULT_ACCOUNT_POLICY_SCOPE: &str = "ALL";

/// The longest `policyDocument` `PutAccountPolicy` admits, from that member's own
/// documentation: "The JSON specified in policyDocument can be up to 30,720 characters
/// long." The model puts no `@length` on `AccountPolicyDocument`, so nothing refuses an
/// over-long document before the handler runs.
///
/// It is the same bound the log-group-level
/// [`MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS`] states, named separately because it is a
/// different member's sentence; an account-level data protection document is held to both,
/// which is one bound either way.
pub const MAX_ACCOUNT_POLICY_DOCUMENT_CHARS: usize = MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS;

/// The longest `selectionCriteria` `PutAccountPolicy` admits, from that member's own
/// documentation: "The selectionCriteria string can be up to 25KB in length. The length is
/// determined by using its UTF-8 bytes."
pub const MAX_SELECTION_CRITERIA_BYTES: usize = 25 * 1024;

/// The policy types a `selectionCriteria` may be put with.
/// `PutAccountPolicy$selectionCriteria` names three — "Specifying selectionCriteria is
/// valid only when you specify SUBSCRIPTION_FILTER_POLICY, FIELD_INDEX_POLICY or
/// TRANSFORMER_POLICY for policyType" — and the operation's own documentation adds the
/// fourth, describing metric extraction policies "that are each scoped to a subset of log
/// groups with the selectionCriteria parameter", so a put of one is accepted rather than
/// refused on the member sentence alone. A data protection policy is the one type neither
/// sentence admits: it "applies to all log groups in the account".
pub const SELECTION_CRITERIA_POLICY_TYPES: [&str; 4] = [
    POLICY_TYPE_SUBSCRIPTION_FILTER,
    POLICY_TYPE_FIELD_INDEX,
    POLICY_TYPE_TRANSFORMER,
    POLICY_TYPE_METRIC_EXTRACTION,
];

/// How many values one `IN` or `NOT IN` list of a `selectionCriteria` may carry, from the
/// metric extraction policy section of `PutAccountPolicy`: "The selection criteria supports
/// filtering by LogGroupName and LogGroupNamePrefix using the operators IN and NOT IN. You
/// can specify up to 50 values in each IN or NOT IN list." The sentence is stated for that
/// type's criteria, so it is the bound that type's criteria is held to.
pub const MAX_SELECTION_CRITERIA_VALUES: usize = 50;

/// The `selectionCriteria` filter that selects log groups by their exact name, from
/// `PutAccountPolicy$selectionCriteria` ("the only supported selectionCriteria filter is
/// `LogGroupName NOT IN []`") and from the metric extraction formats `PutAccountPolicy`
/// spells out (`LogGroupName IN ["log-group-1", "log-group-2"]`).
pub const SELECTION_FIELD_LOG_GROUP_NAME: &str = "LogGroupName";

/// The `selectionCriteria` filter that selects log groups by a prefix of their name, from
/// the same two places (`LogGroupNamePrefix NOT IN ["/aws/prefix1", "/aws/prefix2"]`).
pub const SELECTION_FIELD_LOG_GROUP_NAME_PREFIX: &str = "LogGroupNamePrefix";

/// The `selectionCriteria` filter naming a data source, from
/// `PutAccountPolicy$selectionCriteria`: a field index policy may "use either
/// LogGroupNamePrefix by itself or DataSourceName and DataSourceType together".
pub const SELECTION_FIELD_DATA_SOURCE_NAME: &str = "DataSourceName";

/// The `selectionCriteria` filter naming a data source's type. See
/// [`SELECTION_FIELD_DATA_SOURCE_NAME`].
pub const SELECTION_FIELD_DATA_SOURCE_TYPE: &str = "DataSourceType";

/// The member of a `METRIC_EXTRACTION_POLICY` document that says what the policy does to
/// the Embedded Metrics Format. Such a policy exists to "disable EMF metric creation for
/// your entire account or specific log groups" (`PutAccountPolicy`), and the recorded
/// behaviour of the real service is that the document carries
/// `EmbeddedMetricFormat.Status` and that the service "accepts only the exact string
/// 'Disabled'", refusing anything else with `InvalidParameterException` and
/// [`INVALID_JSON_POLICY_DOCUMENT_MESSAGE`].
pub const METRIC_EXTRACTION_EMF_MEMBER: &str = "EmbeddedMetricFormat";

/// The member of `EmbeddedMetricFormat` carrying the status. See
/// [`METRIC_EXTRACTION_EMF_MEMBER`].
pub const METRIC_EXTRACTION_STATUS_MEMBER: &str = "Status";

/// The one `EmbeddedMetricFormat.Status` a metric extraction policy document may carry:
/// EMF metric creation "is enabled for all log groups" by default and a policy only ever
/// turns it off, so `Disabled` is the whole of what such a document can say. See
/// [`METRIC_EXTRACTION_EMF_MEMBER`].
pub const METRIC_EXTRACTION_STATUS_DISABLED: &str = "Disabled";

/// How the real service words its refusal of a metric extraction policy document it does
/// not accept, as the corpus recorded it: `('InvalidParameterException', 'Invalid json
/// policy document')`. See [`METRIC_EXTRACTION_EMF_MEMBER`].
pub const INVALID_JSON_POLICY_DOCUMENT_MESSAGE: &str = "Invalid json policy document";

/// How many account-level policies of one type the account may hold when the policy being
/// put is "scoped to a subset of log groups with the selectionCriteria parameter": for a
/// transformer policy, "you can create as many as 20 account-level transformer policies",
/// and for a field index policy, "as many as 20 account-level field index policies".
pub const MAX_SCOPED_ACCOUNT_POLICIES: usize = 20;

/// The same quota for a metric extraction policy, which has its own number: "Each account
/// can have either one account-level metric extraction policy that applies to all log
/// groups, or up to 5 policies that are each scoped to a subset of log groups with the
/// selectionCriteria parameter."
pub const MAX_SCOPED_METRIC_EXTRACTION_ACCOUNT_POLICIES: usize = 5;

/// How many account-level policies of one type the account may hold when the policy being
/// put applies to EVERY log group — the "one account-level transformer policy that applies
/// to all log groups in the account" half of each quota sentence, and the whole quota for
/// the two types that have no scoped form: "Each account can have only one account-level
/// data protection policy" and "Each account can have one account-level subscription filter
/// policy per Region."
pub const MAX_UNSCOPED_ACCOUNT_POLICIES: usize = 1;

/// The prefix a `policyName` may not start with, from `PutAccountPolicy$policyName`: "A
/// name for the policy. This must be unique within the account and cannot start with
/// `aws/`." The model puts no `@pattern` on `PolicyName`, so nothing refuses such a name
/// before the handler runs.
pub const RESERVED_POLICY_NAME_PREFIX: &str = "aws/";

/// The `dataProtectionStatus` of a log group that has a data protection policy right
/// now, from the model's `DataProtectionStatus` enum (`ACTIVATED` | `DELETED` |
/// `ARCHIVED` | `DISABLED`).
pub const DATA_PROTECTION_STATUS_ACTIVATED: &str = "ACTIVATED";

/// The `dataProtectionStatus` of a log group whose data protection policy has been
/// deleted — the "or whether it had one in the past" half of `LogGroup$dataProtectionStatus`
/// ("Displays whether this log group has a protection policy, or whether it had one in
/// the past"). From the same `DataProtectionStatus` enum.
pub const DATA_PROTECTION_STATUS_DELETED: &str = "DELETED";

/// The scope of a policy attached to no resource, from the model's `PolicyScope` enum
/// (`ACCOUNT` | `RESOURCE`). This is also the scope `DescribeResourcePolicies` reports
/// on when the request names no `policyScope` ("When not specified, defaults to
/// ACCOUNT").
pub const POLICY_SCOPE_ACCOUNT: &str = "ACCOUNT";

/// The scope of a policy attached to one CloudWatch Logs resource, from the model's
/// `PolicyScope` enum (`ACCOUNT` | `RESOURCE`).
pub const POLICY_SCOPE_RESOURCE: &str = "RESOURCE";

/// The retentions a log group can be set to, from the model's `Days` shape: "The number
/// of days to retain the log events in the specified log group. Possible values are: 1,
/// 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557,
/// 2922, 3288, and 3653." `Days` is a plain integer in the model, so nothing refuses a
/// value outside this list before a handler runs. Ascending, so the list renders in the
/// documented order in a refusal message.
pub const RETENTION_IN_DAYS_VALUES: [i32; 22] = [
    1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
    3288, 3653,
];

/// AWS's documented subscription-filter quota, from `PutSubscriptionFilter`: "Each log
/// group can have up to two subscription filters associated with it." A put naming a
/// filter the group already holds is an update of that filter, so it does not count
/// against this.
pub const MAX_SUBSCRIPTION_FILTERS_PER_GROUP: usize = 2;

/// The destinations `PutSubscriptionFilter` documents as supported, as the service
/// segment of the `destinationArn` naming each: "An Amazon Kinesis stream", "A logical
/// destination (specified using an ARN)" created with `PutDestination` (a CloudWatch
/// Logs `destination` resource), "A Kinesis Data Firehose delivery stream" and "A Lambda
/// function". The resource prefix is the one each of those services' ARNs carries.
pub const SUBSCRIPTION_DESTINATIONS: [(&str, &str); 4] = [
    ("kinesis", "stream/"),
    ("firehose", "deliverystream/"),
    ("lambda", "function:"),
    ("logs", "destination:"),
];

/// The system fields a subscription filter can emit, from
/// `PutSubscriptionFilter$emitSystemFields`: "Valid values are @aws.account,
/// @aws.region, and @source.log".
pub const SUBSCRIPTION_SYSTEM_FIELDS: [&str; 3] = ["@aws.account", "@aws.region", "@source.log"];

/// The `distribution` a subscription filter carries when the put naming it specified
/// none, from `PutSubscriptionFilter$distribution`: "By default, log data is grouped by
/// log stream, but the grouping can be set to random for a more even distribution." It is
/// one of the model's `Distribution` enum values (`ByLogStream` | `Random`), so a filter
/// always has a distribution to report.
pub const DEFAULT_DISTRIBUTION: &str = "ByLogStream";

/// The query language a query definition is saved with when the put named no
/// `queryLanguage`. `CWLI` is how the model's `QueryLanguage` enum spells Logs Insights
/// QL (`QUERY_LANGUAGE_VALUES`), and `PutQueryDefinition` offers `parameters` —
/// "supported only for Logs Insights QL queries" — on a request that names no language
/// at all, so a definition saved without one is a Logs Insights QL definition. Storing it
/// is what lets `DescribeQueryDefinitions$queryLanguage` ("limits the returned queries to
/// only the queries that use the specified query language") place every definition.
pub const DEFAULT_QUERY_LANGUAGE: &str = "CWLI";

/// The largest page `DescribeQueryDefinitions` hands back, which is also the page a
/// request that names no `maxResults` gets: `maxResults` "limits the number of returned
/// query definitions to the specified number" and documents no default, and the member's
/// own `QueryListMaxResults` range (`range 1..1000`) is the most the API admits in one
/// page.
pub const MAX_QUERY_DEFINITIONS_PER_PAGE: i32 = 1_000;

/// The bracketed forms a filter pattern can open, as `(open, close)` pairs: a JSON
/// selector pattern is wrapped in `{ … }` and a space-delimited pattern in `[ … ]`
/// ("Filter pattern syntax for metric filters, subscription filters, filter log events,
/// and Live Tail", referenced from `PutSubscriptionFilter`). A pattern that opens one of
/// these forms must close it, which is the rule
/// [`validate_filter_pattern`] holds a pattern to.
pub const FILTER_PATTERN_DELIMITERS: [(char, char); 2] = [('{', '}'), ('[', ']')];

/// THE EMULATOR'S OWN CEILING on the log streams one group holds, not an AWS quota:
/// CloudWatch Logs documents no limit on log streams, but this emulator keeps every
/// stream in memory, so it refuses to create more than this many in one group rather
/// than growing without bound on request.
pub const MAX_STREAMS_PER_GROUP: usize = 10_000;

/// THE EMULATOR'S OWN STORAGE CEILING for one log stream, not an AWS quota: the events
/// a stream holds live in memory, so it keeps at most this many of them, and at most
/// [`MAX_STREAM_BYTES`] of message bytes, discarding the OLDEST first when a write would
/// exceed either. That is the effect a caller already observes from the real service
/// when events pass a log group's retention period and are deleted.
pub const MAX_STREAM_EVENTS: usize = 20_000;

/// The byte half of the per-stream storage ceiling. See [`MAX_STREAM_EVENTS`].
pub const MAX_STREAM_BYTES: i64 = 8 * 1024 * 1024;

/// THE EMULATOR'S OWN CEILING on the syslog configurations one log group holds, not an AWS
/// quota: CloudWatch Logs documents no limit on them, but a `vpcEndpointId` is caller-chosen
/// (`^vpce-[0-9a-f]{1,64}$` admits astronomically many) and every configuration a put
/// creates stays resident in this store, so a group refuses to take on more than this many
/// rather than growing without bound on request. A put naming an endpoint the group already
/// holds is an UPDATE of that configuration, so it does not count against this.
pub const MAX_SYSLOG_CONFIGURATIONS_PER_GROUP: usize = 100;

/// An import task that is still running, from the model's `ImportStatus` enum
/// (`IN_PROGRESS` | `CANCELLED` | `COMPLETED` | `FAILED`). This is the only
/// non-terminal one: an import AWS accepts starts here and leaves it on its own.
pub const IMPORT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// An import task a caller cancelled, from the model's `ImportStatus` enum.
pub const IMPORT_STATUS_CANCELLED: &str = "CANCELLED";

/// An import task that finished importing, from the model's `ImportStatus` enum.
pub const IMPORT_STATUS_COMPLETED: &str = "COMPLETED";

/// An import task that stopped on an error, from the model's `ImportStatus` enum. A task
/// in this status reports why in `Import$errorMessage` ("Error message related to any
/// failed imports").
pub const IMPORT_STATUS_FAILED: &str = "FAILED";

/// How long after it is created an import task becomes `COMPLETED`.
///
/// AWS documents no duration for an import ("Starts an import from a data source to
/// CloudWatch Log"), so this span is THE EMULATOR'S OWN CHOICE: the import runs
/// asynchronously, so the task is handed back `IN_PROGRESS` and the clock owes it a
/// terminal status later. The span is deliberately not zero — a caller that reads or
/// cancels the task straight after creating it still observes `IN_PROGRESS`, and the
/// state rules that go with it.
pub const IMPORT_COMPLETION_DELAY_MS: i64 = 500;

/// The status a log anomaly detector is created in, from the model's
/// `AnomalyDetectorStatus` enum: it has to initialize before it can train on a log group's
/// events, so a create hands back a detector that is not analyzing anything yet.
pub const ANOMALY_DETECTOR_STATUS_INITIALIZING: &str = "INITIALIZING";

/// The status a log anomaly detector reaches once it is initialized and is learning the
/// log group's patterns: `CreateLogAnomalyDetector` documents that the detector "uses the
/// evaluationFrequency to compare current log events and patterns with trained baselines",
/// so training precedes analysis.
pub const ANOMALY_DETECTOR_STATUS_TRAINING: &str = "TRAINING";

/// The status a trained log anomaly detector settles in: it is scanning log events for
/// anomalies. Nothing but an `UpdateLogAnomalyDetector` (which can pause it) or a delete
/// moves a detector out of this status, so the clock owes it nothing further.
pub const ANOMALY_DETECTOR_STATUS_ANALYZING: &str = "ANALYZING";

/// How long after it is created a log anomaly detector takes to leave
/// [`ANOMALY_DETECTOR_STATUS_INITIALIZING`] for [`ANOMALY_DETECTOR_STATUS_TRAINING`].
///
/// AWS documents no duration for this — the detector trains "using machine learning
/// algorithms" over however long its log group needs — so this span is THE EMULATOR'S OWN
/// CHOICE. It is deliberately not zero: a caller that reads the detector straight after
/// creating it still observes the status the create handed back.
pub const ANOMALY_DETECTOR_TRAINING_DELAY_MS: i64 = 500;

/// How long after it starts training a log anomaly detector takes to reach
/// [`ANOMALY_DETECTOR_STATUS_ANALYZING`]. The emulator's own choice, for the same reason as
/// [`ANOMALY_DETECTOR_TRAINING_DELAY_MS`].
pub const ANOMALY_DETECTOR_ANALYSIS_DELAY_MS: i64 = 500;

/// How many log groups one log anomaly detector watches, from
/// `CreateLogAnomalyDetector$logGroupArnList`: "An array containing the ARN of the log
/// group that this anomaly detector will watch. You can specify only one log group ARN."
pub const ANOMALY_DETECTOR_LOG_GROUPS: usize = 1;

/// THE EMULATOR'S OWN CEILING on the log anomaly detectors one account holds, not a quota
/// cited from AWS: every accepted create mints a fresh ARN and keeps the detector's
/// configuration in memory for as long as the process runs, and nothing expires one, so
/// `CreateLogAnomalyDetector` refuses to create more than this many with the
/// `LimitExceededException` it models ("You have reached the maximum number of resources
/// that can be created") rather than growing without bound on request. It is set far above
/// the number of detectors a caller legitimately creates, so only a caller hammering the
/// create reaches it.
pub const MAX_ANOMALY_DETECTORS: usize = 1_000;

/// THE EMULATOR'S OWN CEILING on the saved query definitions one account holds, not a
/// quota cited from AWS: each definition keeps its query string (modelled `@length` up to
/// 10,000), name and parameters in memory for as long as the process runs and nothing
/// expires one, so `PutQueryDefinition` refuses to create more than this many with the
/// `LimitExceededException` it models ("You have reached the maximum number of resources
/// that can be created") rather than growing without bound on request. An UPDATE of a
/// definition already stored is never refused by it: it adds nothing.
pub const MAX_QUERY_DEFINITIONS: usize = 1_000;

/// Per-service state for com-amazonaws-cloudwatchlogs-logs-20140328. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Every created log group, keyed by `logGroupName`. A `BTreeMap` so
    /// `DescribeLogGroups` enumerates in log-group-name order without re-sorting.
    pub log_groups: BTreeMap<String, LogGroupRecord>,
    /// Monotonic ingestion counter. It totally orders events that share a timestamp
    /// and makes the pagination cursors and sequence tokens stable.
    pub next_ingestion_seq: u64,
    /// The account-scoped resource policies — the ones `PutResourcePolicy` creates
    /// without a `resourceArn` — keyed by `policyName`. A `BTreeMap` so
    /// `DescribeResourcePolicies` enumerates them in policy-name order. A
    /// resource-SCOPED policy is not here: it is attached to the log group its ARN
    /// names ([`LogGroupRecord::resource_policy`]), so deleting that group revokes its
    /// policy with it.
    pub account_resource_policies: BTreeMap<String, ResourcePolicyRecord>,
    /// Monotonic counter behind the resource-policy revision ids, so every update of a
    /// resource-scoped policy mints a revision no earlier update used.
    pub next_policy_revision: u64,
    /// Every saved CloudWatch Logs Insights query definition, keyed by its
    /// `queryDefinitionId` — the only thing that identifies one. A `BTreeMap` so
    /// `DescribeQueryDefinitions` enumerates them in a stable order and its pagination
    /// cursor resumes at a position that does not move when another definition is saved.
    pub query_definitions: BTreeMap<String, QueryDefinitionRecord>,
    /// Monotonic counter behind the `queryDefinitionId`s `PutQueryDefinition` mints, so
    /// every definition it creates gets an id no earlier create used — including after an
    /// earlier definition has been deleted. See [`mint_query_definition_id`].
    pub next_query_definition_seq: u64,
    /// The KMS key associated with the account's stored query-insights results — the
    /// `AssociateKmsKey` target named by the `query-result` ARN rather than by a log
    /// group ("use that key to encrypt the stored results of all future StartQuery
    /// operations in the account"). `None` until one is associated, and `None` again
    /// after `DisassociateKmsKey`.
    pub query_results_kms_key_id: Option<String>,
    /// The account-level policies of `PutAccountPolicy`, keyed by `(policyType,
    /// policyName)`: the two members `DeleteAccountPolicy` identifies one by, so policies
    /// of different types may share a name and a put under a pair already stored updates
    /// that policy in place. A `BTreeMap` so `DescribeAccountPolicies` enumerates one
    /// type's policies in policy-name order without re-sorting.
    ///
    /// Account-level policies are not attached to any log group — they "apply to all log
    /// groups in the account", including ones created later — so they live here rather
    /// than on [`LogGroupRecord`].
    pub account_policies: BTreeMap<(String, String), AccountPolicyRecord>,
    /// Every import task of this account — the resource `CreateImportTask` starts and
    /// `DescribeImportTasks` reports — keyed by `importId`. A `BTreeMap` so
    /// `DescribeImportTasks` enumerates in import-id order without re-sorting, which is
    /// also what makes its pagination cursor stable.
    pub import_tasks: BTreeMap<String, ImportTaskRecord>,
    /// Every delivery of this account — the resource `CreateDelivery` creates and
    /// `DescribeDeliveries` reports — keyed by `Delivery$id`. A `BTreeMap` so
    /// `DescribeDeliveries` enumerates in delivery-id order without re-sorting, which is
    /// also what makes its pagination cursor stable.
    ///
    /// A delivery is an account-level resource rather than a log group's: its source and
    /// its destination are both named by their own resources, so it lives here and not on
    /// [`LogGroupRecord`].
    pub deliveries: BTreeMap<String, DeliveryRecord>,
    /// Every log anomaly detector of this account — the resource `CreateLogAnomalyDetector`
    /// creates and `GetLogAnomalyDetector` reports — keyed by `anomalyDetectorArn`, the one
    /// member every other anomaly-detector operation identifies one by. A `BTreeMap` so
    /// `ListLogAnomalyDetectors` enumerates in detector-ARN order without re-sorting, which
    /// is also what makes its pagination cursor stable.
    pub anomaly_detectors: BTreeMap<String, AnomalyDetectorRecord>,
    /// Monotonic counter behind the anomaly-detector identifiers, so every create mints an
    /// ARN no earlier create used — including after the detector holding it was deleted.
    pub next_anomaly_detector_seq: u64,
}

impl ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Take the next ingestion sequence number, advancing the counter.
    pub fn take_ingestion_seq(&mut self) -> u64 {
        let seq = self.next_ingestion_seq;
        self.next_ingestion_seq = self.next_ingestion_seq.saturating_add(1);
        seq
    }

    /// Take the next resource-policy revision number, advancing the counter.
    pub fn take_policy_revision(&mut self) -> u64 {
        let seq = self.next_policy_revision;
        self.next_policy_revision = self.next_policy_revision.saturating_add(1);
        seq
    }

    /// Take the next anomaly-detector sequence number, advancing the counter.
    pub fn take_anomaly_detector_seq(&mut self) -> u64 {
        let seq = self.next_anomaly_detector_seq;
        self.next_anomaly_detector_seq = self.next_anomaly_detector_seq.saturating_add(1);
        seq
    }

    /// Take the next query-definition sequence number, advancing the counter.
    pub fn take_query_definition_seq(&mut self) -> u64 {
        let seq = self.next_query_definition_seq;
        self.next_query_definition_seq = self.next_query_definition_seq.saturating_add(1);
        seq
    }

    /// The tags assigned to the resource a tagging operation named, or `None` when the
    /// account holds no such resource.
    ///
    /// One resource holds ONE tag set, on the resource's own record, so every operation
    /// that reads or writes tags — and every reporter of the resource itself, such as
    /// `ListLogGroups`'s `logGroupTags` filter — sees the same map.
    pub fn resource_tags(&self, resource: &TaggedResource) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::LogGroup(name) => self.log_groups.get(name).map(|group| &group.tags),
            TaggedResource::Delivery(id) => self.deliveries.get(id).map(|delivery| &delivery.tags),
            // A logical destination carries tags too, but `PutDestination` is still a
            // scaffold stub, so the account holds no destination at all: an ARN naming one
            // names a resource that does not exist. Implementing that operation is what
            // gives this arm a store to look in.
            TaggedResource::Destination(_) => None,
        }
    }

    /// The same tag set, to write. See [`Self::resource_tags`].
    pub fn resource_tags_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::LogGroup(name) => {
                self.log_groups.get_mut(name).map(|group| &mut group.tags)
            }
            TaggedResource::Delivery(id) => self
                .deliveries
                .get_mut(id)
                .map(|delivery| &mut delivery.tags),
            TaggedResource::Destination(_) => None,
        }
    }

    /// Materialise every import-task status change the clock has ALREADY made due, as of
    /// `now_ms`.
    ///
    /// An import runs asynchronously, so its terminal status is owed by the clock rather
    /// than by a call: the change was due at [`ImportTaskRecord::completion_due_ms`] even
    /// if nobody asked. A reporting operation calls this before it reads, so what it
    /// reports is what the clock already decided — never a status moved on because
    /// something was polled.
    pub fn materialize_due_import_transitions(&mut self, now_ms: i64) {
        for task in self.import_tasks.values_mut() {
            task.materialize_due_transitions(now_ms);
        }
    }

    /// Materialise every anomaly-detector status change the clock has ALREADY made due, as
    /// of `now_ms`.
    ///
    /// A detector initializes and trains on its own, so the status it is in is owed by the
    /// clock rather than by a call ([`AnomalyDetectorRecord::status_due_ms`]). A reporting
    /// operation calls this before it reads, so what it reports is what the clock already
    /// decided — never a status moved on because something was polled.
    pub fn materialize_due_anomaly_detector_transitions(&mut self, now_ms: i64) {
        for detector in self.anomaly_detectors.values_mut() {
            detector.materialize_due_transitions(now_ms);
        }
    }
}

/// One resource policy — "a policy enabling one or more entities to put logs to a log
/// group in this account" — as it is stored.
#[derive(Debug, Clone)]
pub struct ResourcePolicyRecord {
    pub policy_name: String,
    pub policy_document: String,
    pub last_updated_time_ms: i64,
    /// The ARN this policy is attached to, for a resource-scoped policy; `None` for an
    /// account-scoped one (`ResourcePolicy$resourceArn` is "only populated for
    /// resource-scoped policies").
    pub resource_arn: Option<String>,
    /// The revision this policy is at, for a resource-scoped policy; `None` for an
    /// account-scoped one (`ResourcePolicy$revisionId` is "only populated for
    /// resource-scoped policies").
    pub revision_id: Option<String>,
}

impl ResourcePolicyRecord {
    /// This policy's `policyScope`: `RESOURCE` when it is attached to a resource,
    /// `ACCOUNT` when it is attached to none.
    pub fn policy_scope(&self) -> &'static str {
        if self.resource_arn.is_some() {
            POLICY_SCOPE_RESOURCE
        } else {
            POLICY_SCOPE_ACCOUNT
        }
    }
}

/// One account-level policy — what `PutAccountPolicy` stores — as it is stored.
///
/// The document is kept VERBATIM, exactly the bytes the request carried, for the same
/// reason as [`DataProtectionPolicyRecord::policy_document`]: AWS reports it back through
/// `DescribeAccountPolicies` unchanged.
#[derive(Debug, Clone)]
pub struct AccountPolicyRecord {
    /// `AccountPolicy$policyName` — "The name of the account policy."
    pub policy_name: String,
    /// `AccountPolicy$policyDocument` — "The policy document for this account policy."
    pub policy_document: String,
    /// `AccountPolicy$policyType` — "The type of policy for this account policy."
    pub policy_type: String,
    /// `AccountPolicy$scope` — "The scope of the account policy."
    pub scope: String,
    /// `AccountPolicy$selectionCriteria` — "The log group selection criteria that is used
    /// for this policy." `None` for a policy that applies to all log groups.
    pub selection_criteria: Option<String>,
    /// `AccountPolicy$lastUpdatedTime` — "The date and time that this policy was most
    /// recently updated."
    pub last_updated_time_ms: i64,
}

impl AccountPolicyRecord {
    /// This policy as the model's `AccountPolicy` structure, which is what both
    /// `PutAccountPolicy` and `DescribeAccountPolicies` report, so one record renders the
    /// same way through either.
    ///
    /// `accountId` is "The Amazon Web Services account ID that the policy applies to" —
    /// this emulator's own account, because an account-level policy applies to the account
    /// that created it.
    pub fn reported(&self) -> crate::ops::types::AccountPolicy {
        crate::ops::types::AccountPolicy {
            policy_name: Some(self.policy_name.clone()),
            policy_document: Some(self.policy_document.clone()),
            last_updated_time: Some(self.last_updated_time_ms),
            policy_type: Some(self.policy_type.clone()),
            scope: Some(self.scope.clone()),
            selection_criteria: self.selection_criteria.clone(),
            account_id: Some(::engine::emulator_account_id().to_string()),
        }
    }
}

/// One import task — "An import job to move data from CloudTrail Event Data Store to
/// CloudWatch" — as it is stored. Every member of the model's `Import` shape is reported
/// from this record, so `DescribeImportTasks` answers from state alone.
#[derive(Debug, Clone)]
pub struct ImportTaskRecord {
    /// `Import$importId` — "The unique identifier of the import task."
    pub import_id: String,
    /// `Import$importSourceArn` — "The ARN of the CloudTrail Lake Event Data Store being
    /// imported from."
    pub import_source_arn: String,
    /// `CreateImportTask$importRoleArn` — "The ARN of the IAM role that grants CloudWatch
    /// Logs permission to import". No `Import` member reports it, but the task is the only
    /// record of the role the import runs under, so it is stored with the task. Written by
    /// `CreateImportTask` once that operation is implemented (it is still a scaffold stub),
    /// which is also the only caller [`ImportTaskRecord::new`] is waiting for — neither is
    /// dead code to prune.
    pub import_role_arn: String,
    /// `Import$importDestinationArn` — "The ARN of the managed CloudWatch Logs log group
    /// where the events are being imported to."
    pub import_destination_arn: String,
    /// `Import$importStatus`, one of the [`IMPORT_STATUS_IN_PROGRESS`] family.
    pub import_status: String,
    /// `ImportFilter$startEventTime`, when the create named one.
    pub start_event_time_ms: Option<i64>,
    /// `ImportFilter$endEventTime`, when the create named one.
    pub end_event_time_ms: Option<i64>,
    /// `ImportStatistics$bytesImported` — "The total number of bytes that have been
    /// imported to the managed log group."
    pub bytes_imported: i64,
    /// `Import$creationTime`, in epoch milliseconds.
    pub creation_time_ms: i64,
    /// `Import$lastUpdatedTime`, in epoch milliseconds.
    pub last_updated_time_ms: i64,
    /// The instant this task's asynchronous import is due to reach `COMPLETED`, stored
    /// rather than waited for: a reporting operation materialises the transition once the
    /// clock is past it ([`materialize_due_import_transitions`]). `None` once the task is
    /// in a terminal status, which owes no further transition.
    ///
    /// [`materialize_due_import_transitions`]:
    ///     ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_import_transitions
    pub completion_due_ms: Option<i64>,
    /// `Import$errorMessage` — "Error message related to any failed imports". Only a
    /// FAILED task carries one.
    pub error_message: Option<String>,
}

impl ImportTaskRecord {
    /// A brand-new import task: running, nothing imported yet, and due to complete
    /// [`IMPORT_COMPLETION_DELAY_MS`] after it was created. This is what
    /// `CreateImportTask` stores once it is implemented.
    pub fn new(
        import_id: &str,
        import_source_arn: &str,
        import_role_arn: &str,
        import_destination_arn: &str,
        import_filter: Option<&crate::ops::types::ImportFilter>,
        creation_time_ms: i64,
    ) -> Self {
        Self {
            import_id: import_id.to_string(),
            import_source_arn: import_source_arn.to_string(),
            import_role_arn: import_role_arn.to_string(),
            import_destination_arn: import_destination_arn.to_string(),
            import_status: IMPORT_STATUS_IN_PROGRESS.to_string(),
            start_event_time_ms: import_filter.and_then(|filter| filter.start_event_time),
            end_event_time_ms: import_filter.and_then(|filter| filter.end_event_time),
            bytes_imported: 0,
            creation_time_ms,
            last_updated_time_ms: creation_time_ms,
            completion_due_ms: Some(creation_time_ms.saturating_add(IMPORT_COMPLETION_DELAY_MS)),
            error_message: None,
        }
    }

    /// Whether this task has reached a status it can never leave — anything but
    /// [`IMPORT_STATUS_IN_PROGRESS`], the one status the model's `ImportStatus` enum
    /// describes as still running. This is the condition `CancelImportTask` refuses on
    /// ("Cancels an ACTIVE import task"), and the one that tells this record it owes no
    /// further clock transition.
    pub fn is_terminal(&self) -> bool {
        self.import_status != IMPORT_STATUS_IN_PROGRESS
    }

    /// This task's `importFilter`, or `None` when the create bounded the event time at
    /// neither end — `ImportFilter`'s two members are both optional, so a task created
    /// without either carries no filter to report.
    pub fn import_filter(&self) -> Option<crate::ops::types::ImportFilter> {
        let filter = crate::ops::types::ImportFilter {
            start_event_time: self.start_event_time_ms,
            end_event_time: self.end_event_time_ms,
        };
        (filter.start_event_time.is_some() || filter.end_event_time.is_some()).then_some(filter)
    }

    /// This task's `importStatistics`: what the import has moved so far, counted in state
    /// rather than reported as progress nothing made.
    pub fn import_statistics(&self) -> crate::ops::types::ImportStatistics {
        crate::ops::types::ImportStatistics {
            bytes_imported: Some(self.bytes_imported),
        }
    }

    /// Materialise the status change the clock owes this task: a running import whose
    /// completion instant has passed is `COMPLETED`, updated AT that instant and not at
    /// the time something happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        if self.is_terminal() {
            return;
        }
        let Some(due) = self.completion_due_ms else {
            return;
        };
        if due <= now_ms {
            self.import_status = IMPORT_STATUS_COMPLETED.to_string();
            self.last_updated_time_ms = due;
            self.completion_due_ms = None;
        }
    }
}

/// One delivery — "a connection between a logical delivery source and a logical delivery
/// destination" — as it is stored. Every member of the model's `Delivery` shape is
/// reported from this record, so `DescribeDeliveries` answers from state alone.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    /// `Delivery$id` — "The unique ID that identifies this delivery in your account."
    /// Also the key the store enumerates by.
    pub id: String,
    /// `Delivery$arn` — "The Amazon Resource Name (ARN) that uniquely identifies this
    /// delivery", as [`delivery_arn`] builds it.
    pub arn: String,
    /// `Delivery$deliverySourceName` — "The name of the delivery source that is associated
    /// with this delivery."
    pub delivery_source_name: String,
    /// `Delivery$deliveryDestinationArn` — "The ARN of the delivery destination that is
    /// associated with this delivery."
    pub delivery_destination_arn: String,
    /// `Delivery$deliveryDestinationType` — the vendor the destination resource belongs to
    /// ([`delivery_destination_type_for_arn`]); `None` for a destination ARN naming none of
    /// the four the model's `DeliveryDestinationType` enumerates.
    pub delivery_destination_type: Option<String>,
    /// `Delivery$recordFields` — "The record fields used in this delivery". `None` when the
    /// create named no list at all, which is not the same as the empty list it may name
    /// (`RecordFields` is `@length(min: 0)`).
    pub record_fields: Option<Vec<String>>,
    /// `Delivery$fieldDelimiter` — "The field delimiter that is used between record fields
    /// when the final output format of a delivery is in Plain, W3C, or Raw format."
    pub field_delimiter: Option<String>,
    /// `Delivery$s3DeliveryConfiguration` — the configuration that applies "only when the
    /// delivery destination resource is an S3 bucket".
    pub s3_delivery_configuration: Option<crate::ops::types::S3DeliveryConfiguration>,
    /// `Delivery$tags` — "The tags that have been assigned to this delivery." A `BTreeMap`
    /// so the stored order is stable; `Tags` is `@length(min: 1)`, so a delivery carrying
    /// none reports no member at all.
    pub tags: BTreeMap<String, String>,
}

impl DeliveryRecord {
    /// A brand-new delivery between one delivery source and one delivery destination, with
    /// the ARN and the destination type DERIVED rather than taken from the caller: both are
    /// facts about the resources being connected. This is what `CreateDelivery` stores once
    /// it is implemented (it is still a scaffold stub), which is also the only caller this
    /// constructor is waiting for.
    ///
    /// The optional members a create may name — `recordFields`, `fieldDelimiter`,
    /// `s3DeliveryConfiguration`, `tags` — are left at their "the request named none" value
    /// and set on the returned record by the caller that has them.
    pub fn new(
        id: &str,
        region: &str,
        delivery_source_name: &str,
        delivery_destination_arn: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            arn: delivery_arn(region, id),
            delivery_source_name: delivery_source_name.to_string(),
            delivery_destination_arn: delivery_destination_arn.to_string(),
            delivery_destination_type: delivery_destination_type_for_arn(delivery_destination_arn)
                .map(ToString::to_string),
            record_fields: None,
            field_delimiter: None,
            s3_delivery_configuration: None,
            tags: BTreeMap::new(),
        }
    }

    /// This delivery as the model's `Delivery` structure, which is what `CreateDelivery`,
    /// `GetDelivery` and `DescribeDeliveries` all report, so one record renders the same way
    /// through any of them.
    pub fn reported(&self) -> crate::ops::types::Delivery {
        crate::ops::types::Delivery {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            delivery_source_name: Some(self.delivery_source_name.clone()),
            delivery_destination_arn: Some(self.delivery_destination_arn.clone()),
            delivery_destination_type: self.delivery_destination_type.clone(),
            record_fields: self.record_fields.clone(),
            field_delimiter: self.field_delimiter.clone(),
            s3_delivery_configuration: self.s3_delivery_configuration.clone(),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        }
    }
}

/// A delivery's ARN, the form `Delivery$arn` ("The Amazon Resource Name (ARN) that uniquely
/// identifies this delivery") carries: a `delivery` resource of this service, identified by
/// the delivery's own id.
pub fn delivery_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("delivery:{id}"))
}

/// The ARN service namespace of each vendor a delivery destination can be, paired with the
/// `DeliveryDestinationType` the model spells it as.
///
/// The four vendors are the ones `DescribeDeliveries` documents — "The destination can be
/// CloudWatch Logs, Amazon S3, Firehose or X-Ray" — and each spelling is a value of the
/// model's `DeliveryDestinationType` enum
/// ([`crate::ops::types::DELIVERY_DESTINATION_TYPE_VALUES`]), never a string typed here.
pub const DELIVERY_DESTINATION_VENDOR_TYPES: [(&str, &str); 4] = [
    ("logs", "CWL"),
    ("firehose", "FH"),
    ("s3", "S3"),
    ("xray", "XRAY"),
];

/// The `DeliveryDestinationType` of the resource an ARN names, or `None` for an ARN naming
/// none of the four vendors a delivery destination can be.
///
/// The type is a property of the destination RESOURCE, not of the request:
/// `Delivery$deliveryDestinationType` "Displays whether the delivery destination associated
/// with this delivery is CloudWatch Logs, Amazon S3, Firehose, or X-Ray", so it is read off
/// the destination's ARN ([`DELIVERY_DESTINATION_VENDOR_TYPES`]).
pub fn delivery_destination_type_for_arn(arn: &str) -> Option<&'static str> {
    let (vendor, _resource) = arn_service_and_resource(arn)?;
    DELIVERY_DESTINATION_VENDOR_TYPES
        .iter()
        .find(|(namespace, _)| vendor == *namespace)
        .map(|(_, destination_type)| *destination_type)
}

/// One log anomaly detector — "one anomaly detector in the account" — as it is stored.
/// Every member of the model's `AnomalyDetector` shape is reported from this record, so
/// `GetLogAnomalyDetector` and `ListLogAnomalyDetectors` answer from state alone.
#[derive(Debug, Clone)]
pub struct AnomalyDetectorRecord {
    /// `AnomalyDetector$anomalyDetectorArn` — "The ARN of the anomaly detector", as
    /// [`anomaly_detector_arn`] builds it. Also the key the store is keyed by.
    pub arn: String,
    /// `AnomalyDetector$detectorName` — "The name of the anomaly detector". Optional on the
    /// create, so a detector created without one reports no name.
    pub detector_name: Option<String>,
    /// `AnomalyDetector$logGroupArnList` — "A list of the ARNs of the log groups that this
    /// anomaly detector watches", exactly as the create named them.
    pub log_group_arn_list: Vec<String>,
    /// `AnomalyDetector$evaluationFrequency` — "Specifies how often the anomaly detector
    /// runs and look for anomalies", one value of the model's `EvaluationFrequency` enum.
    /// `None` when neither the create nor an update named one: AWS documents no default for
    /// the member, so none is invented here.
    pub evaluation_frequency: Option<String>,
    /// `AnomalyDetector$filterPattern` — the pattern that limits "the anomaly detection
    /// model to examine only log events that match", when one was named.
    pub filter_pattern: Option<String>,
    /// `AnomalyDetector$anomalyDetectorStatus`, one of the
    /// [`ANOMALY_DETECTOR_STATUS_INITIALIZING`] family.
    pub status: String,
    /// `AnomalyDetector$kmsKeyId` — "The ARN of the KMS key assigned to this anomaly
    /// detector, if any."
    pub kms_key_id: Option<String>,
    /// `AnomalyDetector$creationTimeStamp` — "when this anomaly detector was created", in
    /// epoch milliseconds.
    pub creation_time_ms: i64,
    /// `AnomalyDetector$lastModifiedTimeStamp` — "when this anomaly detector was most
    /// recently modified", in epoch milliseconds. A status the clock owes is not a
    /// modification of the detector's configuration, so only a call that changes that
    /// configuration moves this on.
    pub last_modified_time_ms: i64,
    /// `AnomalyDetector$anomalyVisibilityTime` — "The number of days used as the life cycle
    /// of anomalies". `None` when no call named one; AWS documents no default.
    pub anomaly_visibility_time: Option<i64>,
    /// The tags `CreateLogAnomalyDetector$tags` assigned to this detector. A `BTreeMap` so
    /// the stored order is stable; no `AnomalyDetector` member reports them, so they are
    /// read back through the tagging operations once those reach this resource.
    pub tags: BTreeMap<String, String>,
    /// The instant this detector is due to reach its next status, stored rather than waited
    /// for: a reporting operation materialises the transition once the clock is past it
    /// ([`AnomalyDetectorRecord::materialize_due_transitions`]). `None` once the detector is
    /// in a status the clock owes nothing after.
    pub status_due_ms: Option<i64>,
}

impl AnomalyDetectorRecord {
    /// A brand-new anomaly detector over the log groups it was asked to watch: still
    /// initializing, and due to start training [`ANOMALY_DETECTOR_TRAINING_DELAY_MS`] after
    /// it was created. This is what `CreateLogAnomalyDetector` stores.
    ///
    /// The optional members a create may name — `detectorName`, `evaluationFrequency`,
    /// `filterPattern`, `kmsKeyId`, `anomalyVisibilityTime`, `tags` — are left at their "the
    /// request named none" value and set on the returned record by the caller that has them.
    pub fn new(arn: &str, log_group_arn_list: Vec<String>, creation_time_ms: i64) -> Self {
        Self {
            arn: arn.to_string(),
            detector_name: None,
            log_group_arn_list,
            evaluation_frequency: None,
            filter_pattern: None,
            status: ANOMALY_DETECTOR_STATUS_INITIALIZING.to_string(),
            kms_key_id: None,
            creation_time_ms,
            last_modified_time_ms: creation_time_ms,
            anomaly_visibility_time: None,
            tags: BTreeMap::new(),
            status_due_ms: Some(creation_time_ms.saturating_add(ANOMALY_DETECTOR_TRAINING_DELAY_MS)),
        }
    }

    /// This detector as the model's `AnomalyDetector` structure, which is what
    /// `ListLogAnomalyDetectors` reports, so one record renders the same way through any
    /// reader of it.
    pub fn reported(&self) -> crate::ops::types::AnomalyDetector {
        crate::ops::types::AnomalyDetector {
            anomaly_detector_arn: Some(self.arn.clone()),
            detector_name: self.detector_name.clone(),
            log_group_arn_list: Some(self.log_group_arn_list.clone()),
            evaluation_frequency: self.evaluation_frequency.clone(),
            filter_pattern: self.filter_pattern.clone(),
            anomaly_detector_status: Some(self.status.clone()),
            kms_key_id: self.kms_key_id.clone(),
            creation_time_stamp: self.creation_time_ms,
            last_modified_time_stamp: self.last_modified_time_ms,
            anomaly_visibility_time: self.anomaly_visibility_time,
        }
    }

    /// Materialise the status changes the clock owes this detector: a detector whose next
    /// status was due before `now_ms` is already in it, and a detector that then became due
    /// for the one after is in that one too — each change taken AT the instant it was due,
    /// not at the time something happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        while let Some(due) = self.status_due_ms {
            if due > now_ms {
                return;
            }
            match self.status.as_str() {
                ANOMALY_DETECTOR_STATUS_INITIALIZING => {
                    self.status = ANOMALY_DETECTOR_STATUS_TRAINING.to_string();
                    self.status_due_ms =
                        Some(due.saturating_add(ANOMALY_DETECTOR_ANALYSIS_DELAY_MS));
                }
                ANOMALY_DETECTOR_STATUS_TRAINING => {
                    self.status = ANOMALY_DETECTOR_STATUS_ANALYZING.to_string();
                    self.status_due_ms = None;
                }
                // Every other status is one the clock owes nothing after: it was reached
                // because a call put the detector there.
                _ => self.status_due_ms = None,
            }
        }
    }
}

/// An anomaly detector's ARN, the form `AnomalyDetector$anomalyDetectorArn` carries: an
/// `anomaly-detector` resource of this service, identified by the detector's own id
/// ([`mint_anomaly_detector_id`]).
pub fn anomaly_detector_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("anomaly-detector:{id}"))
}

/// The identifier an anomaly detector's ARN carries, minted from the monotonic
/// anomaly-detector counter so it is unique and derived from stored state. AWS gives the
/// resource a UUID-shaped identifier, and the modelled `AnomalyDetectorArn` pattern
/// (`^[\w#+=/:,.@-]*$`) admits exactly that, so it is minted through the shared
/// [`mint_uuid_from_seq`] every UUID-shaped id of this service comes from.
pub fn mint_anomaly_detector_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// How many account-level policies of `policy_type` the account may hold IN TOTAL once a
/// policy whose scoped-ness is `scoped` is added: each quota sentence is an either/or —
/// "either one account-level metric extraction policy that applies to all log groups, or up
/// to 5 policies that are each scoped to a subset of log groups with the selectionCriteria
/// parameter" — so the number the account is held to is the one for the policy being put,
/// and every policy of that type counts against it. Every number is one
/// `PutAccountPolicy`'s own documentation states — see the constants it returns.
pub fn max_account_policies(policy_type: &str, scoped: bool) -> usize {
    if !scoped {
        return MAX_UNSCOPED_ACCOUNT_POLICIES;
    }
    match policy_type {
        POLICY_TYPE_TRANSFORMER | POLICY_TYPE_FIELD_INDEX => MAX_SCOPED_ACCOUNT_POLICIES,
        POLICY_TYPE_METRIC_EXTRACTION => MAX_SCOPED_METRIC_EXTRACTION_ACCOUNT_POLICIES,
        // A data protection policy and a subscription filter policy have no scoped half:
        // the account holds one of each, whatever it is scoped to.
        _ => MAX_UNSCOPED_ACCOUNT_POLICIES,
    }
}

/// What an accepted account-level policy document says: the parts a handler acts on after
/// [`check_account_policy_document`] accepts it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountPolicyShape {
    /// The log groups a data protection policy's Audit action sends findings to, as
    /// [`DataProtectionPolicyShape::findings_destination_log_groups`] reports them. Empty
    /// for every other policy type, none of which names a destination log group.
    pub findings_destination_log_groups: Vec<String>,
}

/// Judge a `PutAccountPolicy$policyDocument` against the rules that member's own
/// documentation states for the `policyType` it is put with, answering the message to
/// refuse it with, or what the accepted document says.
///
/// Every type's document is "the policy, in JSON" and can be "up to 30,720 characters
/// long". Beyond that each type has its own documented shape, and only the rules the
/// documentation states are enforced: which data identifiers, processors or fields exist
/// is a catalogue that lives outside this service, and the grammar of `selectionCriteria`
/// is judged by the caller of this function rather than here.
pub fn check_account_policy_document(
    policy_type: &str,
    document: &str,
) -> Result<AccountPolicyShape, String> {
    if document.chars().count() > MAX_ACCOUNT_POLICY_DOCUMENT_CHARS {
        return Err(format!(
            "policyDocument can be up to {MAX_ACCOUNT_POLICY_DOCUMENT_CHARS} characters"
        ));
    }
    match policy_type {
        // "A data protection policy must include two JSON blocks" — the same document the
        // log-group-level `PutDataProtectionPolicy` takes, so it is held to the same rule.
        POLICY_TYPE_DATA_PROTECTION => {
            let shape = check_data_protection_policy_document(document)?;
            Ok(AccountPolicyShape {
                findings_destination_log_groups: shape.findings_destination_log_groups,
            })
        }
        POLICY_TYPE_SUBSCRIPTION_FILTER => {
            check_subscription_filter_policy_document(document).map(|()| AccountPolicyShape::default())
        }
        POLICY_TYPE_FIELD_INDEX => {
            check_field_index_policy_document(document).map(|()| AccountPolicyShape::default())
        }
        POLICY_TYPE_TRANSFORMER => {
            check_transformer_policy_document(document).map(|()| AccountPolicyShape::default())
        }
        // A metric extraction policy "controls whether CloudWatch Metrics can be created
        // through the Embedded Metrics Format (EMF)", and the one thing it can say is that
        // EMF metric creation is off.
        _ => check_metric_extraction_policy_document(document).map(|()| AccountPolicyShape::default()),
    }
}

/// The document as the JSON object every policy type but a transformer policy is
/// expressed as ("Specify the policy, in JSON").
fn policy_object(document: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    serde_json::from_str::<serde_json::Value>(document)
        .ok()
        .and_then(|parsed| match parsed {
            serde_json::Value::Object(members) => Some(members),
            _ => None,
        })
        .filter(|members| !members.is_empty())
        .ok_or_else(|| "policyDocument must be the policy in JSON (a non-empty JSON object)".to_string())
}

/// Judge a `SUBSCRIPTION_FILTER_POLICY` document. The attributes are the ones
/// `PutAccountPolicy$policyDocument` lists for one: "DestinationArn The ARN of the
/// destination to deliver log events to", "RoleArn The ARN of an IAM role that grants
/// CloudWatch Logs permissions to deliver ingested log events to the destination stream",
/// "FilterPattern A filter pattern for subscribing to a filtered stream of log events" and
/// "Distribution The method used to distribute log data to the destination". Each is held
/// to the rule the log-group-level `PutSubscriptionFilter` holds the member of the same
/// name to, so one destination is as acceptable account-wide as it is per group.
fn check_subscription_filter_policy_document(document: &str) -> Result<(), String> {
    let members = policy_object(document)?;
    let string = |name: &str| members.get(name).and_then(serde_json::Value::as_str);
    let Some(destination_arn) = string("DestinationArn") else {
        return Err(
            "policyDocument for a subscription filter policy must include a DestinationArn"
                .to_string(),
        );
    };
    if let Some(message) = unparsable_arn_message("DestinationArn", destination_arn) {
        return Err(message);
    }
    // The same ordering as `PutSubscriptionFilter`: a destination that needs a role is
    // refused for the missing role before the kind of resource its ARN names is judged.
    if string("RoleArn").is_none() {
        if let Some(vendor) = subscription_role_required_vendor(destination_arn) {
            return Err(format!(
                "DestinationArn for vendor {vendor} cannot be used without RoleArn"
            ));
        }
    }
    if !is_subscription_destination_arn(destination_arn) {
        return Err(format!(
            "DestinationArn must be the ARN of a {} resource",
            SUBSCRIPTION_DESTINATIONS
                .iter()
                .map(|(service, _)| *service)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(filter_pattern) = string("FilterPattern") {
        validate_filter_pattern(filter_pattern)?;
    }
    // "the grouping can be set to Random for a more even distribution": the two values of
    // the model's `Distribution` enum, which no boundary validator holds a JSON document
    // to.
    if let Some(distribution) = string("Distribution") {
        if !crate::ops::types::DISTRIBUTION_VALUES.contains(&distribution) {
            return Err(invalid_value_message(
                "Distribution",
                crate::ops::types::DISTRIBUTION_VALUES.iter().copied(),
            ));
        }
    }
    Ok(())
}

/// Judge a `FIELD_INDEX_POLICY` document. From `PutAccountPolicy$policyDocument`: the
/// attributes are "Fields The array of field indexes to create" and "FieldsV2 The object
/// of field indexes to create along with it's type"; "It must contain at least one field
/// index"; "Supported types are FIELD_INDEX and FACET" (the model's `IndexType` enum);
/// and "Field names within Fields and FieldsV2 must be mutually exclusive."
fn check_field_index_policy_document(document: &str) -> Result<(), String> {
    let members = policy_object(document)?;
    let fields = match members.get("Fields") {
        Some(fields) => fields
            .as_array()
            .ok_or_else(|| "Fields in policyDocument must be an array of field names".to_string())?
            .iter()
            .map(|field| {
                field.as_str().map(str::to_string).ok_or_else(|| {
                    "Fields in policyDocument must be an array of field names".to_string()
                })
            })
            .collect::<Result<Vec<String>, String>>()?,
        None => Vec::new(),
    };
    let fields_v2 = match members.get("FieldsV2") {
        Some(fields_v2) => fields_v2
            .as_object()
            .ok_or_else(|| {
                "FieldsV2 in policyDocument must be an object of field names and their types"
                    .to_string()
            })?
            .clone(),
        None => serde_json::Map::new(),
    };
    for (name, configuration) in &fields_v2 {
        let index_type = configuration
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("the FieldsV2 entry '{name}' in policyDocument must carry a type"))?;
        if !crate::ops::types::INDEX_TYPE_VALUES.contains(&index_type) {
            return Err(invalid_value_message(
                &format!("FieldsV2 '{name}' type"),
                crate::ops::types::INDEX_TYPE_VALUES.iter().copied(),
            ));
        }
    }
    if fields.is_empty() && fields_v2.is_empty() {
        return Err(
            "policyDocument for a field index policy must contain at least one field index"
                .to_string(),
        );
    }
    if let Some(name) = fields.iter().find(|field| fields_v2.contains_key(*field)) {
        return Err(format!(
            "the field '{name}' is listed in both Fields and FieldsV2 in policyDocument; field names within Fields and FieldsV2 must be mutually exclusive"
        ));
    }
    Ok(())
}

/// Judge a `TRANSFORMER_POLICY` document: "A transformer policy must include one JSON
/// block with the array of processors and their configurations." Which processors exist,
/// and how each is configured, is the catalogue `PutTransformer` owns, so what is judged
/// here is that the document carries a non-empty array of processor blocks — written
/// either as that array itself or as an object holding it.
fn check_transformer_policy_document(document: &str) -> Result<(), String> {
    let expected =
        "policyDocument for a transformer policy must include an array of processors".to_string();
    let parsed = serde_json::from_str::<serde_json::Value>(document).map_err(|_| expected.clone())?;
    let processors = match &parsed {
        serde_json::Value::Array(processors) => processors.clone(),
        serde_json::Value::Object(members) => members
            .values()
            .find_map(|value| value.as_array().cloned())
            .ok_or_else(|| expected.clone())?,
        _ => return Err(expected),
    };
    if processors.is_empty() || !processors.iter().all(serde_json::Value::is_object) {
        return Err(expected);
    }
    Ok(())
}

/// Judge a `METRIC_EXTRACTION_POLICY` document.
///
/// Such a policy exists only to turn the Embedded Metrics Format off: "By default, EMF
/// metric creation is enabled for all log groups. You can use metric extraction policies to
/// disable EMF metric creation for your entire account or specific log groups." The
/// document says so through [`METRIC_EXTRACTION_EMF_MEMBER`]'s
/// [`METRIC_EXTRACTION_STATUS_MEMBER`], and the real service "accepts only the exact
/// string" [`METRIC_EXTRACTION_STATUS_DISABLED`] there — a document that says anything
/// else, including that EMF is enabled, is refused with
/// [`INVALID_JSON_POLICY_DOCUMENT_MESSAGE`]. Any other member of the document is left
/// alone: nothing documents one, so nothing is judged about it.
fn check_metric_extraction_policy_document(document: &str) -> Result<(), String> {
    let members = policy_object(document).map_err(|_| INVALID_JSON_POLICY_DOCUMENT_MESSAGE.to_string())?;
    let status = members
        .get(METRIC_EXTRACTION_EMF_MEMBER)
        .and_then(serde_json::Value::as_object)
        .and_then(|embedded_metric_format| embedded_metric_format.get(METRIC_EXTRACTION_STATUS_MEMBER))
        .and_then(serde_json::Value::as_str);
    if status != Some(METRIC_EXTRACTION_STATUS_DISABLED) {
        return Err(INVALID_JSON_POLICY_DOCUMENT_MESSAGE.to_string());
    }
    Ok(())
}

/// One filter of a `selectionCriteria` string: the field it selects on, whether it is
/// written with `NOT IN` rather than `IN`, and the values of its list.
///
/// The forms are the ones `PutAccountPolicy` spells out — `LogGroupName IN ["log-group-1",
/// "log-group-2"]`, `LogGroupNamePrefix NOT IN ["/aws/prefix1", "/aws/prefix2"]` — and the
/// filters are the ones `PutAccountPolicy$selectionCriteria` names per policy type
/// ([`SELECTION_FIELD_LOG_GROUP_NAME`], [`SELECTION_FIELD_LOG_GROUP_NAME_PREFIX`],
/// [`SELECTION_FIELD_DATA_SOURCE_NAME`], [`SELECTION_FIELD_DATA_SOURCE_TYPE`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionClause {
    /// The filter's field, as the criteria string spells it.
    pub field: String,
    /// `true` for a `NOT IN` list, `false` for an `IN` list.
    pub negated: bool,
    /// The values of the list, in the order written. Empty for the documented
    /// `LogGroupName NOT IN []`.
    pub values: Vec<String>,
}

impl SelectionClause {
    /// One clause, from the text before its list and the values of that list; `None` when
    /// the text is not `Field IN` or `Field NOT IN`.
    fn parse(head: &str, values: Vec<String>) -> Option<Self> {
        let mut tokens = head.split_whitespace();
        let field = tokens.next()?.to_string();
        let negated = match (tokens.next()?, tokens.next()) {
            ("IN", None) => false,
            ("NOT", Some("IN")) => true,
            _ => return None,
        };
        if tokens.next().is_some() {
            return None;
        }
        Some(Self {
            field,
            negated,
            values,
        })
    }
}

/// A `selectionCriteria` string, parsed into its filters.
///
/// More than one clause is only the field index policy's `DataSourceName` AND
/// `DataSourceType` pair — "you can use either LogGroupNamePrefix by itself or
/// DataSourceName and DataSourceType together" — which is the one documented form that
/// joins two filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionCriteria {
    /// The criteria's clauses, in the order written.
    pub clauses: Vec<SelectionClause>,
}

impl SelectionCriteria {
    /// This criteria's clause on `field`, when it carries one.
    pub fn clause(&self, field: &str) -> Option<&SelectionClause> {
        self.clauses.iter().find(|clause| clause.field == field)
    }

    /// Whether this criteria is exactly one clause on `field`, optionally with the
    /// operator `negated` names.
    fn is_single(&self, field: &str, negated: Option<bool>) -> bool {
        match self.clauses.as_slice() {
            [clause] => {
                clause.field == field && negated.is_none_or(|negated| clause.negated == negated)
            }
            _ => false,
        }
    }

    /// Whether this criteria is the `DataSourceName` AND `DataSourceType` pair.
    fn is_data_source_pair(&self) -> bool {
        self.clauses.len() == 2
            && self.clause(SELECTION_FIELD_DATA_SOURCE_NAME).is_some()
            && self.clause(SELECTION_FIELD_DATA_SOURCE_TYPE).is_some()
    }
}

/// One set of log groups a `selectionCriteria` value names.
///
/// A `LogGroupName` value names the one group called that; a `LogGroupNamePrefix` value
/// names every group whose name starts with it. Both comparisons below follow from those
/// two sets, which is what `PutAccountPolicy`'s overlap sentences are stated in terms of
/// ("the set of log groups matching these prefixes would be a subset of the log groups
/// matching the first policy's prefix, creating an overlap").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupSelector<'a> {
    /// The group whose name is exactly this.
    Name(&'a str),
    /// Every group whose name starts with this.
    Prefix(&'a str),
}

impl GroupSelector<'_> {
    /// Whether the two sets of log groups share a member.
    fn intersects(self, other: Self) -> bool {
        match (self, other) {
            (Self::Name(one), Self::Name(two)) => one == two,
            (Self::Name(name), Self::Prefix(prefix)) | (Self::Prefix(prefix), Self::Name(name)) => {
                name.starts_with(prefix)
            }
            (Self::Prefix(one), Self::Prefix(two)) => {
                one.starts_with(two) || two.starts_with(one)
            }
        }
    }

    /// Whether this set of log groups is wholly inside `other` — "completely contained
    /// within", the test `PutAccountPolicy` states for an `IN` value against a `NOT IN`
    /// one. A prefix names groups a single name does not, so it is contained in no name.
    fn contained_in(self, other: Self) -> bool {
        match (self, other) {
            (Self::Name(one), Self::Name(two)) => one == two,
            (Self::Name(name), Self::Prefix(prefix)) => name.starts_with(prefix),
            (Self::Prefix(one), Self::Prefix(two)) => one.starts_with(two),
            (Self::Prefix(_), Self::Name(_)) => false,
        }
    }
}

/// What one criteria selects: a set of log groups, or a data source name and type
/// combination. `None` for a criteria that selects neither, which nothing can overlap.
enum CriteriaSelection<'a> {
    /// The log groups the criteria's `LogGroupName` or `LogGroupNamePrefix` clause names,
    /// and whether that clause excludes them (`NOT IN`) rather than selects them.
    Groups {
        negated: bool,
        selectors: Vec<GroupSelector<'a>>,
    },
    /// The data source names and types of the criteria's `DataSourceName` AND
    /// `DataSourceType` pair.
    DataSource {
        names: &'a [String],
        types: &'a [String],
    },
}

impl<'a> CriteriaSelection<'a> {
    /// What `criteria` selects.
    fn of(criteria: &'a SelectionCriteria) -> Option<Self> {
        if let (Some(names), Some(types)) = (
            criteria.clause(SELECTION_FIELD_DATA_SOURCE_NAME),
            criteria.clause(SELECTION_FIELD_DATA_SOURCE_TYPE),
        ) {
            return Some(Self::DataSource {
                names: &names.values,
                types: &types.values,
            });
        }
        let clause = criteria
            .clause(SELECTION_FIELD_LOG_GROUP_NAME)
            .or_else(|| criteria.clause(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX))?;
        let selectors = clause
            .values
            .iter()
            .map(|value| {
                if clause.field == SELECTION_FIELD_LOG_GROUP_NAME {
                    GroupSelector::Name(value)
                } else {
                    GroupSelector::Prefix(value)
                }
            })
            .collect();
        Some(Self::Groups {
            negated: clause.negated,
            selectors,
        })
    }
}

/// Judge a `PutAccountPolicy$selectionCriteria` string against the filters that member
/// documents for the `policyType` it is put with, answering the message to refuse it with,
/// or the parsed criteria.
///
/// The filters are exactly the documented ones:
///
/// * `SUBSCRIPTION_FILTER_POLICY` — "the only supported selectionCriteria filter is
///   `LogGroupName NOT IN []`";
/// * `TRANSFORMER_POLICY` — "the only supported selectionCriteria filter is
///   `LogGroupNamePrefix`";
/// * `FIELD_INDEX_POLICY` — "you can use either `LogGroupNamePrefix` by itself or
///   `DataSourceName` and `DataSourceType` together";
/// * `METRIC_EXTRACTION_POLICY` — "The selection criteria supports filtering by
///   `LogGroupName` and `LogGroupNamePrefix` using the operators `IN` and `NOT IN`. You can
///   specify up to 50 values in each `IN` or `NOT IN` list."
///
/// Which operator a `LogGroupNamePrefix` filter is written with is not documented for a
/// transformer or a field index policy, so both are accepted there.
pub fn check_selection_criteria(policy_type: &str, criteria: &str) -> Result<SelectionCriteria, String> {
    let parsed = SelectionCriteria {
        clauses: parse_selection_clauses(criteria)?,
    };
    let supported = match policy_type {
        POLICY_TYPE_SUBSCRIPTION_FILTER => {
            parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME, Some(true))
        }
        POLICY_TYPE_TRANSFORMER => parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX, None),
        POLICY_TYPE_FIELD_INDEX => {
            parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX, None)
                || parsed.is_data_source_pair()
        }
        POLICY_TYPE_METRIC_EXTRACTION => {
            parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME, None)
                || parsed.is_single(SELECTION_FIELD_LOG_GROUP_NAME_PREFIX, None)
        }
        // A data protection policy admits no criteria at all, and its caller refuses one
        // before it gets here.
        _ => false,
    };
    if !supported {
        return Err(format!(
            "selectionCriteria for a {policy_type} must be {}",
            selection_criteria_forms(policy_type)
        ));
    }
    if policy_type == POLICY_TYPE_METRIC_EXTRACTION
        && parsed
            .clauses
            .iter()
            .any(|clause| clause.values.len() > MAX_SELECTION_CRITERIA_VALUES)
    {
        return Err(format!(
            "selectionCriteria can specify up to {MAX_SELECTION_CRITERIA_VALUES} values in each IN or NOT IN list"
        ));
    }
    Ok(parsed)
}

/// The `selectionCriteria` filters one policy type admits, as a refusal names them. See
/// [`check_selection_criteria`] for where each comes from.
fn selection_criteria_forms(policy_type: &str) -> &'static str {
    match policy_type {
        POLICY_TYPE_SUBSCRIPTION_FILTER => "the filter LogGroupName NOT IN [...]",
        POLICY_TYPE_TRANSFORMER => "a LogGroupNamePrefix IN [...] filter",
        POLICY_TYPE_FIELD_INDEX => {
            "a LogGroupNamePrefix IN [...] filter, or DataSourceName IN [...] AND DataSourceType IN [...] together"
        }
        POLICY_TYPE_METRIC_EXTRACTION => {
            "a LogGroupName or LogGroupNamePrefix filter with the IN or NOT IN operator"
        }
        _ => "omitted: this policy type applies to all log groups in the account",
    }
}

/// Split a `selectionCriteria` string into its clauses — each `Field IN [...]` or `Field
/// NOT IN [...]`, joined by `AND` — refusing a string that is not written that way.
fn parse_selection_clauses(criteria: &str) -> Result<Vec<SelectionClause>, String> {
    let malformed = || {
        "selectionCriteria must be one or more filters of the form 'Field IN [\"value\", ...]' or 'Field NOT IN [\"value\", ...]', joined by AND"
            .to_string()
    };
    let mut clauses = Vec::new();
    let mut rest = criteria.trim();
    loop {
        let open = rest.find('[').ok_or_else(malformed)?;
        let (head, list) = rest.split_at(open);
        let close = selection_list_end(list).ok_or_else(malformed)?;
        // The list is a JSON array of strings in every documented form, so it is read as
        // one rather than scanned by hand.
        let values = serde_json::from_str::<Vec<String>>(&list[..=close]).map_err(|_| malformed())?;
        clauses.push(SelectionClause::parse(head, values).ok_or_else(malformed)?);
        rest = list[close + 1..].trim_start();
        if rest.is_empty() {
            break;
        }
        let joined = rest.strip_prefix("AND").ok_or_else(malformed)?;
        if !joined.starts_with(char::is_whitespace) {
            return Err(malformed());
        }
        rest = joined.trim_start();
        if rest.is_empty() {
            return Err(malformed());
        }
    }
    if clauses.is_empty() {
        return Err(malformed());
    }
    Ok(clauses)
}

/// The byte index of the `]` closing the list that starts at the beginning of `list`, or
/// `None` for a list that is never closed. A `]` inside a quoted value does not close it.
fn selection_list_end(list: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in list.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            ']' => return Some(index),
            _ => {}
        }
    }
    None
}

/// Refuse a `selectionCriteria` that conflicts with the criteria of another account-level
/// policy of the same type, answering the message to refuse it with.
///
/// `PutAccountPolicy` states the rule per type:
///
/// * a transformer policy — "If you have multiple account-level transformer policies with
///   selection criteria, no two of them can use the same or overlapping log group name
///   prefixes" — and a field index policy, with the same sentence for its
///   `LogGroupNamePrefix` criteria plus "no two of them can use the same data source name
///   and type combination" for its data source criteria;
/// * a metric extraction policy — "no two of them can have overlapping criteria", "When
///   using NOT IN, only one policy with this operator is allowed per account", and two
///   policies with `IN` and `NOT IN` "do not overlap if and only if every value in the IN
///   policy is completely contained within some value in the NOT IN policy".
///
/// A policy is identified by its type and name, so the policy being updated is not
/// compared with itself.
pub fn check_account_policy_criteria_overlap(
    policies: &BTreeMap<(String, String), AccountPolicyRecord>,
    policy_type: &str,
    policy_name: &str,
    criteria: &SelectionCriteria,
) -> Result<(), String> {
    let Some(putting) = CriteriaSelection::of(criteria) else {
        return Ok(());
    };
    for held in policies.values() {
        if held.policy_type != policy_type || held.policy_name == policy_name {
            continue;
        }
        let Some(held_criteria) = held.selection_criteria.as_deref() else {
            continue;
        };
        // A stored criteria string went through the same grammar when it was put, so one
        // that does not parse selects nothing there is to conflict with.
        let Ok(held_criteria) = check_selection_criteria(policy_type, held_criteria) else {
            continue;
        };
        let Some(held_selection) = CriteriaSelection::of(&held_criteria) else {
            continue;
        };
        if let Some(conflict) = criteria_conflict(policy_type, &putting, &held_selection) {
            return Err(format!(
                "selectionCriteria conflicts with the account-level {policy_type} policy '{}': {conflict}",
                held.policy_name
            ));
        }
    }
    Ok(())
}

/// Why two criteria of one policy type conflict, or `None` when they do not. See
/// [`check_account_policy_criteria_overlap`] for the sentence behind each answer.
fn criteria_conflict(
    policy_type: &str,
    putting: &CriteriaSelection<'_>,
    held: &CriteriaSelection<'_>,
) -> Option<&'static str> {
    match (putting, held) {
        (
            CriteriaSelection::DataSource { names, types },
            CriteriaSelection::DataSource {
                names: held_names,
                types: held_types,
            },
        ) => {
            let same_name = names.iter().any(|name| held_names.contains(name));
            let same_type = types.iter().any(|source| held_types.contains(source));
            (same_name && same_type)
                .then_some("no two of them can use the same data source name and type combination")
        }
        (
            CriteriaSelection::Groups { negated, selectors },
            CriteriaSelection::Groups {
                negated: held_negated,
                selectors: held_selectors,
            },
        ) => {
            if policy_type != POLICY_TYPE_METRIC_EXTRACTION {
                // Every other type's sentence is about the prefixes themselves: "no two of
                // them can use the same or overlapping log group name prefixes".
                return selectors
                    .iter()
                    .any(|one| held_selectors.iter().any(|two| one.intersects(*two)))
                    .then_some("no two of them can use the same or overlapping log group name prefixes");
            }
            match (negated, held_negated) {
                (true, true) => {
                    Some("when using NOT IN, only one policy with this operator is allowed per account")
                }
                (false, false) => selectors
                    .iter()
                    .any(|one| held_selectors.iter().any(|two| one.intersects(*two)))
                    .then_some("no two of them can have overlapping criteria"),
                // One selects the log groups it lists and the other excludes the ones it
                // lists: they do not overlap only when every selected value is completely
                // contained within some excluded value.
                (false, true) => (!selectors
                    .iter()
                    .all(|one| held_selectors.iter().any(|two| one.contained_in(*two))))
                .then_some(
                    "every value of an IN policy must be completely contained within some value of the NOT IN policy",
                ),
                (true, false) => (!held_selectors
                    .iter()
                    .all(|one| selectors.iter().any(|two| one.contained_in(*two))))
                .then_some(
                    "every value of an IN policy must be completely contained within some value of the NOT IN policy",
                ),
            }
        }
        // One criteria selects log groups and the other a data source name and type: they
        // select along different dimensions, which the field index policy's quota sentences
        // count separately, so neither constrains the other.
        _ => None,
    }
}

/// One log group's data protection policy — what `PutDataProtectionPolicy` stores — as
/// it is stored.
///
/// The document is kept VERBATIM, exactly the bytes the request carried: AWS reports it
/// back through `GetDataProtectionPolicy` unchanged, so re-serializing the parsed JSON
/// (which would reorder members and drop whitespace) would not be the same document.
#[derive(Debug, Clone)]
pub struct DataProtectionPolicyRecord {
    /// `policyDocument`, byte-for-byte as the put carried it.
    pub policy_document: String,
    /// "The date and time that this policy was most recently updated."
    pub last_updated_time_ms: i64,
}

/// What a data protection policy document says, once it has been judged: the parts a
/// handler acts on after [`check_data_protection_policy_document`] accepts it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataProtectionPolicyShape {
    /// The data identifiers both of the document's blocks list, in the order the Audit
    /// block lists them.
    pub data_identifiers: Vec<String>,
    /// The log groups the Audit action's `FindingsDestination` sends audit findings to
    /// (`FindingsDestination.CloudWatchLogs.LogGroup`). A destination this service does
    /// not own (a Firehose stream, an S3 bucket) is not here: only a log group lives in
    /// this service's state.
    pub findings_destination_log_groups: Vec<String>,
}

/// Judge a `PutDataProtectionPolicy$policyDocument`, answering the message to refuse it
/// with, or what the accepted document says.
///
/// Every rule here is one that member's own documentation states:
///
/// * "Specify the data protection policy, in JSON" — and "This policy must include two
///   JSON blocks", so the document is a JSON object carrying a `Statement` array;
/// * "The first block must include both a `DataIdentifer` array and an `Operation`
///   property with an `Audit` action", and "This `Audit` action must contain a
///   `FindingsDestination` object";
/// * "The second block must include both a `DataIdentifer` array and an `Operation`
///   property with an `Deidentify` action", which "must contain the `"MaskConfig": {}`
///   object. The `"MaskConfig": {}` object must be empty";
/// * "The contents of the two `DataIdentifer` arrays must match exactly";
/// * "The JSON specified in `policyDocument` can be up to 30,720 characters."
///
/// The documentation spells the member `DataIdentifer` while the API's own policy
/// documents spell it `DataIdentifier`, so both spellings are read.
///
/// Which data identifiers exist, and whether a Firehose stream or S3 bucket named as a
/// findings destination exists, are NOT judged here: that catalogue and those resources
/// live outside this service.
pub fn check_data_protection_policy_document(
    document: &str,
) -> Result<DataProtectionPolicyShape, String> {
    if document.chars().count() > MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS {
        return Err(format!(
            "policyDocument can be up to {MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS} characters"
        ));
    }
    let parsed = serde_json::from_str::<serde_json::Value>(document)
        .ok()
        .filter(serde_json::Value::is_object)
        .ok_or_else(|| {
            "policyDocument must be a data protection policy in JSON (a JSON object)".to_string()
        })?;
    let statements = parsed
        .get("Statement")
        .and_then(serde_json::Value::as_array)
        .filter(|statements| !statements.is_empty())
        .ok_or_else(|| {
            "policyDocument must include a Statement array of the policy's two blocks".to_string()
        })?;

    // One block per action: the Audit block that finds the sensitive data terms, and the
    // Deidentify block that masks them.
    let block = |action: &str| {
        statements.iter().find(|statement| {
            statement
                .get("Operation")
                .and_then(|operation| operation.get(action))
                .is_some_and(serde_json::Value::is_object)
        })
    };
    let audit = block("Audit").ok_or_else(|| {
        "policyDocument must include a Statement block whose Operation has an Audit action"
            .to_string()
    })?;
    let deidentify = block("Deidentify").ok_or_else(|| {
        "policyDocument must include a Statement block whose Operation has a Deidentify action"
            .to_string()
    })?;

    let findings_destination = audit
        .get("Operation")
        .and_then(|operation| operation.get("Audit"))
        .and_then(|action| action.get("FindingsDestination"))
        .filter(|destination| destination.is_object())
        .ok_or_else(|| {
            "the Audit action in policyDocument must contain a FindingsDestination object"
                .to_string()
        })?;
    let mask_config = deidentify
        .get("Operation")
        .and_then(|operation| operation.get("Deidentify"))
        .and_then(|action| action.get("MaskConfig"))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            "the Deidentify action in policyDocument must contain a MaskConfig object".to_string()
        })?;
    if !mask_config.is_empty() {
        return Err("the MaskConfig object in policyDocument must be empty".to_string());
    }

    let identifiers = |statement: &serde_json::Value| -> Option<Vec<String>> {
        let listed = statement
            .get("DataIdentifier")
            .or_else(|| statement.get("DataIdentifer"))?
            .as_array()?;
        if listed.is_empty() {
            return None;
        }
        listed
            .iter()
            .map(|identifier| identifier.as_str().map(str::to_string))
            .collect()
    };
    let audit_identifiers = identifiers(audit).ok_or_else(|| {
        "the Audit block in policyDocument must include a DataIdentifier array of data identifiers"
            .to_string()
    })?;
    let deidentify_identifiers = identifiers(deidentify).ok_or_else(|| {
        "the Deidentify block in policyDocument must include a DataIdentifier array of data identifiers"
            .to_string()
    })?;
    // "must match exactly" is about the CONTENTS of the two arrays, so the order they are
    // listed in is not part of the match.
    let sorted = |mut identifiers: Vec<String>| {
        identifiers.sort();
        identifiers
    };
    if sorted(audit_identifiers.clone()) != sorted(deidentify_identifiers) {
        return Err(
            "the contents of the two DataIdentifier arrays in policyDocument must match exactly"
                .to_string(),
        );
    }

    Ok(DataProtectionPolicyShape {
        data_identifiers: audit_identifiers,
        findings_destination_log_groups: findings_destination
            .get("CloudWatchLogs")
            .and_then(|destination| destination.get("LogGroup"))
            .and_then(serde_json::Value::as_str)
            .map(|log_group| vec![log_group.to_string()])
            .unwrap_or_default(),
    })
}

/// One created log group and everything stored under it.
#[derive(Debug, Clone)]
pub struct LogGroupRecord {
    pub name: String,
    /// The ARN version that carries a trailing `:*` (the `LogGroup$arn` member).
    pub arn: String,
    /// The ARN version without the trailing `:*` (the `LogGroup$logGroupArn` member).
    pub log_group_arn: String,
    pub creation_time_ms: i64,
    pub retention_in_days: Option<i32>,
    pub kms_key_id: Option<String>,
    pub log_group_class: String,
    pub deletion_protection_enabled: bool,
    /// Whether bearer token authentication is enabled for this group
    /// (`LogGroup$bearerTokenAuthenticationEnabled`: "Indicates whether bearer token
    /// authentication is enabled for this log group. When enabled, bearer token
    /// authentication is allowed on operations until it is explicitly disabled.").
    /// `PutBearerTokenAuthentication` is what moves it; a new group is not enabled.
    pub bearer_token_authentication_enabled: bool,
    pub stored_bytes: i64,
    pub metric_filter_count: i32,
    pub tags: BTreeMap<String, String>,
    /// The group's log streams, keyed by `logStreamName`.
    pub streams: BTreeMap<String, LogStreamRecord>,
    /// The group's subscription filters, keyed by `filterName`. A `BTreeMap` so
    /// `DescribeSubscriptionFilters` enumerates them ASCII-sorted by filter name
    /// without re-sorting, and so a put under a name already stored updates that
    /// filter in place.
    pub subscription_filters: BTreeMap<String, SubscriptionFilterRecord>,
    /// The resource-scoped policy attached to this group, if one has been put: AWS
    /// allows "one per LogGroup resourceARN", so at most one.
    pub resource_policy: Option<ResourcePolicyRecord>,
    /// The log-group level transformer `PutTransformer` created for this group, if one
    /// has been put. A log group has at most one — `PutTransformer` "creates or updates
    /// a log transformer for a single log group" and names it by nothing but the group —
    /// and it lives on the group, so deleting the group takes its transformer with it.
    pub transformer: Option<TransformerRecord>,
    /// The data protection policy attached to this group, if one has been put:
    /// `PutDataProtectionPolicy` "applies to only the specified log group" and a put
    /// replaces the policy the group holds, so at most one. `None` again after
    /// `DeleteDataProtectionPolicy`, and gone with the group when it is deleted.
    pub data_protection_policy: Option<DataProtectionPolicyRecord>,
    /// Whether this group has EVER held a data protection policy, which is what lets
    /// `LogGroup$dataProtectionStatus` report "whether this log group has a protection
    /// policy, or whether it had one in the past". See
    /// [`LogGroupRecord::data_protection_status`].
    pub had_data_protection_policy: bool,
    /// The data sources that ingest into this group — what `ListLogGroups`'s
    /// `dataSources` filter selects on and what `ListAggregateLogGroupSummaries`
    /// counts log groups by. See [`DataSourceRecord`] for why a group created through
    /// this emulator's API carries none.
    pub data_sources: Vec<DataSourceRecord>,
    /// The syslog configurations put on this group, keyed by the `vpcEndpointId` the
    /// configuration ingests through — `PutSyslogConfiguration` "enables ingestion of
    /// syslog data through the specified VPC endpoint into the log group", so one group
    /// holds at most one configuration per endpoint and a put under an endpoint already
    /// stored updates that configuration in place. A `BTreeMap` so
    /// `ListSyslogConfigurations` enumerates them ASCII-sorted by endpoint id without
    /// re-sorting. Every configuration names an endpoint ([`VPC_ENDPOINT_ID_REQUIRED`]).
    pub syslog_configurations: BTreeMap<String, SyslogConfigurationRecord>,
}

impl LogGroupRecord {
    /// A brand-new log group: the class default, no retention, no stored bytes.
    pub fn new(region: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_group_arn(region, name, true),
            log_group_arn: log_group_arn(region, name, false),
            creation_time_ms,
            retention_in_days: None,
            kms_key_id: None,
            log_group_class: DEFAULT_LOG_GROUP_CLASS.to_string(),
            deletion_protection_enabled: false,
            bearer_token_authentication_enabled: false,
            stored_bytes: 0,
            metric_filter_count: 0,
            tags: BTreeMap::new(),
            streams: BTreeMap::new(),
            subscription_filters: BTreeMap::new(),
            resource_policy: None,
            transformer: None,
            data_protection_policy: None,
            had_data_protection_policy: false,
            data_sources: Vec::new(),
            syslog_configurations: BTreeMap::new(),
        }
    }

    /// This group's `dataProtectionStatus`: `ACTIVATED` while it holds a data protection
    /// policy, `DELETED` once one it held has been deleted, and absent for a group that
    /// has never held one — the member "displays whether this log group has a protection
    /// policy, or whether it had one in the past", so a group with neither has nothing to
    /// report. The other two values of the `DataProtectionStatus` enum belong to the
    /// account-level policies of `PutAccountPolicy`
    /// ([`ComAmazonawsCloudwatchlogsLogs20140328State::account_policies`]), which no
    /// operation of this service moves a per-group status for yet.
    pub fn data_protection_status(&self) -> Option<&'static str> {
        if self.data_protection_policy.is_some() {
            Some(DATA_PROTECTION_STATUS_ACTIVATED)
        } else if self.had_data_protection_policy {
            Some(DATA_PROTECTION_STATUS_DELETED)
        } else {
            None
        }
    }
}

/// The one value the model's `SyslogSourceType` enum admits
/// ([`crate::ops::types::SYSLOG_SOURCE_TYPE_VALUES`]), and the source a syslog
/// configuration ingests through: the VPC endpoint `PutSyslogConfiguration` names. Kept as
/// a named constant so the stamped `SyslogConfiguration$sourceType` is the model's value
/// rather than a literal typed into a handler; `tests/all/syslog_configurations.rs` pins
/// what a listed configuration reports to that catalogue.
pub const SYSLOG_SOURCE_TYPE: &str = "VPCE";

/// The key a syslog configuration is stored under: its `vpcEndpointId`. The member is
/// OPTIONAL in the model on both `PutSyslogConfiguration` and
/// `DeleteSyslogConfiguration`, but a syslog configuration IS the ingestion a log group
/// accepts through one VPC endpoint — the only `SyslogSourceType` the model admits is
/// `VPCE`, and the endpoint is the only thing that distinguishes one configuration on a log
/// group from another — so the service refuses a request that names none. This is that
/// refusal's sentence: the service's own fixed wording for the constraint, naming the member
/// and what it requires.
pub const VPC_ENDPOINT_ID_REQUIRED: &str = "vpcEndpointId is required";

/// One syslog configuration stored on a log group: the model's `SyslogConfiguration`
/// minus the `logGroupArn` the owning group already carries.
#[derive(Debug, Clone)]
pub struct SyslogConfigurationRecord {
    /// `SyslogConfiguration$vpcEndpointId` — "The ID of the VPC endpoint used for syslog
    /// ingestion", which is also the key the owning group stores this configuration under
    /// (see [`VPC_ENDPOINT_ID_REQUIRED`]).
    pub vpc_endpoint_id: String,
    /// `SyslogConfiguration$sourceType` — "The source type for the syslog
    /// configuration": [`SYSLOG_SOURCE_TYPE`].
    pub source_type: String,
    /// `SyslogConfiguration$createdAt` — "The time when the syslog configuration was
    /// created, expressed as the number of milliseconds after Jan 1, 1970 00:00:00 UTC".
    /// A put over a configuration already stored UPDATES it, so this keeps the instant the
    /// first put created it.
    pub created_at_ms: i64,
}

/// One data source that ingests into a log group: the model's `DataSource` — "a data
/// source that categorizes logs by originating service and log type" — plus the format
/// the aggregate summaries can additionally group by (`dataSource.Format`).
///
/// A log group created through this emulator's API carries none, and that is a property
/// of the API surface rather than a shortcut: the model's `LogGroup` has no data-source
/// member, no operation associates a data source WITH a log group (the one operation that
/// names a `DataSource`, `AssociateSourceToS3TableIntegration`, attaches it to an S3
/// Tables integration, and is a 501 stub), and the data sources AWS reports are the ones
/// its own vended-log ingestion created. So the aggregate grouped by data-source
/// characteristics counts nothing here — which is a 200 with an empty list, not a
/// refusal — and both readers go through this field, so they report whatever a later
/// operation stores in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DataSourceRecord {
    /// `DataSource$name` — "The name of the data source."
    pub name: String,
    /// `DataSource$type` — "The type of the data source."
    pub source_type: Option<String>,
    /// The data source's format, which `groupBy=DATA_SOURCE_NAME_TYPE_AND_FORMAT`
    /// groups by (`GroupingIdentifier`: "Examples are, dataSource.Name,
    /// dataSource.Type, and dataSource.Format").
    pub format: Option<String>,
}

/// Whether one data source satisfies a `dataSources` filter list.
///
/// From `ListLogGroups$dataSources`: "You can filter by data source name, type, or both.
/// Multiple filters within the same dimension are combined with OR logic, while filters
/// across different dimensions are combined with AND logic" — so every filter's `name`
/// forms one dimension and every filter's `type` the other, and each is matched as the
/// "name pattern" / "type pattern" the members document: a case-sensitive substring.
pub fn data_source_matches_filters(
    source: &DataSourceRecord,
    filters: &[crate::ops::types::DataSourceFilter],
) -> bool {
    // `DataSourceFilter$name` is `@required`, so the name dimension is never empty.
    let name_matches = filters
        .iter()
        .any(|filter| source.name.contains(&filter.name));
    let wanted_types: Vec<&str> = filters
        .iter()
        .filter_map(|filter| filter.r#type.as_deref())
        .collect();
    let type_matches = wanted_types.is_empty()
        || source.source_type.as_deref().is_some_and(|source_type| {
            wanted_types
                .iter()
                .any(|wanted| source_type.contains(wanted))
        });
    name_matches && type_matches
}

/// Whether one log group satisfies a `dataSources` filter list: one of its data sources
/// does ([`data_source_matches_filters`]).
pub fn log_group_matches_data_source_filters(
    group: &LogGroupRecord,
    filters: &[crate::ops::types::DataSourceFilter],
) -> bool {
    group
        .data_sources
        .iter()
        .any(|source| data_source_matches_filters(source, filters))
}

/// Whether one character may appear in a `logGroupNamePattern` alternative, from the
/// `LogGroupNameRegexPattern` `@pattern`'s character class `[\.\-_\/#A-Za-z0-9]` — which
/// is exactly the class a `LogGroupName` is built from, so an alternative can only ever
/// be literal text.
fn is_log_group_name_pattern_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '/' | '#')
}

/// The `LogGroupNameRegexPattern` `@pattern` as the service states it in a refusal: the
/// modelled expression itself, anchors included — the one pattern-rendering rule this
/// service words every constraint refusal with, stated on `requirement` in
/// `crate::service`. It includes the per-alternative 24-character maximum the service
/// accepts values past, which the emulator therefore does not enforce (see
/// [`check_log_group_name_pattern`]).
pub const LOG_GROUP_NAME_REGEX_PATTERN: &str =
    r"^(\^?[\.\-_\/#A-Za-z0-9]{3,24})(\|\^?[\.\-_\/#A-Za-z0-9]{3,24}){0,4}$";

/// One refused member, worded on the template CloudWatch Logs words a constraint
/// violation with: "N validation error detected: Value at '<member>' failed to satisfy
/// constraint: <constraint>". The template is built from the request — the member that
/// broke a constraint, and that member's modelled constraint — and carries NOTHING of the
/// submitted value.
///
/// Delegates to `crate::service`, which owns that template for every refusal of this
/// service, so a violation a handler finds and one the boundary walk finds read alike.
pub fn validation_message(member: &str, constraint: &str) -> String {
    crate::service::one_violation_message(member, constraint)
}

/// Judge a `logGroupNamePattern` — the member `ListLogGroups` and
/// `ListAggregateLogGroupSummaries` share — answering the message to refuse it with.
///
/// The member targets `LogGroupNameRegexPattern`, whose `@pattern` decomposes the value
/// into up to five `|`-separated alternatives of 3..24 characters. The 24-character
/// MAXIMUM is the one part of that the service does not apply: it answers 200 for a
/// single-alternative pattern far longer than 24 characters, which is what a pattern
/// naming a real log group looks like. Everything else the model and the documentation
/// state is enforced here, because the member carries `skip_validation` and the boundary
/// walk no longer sees it:
///
/// * the `@length(min: 3, max: 129)` on the whole value;
/// * "as many as five different regular expression patterns in this field";
/// * the minimum of "each of which must be between 3 and 24 characters";
/// * the character class, plus the optional leading `^` anchor on each alternative.
pub fn check_log_group_name_pattern(pattern: &str) -> Result<(), String> {
    const MEMBER: &str = "logGroupNamePattern";
    let refuse = |constraint: String| Err(validation_message(MEMBER, &constraint));

    let length = pattern.chars().count();
    if length < LOG_GROUP_NAME_PATTERN_MIN_LEN {
        return refuse(format!(
            "Member must have length greater than or equal to {LOG_GROUP_NAME_PATTERN_MIN_LEN}"
        ));
    }
    if length > LOG_GROUP_NAME_PATTERN_MAX_LEN {
        return refuse(format!(
            "Member must have length less than or equal to {LOG_GROUP_NAME_PATTERN_MAX_LEN}"
        ));
    }
    let alternatives: Vec<&str> = pattern.split(LOG_GROUP_NAME_PATTERN_SEPARATOR).collect();
    if alternatives.len() > MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES {
        return refuse(format!(
            "Member must specify at most {MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES} \
             '{LOG_GROUP_NAME_PATTERN_SEPARATOR}'-separated patterns"
        ));
    }
    for alternative in alternatives {
        let literal = alternative
            .strip_prefix(LOG_GROUP_NAME_PATTERN_ANCHOR)
            .unwrap_or(alternative);
        let shaped = literal.chars().count() >= MIN_LOG_GROUP_NAME_PATTERN_ALTERNATIVE_LEN
            && literal.chars().all(is_log_group_name_pattern_char);
        if !shaped {
            return refuse(format!(
                "Member must satisfy regular expression pattern: \
                 {LOG_GROUP_NAME_REGEX_PATTERN}"
            ));
        }
    }
    Ok(())
}

/// Whether a log group name matches a `logGroupNamePattern`.
///
/// The member documents the whole match rule: "Use ^ to match log group names by prefix.
/// For a substring match, specify the string to match. All matches are case sensitive To
/// match multiple patterns, separate them with a | as in the example ^/aws/lambda|
/// discovery". The pattern's character class admits no regular-expression metacharacter
/// beyond that `^` and `|`, so each alternative is literal text: anchored, a prefix;
/// otherwise, a substring. A name matches when ANY alternative matches it.
///
/// Caller-side the value has already been through [`check_log_group_name_pattern`].
pub fn log_group_name_matches_pattern(name: &str, pattern: &str) -> bool {
    pattern
        .split(LOG_GROUP_NAME_PATTERN_SEPARATOR)
        .any(|alternative| {
            match alternative.strip_prefix(LOG_GROUP_NAME_PATTERN_ANCHOR) {
                Some(prefix) => name.starts_with(prefix),
                None => name.contains(alternative),
            }
        })
}

/// One log-group level transformer — "a series of processors, where each processor
/// applies one type of transformation to the log events ingested into this log group" —
/// as it is stored, on the single log group it transforms for.
#[derive(Debug, Clone)]
pub struct TransformerRecord {
    /// The processors, in the order the put listed them: "The processors work one after
    /// another, in the order that you list them, like a pipeline", so the order is part
    /// of the configuration and a read reports it back unchanged.
    pub processors: Vec<crate::ops::types::Processor>,
    /// When this transformer was first created for the group. An update keeps it: a put
    /// over an existing transformer is the same transformer, reconfigured.
    pub creation_time_ms: i64,
    /// When the transformer was last put, which for a transformer created and never
    /// updated is its creation time.
    pub last_modified_time_ms: i64,
}

/// The transformations one processor names, as the model spells the members that carry
/// them.
///
/// `Processor` is "the information about one processor in a log transformer" and a
/// `transformerConfig` is "an array of processors, where each processor applies one type
/// of transformation", so exactly one of the structure's members is populated. Reporting
/// the populated ones lets a put refuse a processor that names none, and one that names
/// several, with a message that lists what it named.
pub fn processor_transformations(processor: &crate::ops::types::Processor) -> Vec<&'static str> {
    // Destructured EXHAUSTIVELY — no `..` — so a processor kind the model gains is a
    // COMPILE error here rather than a member this function silently cannot see, which
    // would make a put naming only that kind look like a processor naming none.
    let crate::ops::types::Processor {
        add_keys,
        copy_value,
        csv,
        date_time_converter,
        delete_keys,
        grok,
        list_to_map,
        lower_case_string,
        move_keys,
        parse_cloudfront,
        parse_json,
        parse_key_value,
        parse_route53,
        parse_to_ocsf,
        parse_postgres,
        parse_vpc,
        parse_waf,
        rename_keys,
        split_string,
        substitute_string,
        trim_string,
        type_converter,
        upper_case_string,
    } = processor;
    // The member names are the model's own (`Processor`'s members), in model order.
    [
        ("addKeys", add_keys.is_some()),
        ("copyValue", copy_value.is_some()),
        ("csv", csv.is_some()),
        ("dateTimeConverter", date_time_converter.is_some()),
        ("deleteKeys", delete_keys.is_some()),
        ("grok", grok.is_some()),
        ("listToMap", list_to_map.is_some()),
        ("lowerCaseString", lower_case_string.is_some()),
        ("moveKeys", move_keys.is_some()),
        ("parseCloudfront", parse_cloudfront.is_some()),
        ("parseJSON", parse_json.is_some()),
        ("parseKeyValue", parse_key_value.is_some()),
        ("parseRoute53", parse_route53.is_some()),
        ("parseToOCSF", parse_to_ocsf.is_some()),
        ("parsePostgres", parse_postgres.is_some()),
        ("parseVPC", parse_vpc.is_some()),
        ("parseWAF", parse_waf.is_some()),
        ("renameKeys", rename_keys.is_some()),
        ("splitString", split_string.is_some()),
        ("substituteString", substitute_string.is_some()),
        ("trimString", trim_string.is_some()),
        ("typeConverter", type_converter.is_some()),
        ("upperCaseString", upper_case_string.is_some()),
    ]
    .into_iter()
    .filter_map(|(member, populated)| populated.then_some(member))
    .collect()
}

/// One saved CloudWatch Logs Insights query definition — "details about a saved
/// CloudWatch Logs Insights query" — as it is stored.
///
/// Every member a put names is replaced wholesale on an update, because
/// `PutQueryDefinition` states that "the values of name, queryString, and logGroupNames
/// are changed to the values that you specify in your update operation. No current values
/// are retained from the current query definition."
#[derive(Debug, Clone)]
pub struct QueryDefinitionRecord {
    /// The id CloudWatch generated for this definition, kept on the record as well as in
    /// the map key so a report built by enumerating the map carries it.
    pub query_definition_id: String,
    pub name: String,
    pub query_string: String,
    /// The language this definition's query string is written in. Never absent: a put
    /// that names no `queryLanguage` saves the definition with
    /// [`DEFAULT_QUERY_LANGUAGE`], so `DescribeQueryDefinitions$queryLanguage` —
    /// "limits the returned queries to only the queries that use the specified query
    /// language" — can place every stored definition.
    pub query_language: String,
    /// The log groups the definition "is limited to", when the put named any; `None`
    /// when it named none, which `QueryDefinition$logGroupNames` reports by omitting the
    /// member ("If this query definition contains a list of log groups that it is
    /// limited to, that list appears here").
    pub log_group_names: Option<Vec<String>>,
    /// The placeholder variables the definition's query string references, when the put
    /// named any; `None` otherwise, on the same rule as `logGroupNames`.
    pub parameters: Option<Vec<crate::ops::types::QueryParameter>>,
    pub last_modified_ms: i64,
    /// The `clientToken` of the CREATE that minted this definition, so a create retried
    /// "twice because of a network error" with the same token answers with this definition
    /// instead of saving a second one. An update never re-points it: the token identifies
    /// the create it de-duplicates, not the definition's current values.
    pub client_token: Option<String>,
}

/// One subscription filter — "a real-time stream of log events ingested through
/// PutLogEvents" delivered to one destination — as it is stored. It lives on the log
/// group it is "associated with", so deleting that group removes its filters with it.
#[derive(Debug, Clone)]
pub struct SubscriptionFilterRecord {
    pub filter_name: String,
    pub filter_pattern: String,
    pub destination_arn: String,
    pub role_arn: Option<String>,
    /// How log data is spread over the destination. Never absent: a put that names no
    /// `distribution` gets the documented default ([`DEFAULT_DISTRIBUTION`]), so a
    /// client reads back the grouping the filter is actually configured with.
    pub distribution: String,
    pub apply_on_transformed_logs: bool,
    pub field_selection_criteria: Option<String>,
    pub emit_system_fields: Option<Vec<String>>,
    pub creation_time_ms: i64,
}

/// One created log stream and its events.
#[derive(Debug, Clone)]
pub struct LogStreamRecord {
    pub name: String,
    pub arn: String,
    pub creation_time_ms: i64,
    pub first_event_timestamp: Option<i64>,
    pub last_event_timestamp: Option<i64>,
    pub last_ingestion_time: Option<i64>,
    /// `None` until the first `PutLogEvents`: AWS reports no `uploadSequenceToken`
    /// for a stream nothing has been written to.
    pub upload_sequence_token: Option<String>,
    /// Events kept sorted by `(timestamp, ingestion_seq)`.
    pub events: Vec<StoredEvent>,
}

impl LogStreamRecord {
    /// A brand-new, empty log stream.
    pub fn new(region: &str, group_name: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_stream_arn(region, group_name, name),
            creation_time_ms,
            first_event_timestamp: None,
            last_event_timestamp: None,
            last_ingestion_time: None,
            upload_sequence_token: None,
            events: Vec::new(),
        }
    }

    /// Re-derive the event-time summary members from the stored events.
    pub fn refresh_event_times(&mut self) {
        self.first_event_timestamp = self.events.first().map(|e| e.timestamp);
        self.last_event_timestamp = self.events.last().map(|e| e.timestamp);
    }

    /// The bytes the stream's events occupy, counted the way a log group's
    /// `storedBytes` counts them.
    pub fn stored_bytes(&self) -> i64 {
        self.events
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add)
    }

    /// Hold the stream inside the emulator's storage ceiling ([`MAX_STREAM_EVENTS`] /
    /// [`MAX_STREAM_BYTES`]) by discarding the oldest events, and answer how many bytes
    /// went, so the owning log group's `storedBytes` can be reduced by the same amount.
    ///
    /// Events are stored oldest-first, so the discarded run is a prefix. A single event
    /// can never exceed the byte ceiling (a log event is capped at 1 MB on the way in),
    /// so the newest event always survives.
    pub fn enforce_storage_ceiling(&mut self) -> i64 {
        let mut kept_bytes = self.stored_bytes();
        let mut discard = 0usize;
        while discard < self.events.len()
            && (self.events.len() - discard > MAX_STREAM_EVENTS || kept_bytes > MAX_STREAM_BYTES)
        {
            kept_bytes = kept_bytes.saturating_sub(self.events[discard].stored_bytes());
            discard += 1;
        }
        let discarded_bytes = self.events[..discard]
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add);
        self.events.drain(..discard);
        discarded_bytes
    }
}

/// One ingested log event.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub timestamp: i64,
    pub message: String,
    pub ingestion_time: i64,
    pub ingestion_seq: u64,
}

impl StoredEvent {
    /// The total order events are stored and paginated in.
    pub fn key(&self) -> EventKey {
        EventKey {
            timestamp: self.timestamp,
            seq: self.ingestion_seq,
        }
    }

    /// What this event adds to its log group's `storedBytes`: the UTF-8 message plus
    /// the documented 26-byte per-event overhead.
    pub fn stored_bytes(&self) -> i64 {
        i64::try_from(self.message.len()).unwrap_or(i64::MAX) + EVENT_OVERHEAD_BYTES
    }
}

/// A position in a stream's event order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventKey {
    pub timestamp: i64,
    pub seq: u64,
}

impl EventKey {
    /// The position before every event, so a forward scan from it starts at the head.
    pub const BEFORE_ALL: Self = Self {
        timestamp: i64::MIN,
        seq: 0,
    };
    /// The position after every event, so a backward scan from it starts at the tail.
    pub const AFTER_ALL: Self = Self {
        timestamp: i64::MAX,
        seq: u64::MAX,
    };
}

/// The region to stamp into the ARNs this request's resources get: the SigV4
/// credential scope's region, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Wall-clock milliseconds since the Unix epoch — what every CloudWatch Logs
/// timestamp member is expressed in.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// A log group's ARN, with the trailing `:*` when `wildcard_suffix` is set (the two
/// versions the model's `arn` and `logGroupArn` members document).
pub fn log_group_arn(region: &str, name: &str, wildcard_suffix: bool) -> String {
    let resource = if wildcard_suffix {
        format!("log-group:{name}:*")
    } else {
        format!("log-group:{name}")
    };
    ::engine::emulator_arn("logs", region, &resource)
}

/// A log stream's ARN.
pub fn log_stream_arn(region: &str, group_name: &str, stream_name: &str) -> String {
    ::engine::emulator_arn(
        "logs",
        region,
        &format!("log-group:{group_name}:log-stream:{stream_name}"),
    )
}

/// The service and resource segments of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything
/// that is not an ARN. The resource segment is whatever follows the account, colons and
/// slashes included, because a resource id carries its own separators.
pub fn arn_service_and_resource(arn: &str) -> Option<(&str, &str)> {
    arn_parts(arn).map(|(service, _account, resource)| (service, resource))
}

/// The `(service, account, resource)` segments of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything
/// that is not an ARN.
fn arn_parts(arn: &str) -> Option<(&str, &str, &str)> {
    let rest = arn.strip_prefix("arn:")?;
    // partition, service, region, account, then the whole resource segment.
    let mut segments = rest.splitn(5, ':');
    let _partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let account = segments.next()?;
    Some((service, account, segments.next().unwrap_or_default()))
}

/// The destination vendor CloudWatch Logs delivers to without assuming a role: a Lambda
/// function is invoked directly, which is why `PutSubscriptionFilter` documents the
/// `iam:PassRole` requirement for "any destination except a Lambda function".
pub const SELF_DELIVERED_DESTINATION_VENDOR: &str = "lambda";

/// The vendor of a `destinationArn` whose delivery needs the role `roleArn` names, or
/// `None` when the filter can be configured without one.
///
/// "To perform a PutSubscriptionFilter operation for any destination except a Lambda
/// function, you must also have the iam:PassRole permission": every vendor but Lambda is
/// delivered to through a role CloudWatch Logs assumes. The exception is a logical
/// destination owned by ANOTHER account, where the destination's own policy authorises
/// the delivery (`roleArn`: "You don't need to provide the ARN when you are working with
/// a logical destination for cross-account delivery").
///
/// A vendor this operation delivers to nothing in ([`SUBSCRIPTION_DESTINATIONS`]) needs
/// no role either: there is no delivery to authorise, and such a `destinationArn` is
/// refused for the resource it names instead.
pub fn subscription_role_required_vendor(arn: &str) -> Option<&str> {
    let (vendor, account, resource) = arn_parts(arn)?;
    if vendor == SELF_DELIVERED_DESTINATION_VENDOR
        || !SUBSCRIPTION_DESTINATIONS
            .iter()
            .any(|(name, _)| vendor == *name)
    {
        return None;
    }
    let cross_account_logical_destination = vendor == "logs"
        && resource.starts_with("destination:")
        && account != ::engine::emulator_account_id();
    if cross_account_logical_destination {
        return None;
    }
    Some(vendor)
}

/// Whether an ARN names a CloudWatch Logs log group — the only resource type a resource
/// policy can be attached to (`PutResourcePolicy$resourceArn`: "Currently only supports
/// LogGroup ARN").
///
/// A log group's ARN is `arn:<partition>:logs:<region>:<account>:log-group:<name>`, with
/// or without the trailing `:*` the `LogGroup$arn` member carries. A log STREAM's ARN
/// also names a log group, so it is excluded here: it names a stream, which cannot carry
/// a resource policy.
pub fn is_log_group_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        service == "logs"
            && resource.starts_with("log-group:")
            && !resource.contains(":log-stream:")
    })
}

/// The CloudWatch Logs resource one tagging operation's `resourceArn` names, resolved to
/// the key this service's store holds that resource under.
///
/// `TagResource` names the kinds that carry tags — "Currently, the only CloudWatch Logs
/// resources that can be tagged are log groups and destinations" — and `resourceArn`'s own
/// documentation gives both ARN forms: "The ARN format of a log group is
/// arn:aws:logs:Region:account-id:log-group:log-group-name The ARN format of a destination is
/// arn:aws:logs:Region:account-id:destination:destination-name". A delivery is here as well,
/// because the model gives that resource its own tag member (`Delivery$tags`: "The tags that
/// have been assigned to this delivery") and this service stores it
/// ([`DeliveryRecord::tags`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// One log group, by the name the store is keyed by.
    LogGroup(String),
    /// One logical destination, by the `Destination$destinationName` it is created under.
    Destination(String),
    /// One delivery, by the `Delivery$id` the store is keyed by.
    Delivery(String),
}

impl TaggedResource {
    /// The resource kind a refusal names, in the words this service uses for it — what
    /// [`resource_not_found_message`] states when the ARN names no stored resource.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::LogGroup(_) => "log group",
            Self::Destination(_) => "destination",
            Self::Delivery(_) => "delivery",
        }
    }
}

/// The ARN resource-segment prefix of a logical destination, from the ARN format
/// `TagResource$resourceArn` documents for one.
const DESTINATION_ARN_PREFIX: &str = "destination:";

/// The ARN resource-segment prefix of a delivery, as [`delivery_arn`] builds one.
const DELIVERY_ARN_PREFIX: &str = "delivery:";

/// The resource a tagging operation's `resourceArn` names ([`TaggedResource`]), or `None`
/// for an ARN that names no CloudWatch Logs resource carrying tags — another service's
/// resource, or a CloudWatch Logs resource kind that holds no tag set (a log stream, say).
/// Whether the named resource EXISTS is a separate question, answered by
/// [`ComAmazonawsCloudwatchlogsLogs20140328State::resource_tags`].
pub fn tagged_resource(arn: &str) -> Option<TaggedResource> {
    let (service, resource) = arn_service_and_resource(arn)?;
    if service != "logs" {
        return None;
    }
    // Either log-group ARN form, with or without the trailing `:*` the `LogGroup$arn`
    // member carries, names the same group.
    if is_log_group_arn(arn) {
        return Some(TaggedResource::LogGroup(
            log_group_name_from_identifier(arn).to_string(),
        ));
    }
    if let Some(name) = resource.strip_prefix(DESTINATION_ARN_PREFIX) {
        return (!name.is_empty()).then(|| TaggedResource::Destination(name.to_string()));
    }
    if let Some(id) = resource.strip_prefix(DELIVERY_ARN_PREFIX) {
        return (!id.is_empty()).then(|| TaggedResource::Delivery(id.to_string()));
    }
    None
}

/// The refusal for a `resourceArn` that is a well-formed ARN but names no resource this
/// service can tag, stated as `TagResource` states the rule: "Currently, the only CloudWatch
/// Logs resources that can be tagged are log groups and destinations." A value that is not
/// an ARN at all is a different refusal ([`unparsable_arn_message`]), and a taggable kind
/// the account holds no instance of is a third (`ResourceNotFoundException`).
pub const UNTAGGABLE_RESOURCE_ARN: &str =
    "resourceArn must name a resource that supports tagging. Currently, the only CloudWatch \
     Logs resources that can be tagged are log groups and destinations.";

/// Whether an ARN names the account's stored query-insights results — the non-log-group
/// target `AssociateKmsKey` / `DisassociateKmsKey` document: "Specify the following ARN
/// to have future GetQueryResults operations in this account encrypt the results with
/// the specified KMS key [...] arn:aws:logs:REGION:ACCOUNT_ID:query-result:*".
pub fn is_query_result_arn(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .is_some_and(|(service, resource)| service == "logs" && resource == "query-result:*")
}

/// Whether a `kmsKeyId` names a KMS key: the member is documented as "The Amazon
/// Resource Name (ARN) of the KMS key to use when encrypting log data", so a value that
/// is an ARN must be one of KMS's. A bare key id or alias carries no service segment and
/// is left to KMS to resolve, but an empty value names nothing at all.
pub fn is_kms_key_id(kms_key_id: &str) -> bool {
    if kms_key_id.trim().is_empty() {
        return false;
    }
    match arn_service_and_resource(kms_key_id) {
        Some((service, resource)) => service == "kms" && !resource.is_empty(),
        None => !kms_key_id.starts_with("arn:"),
    }
}

/// Whether an ARN names a destination `PutSubscriptionFilter` supports delivering to:
/// one of the services in [`SUBSCRIPTION_DESTINATIONS`], naming a resource of the kind
/// that service's entry there gives.
///
/// A CloudWatch Logs ARN qualifies only when it names a logical `destination` (the
/// resource `PutDestination` creates), never a log group or a log stream.
pub fn is_subscription_destination_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        SUBSCRIPTION_DESTINATIONS
            .iter()
            .any(|(name, prefix)| service == *name && resource.starts_with(prefix))
    })
}

/// AWS's request-derived refusal message for a member the service parses as an ARN and
/// was handed something that is not one: the validation-error template naming the member
/// and the value it could not parse. `None` when the value parses as an ARN, whatever
/// resource it names — a well-formed ARN of the wrong kind is a different refusal, with
/// its own message.
pub fn unparsable_arn_message(member: &str, value: &str) -> Option<String> {
    arn_service_and_resource(value)
        .is_none()
        .then(|| format!("1 validation error detected: Unable to parse {member} {value}"))
}

/// Hold a `filterPattern` to the filter-pattern syntax's bracket rule: a pattern that
/// opens one of the bracketed forms in [`FILTER_PATTERN_DELIMITERS`] must close it with
/// the matching delimiter, and the brackets it nests inside that form must balance. The
/// `Err` carries the message to refuse with, as the service states the rule.
///
/// A pattern that opens no bracketed form — a term, a text or a regular-expression
/// pattern — carries no bracket structure to check, and the empty pattern (the modelled
/// `FilterPattern` is `length 0..1024`) matches every event.
pub fn validate_filter_pattern(pattern: &str) -> Result<(), String> {
    let pattern = pattern.trim();
    let Some(first) = pattern.chars().next() else {
        return Ok(());
    };
    let Some((open, close)) = FILTER_PATTERN_DELIMITERS
        .iter()
        .copied()
        .find(|(open, _)| *open == first)
    else {
        return Ok(());
    };
    if !pattern.ends_with(close) {
        return Err(format!(
            "If a filter pattern starts with '{open}' it must end with '{close}'"
        ));
    }
    if let Err(defect) = filter_pattern_structure(pattern, open) {
        return Err(match defect {
            PatternDefect::UnbalancedBrackets => format!(
                "Invalid filter pattern: the brackets in a '{open}' … '{close}' filter pattern must be balanced"
            ),
            PatternDefect::UnterminatedQuotedString => {
                "Invalid filter pattern: a quoted string is not terminated".to_string()
            }
            PatternDefect::UnterminatedRegex => {
                "Invalid filter pattern: a regular expression is not terminated".to_string()
            }
        });
    }
    Ok(())
}

/// What [`filter_pattern_structure`] found wrong with a pattern, so the refusal names the
/// defect it detected rather than one of them for all three.
enum PatternDefect {
    UnbalancedBrackets,
    UnterminatedQuotedString,
    UnterminatedRegex,
}

/// Whether a filter pattern's structure closes: its brackets open and close in order —
/// the bracketed forms in [`FILTER_PATTERN_DELIMITERS`], plus the `()` that group a JSON
/// pattern's terms, only in a JSON pattern, because a space-delimited pattern's terms can
/// carry a bare parenthesis — and the literals it opens are terminated.
///
/// A quoted string (`"…"`) and a regular-expression literal (`%…%`) are opaque: the
/// pattern syntax lets them carry any character, so a bracket inside one is data rather
/// than structure. A backslash escapes the character after it.
fn filter_pattern_structure(pattern: &str, opened: char) -> Result<(), PatternDefect> {
    let groups_terms_in_parentheses = opened == '{';
    let mut open_brackets: Vec<char> = Vec::new();
    let mut quoted = false;
    let mut regex = false;
    let mut chars = pattern.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                chars.next();
            }
            '"' if !regex => quoted = !quoted,
            '%' if !quoted => regex = !regex,
            _ if quoted || regex => {}
            '(' | ')' if !groups_terms_in_parentheses => {}
            '{' | '[' | '(' => open_brackets.push(character),
            '}' | ']' | ')' => {
                let opened = match character {
                    '}' => '{',
                    ']' => '[',
                    _ => '(',
                };
                if open_brackets.pop() != Some(opened) {
                    return Err(PatternDefect::UnbalancedBrackets);
                }
            }
            _ => {}
        }
    }
    if quoted {
        return Err(PatternDefect::UnterminatedQuotedString);
    }
    if regex {
        return Err(PatternDefect::UnterminatedRegex);
    }
    if open_brackets.is_empty() {
        Ok(())
    } else {
        Err(PatternDefect::UnbalancedBrackets)
    }
}

/// The log group name a `logGroupIdentifier` names: the identifier itself when it is a
/// name, or the name segment of a log-group ARN (either ARN version).
pub fn log_group_name_from_identifier(identifier: &str) -> &str {
    const MARKER: &str = ":log-group:";
    match identifier.find(MARKER) {
        Some(at) => {
            let rest = &identifier[at + MARKER.len()..];
            let rest = rest.strip_suffix(":*").unwrap_or(rest);
            // A log-stream ARN carries the group name before `:log-stream:`.
            match rest.find(":log-stream:") {
                Some(end) => &rest[..end],
                None => rest,
            }
        }
        None => identifier,
    }
}

/// The refusal for a request that names BOTH log-group selectors, and the one that names
/// NEITHER ([`resolve_log_group_target`]).
///
/// This pair's refusal is the one CloudWatch Logs answers with `ValidationException`
/// (HTTP 400) rather than with the operation's own `InvalidParameterException`: it is
/// checked across two members, which no single member's constraint can express, and the
/// recorded refusal for a request naming both selectors is `400 ValidationException:
/// LogGroup name and LogGroup ARN are mutually exclusive parameters.` — which is why each
/// caller widens that shape in through `extra_errors`. The sentences live here, beside the
/// check that decides them, and name the two selectors the way the service names them: the
/// log group's NAME and its ARN.
pub const LOG_GROUP_SELECTORS_EXCLUSIVE: &str =
    "LogGroup name and LogGroup ARN are mutually exclusive parameters.";

/// The refusal for a request that names neither selector; see
/// [`LOG_GROUP_SELECTORS_EXCLUSIVE`]. Both sentences are the service's own fixed wording
/// for a request-independent refusal, and both name the selectors as the service names
/// them rather than as the members spell them.
pub const LOG_GROUP_SELECTOR_REQUIRED: &str = "Either LogGroupName or LogGroupArn must be provided.";

/// Resolve the log group an operation's `logGroupName` / `logGroupIdentifier` pair
/// names. The model documents them as mutually exclusive with one of them required, so
/// neither-or-both is a client error; the `Err` carries the message to refuse with, and
/// [`LOG_GROUP_SELECTORS_EXCLUSIVE`] records which error shape a caller refuses with.
pub fn resolve_log_group_target(
    log_group_name: Option<&str>,
    log_group_identifier: Option<&str>,
) -> Result<String, String> {
    match (log_group_name, log_group_identifier) {
        (Some(name), None) => Ok(name.to_string()),
        (None, Some(identifier)) => Ok(log_group_name_from_identifier(identifier).to_string()),
        (Some(_), Some(_)) => Err(LOG_GROUP_SELECTORS_EXCLUSIVE.to_string()),
        (None, None) => Err(LOG_GROUP_SELECTOR_REQUIRED.to_string()),
    }
}

/// What an `AssociateKmsKey` / `DisassociateKmsKey` request names: "you specify either
/// the logGroupName parameter or the resourceIdentifier parameter", and a
/// `resourceIdentifier` is either a log group's ARN or the account's query-result ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KmsKeyTarget {
    /// One log group, by the name the store is keyed by.
    LogGroup(String),
    /// The account's stored query-insights results.
    QueryResults,
}

/// Resolve the target of an `AssociateKmsKey` / `DisassociateKmsKey` request. The two
/// parameters are documented as mutually exclusive with one of them required, and a
/// `resourceIdentifier` naming neither a log group nor the query-result resource names
/// no target at all; the `Err` carries the message to refuse with.
pub fn resolve_kms_key_target(
    log_group_name: Option<&str>,
    resource_identifier: Option<&str>,
) -> Result<KmsKeyTarget, String> {
    match (log_group_name, resource_identifier) {
        (Some(name), None) => Ok(KmsKeyTarget::LogGroup(name.to_string())),
        (None, Some(identifier)) => {
            if is_query_result_arn(identifier) {
                Ok(KmsKeyTarget::QueryResults)
            } else if is_log_group_arn(identifier) {
                Ok(KmsKeyTarget::LogGroup(
                    log_group_name_from_identifier(identifier).to_string(),
                ))
            } else {
                Err(format!(
                    "resourceIdentifier must be a log group ARN or a query-result ARN, got '{identifier}'"
                ))
            }
        }
        (Some(_), Some(_)) => Err(
            "logGroupName and resourceIdentifier are mutually exclusive; specify only one"
                .to_string(),
        ),
        (None, None) => {
            Err("either logGroupName or resourceIdentifier must be specified".to_string())
        }
    }
}

/// The revision id a resource-scoped policy carries, minted from the monotonic policy
/// revision counter so it is unique and derived from stored state. AWS documents no
/// format for it — only that a put must present the revision the policy is currently
/// at — so this renders the counter as a fixed-width hex string.
pub fn mint_revision_id(seq: u64) -> String {
    format!("{seq:032x}")
}

/// The sequence token a `PutLogEvents` call hands back, minted from the monotonic
/// ingestion counter so it is unique and derived from stored state. Rendered the way
/// CloudWatch Logs renders one: a fixed-width decimal string.
pub fn mint_sequence_token(seq: u64) -> String {
    format!("{seq:056}")
}

/// A canonical hyphenated UUID minted from one of the store's monotonic counters, which
/// is how every resource of this service whose id AWS renders as a UUID gets one: unique
/// and DERIVED FROM STORED STATE rather than from a random source this crate would need a
/// new dependency for — the way [`mint_revision_id`] and [`mint_sequence_token`] mint from
/// stored counters. The version (`4`) and variant (`8`) nibbles sit where a UUID carries
/// them, and the counter fills the node field, offset by one so the first id minted is not
/// the nil UUID.
pub fn mint_uuid_from_seq(seq: u64) -> String {
    format!("00000000-0000-4000-8000-{:012x}", seq.saturating_add(1))
}

/// The id `PutQueryDefinition` hands back for a definition it creates: "CloudWatch
/// generates a unique ID for the new query definition and include it in the response to
/// this operation."
///
/// CloudWatch Logs renders one as a canonical hyphenated UUID, so it is minted through the
/// shared [`mint_uuid_from_seq`] from the store's monotonic query-definition counter.
pub fn mint_query_definition_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The refusal CloudWatch Logs answers when the resource an operation names is not
/// there. The service states the resource KIND that is missing, in the sentence form the
/// model documents for `ResourceNotFoundException` — "The specified resource does not
/// exist." — terminated like that sentence, so every operation of this service refuses a
/// missing resource the same way.
pub fn resource_not_found_message(resource_kind: &str) -> String {
    format!("The specified {resource_kind} does not exist.")
}

/// The refusal CloudWatch Logs builds from the request when the resource policy the
/// caller named is not there. The message interpolates the requested `policyName`, so a
/// caller deleting policies in a loop can attribute the refusal to one policy name
/// instead of guessing which of its requests failed.
pub fn policy_not_found_message(policy_name: &str) -> String {
    format!("Policy with name [{policy_name}] does not exist.")
}

/// The refusal CloudWatch Logs builds from the request when the log group names no syslog
/// configuration for the endpoint a `DeleteSyslogConfiguration` names. The message
/// interpolates the requested `vpcEndpointId`, so a caller deleting configurations in a
/// loop can attribute the refusal to one endpoint instead of guessing which of its
/// requests failed.
pub fn syslog_configuration_not_found_message(vpc_endpoint_id: &str) -> String {
    format!("No syslog configuration for vpcEndpointId [{vpc_endpoint_id}] exists on this log group.")
}

/// The refusal for a value outside the set a member's own documentation enumerates: the
/// value is declared invalid and the permitted set is then listed, so the refusal itself
/// tells a caller which values it may use.
///
/// `subject` names what was invalid (the "retention" of `retentionInDays`, say) and
/// `valid` is the catalogue the model states — [`RETENTION_IN_DAYS_VALUES`] or
/// [`SUBSCRIPTION_SYSTEM_FIELDS`] — rendered in the documented order, never a list typed
/// into a message.
pub fn invalid_value_message<T: std::fmt::Display>(
    subject: &str,
    valid: impl IntoIterator<Item = T>,
) -> String {
    let listed = valid
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("Invalid {subject} value. Valid values are: [{listed}]")
}

/// Encode a pagination cursor as an opaque token.
///
/// The payload is this service's own structure; hex-encoding it keeps the token
/// opaque to a client, which must only round-trip it back to the next call.
pub fn encode_token(payload: &serde_json::Value) -> String {
    hex::encode(serde_json::to_vec(payload).unwrap_or_default())
}

/// Decode a token [`encode_token`] produced. `None` for anything else, which a
/// handler refuses as an invalid `nextToken`.
pub fn decode_token(token: &str) -> Option<serde_json::Value> {
    let bytes = hex::decode(token).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatchlogsLogs20140328State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatchlogsLogs20140328State>)
}
