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
//! A log group's metric filters live on the group too
//! ([`LogGroupRecord::metric_filters`]), because `PutMetricFilter` "creates or updates a
//! metric filter and associates it with the specified log group" — which is also what
//! makes the group's `metricFilterCount` ([`LogGroupRecord::metric_filter_count`]) a
//! derived value rather than a second copy of the same truth.
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
//! A log group's own field index policy lives on the group too
//! ([`LogGroupRecord::index_policy`]), because a log-group level index policy is named by
//! nothing but the group it applies to — so one group holds at most one and deleting the
//! group takes it with it. The account-wide field index policies that apply where a group
//! holds none of its own are the account policies in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::account_policies`], and which log groups
//! one of those selects is [`account_policy_selects_log_group`].
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
//! a [`DeliveryRecord`], keyed by the Region it was created in and its own id
//! ([`RegionalKey`]).
//!
//! The account's storage tier policy lives here too
//! ([`ComAmazonawsCloudwatchlogsLogs20140328State::storage_tier_policy`]): it is "the
//! storage tier policy for the account", so there is one of it and no log group owns it.
//!
//! The account's delivery destinations — each "a logical object that represents the actual
//! delivery destination", which `PutDeliveryDestination` creates or updates and
//! `GetDeliveryDestination` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::delivery_destinations`], each one a
//! [`DeliveryDestinationRecord`], keyed by the Region it was created in and the name that
//! identifies one within it ([`RegionalKey`]). A destination's
//! delivery destination policy lives on that destination
//! ([`DeliveryDestinationRecord::policy`]), because `PutDeliveryDestinationPolicy` "assigns
//! an IAM policy to the destination" and nothing but the destination names one: one
//! destination holds at most one, and removing the destination takes its policy with it.
//!
//! The account's delivery sources — each "a logical object that represents the resource that
//! is actually sending the logs", which `PutDeliverySource` creates or updates and
//! `GetDeliverySource` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::delivery_sources`], each one a
//! [`DeliverySourceRecord`], keyed by the Region it was created in and the name that
//! identifies one within it ([`RegionalKey`]).
//!
//! The account's logical destinations — each "a cross-account destination that receives
//! subscription log events", which `PutDestination` creates or updates and
//! `DescribeDestinations` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::destinations`], each one a
//! [`DestinationRecord`], keyed by the `destinationName` that identifies one. A destination
//! names a physical resource of another service, so it is an account-level resource and no
//! log group owns it. Its access policy lives on that destination
//! ([`DestinationRecord::access_policy`]), because `PutDestinationPolicy` creates or updates
//! "an access policy associated with an existing destination" and nothing but the destination
//! names one: one destination holds at most one, and removing the destination takes its policy
//! with it.
//!
//! The account's log anomaly detectors — "one anomaly detector in the account", which
//! `CreateLogAnomalyDetector` creates and `GetLogAnomalyDetector` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::anomaly_detectors`], each one an
//! [`AnomalyDetectorRecord`] carrying the instant the clock owes it its next status. A
//! detector is an account-level resource rather than a log group's: it watches log groups
//! named by their ARNs and outlives none of them, so it lives beside the groups and not on
//! [`LogGroupRecord`].
//!
//! The account's integrations — each "an integration between CloudWatch Logs and another
//! service in this account", which `PutIntegration` creates and `GetIntegration` /
//! `ListIntegrations` report — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::integrations`], each one an
//! [`IntegrationRecord`] carrying the instant the clock owes it its ACTIVE status.
//!
//! The data-source associations of the account's S3 Table Integrations — each the pairing
//! `AssociateSourceToS3TableIntegration` creates and `ListSourcesForS3TableIntegration`
//! reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::s3_table_integration_sources`], each one
//! an [`S3TableIntegrationSourceRecord`] carrying the instant the clock owes its deletion.
//! An association names its integration by the ARN the associate was given rather than by a
//! record of this store: no operation of this service creates an S3 Table Integration.
//!
//! The account's lookup tables — the reference data `CreateLookupTable` uploads and
//! `GetLookupTable` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::lookup_tables`], each one a
//! [`LookupTableRecord`], keyed by the name that identifies one ("The name must be unique
//! within your account and Region"). A lookup table enriches the queries of any log group,
//! so it is an account-level resource rather than one group's and lives beside the groups
//! and not on [`LogGroupRecord`].
//!
//! The account's scheduled queries — the queries `CreateScheduledQuery` schedules "at regular
//! intervals" and `GetScheduledQuery` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::scheduled_queries`], each one a
//! [`ScheduledQueryRecord`], keyed by the name that identifies one ("The name must be unique
//! within your account and region"). A scheduled query names the log groups it queries as a
//! value it carries, and outlives none of them, so it is an account-level resource rather
//! than one group's and lives beside the groups and not on [`LogGroupRecord`]. Either the name
//! or the ARN identifies one to a reader
//! ([`ComAmazonawsCloudwatchlogsLogs20140328State::scheduled_query`]). The scheduled query
//! CloudWatch MANAGES for a log alarm is not here: it belongs to that alarm, so it is derived
//! from it ([`log_alarm_queries`]) rather than copied into this store.
//!
//! The vended-log delivery CONFIGURATION TEMPLATES — "the valid and default values that are
//! used when creating delivery sources, delivery destinations, and deliveries", which
//! `DescribeConfigurationTemplates` reports — are not an account's resources at all: no
//! operation creates, updates or deletes one, so they live in the catalogue
//! [`CONFIGURATION_TEMPLATE_SOURCES`] and are rendered by [`configuration_templates`]
//! rather than in a map of
//! [`ComAmazonawsCloudwatchlogsLogs20140328State`]. That is also what makes two calls
//! report the same templates.

use std::collections::BTreeMap;

/// Compiling a CloudWatch Logs Insights QL query string, which is what decides whether
/// `StartQuery` accepts one.
pub mod query_compile;

/// Running a `filterPattern` over a log event message, which is what `TestMetricFilter`
/// reports.
pub mod filter_pattern;

/// Metric extraction from an ingested Embedded Metric Format log event.
pub mod emf;

/// Metric extraction from an ingested log event through a log group's metric filters.
pub mod metric_filters;

/// Delivery of an ingested log event to the destinations of its log group's subscription
/// filters.
pub mod subscription_delivery;

/// Resolving the S3 bucket an export task's `destination` names against the object store.
pub mod export_destination;

/// The AWS-vended log record formats a log group's transformer parses.
pub mod log_transformer;

/// The transformation a log group's transformer applies to an ingested log event.
pub mod transformer;

/// The scheduled query CloudWatch manages on a log alarm's behalf.
pub mod log_alarm_queries;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Generated emulators default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The log group class `CreateLogGroup` applies when the request omits
/// `logGroupClass` ("If you omit this parameter, the default of STANDARD is used").
pub const DEFAULT_LOG_GROUP_CLASS: &str = "STANDARD";

/// The storage tier `GetStorageTierPolicy` reports for an account no `PutStorageTierPolicy`
/// has ever set: the tier log data is stored in until the account opts into
/// `INTELLIGENT_TIERING`, which is what `PutStorageTierPolicy` documents as the opt-in
/// ("When you set the storage tier to INTELLIGENT_TIERING, the service automatically moves
/// log data to the most cost-effective storage tier based on access frequency"). Spelled
/// as the model's `StorageTier` enum spells it (`STORAGE_TIER_VALUES`).
pub const DEFAULT_STORAGE_TIER: &str = "STANDARD";

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

/// The largest single log event CloudWatch Logs holds, from `PutLogEvents`: "Each log event
/// can be no larger than 1 MB."
///
/// A bound on a log event MESSAGE wherever one arrives, not only on an ingested batch: a
/// `TestMetricFilter` sample message larger than the largest log event the service holds is
/// no log event message at all. It lives here so the two operations are held to the one
/// number.
pub const MAX_LOG_EVENT_BYTES: i64 = 1_048_576;

/// The refusal for a log event message larger than one CloudWatch Logs holds: the
/// service's own documented sentence for the condition, which states the ceiling and
/// interpolates nothing from the request. Kept beside
/// [`MAX_LOG_EVENT_BYTES`] so the sentence and the number it states cannot disagree.
pub const LOG_EVENT_TOO_LARGE: &str = "Each log event can be no larger than 1 MB.";

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

/// How long after a log group is created the AGGREGATE summaries of
/// `ListAggregateLogGroupSummaries` count it.
///
/// That operation "returns an aggregate summary of all log groups in the Region grouped by
/// specified data source characteristics" — a view maintained beside the log groups
/// themselves rather than read off them, so a group enters it a short span after the create
/// that made it, and a caller that aggregates straight after creating one is answered
/// without it while every later call counts it. The span is this emulator's own choice (the
/// service publishes none) and is the one every clock-owed transition here takes, so a
/// caller sees the view catch up rather than never settle. Only the aggregate view lags:
/// `DescribeLogGroups` and `ListLogGroups` report a group as soon as it is created.
pub const AGGREGATE_SUMMARY_VISIBILITY_DELAY_MS: i64 = 3_000;

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

/// The tag-key namespace Amazon Web Services keeps for its own tags, from AWS's tag
/// restrictions: "Do not use the aws: prefix in your tag names or values because it is
/// reserved for Amazon Web Services use. You can't edit or delete tag names or values with
/// this prefix."
///
/// So a key in it is never a caller's to assign: a tagging request that names one is refused
/// and assigns nothing. `TagKey`'s own `@pattern` admits `:`, so nothing refuses such a key
/// before a handler runs.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether a tagging request names any key in [`RESERVED_TAG_KEY_PREFIX`] — the keys that are
/// not the caller's to assign.
///
/// What is reserved is the prefix itself rather than one spelling of it, so a key is matched
/// against it without regard to case. The comparison is on BYTES, so a key whose fifth byte
/// falls inside a multi-byte character is answered rather than panicked on.
pub fn names_reserved_tag_key<'a>(keys: impl IntoIterator<Item = &'a str>) -> bool {
    keys.into_iter().any(|key| {
        key.as_bytes()
            .get(..RESERVED_TAG_KEY_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
    })
}

/// The refusal CloudWatch Logs answers a tagging request that names a key in the reserved
/// namespace. ONE sentence for every such request: what is refused is the namespace itself,
/// so the refusal carries nothing the request chose, and it states the prefix in the casing
/// the service names it by even though the reservation ignores case.
pub const RESERVED_TAG_KEY_MESSAGE: &str = "Tags starting with 'AWS:' are reserved for AWS use.";

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

/// The `IndexPolicy$source` of a policy that applies to a subset of the account's log
/// groups rather than to the one group it is reported for, from the model's `IndexSource`
/// enum (`ACCOUNT` | `LOG_GROUP`, the pair listed as
/// [`crate::ops::types::INDEX_SOURCE_VALUES`]). The member "indicates whether this is an
/// account-level index policy or an index policy that applies only to a single log group".
pub const INDEX_SOURCE_ACCOUNT: &str = "ACCOUNT";

/// The `IndexPolicy$source` of a policy stored on the one log group it applies to. See
/// [`INDEX_SOURCE_ACCOUNT`].
pub const INDEX_SOURCE_LOG_GROUP: &str = "LOG_GROUP";

/// The one log group class a log-group-level field index policy can be put on, from
/// `PutIndexPolicy`: "Only log groups in the Standard log class support field index
/// policies." Spelled as the model's `LogGroupClass` spells it
/// (`LOG_GROUP_CLASS_VALUES`), and named apart from [`TRANSFORMER_LOG_GROUP_CLASS`]
/// because it is a different feature's sentence even where the class is spelled the same.
pub const INDEX_POLICY_LOG_GROUP_CLASS: &str = "STANDARD";

/// How many field indexes one log-group-level field index policy can name, from
/// `PutIndexPolicy`'s quota list: "As many as 20 fields can be included in the policy."
/// Only the fields the document names count — "Default field indexes are not counted
/// towards your field index quota."
pub const MAX_INDEX_POLICY_FIELDS: usize = 20;

/// How long one indexed field's name can be, from the same quota list: "Each field name
/// can include as many as 100 characters."
pub const MAX_INDEX_FIELD_NAME_CHARS: usize = 100;

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

/// The subscription filters one log group can have ATTACHED at any one time. The
/// operation's own documentation still says "Each log group can have up to two
/// subscription filters associated with it", but the service accepts five attached
/// filters on one log group and refuses the one after that, so the quota is five.
///
/// It counts the filters a group HOLDS, not the puts made against it: a put naming a
/// filter the group already holds is an update of that filter, and deleting one frees its
/// slot for the next put.
pub const MAX_SUBSCRIPTION_FILTERS_PER_GROUP: usize = 5;

/// The refusal CloudWatch Logs answers when a request would take the account, or one log
/// group, past the resources it may hold at one time: the subscription filters one group can
/// have attached ([`MAX_SUBSCRIPTION_FILTERS_PER_GROUP`]), or the single export task an
/// account can have active ([`ExportTaskRecord::is_active`]). The text is FIXED for the error
/// class — it names neither the limit that was reached, nor anything from the request, nor the
/// resource holding the slot — so every refusal of that class reads identically.
pub const LIMIT_EXCEEDED_MESSAGE: &str = "Resource limit exceeded.";

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

/// AWS's documented metric-filter quota, from `PutMetricFilter`: "The maximum number of
/// metric filters that can be associated with a log group is 100." A put naming a filter
/// the group already holds is an update of that filter, so it does not count against this.
pub const MAX_METRIC_FILTERS_PER_GROUP: usize = 100;

/// The dimensions one metric filter can declare, from `MetricTransformation$dimensions`:
/// "One metric filter can include as many as three dimensions." The system fields a filter
/// emits as dimensions share the budget — `PutMetricFilter$emitSystemFieldDimensions`:
/// "These dimensions [...] count toward the total dimension limit for metric filters" — so
/// the two are counted together against this.
pub const MAX_METRIC_FILTER_DIMENSIONS: usize = 3;

/// The system fields a metric filter can emit as additional dimensions, from
/// `PutMetricFilter$emitSystemFieldDimensions`: "Valid values are @aws.account and
/// @aws.region." A SHORTER catalogue than a subscription filter's
/// ([`SUBSCRIPTION_SYSTEM_FIELDS`]), which also admits `@source.log`: the two members
/// document two different sets, so they are two constants.
pub const METRIC_FILTER_SYSTEM_FIELDS: [&str; 2] = ["@aws.account", "@aws.region"];

/// The regular expression patterns one `filterPattern` may carry, from
/// `PutMetricFilter`: "there is a quota of two regular expression patterns within a single
/// filter pattern".
pub const MAX_REGEX_PATTERNS_PER_FILTER_PATTERN: usize = 2;

/// The refusal CloudWatch Logs answers a `filterPattern` carrying more regular expression
/// patterns than one may hold. The quota is stated once
/// ([`MAX_REGEX_PATTERNS_PER_FILTER_PATTERN`]) and so is the sentence, so no two operations
/// that take a filter pattern can word the same refusal differently.
pub fn filter_pattern_regex_quota_message() -> String {
    format!(
        "a single filter pattern can contain at most \
         {MAX_REGEX_PATTERNS_PER_FILTER_PATTERN} regular expression patterns"
    )
}

/// The regular expression patterns one log group's metric filters may carry between them,
/// from `PutMetricFilter`: "There is also a quota of five regular expression patterns per
/// log group."
pub const MAX_REGEX_PATTERNS_PER_LOG_GROUP: usize = 5;

/// The prefix a metric filter's dimension value carries, because the value names a FIELD
/// of the log event rather than a literal: the filter-pattern syntax selects a JSON field
/// with `$.path` and a space-delimited field with `$name`. See
/// [`is_log_event_field_reference`].
pub const FIELD_REFERENCE_PREFIX: char = '$';

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

/// An export task that has been accepted but has not started moving data yet, from the
/// model's `ExportTaskStatusCode` enum (`CANCELLED` | `COMPLETED` | `FAILED` | `PENDING` |
/// `PENDING_CANCEL` | `RUNNING`). `CreateExportTask` "is an asynchronous call. If all the
/// required information is provided, this operation initiates an export task and responds
/// with the ID of the task", so an accepted task starts here.
pub const EXPORT_TASK_STATUS_PENDING: &str = "PENDING";

/// An export task that is moving data, from the model's `ExportTaskStatusCode` enum. One of
/// the two statuses `CreateExportTask` calls active ("Each account can only have one active
/// (RUNNING or PENDING) export task at a time").
pub const EXPORT_TASK_STATUS_RUNNING: &str = "RUNNING";

/// An export task that finished exporting, from the model's `ExportTaskStatusCode` enum.
pub const EXPORT_TASK_STATUS_COMPLETED: &str = "COMPLETED";

/// An export task a caller cancelled, from the model's `ExportTaskStatusCode` enum.
pub const EXPORT_TASK_STATUS_CANCELLED: &str = "CANCELLED";

/// An export task whose export stopped on an error, from the model's `ExportTaskStatusCode`
/// enum. Terminal like [`EXPORT_TASK_STATUS_COMPLETED`]: the task has finished, so nothing
/// moves it on and a cancel of it has no execution left to stop.
pub const EXPORT_TASK_STATUS_FAILED: &str = "FAILED";

/// The status message a cancelled export task reports beside that status code
/// ([`ExportTaskRecord::status_message`]): `ExportTaskStatus$message` is "the status message
/// related to the status code", and what there is to say about a task in this status is that
/// it stopped because a caller cancelled it rather than because the export itself ended. The
/// text is fixed for the status: it interpolates neither the task nor the instant.
pub const EXPORT_TASK_CANCELLED_STATUS_MESSAGE: &str = "Export task cancelled by user";

/// The status message an export task that finished exporting reports beside that status code,
/// on the same terms as [`EXPORT_TASK_CANCELLED_STATUS_MESSAGE`]: what there is to say about a
/// task in this status is that the export ran to its end without an error. Fixed text for the
/// status, interpolating nothing of the task.
pub const EXPORT_TASK_COMPLETED_STATUS_MESSAGE: &str = "Completed successfully";

/// How long after it is created an export task moves `PENDING` -> `RUNNING`, and how long
/// after that it reaches `COMPLETED`.
///
/// AWS documents no duration for an export, only that the call "is an asynchronous call",
/// so these spans are THE EMULATOR'S OWN CHOICE. Each is a WHOLE span of the length this
/// emulator gives a transition the clock owes, not a fraction of a client round-trip, which
/// is what makes the statuses the model calls active observable: a caller that creates an
/// export and then reads, cancels or polls it over one or more further API calls still finds
/// it `PENDING` and then `RUNNING`, and so still meets the state rules those statuses carry —
/// chief among them that "each account can only have one active (RUNNING or PENDING) export
/// task at a time", a limit a task settling inside one round-trip could never impose. The
/// instants both fall due at are stored on the record and never slept on.
pub const EXPORT_TASK_RUNNING_DELAY_MS: i64 = 3_000;

/// See [`EXPORT_TASK_RUNNING_DELAY_MS`]: the further span a `RUNNING` export task takes to
/// reach `COMPLETED`. An export finishes moving data only after it has started moving it, so
/// this span follows that one instead of overlapping it, and a task is therefore active for
/// both of them together.
pub const EXPORT_TASK_COMPLETION_DELAY_MS: i64 = 3_000;

/// The `destinationPrefix` an export task gets when the create names none: "If you don't
/// specify a value, the default is exportedlogs" (`CreateExportTask$destinationPrefix`).
pub const DEFAULT_EXPORT_DESTINATION_PREFIX: &str = "exportedlogs";

/// THE EMULATOR'S OWN CEILING on the export tasks one account's store holds, not a quota
/// cited from AWS: a task stays in memory for as long as the process runs and nothing
/// expires one, so the oldest completed tasks are dropped once the store holds this many
/// rather than growing without bound on request.
pub const MAX_EXPORT_TASKS: usize = 1_000;

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

/// The status a log anomaly detector a call has PAUSED sits in, from the model's
/// `AnomalyDetectorStatus` enum: `UpdateLogAnomalyDetector$enabled` is used "to pause or
/// restart the anomaly detector", so a detector paused that way is not initializing,
/// training or analyzing, and only another call restarts it — the clock owes it nothing.
pub const ANOMALY_DETECTOR_STATUS_PAUSED: &str = "PAUSED";

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

/// `INFINITE`, one of the two values the model's `SuppressionType` enum carries: a
/// suppression that does not run out — "If you specify INFINITE, any value for
/// suppressionPeriod is ignored." (`UpdateAnomaly$suppressionType`)
pub const SUPPRESSION_TYPE_INFINITE: &str = "INFINITE";

/// `Active`, one value of the model's `State` enum: the state an anomaly nothing has
/// suppressed or baselined reports — "Indicates the current state of this anomaly. If it is
/// still being treated as an anomaly, the value is `Active`." (`Anomaly$state`)
pub const ANOMALY_STATE_ACTIVE: &str = "Active";

/// `Suppressed`, one value of the model's `State` enum: the state an anomaly a suppression
/// is in force over reports — "If you have suppressed this anomaly by using the
/// UpdateAnomaly operation, the value is `Suppressed`." (`Anomaly$state`)
pub const ANOMALY_STATE_SUPPRESSED: &str = "Suppressed";

/// `Baseline`, one value of the model's `State` enum: the state an anomaly whose behavior
/// has been accepted as normal reports — "If this behavior is now considered to be normal,
/// the value is `Baseline`." (`Anomaly$state`), which is what `UpdateAnomaly$baseline` says
/// of the suppression it places: "Set this to true to prevent CloudWatch Logs from
/// displaying this behavior as an anomaly in the future. The behavior is then treated as
/// baseline behavior."
pub const ANOMALY_STATE_BASELINE: &str = "Baseline";

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

/// THE EMULATOR'S OWN CEILING on the lookup tables one account holds, not a quota cited
/// from AWS: each table keeps its whole CSV body (modelled `@length` up to 10 MB) in memory
/// for as long as the process runs and nothing expires one, so `CreateLookupTable` refuses
/// to create more than this many with the `LimitExceededException` it models ("You have
/// reached the maximum number of resources that can be created") rather than growing
/// without bound on request. It is set far above the number of tables a caller legitimately
/// creates, so only a caller hammering the create reaches it.
pub const MAX_LOOKUP_TABLES: usize = 1_000;

/// How many BYTES of lookup-table CSV content this emulator will hold across every table of
/// the account.
///
/// [`MAX_LOOKUP_TABLES`] alone would leave total memory bounded only by its product with
/// the modelled `TableBody` length (10 MB), so this is what actually bounds the process:
/// whatever a caller does — one table or a thousand — the lookup-table store holds at most
/// this many bytes of caller-chosen content. Not a fidelity claim, and the same order as
/// the service-wide retained budgets of this workspace's peer services (s3's
/// `MAX_OBJECT_BYTES_TOTAL`, appconfig's `MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`,
/// codeguruprofiler's `MAX_RETAINED_PROFILE_BYTES`). A create that would take the store past
/// it is refused with the `LimitExceededException` `CreateLookupTable` models, BEFORE the
/// body is retained, rather than recorded.
pub const MAX_LOOKUP_TABLE_BYTES_TOTAL: usize = 64 * 1024 * 1024;

/// The only integration type CloudWatch Logs supports, from the model's `IntegrationType`
/// enum (`OPENSEARCH`), which `PutIntegration` states as well: "Currently, only
/// integrations with OpenSearch Service are supported".
pub const INTEGRATION_TYPE_OPENSEARCH: &str = "OPENSEARCH";

/// The status an integration is created in, from the model's `IntegrationStatus` enum:
/// "After you create an integration, it takes a few minutes to complete. During this time,
/// you'll see the status as PROVISIONING" (`PutIntegrationResponse$integrationStatus`).
pub const INTEGRATION_STATUS_PROVISIONING: &str = "PROVISIONING";

/// The status an integration settles in once its OpenSearch Service resources are
/// provisioned, from the model's `IntegrationStatus` enum. Nothing but a delete moves an
/// integration out of it, so the clock owes it nothing further.
pub const INTEGRATION_STATUS_ACTIVE: &str = "ACTIVE";

/// The status an integration whose provisioning failed carries, from the model's
/// `IntegrationStatus` enum. Nothing in this emulator puts an integration here: a
/// `PutIntegration` it cannot accept is refused outright, and one it accepts provisions.
pub const INTEGRATION_STATUS_FAILED: &str = "FAILED";

/// How long after it is created an integration takes to leave
/// [`INTEGRATION_STATUS_PROVISIONING`] for [`INTEGRATION_STATUS_ACTIVE`].
///
/// AWS states only that it "takes a few minutes to complete", so the span is THE
/// EMULATOR'S OWN CHOICE: the provisioning runs asynchronously, so the create hands back a
/// PROVISIONING integration and the clock owes it ACTIVE later. It is deliberately not
/// zero — a caller that reads, lists or deletes the integration straight after creating it
/// still observes PROVISIONING, and the rules that go with that status.
pub const INTEGRATION_PROVISIONING_DELAY_MS: i64 = 500;

/// How many integrations one account may hold, from `PutIntegration`: "currently you can
/// have only one integration in your account", which `ListIntegrations` repeats
/// ("Currently, only one integration can be created in an account").
pub const MAX_INTEGRATIONS: usize = 1;

/// The status a data-source association of an S3 Table Integration is in while it gives
/// query access, from the model's `S3TableIntegrationSourceStatus` enum (`ACTIVE` |
/// `UNHEALTHY` | `FAILED` | `DATA_SOURCE_DELETE_IN_PROGRESS`).
///
/// `AssociateSourceToS3TableIntegration` "enables querying log data using analytics
/// engines that support Iceberg" and the enum holds no status for an association still
/// being made, so an accepted associate stores one already in this status.
pub const S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE: &str = "ACTIVE";

/// The status a data-source association carries once it has been disassociated, from the
/// model's `S3TableIntegrationSourceStatus` enum.
///
/// `DisassociateSourceFromS3TableIntegration` is "removing query access and deleting all
/// associated data from the integration": the deletion of that data is work the call leaves
/// running, which is what this status reports while it runs.
pub const S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS: &str =
    "DATA_SOURCE_DELETE_IN_PROGRESS";

/// How long after it is disassociated a data-source association takes to be gone — the
/// span it is reported in [`S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS`] for.
///
/// AWS documents no duration for deleting the associated data, so the span is THE
/// EMULATOR'S OWN CHOICE. It is deliberately not zero: the model carries a status for an
/// association whose data is still being deleted, so a caller that lists the integration's
/// sources straight after disassociating one still observes it in that status.
pub const S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS: i64 = 3_000;

/// The most data-source associations of S3 Table Integrations this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: CloudWatch Logs publishes no quota for the
/// data sources an S3 Table Integration may have, and nothing in the request bounds how
/// many a caller makes — the integration they are made against is named by an ARN rather
/// than by a resource of this store, so no other cap dominates this one. An associate past
/// it is refused with the only quota-shaped error the operation models
/// (`ThrottlingException`, "The request was throttled because of quota limits") BEFORE a
/// record is allocated, rather than retained.
pub const MAX_S3_TABLE_INTEGRATION_SOURCES: usize = 500;

/// The refusal an associate past [`MAX_S3_TABLE_INTEGRATION_SOURCES`] carries: the cap that
/// refused it, so a caller can tell this guard from a request it could fix.
pub fn s3_table_integration_sources_cap_message() -> String {
    format!(
        "the maximum of {MAX_S3_TABLE_INTEGRATION_SOURCES} data source association(s) has been \
         reached"
    )
}

/// An OpenSearch Service resource of an integration that is there, from the model's
/// `OpenSearchResourceStatusType` enum (`ACTIVE` | `NOT_FOUND` | `ERROR`).
pub const OPENSEARCH_RESOURCE_STATUS_ACTIVE: &str = "ACTIVE";

/// An OpenSearch Service resource of an integration that is not there yet, from the
/// model's `OpenSearchResourceStatusType` enum.
pub const OPENSEARCH_RESOURCE_STATUS_NOT_FOUND: &str = "NOT_FOUND";

/// An OpenSearch Service resource of an integration that failed, from the model's
/// `OpenSearchResourceStatusType` enum.
pub const OPENSEARCH_RESOURCE_STATUS_ERROR: &str = "ERROR";

/// The ARN service segment of IAM, the service whose roles and users
/// `OpenSearchResourceConfig$dataSourceRoleArn` and `$dashboardViewerPrincipals` name.
const IAM_SERVICE: &str = "iam";

/// The ARN service segment of KMS, the service whose keys
/// `OpenSearchResourceConfig$kmsKeyArn` names.
const KMS_SERVICE: &str = "kms";

/// The ARN service segment of OpenSearch Service, whose applications
/// `OpenSearchResourceConfig$applicationArn` names and whose application an integration's
/// setup creates.
const OPENSEARCH_SERVICE: &str = "opensearch";

/// The ARN service segment of OpenSearch Serverless, which owns the collection an
/// integration's setup creates (`OpenSearchCollection$collectionArn`).
const OPENSEARCH_SERVERLESS_SERVICE: &str = "aoss";

/// The suffixes the OpenSearch Service resources an integration's setup creates are named
/// with, each appended to the integration's own name so one integration's resources are
/// distinguishable from another's and stable across reads. They are the emulator's own
/// naming: AWS documents that the setup creates these resources but not what it calls
/// them. Every suffix uses only characters the resource-name patterns admit
/// (`^[\.\-_/#A-Za-z0-9]+$`).
const APPLICATION_SUFFIX: &str = "-application";
/// See [`APPLICATION_SUFFIX`].
const COLLECTION_SUFFIX: &str = "-collection";
/// See [`APPLICATION_SUFFIX`].
const WORKSPACE_SUFFIX: &str = "-workspace";
/// See [`APPLICATION_SUFFIX`].
const ENCRYPTION_POLICY_SUFFIX: &str = "-encryption-policy";
/// See [`APPLICATION_SUFFIX`].
const NETWORK_POLICY_SUFFIX: &str = "-network-policy";
/// See [`APPLICATION_SUFFIX`].
const ACCESS_POLICY_SUFFIX: &str = "-access-policy";
/// See [`APPLICATION_SUFFIX`].
const LIFECYCLE_POLICY_SUFFIX: &str = "-lifecycle-policy";

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
    /// Every export task of this account — the resource `CreateExportTask` starts and
    /// `DescribeExportTasks` reports — keyed by `ExportTask$taskId`. A `BTreeMap` so
    /// `DescribeExportTasks` enumerates in task-id order without re-sorting, which is also
    /// what makes its pagination cursor stable.
    pub export_tasks: BTreeMap<String, ExportTaskRecord>,
    /// Every CloudWatch Logs Insights query of this account — the one `StartQuery` starts
    /// and `DescribeQueries` reports — keyed by `QueryInfo$queryId`. A `BTreeMap` so
    /// `DescribeQueries` enumerates in query-id order without re-sorting, which is also what
    /// makes its pagination cursor stable.
    pub queries: BTreeMap<String, QueryRecord>,
    /// Monotonic counter behind the `queryId`s `StartQuery` mints, so every query it starts
    /// gets an id no earlier one used. See [`mint_query_id`].
    pub next_query_seq: u64,
    /// Monotonic counter behind the `taskId`s `CreateExportTask` mints, so every task it
    /// creates gets an id no earlier create used. See [`mint_export_task_id`].
    pub next_export_task_seq: u64,
    /// Every delivery of this account — the resource `CreateDelivery` creates and
    /// `DescribeDeliveries` reports — keyed by the Region it was created in and its
    /// `Delivery$id` ([`RegionalKey`]). A `BTreeMap` so `DescribeDeliveries` enumerates one
    /// Region's deliveries in delivery-id order without re-sorting, which is also what makes
    /// its pagination cursor stable.
    ///
    /// A delivery is an account-level resource rather than a log group's: its source and
    /// its destination are both named by their own resources, so it lives here and not on
    /// [`LogGroupRecord`].
    pub deliveries: BTreeMap<RegionalKey, DeliveryRecord>,
    /// Monotonic counter behind the `Delivery$id`s `CreateDelivery` mints, so every delivery
    /// it creates gets an id no earlier create used — including after the delivery holding an
    /// id has been deleted. See [`mint_delivery_id`].
    pub next_delivery_seq: u64,
    /// The account's storage tier policy — what `PutStorageTierPolicy` sets and
    /// `GetStorageTierPolicy` reports. `None` until a put sets one, which is why
    /// `GetStorageTierPolicy` reports [`DEFAULT_STORAGE_TIER`] with no
    /// `lastUpdatedTime`: the policy is "for the account", so there is exactly one of it
    /// and reading it before any put is not a missing resource.
    pub storage_tier_policy: Option<StorageTierPolicyRecord>,
    /// Every delivery destination of this account — the resource `PutDeliveryDestination`
    /// creates or updates and `GetDeliveryDestination` / `DescribeDeliveryDestinations`
    /// report — keyed by the Region it was created in and its `DeliveryDestination$name`
    /// ([`RegionalKey`]): the name is what identifies one within a Region ("This name must be
    /// unique for all delivery destinations in your account"). A `BTreeMap` so
    /// `DescribeDeliveryDestinations` enumerates one Region's destinations in
    /// destination-name order without re-sorting, which is also what makes its pagination
    /// cursor stable.
    ///
    /// A delivery destination is a LOGICAL object naming a resource of another service (or
    /// another log group), so it lives here rather than on [`LogGroupRecord`]: deleting the
    /// log group it points at does not delete the destination.
    pub delivery_destinations: BTreeMap<RegionalKey, DeliveryDestinationRecord>,
    /// Every delivery source of this account — the resource `PutDeliverySource` creates or
    /// updates and `GetDeliverySource` / `DescribeDeliverySources` report — keyed by the
    /// Region it was created in and its `DeliverySource$name` ([`RegionalKey`]): the name is
    /// what identifies one within a Region ("This name must be unique for all delivery
    /// sources in your account"). A `BTreeMap` so `DescribeDeliverySources` enumerates one
    /// Region's sources in source-name order without re-sorting, which is also what makes its
    /// pagination cursor stable.
    ///
    /// A delivery source is a LOGICAL object naming the resource of another service that
    /// sends the logs, so it lives here rather than on [`LogGroupRecord`]: no log group owns
    /// one, and a delivery pairs it with a destination by the name it is stored under
    /// ([`DeliveryRecord::delivery_source_name`]).
    pub delivery_sources: BTreeMap<RegionalKey, DeliverySourceRecord>,
    /// Every logical destination of this account — the resource `PutDestination` creates or
    /// updates, `DescribeDestinations` reports and `DeleteDestination` removes — keyed by
    /// `Destination$destinationName`, the one member every other destination operation
    /// identifies one by. A `BTreeMap` so `DescribeDestinations` enumerates them in the
    /// order it documents ("The results are ASCII-sorted by destination name") without
    /// re-sorting, which is also what makes its pagination cursor stable.
    ///
    /// A destination is a LOGICAL object naming a physical resource of another service, so
    /// it lives here rather than on [`LogGroupRecord`]: the log groups whose subscription
    /// filters deliver to it neither own it nor outlive it.
    pub destinations: BTreeMap<String, DestinationRecord>,
    /// Every log anomaly detector of this account — the resource `CreateLogAnomalyDetector`
    /// creates and `GetLogAnomalyDetector` reports — keyed by `anomalyDetectorArn`, the one
    /// member every other anomaly-detector operation identifies one by. A `BTreeMap` so
    /// `ListLogAnomalyDetectors` enumerates in detector-ARN order without re-sorting, which
    /// is also what makes its pagination cursor stable.
    pub anomaly_detectors: BTreeMap<String, AnomalyDetectorRecord>,
    /// Monotonic counter behind the anomaly-detector identifiers, so every create mints an
    /// ARN no earlier create used — including after the detector holding it was deleted.
    pub next_anomaly_detector_seq: u64,
    /// Every lookup table of this account — the reference data `CreateLookupTable` uploads
    /// and `GetLookupTable` reports — keyed by `lookupTableName`, which is what identifies
    /// one ("The name must be unique within your account and Region"). A `BTreeMap` so
    /// `DescribeLookupTables` enumerates in table-name order without re-sorting, which is
    /// what its `lookupTableNamePrefix` filter and its pagination cursor both read.
    pub lookup_tables: BTreeMap<String, LookupTableRecord>,
    /// Every scheduled query of this account — the one `CreateScheduledQuery` creates and
    /// `GetScheduledQuery` reports — keyed by `ScheduledQueryName`, which is what identifies
    /// one ("The name must be unique within your account and region"). A `BTreeMap` so the
    /// stored order is stable in query-name order without re-sorting.
    pub scheduled_queries: BTreeMap<String, ScheduledQueryRecord>,
    /// Every integration of this account — the resource `PutIntegration` creates,
    /// `GetIntegration` and `ListIntegrations` report and `DeleteIntegration` removes —
    /// keyed by `integrationName`, the one member every other integration operation
    /// identifies one by.
    ///
    /// An integration is an account-level resource rather than a log group's: it connects
    /// the account's logs to another service, and no log group owns it.
    pub integrations: BTreeMap<String, IntegrationRecord>,
    /// Monotonic counter behind [`IntegrationRecord::creation_seq`], so the creation order
    /// `ListIntegrations` enumerates by is total and never reused — including after the
    /// integration holding a position has been deleted.
    pub next_integration_seq: u64,
    /// Every data-source association of an S3 Table Integration — the one
    /// `AssociateSourceToS3TableIntegration` creates,
    /// `ListSourcesForS3TableIntegration` reports and
    /// `DisassociateSourceFromS3TableIntegration` removes — keyed by the
    /// `S3TableIntegrationSourceIdentifier` the associate minted for it, which is the one
    /// member every other operation of the family identifies one by.
    ///
    /// An association is account-level rather than a log group's: it pairs a data source
    /// with the integration named by an ARN, and no log group owns either. A `BTreeMap` so
    /// one pass enumerates in identifier order — which is the order the identifiers were
    /// minted in ([`mint_s3_table_integration_source_identifier`]) — without re-sorting,
    /// which is also what makes `ListSourcesForS3TableIntegration`'s pagination cursor
    /// stable.
    pub s3_table_integration_sources: BTreeMap<String, S3TableIntegrationSourceRecord>,
    /// Monotonic counter behind the `S3TableIntegrationSourceIdentifier`s an associate
    /// mints, so every association gets an identifier no earlier one used — including after
    /// the association holding an identifier has been disassociated.
    pub next_s3_table_integration_source_seq: u64,
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

    /// Whether `candidate` is a revision id this service has ever ISSUED for a
    /// resource-scoped policy.
    ///
    /// A revision id is minted from the store's monotonic counter
    /// ([`mint_revision_id`]), so the ids that exist are exactly the renderings of the
    /// counter values already taken: a value in another form, or one past the counter, is
    /// not a revision this service handed to anybody. That is what separates the two
    /// conditions a resource-policy write models — a value the service never issued is a
    /// bad PARAMETER, while a value it did issue and has since superseded is an
    /// optimistic-lock CONFLICT the caller is to re-read and retry.
    pub fn issued_revision_id(&self, candidate: &str) -> bool {
        candidate.len() == REVISION_ID_LEN
            && candidate
                .chars()
                .all(|digit| digit.is_ascii_digit() || matches!(digit, 'a'..='f'))
            && u128::from_str_radix(candidate, 16)
                .is_ok_and(|seq| seq < u128::from(self.next_policy_revision))
    }

    /// Take the next anomaly-detector sequence number, advancing the counter.
    pub fn take_anomaly_detector_seq(&mut self) -> u64 {
        let seq = self.next_anomaly_detector_seq;
        self.next_anomaly_detector_seq = self.next_anomaly_detector_seq.saturating_add(1);
        seq
    }

    /// Take the next integration sequence number, advancing the counter.
    pub fn take_integration_seq(&mut self) -> u64 {
        let seq = self.next_integration_seq;
        self.next_integration_seq = self.next_integration_seq.saturating_add(1);
        seq
    }

    /// Take the next S3-Table-Integration source sequence number, advancing the counter.
    pub fn take_s3_table_integration_source_seq(&mut self) -> u64 {
        let seq = self.next_s3_table_integration_source_seq;
        self.next_s3_table_integration_source_seq =
            self.next_s3_table_integration_source_seq.saturating_add(1);
        seq
    }

    /// Drop every data-source association whose deletion the clock has ALREADY made due, as
    /// of `now_ms`.
    ///
    /// Disassociating one is "removing query access and deleting all associated data from
    /// the integration", work that outlives the call: the association is left in
    /// [`S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS`] with the instant its
    /// deletion is due ([`S3TableIntegrationSourceRecord::removal_due_ms`]), and every
    /// operation of the family calls this before it reads, so what it reports is what the
    /// clock already decided — never a deletion finished because something looked.
    pub fn materialize_due_s3_table_integration_source_removals(&mut self, now_ms: i64) {
        self.s3_table_integration_sources
            .retain(|_, source| !source.removal_due(now_ms));
    }

    /// The associations of ONE S3 Table Integration, in the order their identifiers were
    /// minted, which is what `ListSourcesForS3TableIntegration` reports: "a list of data
    /// source associations for a specified S3 Table Integration".
    ///
    /// Read AFTER [`Self::materialize_due_s3_table_integration_source_removals`], so an
    /// association the clock has already deleted is not reported.
    pub fn s3_table_integration_sources_of(
        &self,
        integration_arn: &str,
    ) -> Vec<&S3TableIntegrationSourceRecord> {
        self.s3_table_integration_sources
            .values()
            .filter(|source| source.integration_arn == integration_arn)
            .collect()
    }

    /// The association that already gives `data_source` query access through the
    /// integration `integration_arn`, if the account holds one.
    ///
    /// An association IS the pairing of one data source with one integration, so a second
    /// associate of the same pair names the same association rather than a second one. An
    /// association whose data is being deleted
    /// ([`S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS`]) is not one of them: its
    /// query access is already removed, so associating the pair again is a new association.
    pub fn associated_s3_table_integration_source(
        &self,
        integration_arn: &str,
        data_source: &crate::ops::types::DataSource,
    ) -> Option<&S3TableIntegrationSourceRecord> {
        self.s3_table_integration_sources.values().find(|source| {
            source.integration_arn == integration_arn
                && source.removal_due_ms.is_none()
                && source.data_source.name == data_source.name
                && source.data_source.source_type == data_source.r#type
        })
    }

    /// Take the next query-definition sequence number, advancing the counter.
    pub fn take_query_definition_seq(&mut self) -> u64 {
        let seq = self.next_query_definition_seq;
        self.next_query_definition_seq = self.next_query_definition_seq.saturating_add(1);
        seq
    }

    /// Take the next delivery sequence number, advancing the counter.
    pub fn take_delivery_seq(&mut self) -> u64 {
        let seq = self.next_delivery_seq;
        self.next_delivery_seq = self.next_delivery_seq.saturating_add(1);
        seq
    }

    /// Take the next export-task sequence number, advancing the counter.
    pub fn take_export_task_seq(&mut self) -> u64 {
        let seq = self.next_export_task_seq;
        self.next_export_task_seq = self.next_export_task_seq.saturating_add(1);
        seq
    }

    /// Take the next query sequence number, advancing the counter.
    pub fn take_query_seq(&mut self) -> u64 {
        let seq = self.next_query_seq;
        self.next_query_seq = self.next_query_seq.saturating_add(1);
        seq
    }

    /// The delivery source `region`'s endpoint holds under `name`, if any.
    ///
    /// A delivery source belongs to the Region it was created in ([`RegionalKey`]), so a name
    /// another Region holds a source under names none here.
    pub fn delivery_source(&self, region: &str, name: &str) -> Option<&DeliverySourceRecord> {
        self.delivery_sources.get(&RegionalKey::new(region, name))
    }

    /// The delivery sources `region`'s endpoint holds, in source-name order — the
    /// enumeration `DescribeDeliverySources` reports and pages.
    pub fn delivery_sources_in<'store>(
        &'store self,
        region: &str,
    ) -> impl Iterator<Item = &'store DeliverySourceRecord> {
        let region = region.to_string();
        self.delivery_sources
            .iter()
            .filter(move |(key, _record)| key.region == region)
            .map(|(_key, record)| record)
    }

    /// The delivery destination `region`'s endpoint holds under `name`, if any. See
    /// [`Self::delivery_source`]: a destination belongs to the Region it was created in too.
    pub fn delivery_destination(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&DeliveryDestinationRecord> {
        self.delivery_destinations
            .get(&RegionalKey::new(region, name))
    }

    /// The same delivery destination, to write — what assigning or removing its delivery
    /// destination policy reaches.
    pub fn delivery_destination_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut DeliveryDestinationRecord> {
        self.delivery_destinations
            .get_mut(&RegionalKey::new(region, name))
    }

    /// The delivery destinations `region`'s endpoint holds, in destination-name order — the
    /// enumeration `DescribeDeliveryDestinations` reports and pages.
    pub fn delivery_destinations_in<'store>(
        &'store self,
        region: &str,
    ) -> impl Iterator<Item = &'store DeliveryDestinationRecord> {
        let region = region.to_string();
        self.delivery_destinations
            .iter()
            .filter(move |(key, _record)| key.region == region)
            .map(|(_key, record)| record)
    }

    /// The delivery `region`'s endpoint holds under `id`, if any. See
    /// [`Self::delivery_source`]: a delivery belongs to the Region it was created in too.
    pub fn delivery(&self, region: &str, id: &str) -> Option<&DeliveryRecord> {
        self.deliveries.get(&RegionalKey::new(region, id))
    }

    /// The same delivery, to write — what `UpdateDeliveryConfiguration` reaches.
    pub fn delivery_mut(&mut self, region: &str, id: &str) -> Option<&mut DeliveryRecord> {
        self.deliveries.get_mut(&RegionalKey::new(region, id))
    }

    /// The deliveries `region`'s endpoint holds, in delivery-id order — the enumeration
    /// `DescribeDeliveries` reports and pages, and what answers whether a source or a
    /// destination of that Region is still in use.
    pub fn deliveries_in<'store>(
        &'store self,
        region: &str,
    ) -> impl Iterator<Item = &'store DeliveryRecord> {
        let region = region.to_string();
        self.deliveries
            .iter()
            .filter(move |(key, _record)| key.region == region)
            .map(|(_key, record)| record)
    }

    /// The tags assigned to the resource a tagging operation named, or `None` when
    /// `region`'s endpoint holds no such resource.
    ///
    /// One resource holds ONE tag set, on the resource's own record, so every operation
    /// that reads or writes tags — and every reporter of the resource itself, such as
    /// `ListLogGroups`'s `logGroupTags` filter — sees the same map.
    pub fn resource_tags(
        &self,
        region: &str,
        resource: &TaggedResource,
    ) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::LogGroup(name) => self.log_groups.get(name).map(|group| &group.tags),
            TaggedResource::Delivery(id) => {
                self.delivery(region, id).map(|delivery| &delivery.tags)
            }
            TaggedResource::Destination(name) => self
                .destinations
                .get(name)
                .map(|destination| &destination.tags),
            TaggedResource::DeliverySource(name) => {
                self.delivery_source(region, name).map(|source| &source.tags)
            }
            TaggedResource::DeliveryDestination(name) => self
                .delivery_destination(region, name)
                .map(|destination| &destination.tags),
            TaggedResource::LookupTable(name) => {
                self.lookup_tables.get(name).map(|table| &table.tags)
            }
        }
    }

    /// The delivery destination `region`'s endpoint holds under `arn`, if any.
    ///
    /// A destination is keyed by the Region it was created in and the name that identifies
    /// one there, but a delivery names its destination by ARN
    /// (`CreateDelivery$deliveryDestinationArn`), so the lookup a delivery needs is by the ARN
    /// each stored destination carries — among the destinations of the Region the delivery is
    /// being made in, which is the only Region's resources that endpoint serves.
    pub fn delivery_destination_by_arn(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&DeliveryDestinationRecord> {
        self.delivery_destinations_in(region)
            .find(|destination| destination.arn == arn)
    }

    /// The logical destination this account holds under the ARN `destination_arn`, if any.
    ///
    /// A destination is keyed by its `destinationName`, which is the only thing
    /// `PutDestinationPolicy` and `DeleteDestination` identify one by, while a subscription
    /// filter names its destination by ARN (`PutSubscriptionFilter$destinationArn`) — so the
    /// name is read off the ARN's `destination` resource segment
    /// ([`destination_name_from_arn`]) and the store is asked for it. `None` for an ARN that
    /// names no `destination` resource at all, and for one naming a destination this account
    /// does not hold — another account's destination, which this store knows nothing about.
    pub fn destination_by_arn(&self, destination_arn: &str) -> Option<&DestinationRecord> {
        self.destinations.get(destination_name_from_arn(destination_arn)?)
    }

    /// The scheduled query this account holds under `identifier`, if any.
    ///
    /// A scheduled query is keyed by its name, but every reader identifies one by "The ARN or
    /// name of the scheduled query" (`ScheduledQueryIdentifier`), so both forms resolve to the
    /// same record: the name is the store's key, and an ARN names the query its resource
    /// segment does ([`scheduled_query_name_from_identifier`]).
    pub fn scheduled_query(&self, identifier: &str) -> Option<&ScheduledQueryRecord> {
        self.scheduled_queries.get(identifier).or_else(|| {
            self.scheduled_queries
                .get(scheduled_query_name_from_identifier(identifier))
        })
    }

    /// The same scheduled query, to write. See [`Self::scheduled_query`]: both forms of
    /// `ScheduledQueryIdentifier` resolve to the one record, so a write addressed by an ARN
    /// and a write addressed by a name reach the same query.
    pub fn scheduled_query_mut(&mut self, identifier: &str) -> Option<&mut ScheduledQueryRecord> {
        let key = if self.scheduled_queries.contains_key(identifier) {
            identifier
        } else {
            scheduled_query_name_from_identifier(identifier)
        };
        self.scheduled_queries.get_mut(key)
    }

    /// Remove the scheduled query `identifier` names, returning the record that was stored
    /// under it, or `None` when the account holds none under it.
    ///
    /// Resolved exactly as the readers resolve one (see [`Self::scheduled_query`]), so a
    /// removal addressed by an ARN and a removal addressed by a name reach the same query —
    /// and the whole configuration goes with the record, because the record is where all of it
    /// (the query, the schedule, the destinations and the tags) lives.
    pub fn remove_scheduled_query(&mut self, identifier: &str) -> Option<ScheduledQueryRecord> {
        let key = if self.scheduled_queries.contains_key(identifier) {
            identifier.to_string()
        } else {
            scheduled_query_name_from_identifier(identifier).to_string()
        };
        self.scheduled_queries.remove(&key)
    }

    /// The same tag set, to write. See [`Self::resource_tags`].
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        resource: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::LogGroup(name) => {
                self.log_groups.get_mut(name).map(|group| &mut group.tags)
            }
            TaggedResource::Delivery(id) => self
                .delivery_mut(region, id)
                .map(|delivery| &mut delivery.tags),
            TaggedResource::Destination(name) => self
                .destinations
                .get_mut(name)
                .map(|destination| &mut destination.tags),
            TaggedResource::DeliverySource(name) => self
                .delivery_sources
                .get_mut(&RegionalKey::new(region, name))
                .map(|source| &mut source.tags),
            TaggedResource::DeliveryDestination(name) => self
                .delivery_destination_mut(region, name)
                .map(|destination| &mut destination.tags),
            TaggedResource::LookupTable(name) => self
                .lookup_tables
                .get_mut(name)
                .map(|table| &mut table.tags),
        }
    }

    /// Whether an ingested log event of `group` has the metrics its Embedded Metric Format
    /// document declares extracted into CloudWatch metric data.
    ///
    /// "By default, EMF metric creation is enabled for all log groups. You can use metric
    /// extraction policies to disable EMF metric creation for your entire account or specific
    /// log groups" — and a `METRIC_EXTRACTION_POLICY` document is accepted only when it says
    /// the format is disabled ([`check_account_policy_document`]), so extraction is on for
    /// every group no such policy SELECTS ([`account_policy_selects_log_group`]) and off for
    /// the groups one selects.
    pub fn emf_extraction_enabled_for(&self, group: &LogGroupRecord) -> bool {
        !self
            .account_policies
            .values()
            .filter(|policy| policy.policy_type == POLICY_TYPE_METRIC_EXTRACTION)
            .any(|policy| account_policy_selects_log_group(policy, group))
    }

    /// Materialise every CloudWatch Logs Insights query status change the clock has ALREADY
    /// made due, as of `now_ms`.
    ///
    /// A query runs asynchronously — `StartQuery` hands back a `queryId` and the results are
    /// fetched later — so the status it is in is owed by the clock rather than by a call
    /// ([`QueryRecord::running_due_ms`], [`QueryRecord::complete_due_ms`]). A reporting
    /// operation calls this before it reads, so what it reports is what the clock already
    /// decided — never a status moved on because something was polled.
    pub fn materialize_due_query_transitions(&mut self, now_ms: i64) {
        for query in self.queries.values_mut() {
            query.materialize_due_transitions(now_ms);
        }
    }

    /// How many queries of this account are still in flight, which is what
    /// [`MAX_CONCURRENT_QUERIES`] bounds.
    ///
    /// Read AFTER [`Self::materialize_due_query_transitions`], so a query the clock has
    /// already completed does not hold a slot.
    pub fn active_queries(&self) -> usize {
        self.queries.values().filter(|query| query.is_active()).count()
    }

    /// Drop the oldest finished queries the store holds beyond [`MAX_QUERIES`], so repeated
    /// starts cannot grow it without bound. An ACTIVE query is never dropped: it has not run
    /// yet.
    pub fn enforce_query_ceiling(&mut self) {
        while self.queries.len() > MAX_QUERIES {
            let Some(oldest) = self
                .queries
                .values()
                .filter(|query| !query.is_active())
                .min_by_key(|query| (query.create_time_ms, query.query_id.clone()))
                .map(|query| query.query_id.clone())
            else {
                return;
            };
            self.queries.remove(&oldest);
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

    /// Materialise every export-task status change the clock has ALREADY made due, as of
    /// `now_ms`.
    ///
    /// `CreateExportTask` "is an asynchronous call", so an export task's statuses are owed by
    /// the clock rather than by a call: each change was due at the instant the record stores
    /// for it ([`ExportTaskRecord::running_due_ms`], [`ExportTaskRecord::completion_due_ms`])
    /// even if nobody asked. A reporting operation calls this before it reads, so what it
    /// reports is what the clock already decided — never a status moved on because something
    /// was polled.
    pub fn materialize_due_export_transitions(&mut self, now_ms: i64) {
        for task in self.export_tasks.values_mut() {
            task.materialize_due_transitions(now_ms);
        }
    }

    /// Whether this account already has an export task in one of the two statuses
    /// `CreateExportTask` counts as active ("Each account can only have one active (RUNNING
    /// or PENDING) export task at a time"). The slot is the account's, so holding it is all
    /// there is to know about it: the refusal names nothing of the task that holds it.
    ///
    /// Read AFTER [`Self::materialize_due_export_transitions`], so a task the clock has
    /// already completed does not count as active.
    pub fn has_active_export_task(&self) -> bool {
        self.export_tasks.values().any(ExportTaskRecord::is_active)
    }

    /// Drop the oldest export tasks the store holds beyond [`MAX_EXPORT_TASKS`], so repeated
    /// creates cannot grow it without bound. An ACTIVE task is never dropped: it is still
    /// running, and only one of those can exist at a time.
    pub fn enforce_export_task_ceiling(&mut self) {
        while self.export_tasks.len() > MAX_EXPORT_TASKS {
            let Some(oldest) = self
                .export_tasks
                .values()
                .filter(|task| !task.is_active())
                .min_by_key(|task| (task.creation_time_ms, task.task_id.clone()))
                .map(|task| task.task_id.clone())
            else {
                return;
            };
            self.export_tasks.remove(&oldest);
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

    /// How many bytes of lookup-table CSV content the account's store currently holds —
    /// what [`MAX_LOOKUP_TABLE_BYTES_TOTAL`] bounds. DERIVED by summing the stored bodies,
    /// so it cannot drift from what the store actually holds.
    pub fn lookup_table_bytes(&self) -> usize {
        self.lookup_tables
            .values()
            .map(|table| table.table_body.len())
            .sum()
    }

    /// Materialise every integration status change the clock has ALREADY made due, as of
    /// `now_ms`.
    ///
    /// An integration provisions on its own — "it takes a few minutes to complete" — so the
    /// status it is in is owed by the clock rather than by a call
    /// ([`IntegrationRecord::status_due_ms`]). A reporting operation calls this before it
    /// reads, so what it reports is what the clock already decided — never a status moved
    /// on because something was polled.
    pub fn materialize_due_integration_transitions(&mut self, now_ms: i64) {
        for integration in self.integrations.values_mut() {
            integration.materialize_due_transitions(now_ms);
        }
    }

    /// The account's integrations in the order they were created
    /// ([`IntegrationRecord::creation_seq`]), which is what `ListIntegrations` reports.
    ///
    /// AWS documents no order for that operation, so this one is the emulator's: creation
    /// order is stable across calls and does not move when another integration is created
    /// or deleted, which name order would not be.
    pub fn integrations_in_creation_order(&self) -> Vec<&IntegrationRecord> {
        let mut integrations: Vec<&IntegrationRecord> = self.integrations.values().collect();
        integrations.sort_by_key(|integration| integration.creation_seq);
        integrations
    }
}

/// One resource policy — "a policy enabling one or more entities to put logs to a log
/// group in this account" — as it is stored.
#[derive(Debug, Clone)]
pub struct ResourcePolicyRecord {
    /// `ResourcePolicy$policyName` — "The name of the resource policy." The name is the
    /// whole identity of an account-scoped policy, so one always carries it; a
    /// resource-scoped policy is identified by the resource it is attached to, so its
    /// name is whatever the request supplied and `None` when it supplied none.
    pub policy_name: Option<String>,
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

/// One CloudWatch Logs Insights query — what `StartQuery` starts and `DescribeQueries`
/// reports — as it is stored. Every member of the model's `QueryInfo` shape is reported from
/// this record, so `DescribeQueries` answers from state alone.
#[derive(Debug, Clone)]
pub struct QueryRecord {
    /// `QueryInfo$queryId` — "The unique ID number of this query." Also the key the store
    /// enumerates by.
    pub query_id: String,
    /// `QueryInfo$queryLanguage` — "The query language used for this query",
    /// [`DEFAULT_QUERY_LANGUAGE`] when the start named none.
    pub query_language: String,
    /// `QueryInfo$queryString` — "The query string used in this query."
    pub query_string: String,
    /// `QueryInfo$logGroupName` — "The name of the log group scanned by this query", for a
    /// query that named exactly one. A query over several log groups, or one whose `SOURCE`
    /// command selects them, carries the whole list in [`Self::log_group_names`] and reports
    /// no single name.
    pub log_group_name: Option<String>,
    /// Every log group this query reads, resolved to log group NAMES from whichever member
    /// the start named them by. Empty for a query whose `SOURCE` command selects its own.
    pub log_group_names: Vec<String>,
    /// `StartQuery$startTime` — the beginning of the time range queried, in epoch SECONDS,
    /// the unit that member documents ("Specified as epoch time, the number of seconds since
    /// January 1, 1970, 00:00:00 UTC").
    pub start_time_secs: i64,
    /// `StartQuery$endTime` — the end of that range, in epoch seconds. See
    /// [`Self::start_time_secs`].
    pub end_time_secs: i64,
    /// `StartQuery$limit` — "The maximum number of log events to return from the query",
    /// when the start named one.
    pub limit: Option<i32>,
    /// `QueryInfo$status`, one of the [`QUERY_STATUS_SCHEDULED`] family.
    pub status: String,
    /// `QueryInfo$createTime` — "The date and time that this query was created", in epoch
    /// milliseconds.
    pub create_time_ms: i64,
    /// The instant this query is due to start running, stored rather than waited for: a
    /// reporting operation materialises the transition once the clock is past it
    /// ([`materialize_due_query_transitions`]). `None` once it has left `Scheduled`.
    ///
    /// [`materialize_due_query_transitions`]:
    ///     ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_query_transitions
    pub running_due_ms: Option<i64>,
    /// The instant this query is due to reach `Complete`, on the same terms as
    /// [`Self::running_due_ms`]. `None` once it is in a terminal status, which owes no
    /// further transition.
    pub complete_due_ms: Option<i64>,
}

impl QueryRecord {
    /// A brand-new query: accepted, not running yet, and due to run
    /// [`QUERY_RUNNING_DELAY_MS`] after it was started and to complete
    /// [`QUERY_COMPLETION_DELAY_MS`] after that.
    pub fn new(
        query_id: &str,
        query_language: &str,
        query_string: &str,
        log_group_names: Vec<String>,
        start_time_secs: i64,
        end_time_secs: i64,
        create_time_ms: i64,
    ) -> Self {
        let running_due_ms = create_time_ms.saturating_add(QUERY_RUNNING_DELAY_MS);
        Self {
            query_id: query_id.to_string(),
            query_language: query_language.to_string(),
            query_string: query_string.to_string(),
            // "The name of the log group scanned by this query" is one name, so it is
            // reported only for a query that reads exactly one log group.
            log_group_name: match log_group_names.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            },
            log_group_names,
            start_time_secs,
            end_time_secs,
            limit: None,
            status: QUERY_STATUS_SCHEDULED.to_string(),
            create_time_ms,
            running_due_ms: Some(running_due_ms),
            complete_due_ms: Some(running_due_ms.saturating_add(QUERY_COMPLETION_DELAY_MS)),
        }
    }

    /// Whether this query is still one of the statuses a concurrent-query quota counts: a
    /// query that has finished, failed or been cancelled holds no slot.
    pub fn is_active(&self) -> bool {
        self.status == QUERY_STATUS_SCHEDULED || self.status == QUERY_STATUS_RUNNING
    }

    /// Stop this query: it reaches [`QUERY_STATUS_CANCELLED`] and owes the clock no further
    /// transition, so the completion it was armed for is DISCARDED rather than materialised
    /// later over the cancellation. Only a query that [`Self::is_active`] has an execution
    /// left to stop.
    pub fn cancel(&mut self) {
        self.status = QUERY_STATUS_CANCELLED.to_string();
        self.running_due_ms = None;
        self.complete_due_ms = None;
    }

    /// Materialise the status changes the clock owes this query: a scheduled query whose
    /// start instant has passed is `Running`, and a running one whose completion instant has
    /// passed is `Complete` — each decided AT that instant and not at the time something
    /// happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        if let Some(due) = self.running_due_ms {
            if due <= now_ms {
                self.status = QUERY_STATUS_RUNNING.to_string();
                self.running_due_ms = None;
            }
        }
        if self.running_due_ms.is_some() {
            return;
        }
        if let Some(due) = self.complete_due_ms {
            if due <= now_ms {
                self.status = QUERY_STATUS_COMPLETE.to_string();
                self.complete_due_ms = None;
            }
        }
    }

    /// This query as the model's `QueryInfo` structure, which is what `DescribeQueries`
    /// reports.
    pub fn reported(&self) -> crate::ops::types::QueryInfo {
        crate::ops::types::QueryInfo {
            query_language: Some(self.query_language.clone()),
            query_id: Some(self.query_id.clone()),
            query_string: Some(self.query_string.clone()),
            status: Some(self.status.clone()),
            create_time: Some(self.create_time_ms),
            log_group_name: self.log_group_name.clone(),
            // "The duration in milliseconds that the query took to execute", "The total
            // number of bytes scanned by the query" and "The ARN of the user who ran the
            // query" are facts about an EXECUTION and an identity this emulator does not
            // hold: nothing it stores can report any of them, so none is reported rather
            // than filled with a number nothing measured.
            query_duration: None,
            bytes_scanned: None,
            user_identity: None,
        }
    }
}

/// A query that has been accepted but has not started running, from the model's
/// `QueryStatus` enum (`Scheduled` | `Running` | `Complete` | `Failed` | `Cancelled` |
/// `Timeout` | `Unknown`). `StartQuery` hands back a `queryId` and the results are fetched
/// later, so an accepted query starts here.
pub const QUERY_STATUS_SCHEDULED: &str = "Scheduled";

/// A query that is running, from the model's `QueryStatus` enum.
pub const QUERY_STATUS_RUNNING: &str = "Running";

/// A query that finished running, from the model's `QueryStatus` enum.
pub const QUERY_STATUS_COMPLETE: &str = "Complete";

/// A query a caller stopped, from the model's `QueryStatus` enum.
pub const QUERY_STATUS_CANCELLED: &str = "Cancelled";

/// How long after it is started a query moves `Scheduled` -> `Running`, and how long after
/// that it reaches `Complete`.
///
/// AWS documents no duration for a query, only the timeout it cannot exceed ("Queries time
/// out after 60 minutes of runtime"), so these spans are THE EMULATOR'S OWN CHOICE: each is
/// deliberately not zero, so a caller that reads the query straight after starting it still
/// observes it scheduled. The instants both fall due at are stored on the record and never
/// slept on.
pub const QUERY_RUNNING_DELAY_MS: i64 = 250;

/// See [`QUERY_RUNNING_DELAY_MS`]: the further span a running query takes to complete.
pub const QUERY_COMPLETION_DELAY_MS: i64 = 250;

/// How many queries one account may have in flight: "You can have up to 100 concurrent
/// CloudWatch Logs insights queries, including queries that have been added to dashboards"
/// (`StartQuery`).
pub const MAX_CONCURRENT_QUERIES: usize = 100;

/// The most log groups one query may read: "You can include up to 50 log groups"
/// (`StartQuery$logGroupNames`, `StartQuery$logGroupIdentifiers`).
pub const MAX_QUERY_LOG_GROUPS: usize = 50;

/// THE EMULATOR'S OWN CEILING on the queries one account's store holds, not a quota cited
/// from AWS: `DescribeQueries` reports "queries that are scheduled, running, or have been run
/// recently", so a finished query stays readable for a while — and the oldest finished ones
/// are dropped once the store holds this many rather than growing without bound on request.
pub const MAX_QUERIES: usize = 1_000;

/// The largest page `DescribeQueries` hands back, which is also the page a request that
/// names no `maxResults` gets: `maxResults` "limits the number of returned queries to the
/// specified number" and documents no default, and the member's own
/// `DescribeQueriesMaxResults` range (`range 1..1000`) is the most the API admits in one
/// page.
pub const MAX_QUERIES_PER_PAGE: i32 = 1_000;

/// One export task — "an export task so that you can efficiently export data from a log
/// group to an Amazon S3 bucket" — as it is stored. Every member of the model's `ExportTask`
/// shape is reported from this record, so `DescribeExportTasks` answers from state alone.
#[derive(Debug, Clone)]
pub struct ExportTaskRecord {
    /// `ExportTask$taskId` — "The ID of the export task." Also the key the store enumerates
    /// by.
    pub task_id: String,
    /// `ExportTask$taskName` — "The name of the export task", when the create named one.
    pub task_name: Option<String>,
    /// `ExportTask$logGroupName` — "The name of the log group from which logs data was
    /// exported."
    pub log_group_name: String,
    /// `ExportTask$from` — "Events with a timestamp before this time are not exported."
    pub from_ms: i64,
    /// `ExportTask$to` — "Events with a timestamp later than this time are not exported."
    pub to_ms: i64,
    /// `ExportTask$destination` — "The name of the S3 bucket to which the log data was
    /// exported."
    pub destination: String,
    /// `ExportTask$destinationPrefix` — "The prefix that was used as the start of Amazon S3
    /// key for every object exported", [`DEFAULT_EXPORT_DESTINATION_PREFIX`] when the create
    /// named none.
    pub destination_prefix: String,
    /// `CreateExportTask$logStreamNamePrefix` — "Export only log streams that match the
    /// provided prefix." No `ExportTask` member reports it, but the task is the only record
    /// of which streams it exports, so it is stored with the task.
    pub log_stream_name_prefix: Option<String>,
    /// `ExportTaskStatus$code`, one of the [`EXPORT_TASK_STATUS_PENDING`] family.
    pub status_code: String,
    /// `ExportTaskStatus$message` — "The status message related to the status code" — the
    /// message of the status this task is IN, stamped by the transition that put it there:
    /// the fixed text of the terminal class a task settles at
    /// ([`EXPORT_TASK_CANCELLED_STATUS_MESSAGE`], [`EXPORT_TASK_COMPLETED_STATUS_MESSAGE`]),
    /// or the reason an export that stopped on an error ended. `None` while the export is
    /// still under way: there is nothing to say about a task that has not reached an outcome
    /// yet.
    pub status_message: Option<String>,
    /// `ExportTaskExecutionInfo$creationTime`, in epoch milliseconds.
    pub creation_time_ms: i64,
    /// `ExportTaskExecutionInfo$completionTime`, in epoch milliseconds — set only once the
    /// task has reached a terminal status.
    pub completion_time_ms: Option<i64>,
    /// The instant this task's export is due to start moving data, stored rather than waited
    /// for: a reporting operation materialises the transition once the clock is past it
    /// ([`materialize_due_export_transitions`]). `None` once the task has left `PENDING`.
    ///
    /// [`materialize_due_export_transitions`]:
    ///     ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_export_transitions
    pub running_due_ms: Option<i64>,
    /// The instant this task's export is due to reach `COMPLETED`, on the same terms as
    /// [`Self::running_due_ms`]. `None` once the task is in a terminal status, which owes no
    /// further transition.
    pub completion_due_ms: Option<i64>,
}

impl ExportTaskRecord {
    /// A brand-new export task: accepted, not started yet, and due to start
    /// [`EXPORT_TASK_RUNNING_DELAY_MS`] after it was created and to complete
    /// [`EXPORT_TASK_COMPLETION_DELAY_MS`] after that.
    ///
    /// The optional members a create may name — `taskName`, `destinationPrefix`,
    /// `logStreamNamePrefix` — are taken here because every one of them is a property of the
    /// export the task IS.
    pub fn new(
        task_id: &str,
        log_group_name: &str,
        from_ms: i64,
        to_ms: i64,
        destination: &str,
        creation_time_ms: i64,
    ) -> Self {
        let running_due_ms = creation_time_ms.saturating_add(EXPORT_TASK_RUNNING_DELAY_MS);
        Self {
            task_id: task_id.to_string(),
            task_name: None,
            log_group_name: log_group_name.to_string(),
            from_ms,
            to_ms,
            destination: destination.to_string(),
            destination_prefix: DEFAULT_EXPORT_DESTINATION_PREFIX.to_string(),
            log_stream_name_prefix: None,
            status_code: EXPORT_TASK_STATUS_PENDING.to_string(),
            status_message: None,
            creation_time_ms,
            completion_time_ms: None,
            running_due_ms: Some(running_due_ms),
            completion_due_ms: Some(
                running_due_ms.saturating_add(EXPORT_TASK_COMPLETION_DELAY_MS),
            ),
        }
    }

    /// Whether this task is one of the two statuses `CreateExportTask` counts as active:
    /// "Each account can only have one active (RUNNING or PENDING) export task at a time."
    pub fn is_active(&self) -> bool {
        self.status_code == EXPORT_TASK_STATUS_PENDING
            || self.status_code == EXPORT_TASK_STATUS_RUNNING
    }

    /// Cancel this export task: it reaches [`EXPORT_TASK_STATUS_CANCELLED`] at `now_ms` and
    /// owes the clock no further transition, so the start and the completion it was armed for
    /// are DISCARDED rather than materialised later over the cancellation. The status message
    /// that status carries is stamped with it, the way every terminal status an export task
    /// reaches carries its own. Only a task that [`Self::is_active`] has an export left to
    /// cancel — "The task must be in the PENDING or RUNNING state."
    pub fn cancel(&mut self, now_ms: i64) {
        self.status_code = EXPORT_TASK_STATUS_CANCELLED.to_string();
        self.status_message = Some(EXPORT_TASK_CANCELLED_STATUS_MESSAGE.to_string());
        self.completion_time_ms = Some(now_ms);
        self.running_due_ms = None;
        self.completion_due_ms = None;
    }

    /// Materialise the status changes the clock owes this task: a pending export whose start
    /// instant has passed is `RUNNING`, and a running one whose completion instant has passed
    /// is `COMPLETED` — each stamped AT that instant and not at the time something happened
    /// to look. A task the clock finishes carries the status message that terminal status
    /// carries, the way every status an export task settles at does.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        if let Some(due) = self.running_due_ms {
            if due <= now_ms {
                self.status_code = EXPORT_TASK_STATUS_RUNNING.to_string();
                self.running_due_ms = None;
            }
        }
        if self.running_due_ms.is_some() {
            return;
        }
        if let Some(due) = self.completion_due_ms {
            if due <= now_ms {
                self.status_code = EXPORT_TASK_STATUS_COMPLETED.to_string();
                // The status this transition puts the task in carries its own message, so it
                // is stamped WITH the status, exactly as the cancel transition stamps the one
                // its status carries.
                self.status_message = Some(EXPORT_TASK_COMPLETED_STATUS_MESSAGE.to_string());
                self.completion_time_ms = Some(due);
                self.completion_due_ms = None;
            }
        }
    }

    /// This task as the model's `ExportTask` structure, which is what `DescribeExportTasks`
    /// reports.
    pub fn reported(&self) -> crate::ops::types::ExportTask {
        crate::ops::types::ExportTask {
            task_id: Some(self.task_id.clone()),
            task_name: self.task_name.clone(),
            log_group_name: Some(self.log_group_name.clone()),
            from: Some(self.from_ms),
            to: Some(self.to_ms),
            destination: Some(self.destination.clone()),
            destination_prefix: Some(self.destination_prefix.clone()),
            status: Some(crate::ops::types::ExportTaskStatus {
                code: Some(self.status_code.clone()),
                message: self.status_message.clone(),
            }),
            execution_info: Some(crate::ops::types::ExportTaskExecutionInfo {
                creation_time: Some(self.creation_time_ms),
                completion_time: self.completion_time_ms,
            }),
        }
    }
}

/// The id `CreateExportTask` gives the task it creates — "The ID of the export task" —
/// minted from the store's monotonic export-task counter so it is unique for the life of the
/// store and DERIVED FROM STORED STATE.
///
/// CloudWatch Logs renders one as a canonical hyphenated UUID, so it is minted through the
/// shared [`mint_uuid_from_seq`] every UUID-shaped id of this service comes from.
pub fn mint_export_task_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// What identifies one vended-log delivery resource in this store: the Region it lives in,
/// and the name — or the id — it is known by within that Region.
///
/// A delivery source, a delivery destination and a delivery are REGIONAL resources: each is
/// created through one Region's endpoint, its ARN carries that Region
/// ([`delivery_source_arn`], [`delivery_destination_arn`], [`delivery_arn`]), and the
/// uniqueness each operation documents for a name ("This name must be unique for all delivery
/// sources in your account") holds within that Region — so the same name in two Regions is two
/// separate resources, and an endpoint resolves only the resources of its own Region. The
/// Region comes first in the ordering, so one Region's resources enumerate in name order and a
/// reporting operation's page token stays a position in that order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegionalKey {
    /// The Region the resource was created in — the one the creating request's credential
    /// scope named ([`request_region`]).
    pub region: String,
    /// What identifies the resource within that Region: a delivery source's or destination's
    /// name, or a delivery's id.
    pub name: String,
}

impl RegionalKey {
    /// The key of the resource `name` names in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

/// One delivery — "a connection between a logical delivery source and a logical delivery
/// destination" — as it is stored. Every member of the model's `Delivery` shape is
/// reported from this record, so `DescribeDeliveries` answers from state alone.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    /// `Delivery$id` — "The unique ID that identifies this delivery in your account."
    /// Also the name half of the key the store holds this delivery under ([`RegionalKey`]).
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

/// The id `CreateDelivery` gives the delivery it creates — "The unique ID that identifies
/// this delivery in your account" — minted from the store's monotonic delivery counter so it
/// is unique for the life of the store and DERIVED FROM STORED STATE, including after the
/// delivery holding an id has been deleted.
///
/// AWS documents no format for `Delivery$id` beyond its own constraints (len 1..64, pattern
/// `^[0-9A-Za-z]+$`), so the counter is rendered as a fixed-width hex string, the way
/// [`mint_revision_id`] renders the revision counter.
pub fn mint_delivery_id(seq: u64) -> String {
    format!("{seq:016x}")
}

/// THE EMULATOR'S OWN CEILING on the deliveries this store holds, across every Region, not a
/// quota cited from AWS: each delivery keeps its configuration and tag set in memory for as
/// long as the process runs and nothing expires one, so `CreateDelivery` refuses to create more
/// than this many with the `ServiceQuotaExceededException` it models ("This request exceeds a
/// service quota") rather than growing without bound on request.
pub const MAX_DELIVERIES: usize = 1_000;

/// The refusal CloudWatch Logs answers a delivery operation with when the `id` it names is
/// not one this account holds. Like [`DELIVERY_SOURCE_NOT_FOUND_MESSAGE`], the sentence states
/// that the requested delivery is not in the account and interpolates no id, so every id the
/// account does not hold is refused identically.
pub const DELIVERY_NOT_FOUND_MESSAGE: &str = "Requested Delivery does not exist in this account.";

/// The refusal CloudWatch Logs answers a delivery operation with when the
/// `deliverySourceName` it names is not one this account holds. Like
/// [`DELIVERY_DESTINATION_NOT_FOUND_MESSAGE`], the sentence states that the requested delivery
/// source is not in the account and interpolates no name, so every name the account does not
/// hold is refused identically.
pub const DELIVERY_SOURCE_NOT_FOUND_MESSAGE: &str =
    "Requested Delivery Source does not exist in this account.";

/// The refusal CloudWatch Logs answers the delete of a delivery-pipeline resource with while a
/// delivery still references it — "You can't delete a delivery destination if any current
/// deliveries are associated with it", and the same for a delivery source.
///
/// The sentence is FIXED for that error class apart from the KIND of resource the delete
/// addresses, exactly as [`resource_not_found_message`] is: it interpolates neither the
/// resource's name nor the id of any delivery, and it directs the caller to enumerate the
/// deliveries itself (`DescribeDeliveries`, which the operation's documentation points at)
/// because ANY number of deliveries can reference the resource — a wording that named one of
/// them would promise a caller that deleting that one unblocks the delete.
pub fn delivery_resource_in_use_message(resource_kind: &str) -> String {
    format!(
        "Could not delete this {resource_kind} as it is currently in use. Please delete any \
         Deliveries that reference this resource and try again."
    )
}

/// The resource kind [`delivery_resource_in_use_message`] names for a delivery destination,
/// spelled the way this service's prose spells the resource.
pub const DELIVERY_DESTINATION_RESOURCE_KIND: &str = "Delivery Destination";

/// The resource kind [`delivery_resource_in_use_message`] names for a delivery source.
pub const DELIVERY_SOURCE_RESOURCE_KIND: &str = "Delivery Source";

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

/// The `DeliveryDestinationType` of a TRACE delivery destination: the one value of the
/// model's `DeliveryDestinationType` that `PutDeliveryDestination` documents as needing no
/// `deliveryDestinationConfiguration` ("deliveryDestinationConfiguration is required for
/// CloudWatch Logs, Amazon S3, Firehose log delivery destinations and not required for X-Ray
/// trace delivery destinations"), and the one kind `roleArn` is supported for ("This
/// parameter is supported only for X-Ray trace delivery destinations").
///
/// Spelled as the model's enum spells it
/// ([`crate::ops::types::DELIVERY_DESTINATION_TYPE_VALUES`]), never a string typed here.
pub const TRACE_DELIVERY_DESTINATION_TYPE: &str = "XRAY";

/// The ARN service namespace of CloudWatch Logs itself, the vendor whose
/// `destinationResourceArn` must name a log group.
const CLOUDWATCH_LOGS_VENDOR: &str = "logs";

/// THE EMULATOR'S OWN CEILING on the delivery destinations this store holds, across every
/// Region, not a quota cited from AWS: each destination keeps its configuration and tag set in
/// memory for as long as the process runs and nothing expires one, so `PutDeliveryDestination`
/// refuses to CREATE more than this many with the `ServiceQuotaExceededException` it models
/// ("This request exceeds a service quota") rather than growing without bound on request. A put
/// that UPDATES a destination already stored is never refused by it: it adds nothing.
pub const MAX_DELIVERY_DESTINATIONS: usize = 1_000;

/// Judge a `DeliveryDestinationConfiguration$destinationResourceArn`, answering the
/// `DeliveryDestinationType` of the resource it names or the message to refuse it with.
///
/// The resources a delivery destination can represent are the ones the member documents —
/// "That Amazon Web Services destination can be a log group in CloudWatch Logs, an Amazon S3
/// bucket, or a delivery stream in Firehose" — plus the X-Ray trace destination the model's
/// `DeliveryDestinationType` adds, which is exactly
/// [`DELIVERY_DESTINATION_VENDOR_TYPES`]. A CloudWatch Logs ARN qualifies only when it names
/// a LOG GROUP: a log stream or any other `logs` resource is not a destination logs can be
/// delivered to.
///
/// Whether that resource exists is a question this service cannot answer for another
/// service's bucket or stream, and the operation documents no existence requirement, so it
/// is not asked here.
pub fn check_destination_resource_arn(arn: &str) -> Result<&'static str, String> {
    if let Some(message) = unparsable_arn_message("destinationResourceArn", arn) {
        return Err(message);
    }
    let Some(destination_type) = delivery_destination_type_for_arn(arn) else {
        return Err(format!(
            "destinationResourceArn must name a resource of one of: [{}]",
            DELIVERY_DESTINATION_VENDOR_TYPES
                .iter()
                .map(|(vendor, _)| *vendor)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    let names_a_cloudwatch_logs_resource = arn_service_and_resource(arn)
        .is_some_and(|(vendor, _resource)| vendor == CLOUDWATCH_LOGS_VENDOR);
    if names_a_cloudwatch_logs_resource && !is_log_group_arn(arn) {
        return Err(
            "destinationResourceArn for a CloudWatch Logs delivery destination must name a \
             log group"
                .to_string(),
        );
    }
    Ok(destination_type)
}

/// One delivery destination — "a logical object that represents the actual delivery
/// destination", the Amazon Web Services resource logs are sent to — as it is stored. Every
/// member of the model's `DeliveryDestination` shape is reported from this record, so
/// `GetDeliveryDestination` answers from state alone.
#[derive(Debug, Clone)]
pub struct DeliveryDestinationRecord {
    /// `DeliveryDestination$name` — "The name of this delivery destination." Also the name
    /// half of the key the store holds it under ([`RegionalKey`]), because the name "must be
    /// unique for all delivery destinations in your account".
    pub name: String,
    /// `DeliveryDestination$arn` — "The Amazon Resource Name (ARN) that uniquely identifies
    /// this delivery destination", as [`delivery_destination_arn`] builds it.
    pub arn: String,
    /// `DeliveryDestination$deliveryDestinationType` — "Displays whether this delivery
    /// destination is CloudWatch Logs, Amazon S3, Firehose, or X-Ray". `None` only for a
    /// destination created with neither a configuration nor a type, which no accepted put
    /// produces.
    pub delivery_destination_type: Option<String>,
    /// `DeliveryDestination$outputFormat` — "The format of the logs that are sent to this
    /// delivery destination."
    pub output_format: Option<String>,
    /// `DeliveryDestinationConfiguration$destinationResourceArn` — "The ARN of the Amazon
    /// Web Services destination that this delivery destination represents." `None` for a
    /// trace delivery destination, the one kind that needs no configuration.
    pub destination_resource_arn: Option<String>,
    /// `DeliveryDestination$roleArn` — "The ARN of the IAM role that CloudWatch Logs assumes
    /// to deliver to this delivery destination. This field is present only for X-Ray trace
    /// delivery destinations that were created with a role."
    pub role_arn: Option<String>,
    /// `DeliveryDestination$tags` — "The tags that have been assigned to this delivery
    /// destination." A `BTreeMap` so the stored order is stable; `Tags` is `@length(min: 1)`,
    /// so a destination carrying none reports no member at all.
    pub tags: BTreeMap<String, String>,
    /// `Policy$deliveryDestinationPolicy` — "The contents of the delivery destination
    /// policy", the IAM policy `PutDeliveryDestinationPolicy` assigns to this destination,
    /// `GetDeliveryDestinationPolicy` reports and `DeleteDeliveryDestinationPolicy` takes back
    /// off. `None` for a destination no policy has been assigned to, which is what makes a get
    /// on one report no policy at all rather than refuse.
    ///
    /// It is NOT one of the "delivery destination parameters" a later put overwrites: the
    /// policy is assigned by its own operation, so a put that updates the destination's
    /// parameters leaves it where it is.
    ///
    /// Held as the document the assignment read rather than as the bytes the caller rendered:
    /// the whitespace between its JSON tokens is dropped on the way in
    /// ([`service_common::json_document::minified`]), so every reader of this field reports the
    /// same text whichever way the caller spaced it.
    pub policy: Option<String>,
}

impl DeliveryDestinationRecord {
    /// A brand-new delivery destination of `destination_type`, with its ARN DERIVED rather
    /// than taken from the caller: the ARN identifies the logical object this service just
    /// created, so it is built from the name and the request's Region.
    ///
    /// The optional members a put may name — `outputFormat`, the configuration's
    /// `destinationResourceArn`, `roleArn`, `tags` — are left at their "the request named
    /// none" value and set on the returned record by the caller that has them.
    pub fn new(region: &str, name: &str, destination_type: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            arn: delivery_destination_arn(region, name),
            delivery_destination_type: destination_type.map(ToString::to_string),
            output_format: None,
            destination_resource_arn: None,
            role_arn: None,
            tags: BTreeMap::new(),
            policy: None,
        }
    }

    /// Whether this destination is a TRACE delivery destination
    /// ([`TRACE_DELIVERY_DESTINATION_TYPE`]) — the one kind that carries no destination
    /// configuration and the one kind `roleArn` applies to.
    pub fn is_trace_destination(&self) -> bool {
        self.delivery_destination_type.as_deref() == Some(TRACE_DELIVERY_DESTINATION_TYPE)
    }

    /// This destination as the model's `DeliveryDestination` structure, which is what
    /// `PutDeliveryDestination`, `GetDeliveryDestination` and `DescribeDeliveryDestinations`
    /// all report, so one record renders the same way through any of them.
    pub fn reported(&self) -> crate::ops::types::DeliveryDestination {
        crate::ops::types::DeliveryDestination {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            delivery_destination_type: self.delivery_destination_type.clone(),
            output_format: self.output_format.clone(),
            delivery_destination_configuration: self.destination_resource_arn.clone().map(
                |destination_resource_arn| crate::ops::types::DeliveryDestinationConfiguration {
                    destination_resource_arn,
                },
            ),
            role_arn: self.role_arn.clone(),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        }
    }
}

/// A delivery destination's ARN, the form `DeliveryDestination$arn` ("The Amazon Resource
/// Name (ARN) that uniquely identifies this delivery destination") carries: a
/// `delivery-destination` resource of this service, identified by the destination's own
/// name.
pub fn delivery_destination_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("delivery-destination:{name}"))
}

/// THE EMULATOR'S OWN CEILING on the delivery sources this store holds, across every Region,
/// not a quota cited from AWS: each source keeps its configuration and tag set in memory for as
/// long as the process runs and nothing expires one, so `PutDeliverySource` refuses to CREATE
/// more than this many with the `ServiceQuotaExceededException` it models ("This request
/// exceeds a service quota") rather than growing without bound on request. A put that UPDATES a
/// source already stored is never refused by it: it adds nothing.
pub const MAX_DELIVERY_SOURCES: usize = 1_000;

/// One delivery source — "a logical object that represents the resource that is actually
/// sending the logs" — as it is stored. Every member of the model's `DeliverySource` shape is
/// reported from this record, so `GetDeliverySource` answers from state alone.
#[derive(Debug, Clone)]
pub struct DeliverySourceRecord {
    /// `DeliverySource$name` — "The unique name of the delivery source." Also the name half
    /// of the key the store holds it under ([`RegionalKey`]), because the name "must be unique
    /// for all delivery sources in your account".
    pub name: String,
    /// `DeliverySource$arn` — "The Amazon Resource Name (ARN) that uniquely identifies this
    /// delivery source", as [`delivery_source_arn`] builds it.
    pub arn: String,
    /// `DeliverySource$resourceArns` — "This array contains the ARN of the Amazon Web Services
    /// resource that sends logs and is represented by this delivery source. Currently, only
    /// one ARN can be in the array."
    pub resource_arns: Vec<String>,
    /// `DeliverySource$service` — "The Amazon Web Services service that is sending logs",
    /// DERIVED from the resource that sends them ([`delivery_source_service`]) rather than
    /// taken from the caller, who never states it: `PutDeliverySource` has no `service`
    /// member. `None` for a resource ARN naming no service namespace.
    pub service: Option<String>,
    /// `DeliverySource$logType` — "The type of log that the source is sending."
    pub log_type: String,
    /// `DeliverySource$deliverySourceConfiguration` — "The map of key-value pairs that
    /// configure the delivery source." A `BTreeMap` so the stored order is stable; the
    /// modelled map has no minimum length, so a source configured with none reports no member
    /// at all.
    pub source_configuration: BTreeMap<String, String>,
    /// `DeliverySource$tags` — "The tags that have been assigned to this delivery source." A
    /// `BTreeMap` so the stored order is stable; `Tags` is `@length(min: 1)`, so a source
    /// carrying none reports no member at all.
    pub tags: BTreeMap<String, String>,
}

impl DeliverySourceRecord {
    /// A brand-new delivery source for one resource, with its ARN and its `service` DERIVED
    /// rather than taken from the caller: the ARN identifies the logical object this service
    /// just created, and the service sending the logs is a fact about the resource named.
    ///
    /// The optional members a put may name — the configuration map and `tags` — are left at
    /// their "the request named none" value and set on the returned record by the caller that
    /// has them.
    pub fn new(region: &str, name: &str, resource_arn: &str, log_type: &str) -> Self {
        Self {
            name: name.to_string(),
            arn: delivery_source_arn(region, name),
            resource_arns: vec![resource_arn.to_string()],
            service: delivery_source_service(resource_arn).map(ToString::to_string),
            log_type: log_type.to_string(),
            source_configuration: BTreeMap::new(),
            tags: BTreeMap::new(),
        }
    }

    /// This source as the model's `DeliverySource` structure, which is what
    /// `PutDeliverySource`, `GetDeliverySource` and `DescribeDeliverySources` all report, so
    /// one record renders the same way through any of them.
    ///
    /// `status` and `statusReason` are left unreported: the model documents both as "defined
    /// for selective log types", and nothing in this store makes the resource behind a source
    /// disappear (the one `statusReason` the model enumerates says the resource "has been
    /// deleted"), so there is no state to report them from.
    pub fn reported(&self) -> crate::ops::types::DeliverySource {
        crate::ops::types::DeliverySource {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            resource_arns: Some(self.resource_arns.clone()),
            service: self.service.clone(),
            log_type: Some(self.log_type.clone()),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            delivery_source_configuration: (!self.source_configuration.is_empty()).then(|| {
                self.source_configuration
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            status: None,
            status_reason: None,
        }
    }
}

/// A delivery source's ARN, the form `DeliverySource$arn` ("The Amazon Resource Name (ARN)
/// that uniquely identifies this delivery source") carries: a `delivery-source` resource of
/// this service, identified by the source's own name.
pub fn delivery_source_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("delivery-source:{name}"))
}

/// The Amazon Web Services service that sends a delivery source's logs: the service
/// namespace of the resource ARN the source represents, or `None` for a value that is not an
/// ARN or names no namespace.
///
/// `DeliverySource$service` is "The Amazon Web Services service that is sending logs" and no
/// input member states it, so it is read off `resourceArn` — "The ARN of the Amazon Web
/// Services resource that is generating and sending logs" — exactly as a delivery reads its
/// destination's type off the destination ARN ([`delivery_destination_type_for_arn`]).
pub fn delivery_source_service(resource_arn: &str) -> Option<&str> {
    arn_service_and_resource(resource_arn)
        .map(|(service, _resource)| service)
        .filter(|service| !service.is_empty())
}

/// The `logType` of a delivery source that sends TRACES rather than log records, as the
/// `logType` member's own catalogue spells it ("For Amazon Bedrock AgentCore Runtime, the
/// valid values are `APPLICATION_LOGS`, `USAGE_LOGS`, and `TRACES`"). Source: the `logType`
/// member of `PutDeliverySourceRequest`
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
///
/// It is the one log type delivered to an X-Ray TRACE delivery destination
/// ([`TRACE_DELIVERY_DESTINATION_TYPE`]) rather than to a destination that receives log
/// records.
pub const TRACE_LOG_TYPE: &str = "TRACES";

/// The `DeliveryDestinationType` of a delivery destination that is a log group of this
/// service, as the model's enum spells it (the `logs` vendor of
/// [`DELIVERY_DESTINATION_VENDOR_TYPES`]).
pub const CWL_DELIVERY_DESTINATION_TYPE: &str = "CWL";

/// The `DeliveryDestinationType` of a delivery destination that is an Amazon S3 bucket, as
/// the model's enum spells it (the `s3` vendor of [`DELIVERY_DESTINATION_VENDOR_TYPES`]).
pub const S3_DELIVERY_DESTINATION_TYPE: &str = "S3";

/// The one `OutputFormat` that carries a whole record on its own, needing neither a field
/// order nor a delimiter to be read back. Spelled as the model's `OutputFormat` enum spells
/// it ([`crate::ops::types::OUTPUT_FORMAT_VALUES`]).
pub const STRUCTURED_OUTPUT_FORMAT: &str = "json";

/// The one `OutputFormat` that is a columnar FILE layout rather than a per-record encoding,
/// so it applies only where a delivery writes objects. Spelled as the model's `OutputFormat`
/// enum spells it ([`crate::ops::types::OUTPUT_FORMAT_VALUES`]).
pub const COLUMNAR_OUTPUT_FORMAT: &str = "parquet";

/// The `OutputFormat`s whose records are field values joined by a delimiter — the formats
/// `fieldDelimiter` is about ("The field delimiter that is used between record fields when
/// the final output format of a delivery is in Plain, W3C, or Raw format"). Each is spelled
/// as the model's `OutputFormat` enum spells it
/// ([`crate::ops::types::OUTPUT_FORMAT_VALUES`]).
pub const DELIMITED_OUTPUT_FORMATS: [&str; 3] = ["plain", "raw", "w3c"];

/// The service-managed catalogue of vended-log delivery configuration templates, which
/// `DescribeConfigurationTemplates` reports: the resources that can send vended logs, the log
/// types each of them sends, the record schema of each log type, and how a template's
/// packaging is derived from the destination type.
pub mod vended_log_templates;

pub use vended_log_templates::{
    configuration_template_key, configuration_template_output_formats, configuration_templates,
    ConfigurationTemplateSource, CONFIGURATION_TEMPLATE_SOURCES,
};

/// The log types a delivery source over `resource_arn` may send, or `None` when this service
/// publishes none for the resource's service.
///
/// A `logType` "defines the type of log that the source is sending", and which values are legal
/// is a property of the SENDING SERVICE rather than of the member: the `logType` member's own
/// catalogue lists them per service ("For CloudFront, the valid value is ACCESS_LOGS"), which is
/// the same catalogue [`CONFIGURATION_TEMPLATE_SOURCES`] is built from, keyed by the service
/// namespace of the resource's ARN ([`delivery_source_service`]). A resource whose service that
/// catalogue does not name publishes no list here, so no log type can be judged against it.
///
/// One service sends the log types of EVERY row the catalogue holds for it — a service has one
/// row per kind of resource, and a log type one of its resources sends is one that service
/// sends — listed in the order the catalogue states them and without repeating a type two of
/// its rows share.
pub fn delivery_source_log_types(resource_arn: &str) -> Option<Vec<&'static str>> {
    let service = delivery_source_service(resource_arn)?;
    let mut published: Vec<&'static str> = Vec::new();
    for source in CONFIGURATION_TEMPLATE_SOURCES
        .iter()
        .filter(|source| source.service == service)
    {
        for entry in source.log_types {
            if !published.contains(&entry.log_type) {
                published.push(entry.log_type);
            }
        }
    }
    (!published.is_empty()).then_some(published)
}

/// The refusal for a `logType` the resource named by `resource_arn` does not send
/// ([`delivery_source_log_types`]), or `None` when it is one of them — or when this service
/// publishes no list for that resource's service, which is no basis to refuse one.
///
/// The fixed text states that the provided log type is not valid and then the options that ARE
/// supported, which is the one variable part: the log types the named resource's service sends,
/// in the order the catalogue lists them.
pub fn unsupported_log_type_message(resource_arn: &str, log_type: &str) -> Option<String> {
    let supported = delivery_source_log_types(resource_arn)?;
    (!supported.contains(&log_type)).then(|| {
        format!(
            "Provided LogType is not valid. Supported options are [{}].",
            supported.join(", ")
        )
    })
}

/// The ARN service namespaces of the GLOBAL log senders: a resource of one of these lives in
/// its partition rather than in a Region, which is why an ARN naming one carries an EMPTY
/// Region segment, and each is reached through a single endpoint per partition rather than one
/// per Region.
///
/// Source: these are the senders the `logType` member's own catalogue names that are global —
/// "For CloudFront, the valid value is ACCESS_LOGS", "For Route 53 Global Resolver, the valid
/// value is GLOBAL_RESOLVER_LOGS", "For Amazon Web Services Shield Advanced, the valid value
/// is FLOW_LOGS"
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`) — and the
/// documented ARN form of each carries no Region
/// (`arn:aws:cloudfront::<account>:distribution/<id>`, `arn:aws:route53:::hostedzone/<id>`,
/// `arn:aws:shield::<account>:protection/<id>`). A global vendor the catalogue does not name
/// as a sender is left out rather than extrapolated: some global services publish their one
/// endpoint in a Region other than the partition's usual global endpoint, so each would need
/// its own.
///
/// A Region-less ARN is NOT by itself the mark of a global resource — an Amazon S3 bucket ARN
/// (`arn:aws:s3:::<bucket>`) carries no Region either, and a bucket is regional — so this is a
/// list of namespaces rather than a test on the ARN's Region segment.
pub const GLOBAL_SERVICE_VENDORS: [&str; 3] = ["cloudfront", "route53", "shield"];

/// The one Region each partition's global senders above are reached through, keyed by the
/// partition segment of an ARN.
///
/// Source: the global endpoint each partition publishes for these services — the commercial
/// partition's in US East (N. Virginia), the China partitions' in Beijing, the AWS GovCloud
/// (US) partition's in US-Gov-West. A partition this does not name pins no Region, so nothing
/// is refused for it.
pub const PARTITION_GLOBAL_REGIONS: [(&str, &str); 3] = [
    ("aws", "us-east-1"),
    ("aws-cn", "cn-north-1"),
    ("aws-us-gov", "us-gov-west-1"),
];

/// The partition segment of an ARN (`arn:<partition>:...`), or `None` for a value that is not
/// an ARN at all. One split of an ARN's fields serves the whole crate ([`arn_parts`]).
fn arn_partition(arn: &str) -> Option<&str> {
    arn_parts(arn).map(|(partition, _service, _account, _resource)| partition)
}

/// The Region a delivery source for `resource_arn` can be created in, or `None` when the
/// resource it names pins none.
///
/// A resource of a global service ([`GLOBAL_SERVICE_VENDORS`]) is served by one endpoint for
/// the whole partition ([`PARTITION_GLOBAL_REGIONS`]), so that endpoint's Region is the only
/// one that can hold a delivery source for it. Everything else — a regional resource, or one
/// of a partition with no global endpoint named here — is left unpinned.
pub fn delivery_source_required_region(resource_arn: &str) -> Option<&'static str> {
    let (vendor, _resource) = arn_service_and_resource(resource_arn)?;
    if !GLOBAL_SERVICE_VENDORS.contains(&vendor) {
        return None;
    }
    let partition = arn_partition(resource_arn)?;
    PARTITION_GLOBAL_REGIONS
        .iter()
        .find(|(name, _region)| *name == partition)
        .map(|(_name, region)| *region)
}

/// The refusal for a delivery source whose resource is reached through another Region than the
/// one this request was made in ([`delivery_source_required_region`]), or `None` when the
/// request's Region can hold it.
///
/// What the caller is told is that the ACTION is not served for this kind of log in the Region
/// the request reached: the message names the action and that Region, and the literal word for
/// the log-type parameter rather than its value. Nothing else is interpolated — not the
/// resource, not the Region it is reached through, and not the constraint-violation prefix this
/// service puts on a parameter that fails a modelled constraint, which this refusal is not.
pub fn unsupported_log_type_region_message(
    operation: &str,
    resource_arn: &str,
    request_region: &str,
) -> Option<String> {
    let required = delivery_source_required_region(resource_arn)?;
    (required != request_region).then(|| {
        format!("{operation} action is not supported for this LogType in region {request_region}")
    })
}

/// The number of logical destinations one account holds: CloudWatch Logs documents a quota
/// of 500 destinations per account per Region ("Destinations: 500 per account per Region",
/// CloudWatch Logs quotas), so a put that would create a 501st destination is refused. The
/// ceiling also bounds this store, whose keys are caller-chosen (`DestinationName` is
/// `@length(min: 1, max: 512)`, so distinct names are effectively unlimited) and whose
/// records stay resident for as long as the process runs: a put that UPDATES a destination
/// already stored is never refused by it, because it adds nothing.
pub const MAX_DESTINATIONS: usize = 500;

/// One logical destination — "a cross-account destination that receives subscription log
/// events", which `PutDestination` creates or updates and `DescribeDestinations` reports —
/// as it is stored. Every member of the model's `Destination` shape is reported from this
/// record, so `DescribeDestinations` answers from state alone.
#[derive(Debug, Clone)]
pub struct DestinationRecord {
    /// `Destination$destinationName` — "The name of the destination." Also the key the
    /// store holds this destination under: it is the only thing `DeleteDestination` and
    /// `PutDestinationPolicy` identify one by.
    pub destination_name: String,
    /// `Destination$arn` — "The ARN of this destination", as [`destination_arn`] builds it.
    pub arn: String,
    /// `Destination$targetArn` — "The Amazon Resource Name (ARN) of the physical target
    /// where the log events are delivered (for example, a Kinesis stream)."
    pub target_arn: String,
    /// `Destination$roleArn` — "A role for impersonation, used when delivering log events
    /// to the target."
    pub role_arn: String,
    /// `Destination$accessPolicy` — "An IAM policy document that governs which Amazon Web
    /// Services accounts can create subscription filters against this destination." `None`
    /// until `PutDestinationPolicy` sets one: "By default, PutDestination does not set any
    /// access policy with the destination [...] the destination owner must call
    /// PutDestinationPolicy after PutDestination."
    pub access_policy: Option<String>,
    /// `Destination$creationTime` — "The creation time of the destination, expressed as the
    /// number of milliseconds after Jan 1, 1970 00:00:00 UTC." Stamped when the destination
    /// is created and kept across later puts, which UPDATE that destination rather than
    /// create one.
    pub creation_time_ms: i64,
    /// The tag set `PutDestination$tags` assigns to the destination — "An optional list of
    /// key-value pairs to associate with the resource" — and the one every tagging
    /// operation reads and writes through
    /// [`ComAmazonawsCloudwatchlogsLogs20140328State::resource_tags`]. A `BTreeMap` so the
    /// stored order is stable.
    pub tags: BTreeMap<String, String>,
}

impl DestinationRecord {
    /// A brand-new destination, with its ARN DERIVED from the request's Region and its own
    /// name rather than taken from the caller, and no access policy: "By default,
    /// PutDestination does not set any access policy with the destination."
    pub fn new(
        region: &str,
        destination_name: &str,
        target_arn: &str,
        role_arn: &str,
        creation_time_ms: i64,
    ) -> Self {
        Self {
            destination_name: destination_name.to_string(),
            arn: destination_arn(region, destination_name),
            target_arn: target_arn.to_string(),
            role_arn: role_arn.to_string(),
            access_policy: None,
            creation_time_ms,
            tags: BTreeMap::new(),
        }
    }

    /// This destination as the model's `Destination` structure, which is what
    /// `PutDestination` and `DescribeDestinations` both report.
    pub fn reported(&self) -> crate::ops::types::Destination {
        crate::ops::types::Destination {
            destination_name: Some(self.destination_name.clone()),
            target_arn: Some(self.target_arn.clone()),
            role_arn: Some(self.role_arn.clone()),
            access_policy: self.access_policy.clone(),
            arn: Some(self.arn.clone()),
            creation_time: Some(self.creation_time_ms),
        }
    }
}

/// A logical destination's ARN, the form `TagResource$resourceArn` documents for one ("The
/// ARN format of a destination is
/// arn:aws:logs:Region:account-id:destination:destination-name"): a `destination` resource
/// of this service, identified by the destination's own name.
pub fn destination_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("{DESTINATION_ARN_PREFIX}{name}"))
}

/// AWS's refusal for a `PutDestination` whose `roleArn` or `targetArn` is not an ARN: the
/// service parses the destination's two ARNs together and reports the failure of either
/// without naming which one failed.
pub const DESTINATION_ARN_PARSE_FAILED: &str = "Error parsing ARN from given role or target";

/// The size of the access policy one destination can carry:
/// `PutDestinationPolicy$accessPolicy` is documented "This can be up to 5120 bytes", a ceiling
/// the model's `AccessPolicy` shape does not express (`@length` min 1 only), so the handler
/// holds the document to it. It bounds this store too, since the document is kept verbatim on
/// the destination record.
pub const MAX_ACCESS_POLICY_BYTES: usize = 5_120;

/// The IAM condition key an access policy names an ORGANIZATION by, which is what
/// `PutDestinationPolicy$forceUpdate` ("grant permission to an organization ID instead of
/// granting permission to individual Amazon Web Services accounts") distinguishes a policy by.
const ORGANIZATION_PRINCIPAL_CONDITION_KEY: &str = "aws:PrincipalOrgID";

/// Whether an access policy grants permission to an organization rather than to individual
/// accounts, read off the document itself: a policy names the organization through the
/// [`ORGANIZATION_PRINCIPAL_CONDITION_KEY`] condition key.
///
/// The key is matched without regard to case, because IAM condition keys are
/// case-insensitive, and wherever in the document it appears, because a condition can sit
/// under any condition operator of any statement. The walk carries its own stack rather than
/// recursing, so a deeply nested document costs heap instead of call frames. A document that
/// is not JSON at all grants nothing: the handler refuses it before asking.
pub fn access_policy_grants_organization(document: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(document) else {
        return false;
    };
    let mut pending = vec![parsed];
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::Object(members) => {
                for (key, nested) in members {
                    if key.eq_ignore_ascii_case(ORGANIZATION_PRINCIPAL_CONDITION_KEY) {
                        return true;
                    }
                    pending.push(nested);
                }
            }
            serde_json::Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    false
}

/// The refusal CloudWatch Logs answers an `accessPolicy` its IAM policy parser will not
/// take: a document that is not an IAM policy document at all, and one whose statements do
/// not identify the principals they grant to. The text is FIXED — it carries nothing from
/// the request — so every such refusal reads identically.
pub const ACCESS_POLICY_PARSE_FAILED: &str =
    "Error occurred while parsing accessPolicy. Please check if the accessPolicy has been \
     constructed correctly using IAM grammar.";

/// The principal value that names EVERY principal rather than one: the IAM wildcard.
const WILDCARD_PRINCIPAL: &str = "*";

/// Whether an access policy ALLOWS every principal — a statement that allows the wildcard
/// principal and narrows it with no condition.
///
/// An access policy "authorizes cross-account users to deliver their log events to the
/// associated destination", so its statements have to identify who is authorized: either a
/// concrete principal, or the wildcard narrowed by a condition, which is how a grant to an
/// organization names its members (`PutDestinationPolicy$forceUpdate`, the
/// [`ORGANIZATION_PRINCIPAL_CONDITION_KEY`] form). A statement allowing the wildcard with
/// no condition at all identifies nobody, and the document is refused.
///
/// `Principal` is read in both of its IAM forms — the bare string and the map from
/// principal type to one value or a list of them — and only an `Allow` statement is
/// examined, since a `Deny` over the wildcard restricts rather than grants. A document
/// that is not JSON allows nothing: the handler refuses it before asking.
pub fn access_policy_allows_every_principal(document: &str) -> bool {
    let Ok(serde_json::Value::Object(members)) =
        serde_json::from_str::<serde_json::Value>(document)
    else {
        return false;
    };
    // "Statement" carries either one statement or a list of them.
    let statements: Vec<&serde_json::Value> = match members.get("Statement") {
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(statement) => vec![statement],
        None => Vec::new(),
    };
    statements.into_iter().any(|statement| {
        let allows = statement
            .get("Effect")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|effect| effect.eq_ignore_ascii_case("Allow"));
        let narrowed = statement
            .get("Condition")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|condition| !condition.is_empty());
        allows && !narrowed && names_wildcard_principal(statement.get("Principal"))
    })
}

/// Whether a statement's `Principal` is the wildcard, in either of the forms IAM writes it:
/// the bare string, or a map from principal type to the wildcard (alone or among a list).
fn names_wildcard_principal(principal: Option<&serde_json::Value>) -> bool {
    let is_wildcard =
        |value: &serde_json::Value| value.as_str() == Some(WILDCARD_PRINCIPAL);
    match principal {
        Some(serde_json::Value::Object(by_type)) => by_type.values().any(|named| match named {
            serde_json::Value::Array(items) => items.iter().any(is_wildcard),
            value => is_wildcard(value),
        }),
        Some(value) => is_wildcard(value),
        None => false,
    }
}

/// The IAM action registering a subscription filter is authorized as: this service's action
/// namespace (`logs`, its `arnNamespace`) and the operation's own name. It is the action a
/// destination's access policy has to authorize, since one "is used to authorize claims to
/// register a subscription filter against a given destination" (`PutDestinationPolicy`).
pub const PUT_SUBSCRIPTION_FILTER_ACTION: &str = "logs:PutSubscriptionFilter";

/// AWS's refusal for a call nothing authorizes: the account the request was signed for, the
/// action it asked for and the resource it asked for. Every variable part comes from the
/// request and from this store — the caller's account id, the fully qualified action and the
/// resource's own ARN — so the sentence states the rule rather than one call's values.
pub fn not_authorized_message(account_id: &str, action: &str, resource_arn: &str) -> String {
    format!(
        "User with accountId: {account_id} is not authorized to perform: {action} on \
         resource: {resource_arn}"
    )
}

/// Whether the calling principal may take `action` on the logical destination `destination`.
///
/// The destination's ACCESS POLICY is the only thing that authorizes one: "Through an access
/// policy, a destination controls what is written to it. By default, PutDestination does not
/// set any access policy with the destination, which means a cross-account user cannot call
/// PutSubscriptionFilter against this destination. To enable this, the destination owner must
/// call PutDestinationPolicy after PutDestination" (`PutDestination`). So a destination
/// carrying NO policy authorizes nobody — not even the account that owns it, which is what
/// AWS answers such a call with — and a destination carrying one is decided by the tree's
/// single evaluation of IAM documents ([`service_common::iam_authorization`]).
///
/// The destination's OWN account also reaches it through its identity-based policies, which
/// is why an attached policy naming only another account does not revoke the owner; an
/// explicit `Deny` about this request still refuses it, as IAM evaluates one.
pub fn destination_authorizes(destination: &DestinationRecord, action: &str) -> bool {
    let Some(document) = destination.access_policy.as_deref() else {
        return false;
    };
    // Every spelling an access policy can name the caller by: the principal this emulator's
    // credential speaks for, and the account that principal belongs to, which IAM lets a
    // resource policy name in place of the principal itself.
    let caller = ::service_common::caller_principal::ambient_user_arn();
    let mut principals = vec![caller.clone()];
    principals.extend(::service_common::iam_authorization::account_principals(
        &caller,
    ));
    let stated = ::service_common::iam_authorization::resource_decision(
        document,
        &::service_common::iam_authorization::Request::new(action, &destination.arn, &principals),
    );
    let by_identity = if arn_account_id(&destination.arn) == arn_account_id(&caller) {
        ::service_common::iam_authorization::Decision::Allowed
    } else {
        ::service_common::iam_authorization::Decision::NotStated
    };
    ::service_common::iam_authorization::is_allowed(by_identity, stated)
}

/// The refusal a request that would register a subscription filter against the destination
/// `destination_arn` names is answered with, or `None` when the destination authorizes it.
///
/// `None` as well for an ARN that names no logical destination THIS account holds: a
/// physical destination (a stream, a delivery stream, a function) carries no access policy,
/// and a destination of another account is a resource whose policy this store does not hold
/// — a refusal has to be grounded in a policy that was read, so neither is refused here.
pub fn subscription_destination_refusal(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    destination_arn: &str,
) -> Option<String> {
    let destination = store.destination_by_arn(destination_arn)?;
    (!destination_authorizes(destination, PUT_SUBSCRIPTION_FILTER_ACTION)).then(|| {
        not_authorized_message(
            ::engine::emulator_account_id(),
            PUT_SUBSCRIPTION_FILTER_ACTION,
            destination_arn,
        )
    })
}

/// The refusal CloudWatch Logs answers a `PutDestinationPolicy` that would replace a
/// destination's account-granting access policy with one granting an organization without the
/// `forceUpdate` affirmation that the senders' subscription filters have already been updated.
/// The message names the parameter the caller has to set, so one request carries the whole
/// rule; it says it per CLASS of update rather than per request value, so every such put is
/// refused identically.
pub const DESTINATION_POLICY_FORCE_UPDATE_REQUIRED: &str =
    "forceUpdate must be true to update an existing destination policy to grant permission to \
     an organization ID instead of individual accounts";

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
    /// The status a PAUSED detector returns to when a later call restarts it, and how much
    /// of the span to its next status was still outstanding when it was paused — so a
    /// detector paused mid-lifecycle resumes that lifecycle instead of jumping ahead of it
    /// or starting it over. `None` for a detector no call has paused.
    pub resume_status: Option<String>,
    /// What was left of [`AnomalyDetectorRecord::status_due_ms`] when this detector was
    /// paused, re-armed against the clock by the call that restarts it. `None` when the
    /// status it was paused in is one the clock owed nothing after.
    pub resume_due_in_ms: Option<i64>,
    /// The anomalies this detector has REPORTED, keyed by `anomalyId`: the set
    /// `ListAnomalies` enumerates and every suppression request resolves its
    /// `anomalyId` / `patternId` against.
    ///
    /// An anomaly is stored as the modelled `Anomaly` shape itself rather than a parallel
    /// record, so every member its reader reports round-trips without a field-by-field copy
    /// that can silently drop one; the suppression members are filled in by the reader from
    /// [`AnomalyDetectorRecord::suppressions`], and the detector's own ARN and log groups
    /// from the record around it.
    ///
    /// A detector reports an anomaly from its own pattern-recognition pass over the log
    /// groups it watches — "The anomaly detector uses pattern recognition to find anomalies,
    /// which are unusual log events" (`CreateLogAnomalyDetector`) — so no REQUEST adds one,
    /// and an id outside this map names an anomaly this detector has not found.
    pub anomalies: BTreeMap<String, crate::ops::types::Anomaly>,
    /// The suppressions placed on this detector's findings, keyed by the `anomalyId` or
    /// `patternId` suppressed. `UpdateAnomaly` writes one here and removes it again when a
    /// later call ends the suppression.
    pub suppressions: BTreeMap<String, FindingSuppression>,
}

/// One suppression `UpdateAnomaly` placed on a finding of a detector: "Use this operation to
/// suppress anomaly detection for a specified anomaly or pattern."
#[derive(Debug, Clone)]
pub struct FindingSuppression {
    /// The `SuppressionType` the call named — `INFINITE` or `LIMITED` — or `None` when it
    /// named none and the suppression lasts until a later call ends it.
    pub suppression_type: Option<String>,
    /// When the suppression was placed, in epoch milliseconds (`Anomaly$suppressedDate`).
    pub suppressed_at_ms: i64,
    /// When a `LIMITED` suppression runs out, in epoch milliseconds
    /// (`Anomaly$suppressedUntil`). `None` for one that does not run out.
    pub suppressed_until_ms: Option<i64>,
    /// `UpdateAnomaly$baseline` — "Set this to true to prevent CloudWatch Logs from
    /// displaying this behavior as an anomaly in the future."
    pub baseline: bool,
}

/// How long a `suppressionPeriod` lasts, in milliseconds: its `value` in the
/// `suppressionUnit` beside it (`SuppressionUnit` is `SECONDS | MINUTES | HOURS`). A period
/// naming no unit is taken as the value in seconds, the smallest unit the enum carries.
pub fn suppression_period_ms(period: &crate::ops::types::SuppressionPeriod) -> i64 {
    let unit_ms = match period.suppression_unit.as_deref() {
        Some("MINUTES") => 60 * 1_000,
        Some("HOURS") => 60 * 60 * 1_000,
        _ => 1_000,
    };
    i64::from(period.value).saturating_mul(unit_ms)
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
            // Nothing has paused a detector the create just stored.
            resume_status: None,
            resume_due_in_ms: None,
            // A detector that has not run a pass over its log groups yet has reported
            // nothing, so it has no finding to suppress either.
            anomalies: BTreeMap::new(),
            suppressions: BTreeMap::new(),
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

    /// Whether this detector has reported the anomaly `anomaly_id` names.
    pub fn reports_anomaly(&self, anomaly_id: &str) -> bool {
        self.anomalies.contains_key(anomaly_id)
    }

    /// Whether this detector has reported an anomaly of the pattern `pattern_id` names.
    pub fn reports_pattern(&self, pattern_id: &str) -> bool {
        self.anomalies
            .values()
            .any(|anomaly| anomaly.pattern_id == pattern_id)
    }

    /// The suppression in force over `anomaly` at `now_ms`, and whether it was placed on the
    /// anomaly's PATTERN rather than on the anomaly itself.
    ///
    /// "If you suppress a pattern, CloudWatch Logs won't report any anomalies related to that
    /// pattern" (`UpdateAnomaly`), so a pattern-level suppression covers every anomaly of
    /// that pattern; a suppression placed on the anomaly itself is the more specific one and
    /// wins. A `LIMITED` suppression stops being in force once the clock passes the instant
    /// it was due to run out, which is DERIVED here rather than written by a sweep.
    pub fn suppression_in_force(
        &self,
        anomaly: &crate::ops::types::Anomaly,
        now_ms: i64,
    ) -> Option<(&FindingSuppression, bool)> {
        let in_force = |suppression: &FindingSuppression| {
            suppression
                .suppressed_until_ms
                .is_none_or(|until| now_ms < until)
        };
        if let Some(suppression) = self.suppressions.get(&anomaly.anomaly_id).filter(|s| in_force(s))
        {
            return Some((suppression, false));
        }
        self.suppressions
            .get(&anomaly.pattern_id)
            .filter(|s| in_force(s))
            .map(|suppression| (suppression, true))
    }

    /// This detector's anomalies as `ListAnomalies` reports them, in `anomalyId` order: the
    /// stored shape, with the detector's own ARN and log groups and the suppression members
    /// the store keeps beside it.
    ///
    /// `state` is DERIVED here from the suppression in force at `now_ms` rather than read
    /// off the stored anomaly: "Indicates the current state of this anomaly. If it is still
    /// being treated as an anomaly, the value is Active. If you have suppressed this anomaly
    /// by using the UpdateAnomaly operation, the value is Suppressed. If this behavior is now
    /// considered to be normal, the value is Baseline." (`Anomaly$state`) — so a suppression
    /// placed as baseline behavior reports [`ANOMALY_STATE_BASELINE`], any other suppression
    /// [`ANOMALY_STATE_SUPPRESSED`], and an anomaly whose suppression a later call ended or
    /// the clock ran out of reports [`ANOMALY_STATE_ACTIVE`] again.
    pub fn reported_anomalies(&self, now_ms: i64) -> Vec<crate::ops::types::Anomaly> {
        self.anomalies
            .values()
            .map(|anomaly| {
                let mut reported = anomaly.clone();
                reported.anomaly_detector_arn = self.arn.clone();
                reported.log_group_arn_list = self.log_group_arn_list.clone();
                match self.suppression_in_force(anomaly, now_ms) {
                    Some((suppression, pattern_level)) => {
                        reported.suppressed = Some(true);
                        reported.suppressed_date = suppression.suppressed_at_ms;
                        reported.suppressed_until = suppression.suppressed_until_ms.unwrap_or(0);
                        reported.is_pattern_level_suppression = Some(pattern_level);
                        reported.state = if suppression.baseline {
                            ANOMALY_STATE_BASELINE.to_string()
                        } else {
                            ANOMALY_STATE_SUPPRESSED.to_string()
                        };
                    }
                    None => {
                        reported.suppressed = Some(false);
                        reported.suppressed_date = 0;
                        reported.suppressed_until = 0;
                        reported.is_pattern_level_suppression = Some(false);
                        reported.state = ANOMALY_STATE_ACTIVE.to_string();
                    }
                }
                reported
            })
            .collect()
    }

    /// Suppress the finding `id` names, or end its suppression.
    ///
    /// "If you have previously used this operation to suppress detection of a pattern or
    /// anomaly, you can use it again to cause CloudWatch Logs to end the suppression. To do
    /// this, use this operation and specify the anomaly or pattern to stop suppressing, and
    /// omit the suppressionType and suppressionPeriod parameters." (`UpdateAnomaly`) — so a
    /// call naming neither member removes the suppression the finding carries, and any other
    /// call places one. "If you specify INFINITE, any value for suppressionPeriod is
    /// ignored."
    pub fn suppress_finding(
        &mut self,
        id: &str,
        suppression_type: Option<&str>,
        suppression_period: Option<&crate::ops::types::SuppressionPeriod>,
        baseline: bool,
        now_ms: i64,
    ) {
        if suppression_type.is_none() && suppression_period.is_none() {
            self.suppressions.remove(id);
            return;
        }
        let suppressed_until_ms = (suppression_type != Some(SUPPRESSION_TYPE_INFINITE))
            .then(|| {
                suppression_period
                    .map(|period| now_ms.saturating_add(suppression_period_ms(period)))
            })
            .flatten();
        self.suppressions.insert(
            id.to_string(),
            FindingSuppression {
                suppression_type: suppression_type.map(ToString::to_string),
                suppressed_at_ms: now_ms,
                suppressed_until_ms,
                baseline,
            },
        );
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

    /// Pause this detector, or restart a paused one: "Use this parameter to pause or restart
    /// the anomaly detector." (`UpdateLogAnomalyDetector$enabled`)
    ///
    /// Pausing takes the detector out of the status the clock had it in and into
    /// [`ANOMALY_DETECTOR_STATUS_PAUSED`], which only another call leaves. What the clock
    /// still owed it is kept rather than discarded, so restarting resumes the lifecycle from
    /// where the pause interrupted it — the remaining span is re-armed against the clock at
    /// the restart — instead of jumping a detector that was still initializing straight to
    /// analyzing. A detector the clock had already carried to its last status restarts into
    /// that status with nothing further owed.
    ///
    /// Asking for the state the detector is already in changes no status: pausing a paused
    /// detector keeps the one pause, and restarting a running one leaves the clock's own
    /// progression alone.
    pub fn set_enabled(&mut self, enabled: bool, now_ms: i64) {
        // What the clock already made due happened before this call, so it is materialised
        // first: the status this call pauses, and the span it keeps, are the ones in force at
        // the instant the call arrived.
        self.materialize_due_transitions(now_ms);
        let paused = self.status == ANOMALY_DETECTOR_STATUS_PAUSED;
        match (enabled, paused) {
            (false, false) => {
                self.resume_status = Some(self.status.clone());
                self.resume_due_in_ms = self
                    .status_due_ms
                    .map(|due| due.saturating_sub(now_ms).max(0));
                self.status = ANOMALY_DETECTOR_STATUS_PAUSED.to_string();
                self.status_due_ms = None;
            }
            (true, true) => {
                self.status = self
                    .resume_status
                    .take()
                    .unwrap_or_else(|| ANOMALY_DETECTOR_STATUS_ANALYZING.to_string());
                self.status_due_ms = self
                    .resume_due_in_ms
                    .take()
                    .map(|remaining| now_ms.saturating_add(remaining));
            }
            _ => {}
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

/// The resource-segment prefix of a lookup table's ARN. AWS documents no format for
/// `LookupTable$lookupTableArn`, so it is built the way every other named CloudWatch Logs
/// resource of this service builds one — a resource type then the name that identifies the
/// resource, as `log-group:<name>` and `delivery-destination:<name>` are built.
pub const LOOKUP_TABLE_ARN_PREFIX: &str = "lookup-table:";

/// A lookup table's ARN, derived from the name that identifies it ("The name must be
/// unique within your account and Region"), so the same table always answers under the
/// same ARN and no two tables share one.
pub fn lookup_table_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("{LOOKUP_TABLE_ARN_PREFIX}{name}"))
}

/// The lookup-table name an ARN names, or `None` for anything that is not a lookup
/// table's ARN of this service — which is how every operation that identifies a table by
/// `lookupTableArn` reaches the record the store holds under its name.
pub fn lookup_table_name_from_arn(arn: &str) -> Option<&str> {
    let (service, resource) = arn_service_and_resource(arn)?;
    if service != "logs" {
        return None;
    }
    resource
        .strip_prefix(LOOKUP_TABLE_ARN_PREFIX)
        .filter(|name| !name.is_empty())
}

/// The refusal CloudWatch Logs answers when the lookup table an operation names is not one
/// the account holds. Unlike the generic [`resource_not_found_message`] sentence, a
/// lookup-table refusal states WHICH table and WHICH account was looked in, so a caller
/// whose identifier crossed accounts or Regions can see what was searched for and where.
///
/// Both variable parts come from the request and from state: `name` is the table the request
/// identified and `account_id` the account that identifier names.
pub fn lookup_table_not_found_message(name: &str, account_id: &str) -> String {
    format!("Lookup table '{name}' not found in account '{account_id}'")
}

/// The refusal a lookup-table CREATE answers a name the account already holds a table under
/// with. The sentence is the mirror of [`lookup_table_not_found_message`] — the same two
/// variable parts, the table the request named and the account it was looked for in — because
/// both state what was searched for and where.
pub fn lookup_table_already_exists_message(name: &str, account_id: &str) -> String {
    format!("Lookup table '{name}' already exists in account '{account_id}'")
}

/// The refusal a prefix-filtered lookup-table enumeration answers when the account holds no
/// table whose name starts with that prefix: the request names a set of tables, so a prefix
/// matching none names no resource at all.
///
/// Both variable parts are the request's and the caller's: the account the tables were looked
/// for in and the prefix they were looked for under.
pub fn lookup_tables_not_found_message(account_id: &str, prefix: &str) -> String {
    format!("No lookup tables found for accountId '{account_id}' with prefix: '{prefix}'")
}

/// The same refusal for an operation that identifies the table by its `lookupTableArn`: that
/// ARN carries both the table's name and the account the table would belong to.
///
/// An ARN that carries no lookup-table name identifies no table to name in the refusal — the
/// case [`UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE`] distinguishes elsewhere in this service —
/// so it is refused with the generic [`resource_not_found_message`] sentence rather than with
/// an empty name.
pub fn lookup_table_not_found_message_for_arn(arn: &str) -> String {
    match (lookup_table_name_from_arn(arn), arn_account_id(arn)) {
        (Some(name), Some(account_id)) => lookup_table_not_found_message(name, account_id),
        _ => resource_not_found_message("lookup table"),
    }
}

/// What populates a lookup table: "You must specify either tableBody or queryId, but not
/// both."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupTableSource {
    /// The CSV content the request carried, verbatim.
    TableBody(String),
    /// The id of the completed or cancelled query whose results populate the table.
    QueryId(String),
}

/// The refusal for a request that names BOTH of a lookup table's content sources. Like the
/// log-group selector pair ([`LOG_GROUP_SELECTORS_EXCLUSIVE`]) this is checked ACROSS two
/// members, which no single member's constraint can express, so it is answered with the
/// `ValidationException` the lookup-table operations model rather than with their
/// `InvalidParameterException`.
pub const LOOKUP_TABLE_SOURCE_EXCLUSIVE: &str =
    "tableBody and queryId are mutually exclusive parameters.";

/// The refusal for a lookup-table write that names neither content source: a table is
/// created "by uploading CSV data or from CloudWatch Logs query results" and updated by
/// "replacing all of its content with new CSV data or CloudWatch Logs query results", so a
/// create or an update naming neither names nothing to fill the table from. See
/// [`LOOKUP_TABLE_SOURCE_EXCLUSIVE`] for the error shape.
pub const LOOKUP_TABLE_SOURCE_REQUIRED: &str = "Either tableBody or queryId must be provided.";

/// Resolve what a lookup-table request populates the table from. `Ok(None)` is a request
/// that names NEITHER source, which neither a create nor an update may be: both state "You
/// must specify either tableBody or queryId, but not both", and an update is itself a
/// content replacement ("Updates an existing lookup table by replacing all of its content
/// with new CSV data or CloudWatch Logs query results"). The `Err` carries the message to
/// refuse a request naming BOTH with; a caller refuses `Ok(None)` with
/// [`LOOKUP_TABLE_SOURCE_REQUIRED`].
pub fn resolve_lookup_table_source(
    table_body: Option<&str>,
    query_id: Option<&str>,
) -> Result<Option<LookupTableSource>, String> {
    match (table_body, query_id) {
        (Some(body), None) => Ok(Some(LookupTableSource::TableBody(body.to_string()))),
        (None, Some(query_id)) => Ok(Some(LookupTableSource::QueryId(query_id.to_string()))),
        (Some(_), Some(_)) => Err(LOOKUP_TABLE_SOURCE_EXCLUSIVE.to_string()),
        (None, None) => Ok(None),
    }
}

/// The refusal for a `queryId` that names no query whose results could populate a lookup
/// table — "The ID of a completed or cancelled CloudWatch Logs query". The message is built
/// from the request so a caller can attribute it to the id it sent.
///
/// `StartQuery` is still a scaffold stub, so this account holds no query at all and EVERY
/// `queryId` names one that is not there. Implementing that operation is what gives this
/// service a query store to resolve an id against.
pub fn query_not_found_message(query_id: &str) -> String {
    format!("Query with id [{query_id}] is not a completed or cancelled query.")
}

/// The refusal `StopQuery` answers for a query that has already ended: it "stops a CloudWatch
/// Logs Insights query that is in progress", and "if the query has already ended, the
/// operation returns an error indicating that the specified query is not running". The
/// message is built from the request, in the same `Query with id [..]` form every refusal of
/// this service that names a query by its id takes, so a caller stopping queries in a loop
/// can attribute the refusal to one id.
pub fn query_not_running_message(query_id: &str) -> String {
    format!("Query with id [{query_id}] is not running.")
}

/// What a lookup table's CSV body describes: the column names of its header row and how
/// many data rows follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBodyShape {
    /// `LookupTable$tableFields` — "The column headers from the first row of the CSV file."
    pub fields: Vec<String>,
    /// `LookupTable$recordsCount` — "The number of data rows in the lookup table, excluding
    /// the header row."
    pub records_count: i64,
}

/// The refusal for a CSV header row that names the same column name more than once: such a
/// row names no column SET, so a data row's values name no column unambiguously and the
/// table has no columns to be read by. The text is fixed for the error class — it
/// interpolates neither the repeated name, nor the member, nor a position.
pub const DUPLICATE_TABLE_FIELDS_MESSAGE: &str = "CSV header contains duplicate field names";

/// Why a lookup table's CSV body builds no table, and therefore which modelled error the
/// operation that was handed it refuses the request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableBodyRejection {
    /// Content this service could not read as the CSV the `tableBody` member documents: the
    /// body names the defect and the member it was carried in, so it is refused with the
    /// `InvalidParameterException` every lookup-table write models for a member it will not
    /// accept.
    Unreadable(String),
    /// A body read as CSV whose header row declares one column name twice. The rule is about
    /// the header row as a WHOLE rather than about a value a single member's constraint could
    /// express, so — like the cross-member source rules of the same operations — it is
    /// refused with the `ValidationException` they model, carrying
    /// [`DUPLICATE_TABLE_FIELDS_MESSAGE`].
    DuplicateFields,
}

/// Read a lookup table's CSV body: "the CSV content must include a header row with column
/// names". The body is parsed by the `csv` crate rather than split by hand, so a quoted
/// field carrying a comma, a newline or an escaped quote is one field, as CSV defines it.
///
/// The `Err` says why the body builds no table: a header row naming no column at all, a data
/// row whose field count disagrees with the header's, and a header row naming the same column
/// twice are each content a table cannot be read by — the last one through its own
/// [`TableBodyRejection::DuplicateFields`], because a repeated column name is refused as a
/// validation failure rather than as an unreadable member.
pub fn parse_table_body(body: &str) -> Result<TableBodyShape, TableBodyRejection> {
    let unreadable = |error: csv::Error| TableBodyRejection::Unreadable(table_body_message(&error));
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        // A row must carry exactly the header's fields: a lookup table's rows are keyed by
        // its column names, so a short or long row names no value for some column.
        .flexible(false)
        .from_reader(body.as_bytes());
    let fields: Vec<String> = reader
        .headers()
        .map_err(unreadable)?
        .iter()
        .map(str::to_string)
        .collect();
    if fields.iter().all(|field| field.trim().is_empty()) {
        return Err(TableBodyRejection::Unreadable(
            "tableBody must begin with a header row naming the table's columns".to_string(),
        ));
    }
    // "The first row must be a header row with column names": the names it declares are the
    // columns the table is read by ([`LookupTableRecord::table_fields`]), so a name declared
    // twice leaves every value under it ambiguous and the body names no table. Compared
    // through a set, so a wide header costs one pass and not one per column.
    let mut seen = std::collections::BTreeSet::new();
    if !fields.iter().all(|field| seen.insert(field.as_str())) {
        return Err(TableBodyRejection::DuplicateFields);
    }
    let mut records_count: i64 = 0;
    for record in reader.records() {
        record.map_err(unreadable)?;
        records_count = records_count.saturating_add(1);
    }
    Ok(TableBodyShape {
        fields,
        records_count,
    })
}

/// The refusal for a CSV body the parser would not read, as the defect the parser reported:
/// a row disagreeing with the header names both counts and the row it is at, so a caller
/// can find the row in the body it sent.
fn table_body_message(error: &csv::Error) -> String {
    match error.kind() {
        csv::ErrorKind::UnequalLengths {
            pos,
            expected_len,
            len,
        } => format!(
            "tableBody row {} has {len} fields but the header row names {expected_len}",
            pos.as_ref().map_or(0, csv::Position::line)
        ),
        _ => format!("tableBody is not valid CSV content: {error}"),
    }
}

/// One lookup table — the reference data "you can use [...] to enrich log data in
/// CloudWatch Logs queries" — as it is stored. Every member of the model's `LookupTable`
/// shape is derived from this record, so `GetLookupTable` and `DescribeLookupTables` answer
/// from state alone.
#[derive(Debug, Clone)]
pub struct LookupTableRecord {
    /// `LookupTable$lookupTableName` — "The name of the lookup table". Also the key the
    /// store is keyed by.
    pub name: String,
    /// `LookupTable$lookupTableArn` — the ARN [`lookup_table_arn`] built for this table in
    /// the region the create was signed for.
    pub arn: String,
    /// `LookupTable$description` — "The description of the lookup table", when a call named
    /// one.
    pub description: Option<String>,
    /// The CSV content this table holds, VERBATIM as the call that last wrote it sent it: a
    /// cell's text is the table's data, so nothing re-renders it and no numeric value is
    /// re-formatted on the way in or out. `GetLookupTable$tableBody` reports it.
    pub table_body: String,
    /// `LookupTable$tableFields` — the column names of [`Self::table_body`]'s header row, as
    /// [`parse_table_body`] read them.
    pub table_fields: Vec<String>,
    /// `LookupTable$recordsCount` — the data rows of [`Self::table_body`], excluding its
    /// header row.
    pub records_count: i64,
    /// `LookupTable$kmsKeyId` — "The ARN of the KMS key used to encrypt the lookup table
    /// data, if applicable". `None` when no call named one, which is the Amazon-owned key
    /// `CreateLookupTable` documents as the default ("If you don't specify a key, the data is
    /// encrypted with an Amazon Web Services-owned key") and not a key this emulator invents.
    pub kms_key_id: Option<String>,
    /// `CreateLookupTable$createdAt` — "The time when the lookup table was created", in epoch
    /// milliseconds.
    pub created_at_ms: i64,
    /// `LookupTable$lastUpdatedTime` — "The time when the lookup table was last updated", in
    /// epoch milliseconds. A table nothing has updated since it was created reports the
    /// instant it was created.
    pub last_updated_time_ms: i64,
    /// The tags `CreateLookupTable$tags` assigned to this table. A `BTreeMap` so the stored
    /// order is stable; no `LookupTable` member reports them, so they are read back through
    /// the tagging operations once those reach this resource.
    pub tags: BTreeMap<String, String>,
}

impl LookupTableRecord {
    /// A brand-new lookup table holding `table_body`, whose shape `shape` describes. This is
    /// what `CreateLookupTable` stores; the optional members a create may name —
    /// `description`, `kmsKeyId`, `tags` — are left at their "the request named none" value
    /// and set on the returned record by the caller that has them.
    pub fn new(
        name: &str,
        arn: &str,
        table_body: String,
        shape: TableBodyShape,
        created_at_ms: i64,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            description: None,
            table_body,
            table_fields: shape.fields,
            records_count: shape.records_count,
            kms_key_id: None,
            created_at_ms,
            last_updated_time_ms: created_at_ms,
            tags: BTreeMap::new(),
        }
    }

    /// Replace everything this table holds with `table_body`, whose shape `shape` describes,
    /// as of `updated_at_ms`. This is what `UpdateLookupTable` stores: "This is a full
    /// replacement operation. All existing content is replaced", so the column names and the
    /// record count are re-derived from the new body rather than merged with the old one's,
    /// and the instant of the write becomes the table's last-updated instant. The members a
    /// content replacement does not touch — the name, the ARN, the creation instant, the
    /// description, the KMS key and the tags — are left as they are, for the caller to set
    /// from the members its request named.
    pub fn replace_content(
        &mut self,
        table_body: String,
        shape: TableBodyShape,
        updated_at_ms: i64,
    ) {
        self.table_body = table_body;
        self.table_fields = shape.fields;
        self.records_count = shape.records_count;
        self.last_updated_time_ms = updated_at_ms;
    }

    /// `LookupTable$sizeBytes` — "The size of the lookup table in bytes": the length of the
    /// CSV content this table holds, in the UTF-8 encoding `CreateLookupTable` requires of
    /// it. DERIVED from the body rather than stored beside it, so it cannot disagree with
    /// what the table holds.
    pub fn size_bytes(&self) -> i64 {
        i64::try_from(self.table_body.len()).unwrap_or(i64::MAX)
    }

    /// This table as the model's `LookupTable` structure, which is what
    /// `DescribeLookupTables` reports, so one record renders the same way through any reader
    /// of it. The body itself is not a member of that shape — only `GetLookupTable` reports
    /// it — so a listing carries the table's metadata and not its megabytes.
    pub fn reported(&self) -> crate::ops::types::LookupTable {
        crate::ops::types::LookupTable {
            lookup_table_arn: Some(self.arn.clone()),
            lookup_table_name: Some(self.name.clone()),
            description: self.description.clone(),
            table_fields: Some(self.table_fields.clone()),
            records_count: Some(self.records_count),
            size_bytes: Some(self.size_bytes()),
            last_updated_time: Some(self.last_updated_time_ms),
            kms_key_id: self.kms_key_id.clone(),
        }
    }
}

/// A scheduled query's ARN, the form `ScheduledQuerySummary$scheduledQueryArn` ("The ARN of
/// the scheduled query") carries: a `scheduled-query` resource of this service, identified by
/// the query's own name, which is what makes the ARN and the name interchangeable as the
/// `ScheduledQueryIdentifier` every reader accepts.
///
/// This is also the form CloudWatch mints for the query it manages for a log alarm
/// (`ScheduledQueryConfiguration$QueryARN`), which is why that ARN resolves here too
/// ([`log_alarm_queries`]).
pub fn scheduled_query_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("scheduled-query:{name}"))
}

/// The state a scheduled query a create named none for starts in: `CreateScheduledQuery$state`
/// documents "The initial state of the scheduled query. Valid values are ENABLED and DISABLED.
/// Default is ENABLED."
pub const SCHEDULED_QUERY_STATE_ENABLED: &str = "ENABLED";

/// The `ScheduleType` of a scheduled query a caller created: the schedule is the caller's,
/// stated by its own `scheduleExpression`, so it is the customer-managed one of the two values
/// the model enumerates (`CUSTOMER_MANAGED` | `AWS_MANAGED`) — nothing in this store creates
/// an AWS-managed schedule, and no create member names the type.
pub const SCHEDULE_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// THE EMULATOR'S OWN CEILING on the scheduled queries one account holds, not a quota cited
/// from AWS: each query keeps its configuration in memory for as long as the process runs and
/// nothing expires one, so `CreateScheduledQuery` refuses to create more than this many with
/// the `ServiceQuotaExceededException` it models ("This request exceeds a service quota")
/// rather than growing without bound on request.
pub const MAX_SCHEDULED_QUERIES: usize = 1_000;

/// One scheduled query — the query `CreateScheduledQuery` schedules "at regular intervals" —
/// as it is stored. Every member of `GetScheduledQuery`'s response is reported from this
/// record, so that reader answers from state alone.
#[derive(Debug, Clone)]
pub struct ScheduledQueryRecord {
    /// `name` — "The name of the scheduled query". Also the key the store is keyed by, because
    /// the name "must be unique within your account and region".
    pub name: String,
    /// `scheduledQueryArn` — the ARN [`scheduled_query_arn`] built for this query in the region
    /// the create was signed for.
    pub arn: String,
    /// `description` — "The description of the scheduled query", when a call named one.
    pub description: Option<String>,
    /// `queryLanguage` — "The query language used by the scheduled query" (`CWLI` | `SQL` |
    /// `PPL`). Required of a create, so every stored query carries one.
    pub query_language: String,
    /// `queryString` — "The query string executed by the scheduled query", verbatim as the call
    /// that wrote it sent it.
    pub query_string: String,
    /// `logGroupIdentifiers` — "The log groups queried by the scheduled query", in the order
    /// the call named them. Optional on a create, so a query naming none holds none and reports
    /// no member at all.
    pub log_group_identifiers: Vec<String>,
    /// `scheduleExpression` — "The cron expression that defines when the scheduled query runs".
    /// Required of a create, so every stored query carries one.
    pub schedule_expression: String,
    /// `timezone` — "The timezone used for evaluating the schedule expression", when a call
    /// named one.
    pub timezone: Option<String>,
    /// `startTimeOffset` — "The time offset in seconds that defines the lookback period for the
    /// query", when a call named one.
    pub start_time_offset: Option<i64>,
    /// `endTimeOffset` — "The time offset in seconds that defines the end of the lookback period
    /// for the query", when a call named one.
    pub end_time_offset: Option<i64>,
    /// `destinationConfiguration` — "Configuration for where query results are delivered", as
    /// the call that wrote it stated it.
    pub destination_configuration: Option<crate::ops::types::DestinationConfiguration>,
    /// `state` — "The current state of the scheduled query" (`ENABLED` | `DISABLED`), the one
    /// the create named or [`SCHEDULED_QUERY_STATE_ENABLED`] when it named none.
    pub state: String,
    /// `scheduleType` — "The schedule type of the scheduled query", which for a query a caller
    /// created is [`SCHEDULE_TYPE_CUSTOMER_MANAGED`].
    pub schedule_type: String,
    /// `scheduleStartTime` — "The start time for the scheduled query in Unix epoch format",
    /// when a call named one.
    pub schedule_start_time: Option<i64>,
    /// `scheduleEndTime` — "The end time for the scheduled query in Unix epoch format", when a
    /// call named one.
    pub schedule_end_time: Option<i64>,
    /// `executionRoleArn` — "The ARN of the IAM role used to execute the query and deliver
    /// results". Required of a create, so every stored query carries one.
    pub execution_role_arn: String,
    /// `creationTime` — "The timestamp when the scheduled query was created", in epoch
    /// milliseconds. `None` for a query whose owner records no instant of its creation, which
    /// is a query the SERVICE manages rather than one a create minted
    /// ([`super::state::log_alarm_queries`]).
    pub creation_time_ms: Option<i64>,
    /// `lastUpdatedTime` — "The timestamp when the scheduled query was last updated", in epoch
    /// milliseconds. A query nothing has updated since it was created reports the instant it
    /// was created.
    pub last_updated_time_ms: Option<i64>,
    /// `lastTriggeredTime` — "The timestamp when the scheduled query was last executed", in
    /// epoch milliseconds, and `lastExecutionStatus` — "The status of the most recent execution
    /// of the scheduled query". `None` on a query no execution has been recorded for, which is
    /// every query this store holds until the execution history is implemented: both members
    /// report an execution that happened, so neither is reported for one that has not.
    pub last_triggered_time_ms: Option<i64>,
    /// See [`Self::last_triggered_time_ms`].
    pub last_execution_status: Option<String>,
    /// The tags `CreateScheduledQuery$tags` assigned to this query. A `BTreeMap` so the stored
    /// order is stable; no response member of a scheduled query reports them, so they are read
    /// back through the tagging operations once those reach this resource.
    pub tags: BTreeMap<String, String>,
}

impl ScheduledQueryRecord {
    /// A scheduled query a caller created, from the members a create must name.
    ///
    /// Everything else — the description, the log groups, the timezone, the offsets, the
    /// destination configuration, the schedule window, the state, the tags and the instants —
    /// is left at its "the request named none" value, which for the state is the documented
    /// default ([`SCHEDULED_QUERY_STATE_ENABLED`]), and set on the returned record by the
    /// caller that has them.
    pub fn new(
        name: &str,
        arn: &str,
        query_language: &str,
        query_string: &str,
        schedule_expression: &str,
        execution_role_arn: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            description: None,
            query_language: query_language.to_string(),
            query_string: query_string.to_string(),
            log_group_identifiers: Vec::new(),
            schedule_expression: schedule_expression.to_string(),
            timezone: None,
            start_time_offset: None,
            end_time_offset: None,
            destination_configuration: None,
            state: SCHEDULED_QUERY_STATE_ENABLED.to_string(),
            schedule_type: SCHEDULE_TYPE_CUSTOMER_MANAGED.to_string(),
            schedule_start_time: None,
            schedule_end_time: None,
            execution_role_arn: execution_role_arn.to_string(),
            creation_time_ms: None,
            last_updated_time_ms: None,
            last_triggered_time_ms: None,
            last_execution_status: None,
            tags: BTreeMap::new(),
        }
    }

    /// The scheduled query the SERVICE manages for a log alarm, from the members that alarm's
    /// `ScheduledQueryConfiguration` carries: its schedule type is
    /// [`log_alarm_queries::SCHEDULE_TYPE_AWS_MANAGED`] rather than the caller's, and its query
    /// is in this service's default query language ([`DEFAULT_QUERY_LANGUAGE`]), which the
    /// alarm states no language for.
    ///
    /// `arn` is the ARN the alarm's configuration names the query by
    /// (`ScheduledQueryConfiguration$QueryARN`). The rest is set by the caller, as for
    /// [`Self::new`].
    pub fn managed(
        name: &str,
        arn: String,
        query_string: &str,
        schedule_expression: &str,
        execution_role_arn: &str,
    ) -> Self {
        Self {
            schedule_type: log_alarm_queries::SCHEDULE_TYPE_AWS_MANAGED.to_string(),
            ..Self::new(
                name,
                &arn,
                DEFAULT_QUERY_LANGUAGE,
                query_string,
                schedule_expression,
                execution_role_arn,
            )
        }
    }
}

/// One integration between CloudWatch Logs and another service — the resource
/// `PutIntegration` creates, `GetIntegration` and `ListIntegrations` report and
/// `DeleteIntegration` removes — as it is stored.
///
/// `PutIntegration` is the only call that creates one and "You can't modify an existing
/// integration", so every member here is set once, at create time, from the request's
/// `resourceConfig`; the only thing that moves afterwards is [`Self::status`], which the
/// CLOCK owes ([`Self::status_due_ms`]).
#[derive(Debug, Clone)]
pub struct IntegrationRecord {
    /// `IntegrationSummary$integrationName` — "A name for the integration", which is also
    /// the key the store is keyed by: `GetIntegration` and `DeleteIntegration` identify an
    /// integration by nothing else.
    pub name: String,
    /// `IntegrationSummary$integrationType`, one value of the model's `IntegrationType`
    /// enum ([`INTEGRATION_TYPE_OPENSEARCH`]).
    pub integration_type: String,
    /// `IntegrationSummary$integrationStatus`, one value of the model's `IntegrationStatus`
    /// enum ([`INTEGRATION_STATUS_PROVISIONING`] and its family).
    pub status: String,
    /// The region the create was signed in, so the ARNs this integration's OpenSearch
    /// Service resources are reported under do not move when a later read arrives in
    /// another credential scope.
    pub region: String,
    /// `OpenSearchResourceConfig$kmsKeyArn` — the key "the vended dashboard data" is
    /// encrypted with, when the create named one; `None` leaves "the CloudWatch Logs
    /// default encryption method".
    pub kms_key_arn: Option<String>,
    /// `OpenSearchResourceConfig$dataSourceRoleArn` — "the ARN of an IAM role that
    /// CloudWatch Logs will use to create the integration".
    pub data_source_role_arn: String,
    /// `OpenSearchResourceConfig$dashboardViewerPrincipals` — "the ARNs of IAM roles and
    /// IAM users who you want to grant permission to for viewing the dashboards".
    pub dashboard_viewer_principals: Vec<String>,
    /// The existing OpenSearch Service application the create named, if it named one:
    /// "If you want to use an existing OpenSearch Service application for your integration
    /// [...] specify it here. If you omit this, a new application will be created" — which
    /// is why this is `None` for an integration whose application the setup created, and
    /// [`Self::application_arn`] is what a reader reports either way.
    pub existing_application_arn: Option<String>,
    /// `OpenSearchResourceConfig$retentionDays` — "how many days that you want the data
    /// derived by OpenSearch Service to be retained in the index that the dashboard refers
    /// to", exactly as the create named it.
    pub retention_days: i32,
    /// When this integration was created, in epoch milliseconds. The instant the
    /// provisioning transition is measured from.
    pub created_at_ms: i64,
    /// The position this integration takes in the account's creation order, from the
    /// store's monotonic integration counter: what `ListIntegrations` enumerates by, so
    /// the order it reports does not move when another integration is created or deleted.
    pub creation_seq: u64,
    /// The instant this integration is due to reach its next status, stored rather than
    /// waited for: a reader materialises the transition once the clock is past it
    /// ([`IntegrationRecord::materialize_due_transitions`]). `None` once it is in a status
    /// the clock owes nothing after.
    pub status_due_ms: Option<i64>,
}

impl IntegrationRecord {
    /// A brand-new integration over the configuration the create named: still
    /// provisioning, and due to become [`INTEGRATION_STATUS_ACTIVE`]
    /// [`INTEGRATION_PROVISIONING_DELAY_MS`] after it was created. This is what
    /// `PutIntegration` stores.
    pub fn new(
        region: &str,
        name: &str,
        integration_type: &str,
        config: &crate::ops::types::OpenSearchResourceConfig,
        created_at_ms: i64,
        creation_seq: u64,
    ) -> Self {
        Self {
            name: name.to_string(),
            integration_type: integration_type.to_string(),
            status: INTEGRATION_STATUS_PROVISIONING.to_string(),
            region: region.to_string(),
            kms_key_arn: config.kms_key_arn.clone(),
            data_source_role_arn: config.data_source_role_arn.clone(),
            dashboard_viewer_principals: config.dashboard_viewer_principals.clone(),
            existing_application_arn: config.application_arn.clone(),
            retention_days: config.retention_days,
            created_at_ms,
            creation_seq,
            status_due_ms: Some(created_at_ms.saturating_add(INTEGRATION_PROVISIONING_DELAY_MS)),
        }
    }

    /// Materialise the status change the clock owes this integration: one whose next
    /// status was due before `now_ms` is already in it — the change taken AT the instant it
    /// was due, not at the time something happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        while let Some(due) = self.status_due_ms {
            if due > now_ms {
                return;
            }
            match self.status.as_str() {
                // "After you create an integration, it takes a few minutes to complete.
                // During this time, you'll see the status as PROVISIONING" — so an
                // integration leaves that status on the clock, with nothing further owed
                // after it.
                INTEGRATION_STATUS_PROVISIONING => {
                    self.status = INTEGRATION_STATUS_ACTIVE.to_string();
                    self.status_due_ms = None;
                }
                // Every other status is one the clock owes nothing after.
                _ => self.status_due_ms = None,
            }
        }
    }

    /// This integration as the model's `IntegrationSummary`, which is what
    /// `ListIntegrations` reports, so one record renders the same way through any reader.
    pub fn summary(&self) -> crate::ops::types::IntegrationSummary {
        crate::ops::types::IntegrationSummary {
            integration_name: Some(self.name.clone()),
            integration_type: Some(self.integration_type.clone()),
            integration_status: Some(self.status.clone()),
        }
    }

    /// `OpenSearchDataSource$dataSourceName` — the data source "created as part of the
    /// integration setup", named after the integration it belongs to.
    pub fn data_source_name(&self) -> String {
        self.name.clone()
    }

    /// `OpenSearchApplication$applicationArn` — the existing application the create named,
    /// or the ARN of the one the setup created for it.
    pub fn application_arn(&self) -> String {
        self.existing_application_arn.clone().unwrap_or_else(|| {
            ::engine::emulator_arn(
                OPENSEARCH_SERVICE,
                &self.region,
                &format!("application/{}", self.application_id()),
            )
        })
    }

    /// `OpenSearchApplication$applicationId` — the id the application's ARN carries, which
    /// is the last segment of its resource part for an application the create named, and
    /// the derived id otherwise.
    pub fn application_id(&self) -> String {
        match self
            .existing_application_arn
            .as_deref()
            .and_then(arn_service_and_resource)
            .and_then(|(_, resource)| resource.split('/').next_back())
        {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => format!("{}{}", self.name, APPLICATION_SUFFIX),
        }
    }

    /// `OpenSearchCollection$collectionArn` — the serverless collection the setup created
    /// for this integration, which the dashboards query.
    pub fn collection_arn(&self) -> String {
        ::engine::emulator_arn(
            OPENSEARCH_SERVERLESS_SERVICE,
            &self.region,
            &format!("collection/{}{}", self.name, COLLECTION_SUFFIX),
        )
    }

    /// `OpenSearchWorkspace$workspaceId` — the workspace the setup created for this
    /// integration.
    pub fn workspace_id(&self) -> String {
        format!("{}{}", self.name, WORKSPACE_SUFFIX)
    }

    /// The status every OpenSearch Service resource of this integration is in, derived from
    /// the integration's own status: the setup creates them all together, so until the
    /// integration has finished provisioning none of them is there
    /// ([`OPENSEARCH_RESOURCE_STATUS_NOT_FOUND`]), once it is active they all are, and an
    /// integration that failed reports them in error.
    fn resource_status(&self) -> crate::ops::types::OpenSearchResourceStatus {
        let status = match self.status.as_str() {
            INTEGRATION_STATUS_ACTIVE => OPENSEARCH_RESOURCE_STATUS_ACTIVE,
            INTEGRATION_STATUS_PROVISIONING => OPENSEARCH_RESOURCE_STATUS_NOT_FOUND,
            _ => OPENSEARCH_RESOURCE_STATUS_ERROR,
        };
        crate::ops::types::OpenSearchResourceStatus {
            status: Some(status.to_string()),
            status_message: None,
        }
    }

    /// This integration as the model's `IntegrationDetails` union — "information about
    /// OpenSearch Service resources such as the collection, the workspace, and policies" —
    /// which is what `GetIntegration` reports.
    ///
    /// Every name, id and ARN here is derived from the integration itself, so two reads of
    /// one integration report the same resources. No endpoint is reported: this emulator
    /// stands up no OpenSearch Service collection or application, so there is no host a
    /// client could reach, and `applicationEndpoint` / `collectionEndpoint` are left absent
    /// rather than filled with one nothing answers on.
    pub fn details(&self) -> crate::ops::types::IntegrationDetails {
        let status = self.resource_status();
        crate::ops::types::IntegrationDetails::openSearchIntegrationDetails(
            crate::ops::types::OpenSearchIntegrationDetails {
                data_source: Some(crate::ops::types::OpenSearchDataSource {
                    data_source_name: Some(self.data_source_name()),
                    status: Some(status.clone()),
                }),
                application: Some(crate::ops::types::OpenSearchApplication {
                    application_endpoint: None,
                    application_arn: Some(self.application_arn()),
                    application_id: Some(self.application_id()),
                    status: Some(status.clone()),
                }),
                collection: Some(crate::ops::types::OpenSearchCollection {
                    collection_endpoint: None,
                    collection_arn: Some(self.collection_arn()),
                    status: Some(status.clone()),
                }),
                workspace: Some(crate::ops::types::OpenSearchWorkspace {
                    workspace_id: Some(self.workspace_id()),
                    status: Some(status.clone()),
                }),
                encryption_policy: Some(crate::ops::types::OpenSearchEncryptionPolicy {
                    policy_name: Some(format!("{}{}", self.name, ENCRYPTION_POLICY_SUFFIX)),
                    status: Some(status.clone()),
                }),
                network_policy: Some(crate::ops::types::OpenSearchNetworkPolicy {
                    policy_name: Some(format!("{}{}", self.name, NETWORK_POLICY_SUFFIX)),
                    status: Some(status.clone()),
                }),
                access_policy: Some(crate::ops::types::OpenSearchDataAccessPolicy {
                    policy_name: Some(format!("{}{}", self.name, ACCESS_POLICY_SUFFIX)),
                    status: Some(status.clone()),
                }),
                lifecycle_policy: Some(crate::ops::types::OpenSearchLifecyclePolicy {
                    policy_name: Some(format!("{}{}", self.name, LIFECYCLE_POLICY_SUFFIX)),
                    status: Some(status),
                }),
            },
        )
    }
}

/// One data-source association of an S3 Table Integration as it is stored: the model's
/// `S3TableIntegrationSource` plus the integration it belongs to (the `integrationArn` the
/// associate named, which only `ListSourcesForS3TableIntegration` reads back, as the
/// integration it lists the sources of) and the instant its deletion is due.
///
/// The integration itself is NOT a resource this service holds: `PutIntegration`'s
/// `integrationType` admits only `OPENSEARCH`, so no operation here creates an S3 Table
/// Integration and the ARN the associate names is the only thing identifying one. An
/// association is therefore stored against that ARN as the request gave it, and the
/// family's reader lists the associations whose ARN matches.
#[derive(Debug, Clone)]
pub struct S3TableIntegrationSourceRecord {
    /// `S3TableIntegrationSource$identifier` — "The unique identifier for this data source
    /// association", minted by the associate
    /// ([`mint_s3_table_integration_source_identifier`]) and the key this record is stored
    /// under.
    pub identifier: String,
    /// The `integrationArn` the associate named: "The Amazon Resource Name (ARN) of the S3
    /// Table Integration to associate the data source with", which is what
    /// `ListSourcesForS3TableIntegration` selects an integration's associations by.
    pub integration_arn: String,
    /// `S3TableIntegrationSource$dataSource` — "The data source associated with the S3
    /// Table Integration", exactly as the associate named it. Held as the
    /// [`DataSourceRecord`] every data source of this service is held as; an association's
    /// carries no format, because the request's `DataSource` has no such member.
    pub data_source: DataSourceRecord,
    /// `S3TableIntegrationSource$status` — "The current status of the data source
    /// association", one of the model's `S3TableIntegrationSourceStatus` values.
    pub status: String,
    /// `S3TableIntegrationSource$createdTimeStamp` — "The timestamp when the data source
    /// association was created", in epoch milliseconds.
    pub created_time_stamp_ms: i64,
    /// `S3TableIntegrationSource$parentSourceIdentifier` — "The identifier of the parent
    /// data source for this association". `None` for an association a request made: the
    /// associate names one data source and no parent, so nothing it creates has one.
    pub parent_source_identifier: Option<String>,
    /// The instant this association is due to be GONE, stored rather than waited for: a
    /// reader drops it once the clock is past it
    /// ([`ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_s3_table_integration_source_removals`]).
    /// `None` while the association is still associated, which is also what tells an
    /// associate of the same pair that it names this association rather than a new one.
    pub removal_due_ms: Option<i64>,
}

impl S3TableIntegrationSourceRecord {
    /// A brand-new association of `data_source` with the integration `integration_arn`,
    /// giving query access as of `created_time_stamp_ms`
    /// ([`S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE`]). This is what
    /// `AssociateSourceToS3TableIntegration` stores.
    pub fn new(
        identifier: &str,
        integration_arn: &str,
        data_source: &crate::ops::types::DataSource,
        created_time_stamp_ms: i64,
    ) -> Self {
        Self {
            identifier: identifier.to_string(),
            integration_arn: integration_arn.to_string(),
            data_source: DataSourceRecord {
                name: data_source.name.clone(),
                source_type: data_source.r#type.clone(),
                format: None,
            },
            status: S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE.to_string(),
            created_time_stamp_ms,
            parent_source_identifier: None,
            removal_due_ms: None,
        }
    }

    /// Take this association's query access away and leave its data deletion running, as of
    /// `now_ms`: what `DisassociateSourceFromS3TableIntegration` does. An association whose
    /// deletion is already running keeps the instant it was first due, so a second
    /// disassociate does not postpone it.
    pub fn start_removal(&mut self, now_ms: i64) {
        if self.removal_due_ms.is_some() {
            return;
        }
        self.status = S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS.to_string();
        self.removal_due_ms =
            Some(now_ms.saturating_add(S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS));
    }

    /// Whether the clock has already finished deleting this association's data, so the
    /// association itself is gone as of `now_ms`.
    pub fn removal_due(&self, now_ms: i64) -> bool {
        self.removal_due_ms.is_some_and(|due| due <= now_ms)
    }

    /// This association as the model's `S3TableIntegrationSource`, which is what
    /// `ListSourcesForS3TableIntegration` reports, so one record renders the same way
    /// through any reader.
    ///
    /// No `statusReason` is reported: the member is "additional information about the
    /// status", and both statuses an association of this emulator reaches are already said
    /// by the status itself.
    pub fn source(&self) -> crate::ops::types::S3TableIntegrationSource {
        crate::ops::types::S3TableIntegrationSource {
            identifier: Some(self.identifier.clone()),
            data_source: Some(crate::ops::types::DataSource {
                name: self.data_source.name.clone(),
                r#type: self.data_source.source_type.clone(),
            }),
            status: Some(self.status.clone()),
            status_reason: None,
            created_time_stamp: Some(self.created_time_stamp_ms),
            parent_source_identifier: self.parent_source_identifier.clone(),
        }
    }
}

/// Whether an ARN names an IAM role — what `OpenSearchResourceConfig$dataSourceRoleArn`
/// must be ("Specify the ARN of an IAM role that CloudWatch Logs will use to create the
/// integration"). An IAM ARN's resource part is `role/<path><name>`.
pub fn is_iam_role_arn(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .is_some_and(|(service, resource)| service == IAM_SERVICE && resource.starts_with("role/"))
}

/// Whether an ARN names a principal `OpenSearchResourceConfig$dashboardViewerPrincipals`
/// admits: "the ARNs of IAM roles and IAM users who you want to grant permission to for
/// viewing the dashboards".
pub fn is_dashboard_viewer_principal_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        service == IAM_SERVICE && (resource.starts_with("role/") || resource.starts_with("user/"))
    })
}

/// Whether an ARN names an OpenSearch Service application — what
/// `OpenSearchResourceConfig$applicationArn` must be ("If you want to use an existing
/// OpenSearch Service application for your integration [...] specify it here").
pub fn is_opensearch_application_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        service == OPENSEARCH_SERVICE && resource.starts_with("application/")
    })
}

/// Whether an ARN names a KMS key — what `OpenSearchResourceConfig$kmsKeyArn` must be
/// ("specify the ARN of the KMS key that you want to use"). Unlike a log group's
/// `kmsKeyId`, this member is modelled as an `Arn`, so a bare key id is not one of its
/// values.
pub fn is_kms_key_arn(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .is_some_and(|(service, resource)| service == KMS_SERVICE && !resource.is_empty())
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
            check_field_index_policy_document(document).map(|_| AccountPolicyShape::default())
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
    // to, listed in the one order this service states that enum's values in.
    if let Some(distribution) = string("Distribution") {
        if !crate::ops::types::DISTRIBUTION_VALUES.contains(&distribution) {
            return Err(invalid_value_message(
                "Distribution",
                crate::service::enum_value_set(
                    crate::ops::types::DISTRIBUTION_VALUES.iter().copied(),
                ),
            ));
        }
    }
    Ok(())
}

/// One field index a field index policy document names: the field whose log event values
/// are indexed, and the type the document gives it.
///
/// From `PutIndexPolicy$policyDocument`: "You can use FieldsV2 to specify the type for each
/// field. Supported types are FIELD_INDEX and FACET." A name listed in the plain `Fields`
/// array carries no type of its own, and that array is "The array of field indexes to
/// create", so such a name is a field index ([`FIELDS_ARRAY_INDEX_TYPE`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedField {
    /// The field whose values are indexed — what `FieldIndex$fieldIndexName` reports as
    /// "the string that this field index matches".
    pub name: String,
    /// The field index's type, one of [`crate::ops::types::INDEX_TYPE_VALUES`].
    pub index_type: String,
}

/// The `IndexType` of a field index named in a document's plain `Fields` array, which
/// gives no type: that array is "The array of field indexes to create", so the index it
/// creates is a field index rather than a facet. Spelled as the model's `IndexType` spells
/// it ([`crate::ops::types::INDEX_TYPE_VALUES`]).
pub const FIELDS_ARRAY_INDEX_TYPE: &str = "FIELD_INDEX";

/// Judge a field index policy document, answering the message to refuse it with, or the
/// field indexes it creates — `Fields` in the order the array lists them, then the
/// `FieldsV2` names.
///
/// From `PutAccountPolicy$policyDocument`, which states the same document's rules as
/// `PutIndexPolicy$policyDocument`: the attributes are "Fields The array of field indexes
/// to create" and "FieldsV2 The object of field indexes to create along with it's type";
/// "It must contain at least one field index"; "Supported types are FIELD_INDEX and FACET"
/// (the model's `IndexType` enum); and "Field names within Fields and FieldsV2 must be
/// mutually exclusive."
///
/// The per-policy field quotas are NOT judged here, because they are a log-group-level
/// policy's own ([`check_index_policy_quotas`]).
pub fn check_field_index_policy_document(document: &str) -> Result<Vec<IndexedField>, String> {
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
    let mut typed: Vec<IndexedField> = Vec::new();
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
        typed.push(IndexedField {
            name: name.clone(),
            index_type: index_type.to_string(),
        });
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
    let mut indexed: Vec<IndexedField> = fields
        .into_iter()
        .map(|name| IndexedField {
            name,
            index_type: FIELDS_ARRAY_INDEX_TYPE.to_string(),
        })
        .collect();
    indexed.extend(typed);
    Ok(indexed)
}

/// Why a log-group-level field index policy breaches one of the quotas `PutIndexPolicy`
/// states for "each index policy", carrying the sentence to refuse it with.
///
/// The two breaches are refused differently: too many field indexes is a count of what the
/// policy would create, and an over-long field name is one value of the document being
/// wrong — so the caller maps each onto the error its operation declares for that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexPolicyQuotaBreach {
    /// The document names more than [`MAX_INDEX_POLICY_FIELDS`] field indexes.
    TooManyFields(String),
    /// One field name is longer than [`MAX_INDEX_FIELD_NAME_CHARS`] characters.
    FieldNameTooLong(String),
}

/// Judge the field indexes of a log-group-level field index policy against the quotas
/// `PutIndexPolicy` states for "each index policy": [`MAX_INDEX_POLICY_FIELDS`] fields in
/// the policy, and [`MAX_INDEX_FIELD_NAME_CHARS`] characters in each field name. The names
/// are the ones [`check_field_index_policy_document`] read out of the document.
pub fn check_index_policy_quotas(fields: &[IndexedField]) -> Result<(), IndexPolicyQuotaBreach> {
    if let Some(length) = fields
        .iter()
        .map(|field| field.name.chars().count())
        .find(|length| *length > MAX_INDEX_FIELD_NAME_CHARS)
    {
        return Err(IndexPolicyQuotaBreach::FieldNameTooLong(format!(
            "a field name in policyDocument is {length} characters; each field name can include as many as {MAX_INDEX_FIELD_NAME_CHARS} characters"
        )));
    }
    if fields.len() > MAX_INDEX_POLICY_FIELDS {
        return Err(IndexPolicyQuotaBreach::TooManyFields(format!(
            "policyDocument names {} field indexes; as many as {MAX_INDEX_POLICY_FIELDS} fields can be included in an index policy",
            fields.len()
        )));
    }
    Ok(())
}

/// Judge a `TRANSFORMER_POLICY` document: "A transformer policy must include one JSON
/// block with the array of processors and their configurations." Which processors exist,
/// and how each is configured, is the catalogue `PutTransformer` owns, so what is judged
/// here is that the document carries a non-empty array of processor blocks — written
/// either as that array itself or as an object holding it — whose FIRST block names a
/// parse-type processor, which is the one rule a transformer's pipeline has to satisfy
/// wherever it is configured ([`PARSE_TYPE_PROCESSORS`]).
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
    let leading = processors
        .first()
        .and_then(serde_json::Value::as_object)
        .and_then(|block| block.keys().next())
        .ok_or_else(|| expected.clone())?;
    if !is_parse_type_processor(leading) {
        return Err(transformer_leading_parser_message(leading));
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
        if criteria_conflict(policy_type, &putting, &held_selection) {
            return Err(selection_criteria_overlap_message(&held.policy_name));
        }
    }
    Ok(())
}

/// How CloudWatch Logs refuses a `selectionCriteria` that overlaps the criteria of an
/// account-level policy the account already holds.
///
/// Built FROM the request and the conflicting resource: the refusal names the policy whose
/// criteria the put would overlap, so a caller holding several policies knows which one to
/// narrow without describing them all.
pub fn selection_criteria_overlap_message(held_policy_name: &str) -> String {
    format!(
        "SelectionCriteria is overlapping with an existing policy with policy name: \
         {held_policy_name}"
    )
}

/// Whether two criteria of one policy type conflict. See
/// [`check_account_policy_criteria_overlap`] for the sentence behind each answer.
fn criteria_conflict(
    policy_type: &str,
    putting: &CriteriaSelection<'_>,
    held: &CriteriaSelection<'_>,
) -> bool {
    match (putting, held) {
        (
            CriteriaSelection::DataSource { names, types },
            CriteriaSelection::DataSource {
                names: held_names,
                types: held_types,
            },
        ) => {
            // "no two of them can use the same data source name and type combination"
            let same_name = names.iter().any(|name| held_names.contains(name));
            let same_type = types.iter().any(|source| held_types.contains(source));
            same_name && same_type
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
                    .any(|one| held_selectors.iter().any(|two| one.intersects(*two)));
            }
            match (negated, held_negated) {
                // "When using NOT IN, only one policy with this operator is allowed per
                // account".
                (true, true) => true,
                // "no two of them can have overlapping criteria"
                (false, false) => selectors
                    .iter()
                    .any(|one| held_selectors.iter().any(|two| one.intersects(*two))),
                // One selects the log groups it lists and the other excludes the ones it
                // lists: they do not overlap only when every selected value is completely
                // contained within some excluded value.
                (false, true) => !selectors
                    .iter()
                    .all(|one| held_selectors.iter().any(|two| one.contained_in(*two))),
                (true, false) => !held_selectors
                    .iter()
                    .all(|one| selectors.iter().any(|two| one.contained_in(*two))),
            }
        }
        // One criteria selects log groups and the other a data source name and type: they
        // select along different dimensions, which the field index policy's quota sentences
        // count separately, so neither constrains the other.
        _ => false,
    }
}

/// Whether an account-level policy applies to one log group.
///
/// `PutAccountPolicy$selectionCriteria` is what narrows an account-level policy: "Use this
/// parameter to apply the new policy to a subset of log groups in the account", and a policy
/// put without one is the policy "that applies to all log groups in the account". So a
/// policy holding no criteria selects every group, and one holding criteria selects the
/// groups its filters name: a `LogGroupName` value names the group called that, a
/// `LogGroupNamePrefix` value every group whose name starts with it, and the
/// `DataSourceName` AND `DataSourceType` pair every group with a data source of that name
/// and type. A `NOT IN` list selects every group its values do NOT name, and criteria
/// written with more than one filter are joined by `AND`, so each of them must hold.
///
/// A stored criteria string went through [`check_selection_criteria`] when it was put, so
/// one that no longer parses names no log groups rather than all of them.
pub fn account_policy_selects_log_group(
    policy: &AccountPolicyRecord,
    group: &LogGroupRecord,
) -> bool {
    let Some(criteria) = policy.selection_criteria.as_deref() else {
        return true;
    };
    let Ok(criteria) = check_selection_criteria(&policy.policy_type, criteria) else {
        return false;
    };
    // The one documented form that joins two filters is the `DataSourceName` AND
    // `DataSourceType` pair, and it names ONE data source: a group satisfies that pair only
    // when a single data source of its own satisfies both of its filters. So the pair is ONE
    // conjunct of the criteria rather than two, and any further filter written beside it is
    // still a conjunct of its own.
    let pair = match (
        criteria.clause(SELECTION_FIELD_DATA_SOURCE_NAME),
        criteria.clause(SELECTION_FIELD_DATA_SOURCE_TYPE),
    ) {
        (Some(names), Some(types)) => Some((names, types)),
        _ => None,
    };
    if let Some((names, types)) = pair {
        if !data_source_pair_selects_log_group(names, types, group) {
            return false;
        }
    }
    criteria
        .clauses
        .iter()
        // The two filters of the pair are judged together above, so they are not judged
        // again one at a time here.
        .filter(|clause| pair.is_none() || !is_data_source_selection_field(&clause.field))
        .all(|clause| selection_clause_selects_log_group(clause, group))
}

/// The account-level policy of one type that applies to one log group, or `None` when none
/// of the account's does.
///
/// An account-level policy applies to a log group when its `selectionCriteria` select that
/// group, and a policy put without criteria is the one "that applies to all log groups in
/// the account" ([`account_policy_selects_log_group`]). A group can therefore be selected by
/// both a scoped policy and that account-wide one — `PutAccountPolicy`'s quota sentence
/// admits "either one account-level policy that applies to all log groups in the account, or
/// up to 20 policies that are each scoped to a subset", and its overlap rule ("no two of them
/// can use the same or overlapping log group name prefixes") leaves at most one SCOPED policy
/// selecting any one group. So the policy that applies to the group is the scoped one when it
/// has one, and the account-wide one otherwise.
pub fn applicable_account_policy<'a>(
    account_policies: &'a BTreeMap<(String, String), AccountPolicyRecord>,
    policy_type: &str,
    group: &LogGroupRecord,
) -> Option<&'a AccountPolicyRecord> {
    let applying: Vec<&AccountPolicyRecord> = account_policies
        .values()
        .filter(|policy| policy.policy_type == policy_type)
        .filter(|policy| account_policy_selects_log_group(policy, group))
        .collect();
    applying
        .iter()
        .find(|policy| policy.selection_criteria.is_some())
        .or_else(|| applying.first())
        .copied()
}

/// Whether a log group carries one data source satisfying BOTH filters of a
/// `DataSourceName` AND `DataSourceType` pair; see [`account_policy_selects_log_group`].
fn data_source_pair_selects_log_group(
    names: &SelectionClause,
    types: &SelectionClause,
    group: &LogGroupRecord,
) -> bool {
    group.data_sources.iter().any(|source| {
        let named = names.values.contains(&source.name) != names.negated;
        let typed = source
            .source_type
            .as_ref()
            .is_some_and(|source_type| types.values.contains(source_type))
            != types.negated;
        named && typed
    })
}

/// Whether a `selectionCriteria` filter is one of the two that name a data source
/// ([`SELECTION_FIELD_DATA_SOURCE_NAME`], [`SELECTION_FIELD_DATA_SOURCE_TYPE`]).
fn is_data_source_selection_field(field: &str) -> bool {
    matches!(
        field,
        SELECTION_FIELD_DATA_SOURCE_NAME | SELECTION_FIELD_DATA_SOURCE_TYPE
    )
}

/// Whether one filter of a `selectionCriteria` string selects a log group; see
/// [`account_policy_selects_log_group`] for what each field's values name. A filter on a
/// field that names no property of a log group selects none of them.
fn selection_clause_selects_log_group(clause: &SelectionClause, group: &LogGroupRecord) -> bool {
    let named = match clause.field.as_str() {
        SELECTION_FIELD_LOG_GROUP_NAME => clause.values.contains(&group.name),
        SELECTION_FIELD_LOG_GROUP_NAME_PREFIX => clause
            .values
            .iter()
            .any(|value| group.name.starts_with(value)),
        SELECTION_FIELD_DATA_SOURCE_NAME => group
            .data_sources
            .iter()
            .any(|source| clause.values.contains(&source.name)),
        SELECTION_FIELD_DATA_SOURCE_TYPE => group.data_sources.iter().any(|source| {
            source
                .source_type
                .as_ref()
                .is_some_and(|source_type| clause.values.contains(source_type))
        }),
        _ => return false,
    };
    named != clause.negated
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

/// One log group's own field index policy — the log-group level policy `PutIndexPolicy`
/// stores and `DeleteIndexPolicy` removes — as it is stored.
///
/// A log-group level policy is identified by nothing but the group it applies to ("Responses
/// about log group-level field index policies don't have this field, because those policies
/// don't have names"), so it lives ON the group and a later put replaces this record rather
/// than adding another. The document is kept VERBATIM, exactly the bytes the request
/// carried, the way [`DataProtectionPolicyRecord::policy_document`] is: the service reports
/// a stored policy document back unchanged.
#[derive(Debug, Clone)]
pub struct IndexPolicyRecord {
    /// `policyDocument`, byte-for-byte as the put carried it.
    pub policy_document: String,
    /// "The date and time that this index policy was most recently updated."
    pub last_update_time_ms: i64,
}

/// The account's storage tier policy — what `PutStorageTierPolicy` stores and
/// `GetStorageTierPolicy` reports — as it is stored. One per account: the policy is named
/// by nothing, so a later put replaces this record rather than adding another.
#[derive(Debug, Clone)]
pub struct StorageTierPolicyRecord {
    /// `storageTier`, one of [`crate::ops::types::STORAGE_TIER_VALUES`].
    pub storage_tier: String,
    /// "The time when the storage tier policy was last updated, expressed as the number of
    /// milliseconds after January 1, 1970 00:00:00 UTC" — the instant of the put that
    /// stored this tier.
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
    pub tags: BTreeMap<String, String>,
    /// The group's log streams, keyed by `logStreamName`.
    pub streams: BTreeMap<String, LogStreamRecord>,
    /// The group's subscription filters, keyed by `filterName`. A `BTreeMap` so
    /// `DescribeSubscriptionFilters` enumerates them ASCII-sorted by filter name
    /// without re-sorting, and so a put under a name already stored updates that
    /// filter in place.
    pub subscription_filters: BTreeMap<String, SubscriptionFilterRecord>,
    /// The group's metric filters, keyed by `filterName`. A `BTreeMap` so
    /// `DescribeMetricFilters` enumerates them ASCII-sorted by filter name without
    /// re-sorting, and so a put under a name already stored updates that filter in place.
    /// They live on the group `PutMetricFilter` "associates" them with, so deleting the
    /// group removes its metric filters too.
    pub metric_filters: BTreeMap<String, MetricFilterRecord>,
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
    /// The log-group level field index policy stored on this group, if one has been put:
    /// such a policy is named by nothing but the group, so a group holds at most one and
    /// deleting the group takes it away. `DescribeIndexPolicies` reports this policy for
    /// the group ahead of any account-wide one ([`IndexPolicyRecord`]).
    pub index_policy: Option<IndexPolicyRecord>,
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
            tags: BTreeMap::new(),
            streams: BTreeMap::new(),
            subscription_filters: BTreeMap::new(),
            metric_filters: BTreeMap::new(),
            resource_policy: None,
            transformer: None,
            data_protection_policy: None,
            had_data_protection_policy: false,
            data_sources: Vec::new(),
            syslog_configurations: BTreeMap::new(),
            index_policy: None,
        }
    }

    /// This group's `metricFilterCount` — "The number of metric filters" — which is the
    /// number of metric filters stored on it: `PutMetricFilter` associates one with a log
    /// group and `DeleteMetricFilter` takes it away, so the count is DERIVED from
    /// [`LogGroupRecord::metric_filters`] rather than kept beside it, and no operation can
    /// move the two apart.
    pub fn metric_filter_count(&self) -> i32 {
        i32::try_from(self.metric_filters.len()).unwrap_or(i32::MAX)
    }

    /// The regular expression patterns this group's metric filters carry between them,
    /// EXCLUDING the filter named by `replacing` (a put under a name the group already
    /// holds updates that filter, so the pattern it carried today is replaced rather than
    /// added to). This is the quantity `PutMetricFilter`'s "quota of five regular
    /// expression patterns per log group" bounds.
    pub fn metric_filter_regex_patterns(&self, replacing: &str) -> usize {
        self.metric_filters
            .values()
            .filter(|filter| filter.filter_name != replacing)
            .map(|filter| filter_pattern_regex_count(&filter.filter_pattern))
            .sum()
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
/// Tables integration — [`S3TableIntegrationSourceRecord`] — rather than to a log group),
/// and the data sources AWS reports are the ones
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

/// The `LogGroupNameRegexPattern` expression as the SERVICE states it in a refusal, which
/// is not the trait string the model carries: it is stated UNANCHORED, the form the
/// pattern-rendering rule of `requirement` in `crate::service` states for every shape AWS
/// publishes another form for, and it carries one alternative the model omits — a SINGLE
/// pattern of 3..512 characters, which is the form the service accepts for a pattern naming
/// a whole log group name and which the model's `{3,24}` segments alone would exclude.
///
/// It still includes the per-alternative 24-character maximum of the multi-pattern
/// alternative, which the emulator does not enforce for the same reason the service does
/// not (see [`check_log_group_name_pattern`]).
pub const LOG_GROUP_NAME_REGEX_PATTERN: &str = concat!(
    r"(\^?[\.\-_\/#A-Za-z0-9]{3,512})",
    r"|(\^?[\.\-_\/#A-Za-z0-9]{3,24})(\|\^?[\.\-_\/#A-Za-z0-9]{3,24}){0,4}"
);

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
    // Every shape the value can break beyond its length — more alternatives than the
    // grammar admits, an alternative that is too short, and a character outside the
    // class — is one expression it does not satisfy, so each is stated as the `@pattern`
    // constraint it is, naming the expression itself rather than a prose rule of this
    // emulator's own.
    if alternatives.len() > MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES {
        return refuse(pattern_constraint());
    }
    for alternative in alternatives {
        let literal = alternative
            .strip_prefix(LOG_GROUP_NAME_PATTERN_ANCHOR)
            .unwrap_or(alternative);
        let shaped = literal.chars().count() >= MIN_LOG_GROUP_NAME_PATTERN_ALTERNATIVE_LEN
            && literal.chars().all(is_log_group_name_pattern_char);
        if !shaped {
            return refuse(pattern_constraint());
        }
    }
    Ok(())
}

/// The `@pattern` clause a `logGroupNamePattern` outside the accepted grammar is refused
/// with: the one constraint sentence, stating the expression the service publishes
/// ([`LOG_GROUP_NAME_REGEX_PATTERN`]).
fn pattern_constraint() -> String {
    format!("Member must satisfy regular expression pattern: {LOG_GROUP_NAME_REGEX_PATTERN}")
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

/// The processor kinds that PARSE a log event — the ones that turn the raw message into
/// the structured event the other processors work on — as the model spells the `Processor`
/// members that carry them.
///
/// A transformer "must have at least one parse-type processor" (`ParseJSON`), and a
/// configuration whose FIRST processor is a mutation step structures nothing for the rest of
/// the pipeline to read, which is what [`transformer_leading_parser_message`] refuses. The
/// set is the configurable parsers (`csv`, `grok`, `parseJSON`, `parseKeyValue`) and the
/// parsers for AWS-vended log formats, each of which the model documents as parsing a log
/// event into fields; it is keyed on the member name, so it holds for every operation that
/// takes a `Processors` list.
pub const PARSE_TYPE_PROCESSORS: &[&str] = &[
    "csv",
    "grok",
    "parseCloudfront",
    "parseJSON",
    "parseKeyValue",
    "parsePostgres",
    "parseRoute53",
    "parseToOCSF",
    "parseVPC",
    "parseWAF",
];

/// The processor kinds that may appear ONLY as the first processor of a transformer, as the
/// model states on each of them: "If you use this processor, it must be the first processor
/// in your transformer" (`ParseCloudfront`, `ParseRoute53`, `ParsePostgres`, `ParseVPC`,
/// `ParseWAF`). Each "always processes the entire log event message", so there is nothing
/// for it to do after another processor has already restructured the event.
pub const LEADING_ONLY_PROCESSORS: &[&str] = &[
    "parseCloudfront",
    "parsePostgres",
    "parseRoute53",
    "parseVPC",
    "parseWAF",
];

/// Whether the processor member `member` names a parse-type processor
/// ([`PARSE_TYPE_PROCESSORS`]).
pub fn is_parse_type_processor(member: &str) -> bool {
    PARSE_TYPE_PROCESSORS.contains(&member)
}

/// The refusal for a transformer whose first processor is not a parse-type one: it states
/// the rule and which kinds satisfy it, and names the kind the request put first.
pub fn transformer_leading_parser_message(named: &str) -> String {
    format!(
        "The first processor in a transformer must be a parse-type processor, one of {}; \
         this transformer begins with {named}.",
        PARSE_TYPE_PROCESSORS.join(", ")
    )
}

/// The refusal for a processor the model admits only as the first one of a transformer
/// ([`LEADING_ONLY_PROCESSORS`]), used at any later position.
pub fn transformer_leading_only_message(named: &str, at: usize) -> String {
    format!(
        "{named} must be the first processor in a transformer, and this transformer has it \
         at transformerConfig[{at}]."
    )
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

/// One metric filter — how CloudWatch Logs "would extract metric observations from
/// ingested log events and transform them into metric data in a CloudWatch metric"
/// (`MetricFilter`) — as it is stored. It lives on the log group `PutMetricFilter`
/// "associates" it with, so deleting that group removes its metric filters with it.
#[derive(Debug, Clone)]
pub struct MetricFilterRecord {
    pub filter_name: String,
    pub filter_pattern: String,
    /// The transformations this filter emits metric data through. The modelled
    /// `MetricTransformations` list is `length 1..1`, so this holds exactly one today;
    /// it is a list because the member is one, and a reader hands back what was put.
    pub transformations: Vec<MetricTransformationRecord>,
    pub apply_on_transformed_logs: bool,
    pub field_selection_criteria: Option<String>,
    /// The system fields the filter emits as additional dimensions, when the put named
    /// any; `None` otherwise, which is what `MetricFilter$emitSystemFieldDimensions`
    /// reports by omitting the member ("Returns the emitSystemFieldDimensions value if it
    /// was specified when the metric filter was created").
    pub emit_system_field_dimensions: Option<Vec<String>>,
    /// `MetricFilter$creationTime`. A put over a filter the group already holds UPDATES
    /// it, so this keeps the instant the first put created it.
    pub creation_time_ms: i64,
}

impl MetricFilterRecord {
    /// This filter as `DescribeMetricFilters` reports it, on the log group it is stored
    /// on. Every member comes from what was put, so a reader hands back the filter the
    /// group is actually configured with.
    pub fn to_shape(&self, log_group_name: &str) -> crate::ops::types::MetricFilter {
        crate::ops::types::MetricFilter {
            filter_name: Some(self.filter_name.clone()),
            filter_pattern: Some(self.filter_pattern.clone()),
            metric_transformations: Some(
                self.transformations
                    .iter()
                    .map(MetricTransformationRecord::to_shape)
                    .collect(),
            ),
            creation_time: Some(self.creation_time_ms),
            log_group_name: Some(log_group_name.to_string()),
            apply_on_transformed_logs: self.apply_on_transformed_logs,
            field_selection_criteria: self.field_selection_criteria.clone(),
            emit_system_field_dimensions: self.emit_system_field_dimensions.clone(),
        }
    }
}

/// One metric transformation — "how to transform ingested log events to metric data in a
/// CloudWatch metric" — as it is stored.
///
/// The modelled shape carries its `dimensions` in a `HashMap`, whose iteration order is
/// not stable; a stored transformation keeps them in a `BTreeMap` so everything derived
/// from them (a refusal, a reader's output) is the same on every run.
#[derive(Debug, Clone)]
pub struct MetricTransformationRecord {
    pub metric_name: String,
    pub metric_namespace: String,
    pub metric_value: String,
    /// "(Optional) The value to emit when a filter pattern does not match a log event."
    /// Never set together with `dimensions` — see [`METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS`].
    pub default_value: Option<f64>,
    /// "The fields to use as dimensions for the metric", keyed by dimension name. Empty
    /// for a transformation that declares none, which a reader reports by omitting the
    /// member.
    pub dimensions: BTreeMap<String, String>,
    /// "The unit to assign to the metric. If you omit this, the unit is set as `None`."
    /// `None` here is the ABSENT member: the model's `StandardUnit` enum spells the unit
    /// "None" itself, so a transformation put without a unit reports no unit rather than
    /// the string.
    pub unit: Option<String>,
}

impl MetricTransformationRecord {
    /// This transformation as a reader reports it.
    pub fn to_shape(&self) -> crate::ops::types::MetricTransformation {
        crate::ops::types::MetricTransformation {
            metric_name: self.metric_name.clone(),
            metric_namespace: self.metric_namespace.clone(),
            metric_value: self.metric_value.clone(),
            default_value: self.default_value,
            dimensions: (!self.dimensions.is_empty()).then(|| {
                self.dimensions
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            }),
            unit: self.unit.clone(),
        }
    }
}

/// The refusal for a metric transformation that declares both `dimensions` and a
/// `defaultValue`.
///
/// The two are mutually exclusive by what each one MEANS: a dimensioned metric is one
/// metric per value found for the dimension fields, so there is no dimension value to
/// publish a "value to emit when a filter pattern does not match a log event" under — AWS
/// states the rule as "you can't specify a default value for the metric" when publishing
/// dimensions with metrics from log events.
///
/// This is the whole message the service answers for the error class: fixed text that
/// interpolates nothing from the request or from state — no filter name, no log group, no
/// metric name, no dimension name and no default value — so every transformation that
/// trips the rule is refused with exactly this sentence.
pub const METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS: &str =
    "Invalid metric transformation: dimensions and default value are mutually exclusive \
     properties";

/// The refusal for a dimension value that is not a field of the log event. See
/// [`is_log_event_field_reference`].
pub const METRIC_FILTER_DIMENSION_VALUE_NOT_A_FIELD: &str =
    "each dimension value must name a field of the log event, selected with \
     '$' — '$.path' for a JSON log event and '$field' for a space-delimited one — \
     never a literal value";

/// Whether a metric filter's dimension value names a FIELD of the log event, which is the
/// only thing a dimension value may be: dimensions are published "with metrics from values
/// in JSON or space-delimited log events", so the value is a selector the filter pattern's
/// syntax resolves against each event — `$.some.path` in a JSON log event, `$field` or `$1`
/// in a space-delimited one — and never a literal, which would make the dimension a
/// constant instead of a field.
pub fn is_log_event_field_reference(value: &str) -> bool {
    let Some(selector) = value.strip_prefix(FIELD_REFERENCE_PREFIX) else {
        return false;
    };
    match selector.strip_prefix('.') {
        // A JSON selector: `$.` followed by at least one path segment, each of them named
        // — `$.a.`, `$.a..b` and `$.` select nothing.
        Some(path) => !path.is_empty() && path.split('.').all(|segment| !segment.is_empty()),
        // A space-delimited field: `$` followed by the name the pattern declared, or the
        // 1-based position of the field.
        None => !selector.is_empty(),
    }
}

/// The regular expression patterns a `filterPattern` carries: the `%…%` literals of the
/// filter-pattern syntax, which is the quantity `PutMetricFilter`'s two quotas
/// ([`MAX_REGEX_PATTERNS_PER_FILTER_PATTERN`], [`MAX_REGEX_PATTERNS_PER_LOG_GROUP`]) bound.
///
/// A `%` inside a quoted string is data rather than a delimiter, and a backslash escapes
/// the character after it — the same reading [`filter_pattern_structure`] gives a pattern.
/// An unterminated `%` opens no pattern, so it counts for nothing.
pub fn filter_pattern_regex_count(pattern: &str) -> usize {
    let mut count = 0usize;
    let mut quoted = false;
    let mut open = false;
    let mut chars = pattern.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                chars.next();
            }
            '"' if !open => quoted = !quoted,
            '%' if !quoted => {
                if open {
                    count = count.saturating_add(1);
                }
                open = !open;
            }
            _ => {}
        }
    }
    count
}

/// Hold one `metricTransformations` entry to the rules `PutMetricFilter` documents for it,
/// answering the transformation to store. The `Err` carries the message to refuse with, as
/// the service states the rule.
///
/// `emitted_system_fields` is how many system fields the same put asks to emit as
/// dimensions: they "count toward the total dimension limit for metric filters", so the
/// declared dimensions and the emitted system fields are counted together against
/// [`MAX_METRIC_FILTER_DIMENSIONS`].
///
/// Everything the model already constrains — the metric name's and namespace's pattern and
/// length, the `unit` enum, the one-entry list — is refused by the boundary walk before a
/// handler runs, so this holds only the rules that span members.
pub fn check_metric_transformation(
    transformation: &crate::ops::types::MetricTransformation,
    emitted_system_fields: usize,
) -> Result<MetricTransformationRecord, String> {
    let dimensions: BTreeMap<String, String> = transformation
        .dimensions
        .iter()
        .flatten()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();

    if dimensions.len().saturating_add(emitted_system_fields) > MAX_METRIC_FILTER_DIMENSIONS {
        return Err(format!(
            "a metric filter can include as many as {MAX_METRIC_FILTER_DIMENSIONS} dimensions, \
             counting the system fields emitted as dimensions"
        ));
    }
    if !dimensions.is_empty() && transformation.default_value.is_some() {
        return Err(METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS.to_string());
    }
    if dimensions
        .values()
        .any(|value| !is_log_event_field_reference(value))
    {
        return Err(METRIC_FILTER_DIMENSION_VALUE_NOT_A_FIELD.to_string());
    }

    Ok(MetricTransformationRecord {
        metric_name: transformation.metric_name.clone(),
        metric_namespace: transformation.metric_namespace.clone(),
        metric_value: transformation.metric_value.clone(),
        default_value: transformation.default_value,
        dimensions,
        unit: transformation.unit.clone(),
    })
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
    /// Whether this filter's destination REFUSED its last delivery
    /// ([`subscription_delivery::deliver`]). A destination that answered a delivery with a
    /// failure is not attempted again: the filter delivers nothing more until a
    /// `PutSubscriptionFilter` re-validates it, which is what makes a destination that comes
    /// back serve the batches written AFTER that put rather than replaying the ones written
    /// while it was gone. Never reported: no operation models a filter's delivery health.
    pub destination_unhealthy: bool,
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
    arn_parts(arn).map(|(_partition, service, _account, resource)| (service, resource))
}

/// The account segment of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything that
/// is not an ARN or that names no account — which is how a refusal states the account the
/// identifier it was handed belongs to.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    arn_parts(arn)
        .map(|(_partition, _service, account, _resource)| account)
        .filter(|account| !account.is_empty())
}

/// The `(partition, service, account, resource)` segments of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything
/// that is not an ARN.
fn arn_parts(arn: &str) -> Option<(&str, &str, &str, &str)> {
    let rest = arn.strip_prefix("arn:")?;
    // partition, service, region, account, then the whole resource segment.
    let mut segments = rest.splitn(5, ':');
    let partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let account = segments.next()?;
    Some((
        partition,
        service,
        account,
        segments.next().unwrap_or_default(),
    ))
}

/// The vendors of a `destinationArn` CloudWatch Logs delivers to by ASSUMING the role
/// `roleArn` names, so a subscription against one of them cannot be configured without
/// that role: a stream of one of the two streaming services named as direct destinations
/// ("An Amazon Kinesis stream belonging to the same account as the subscription filter",
/// "A Kinesis Data Firehose delivery stream belonging to the same account").
///
/// The other two destination kinds carry their own authorisation, so neither needs
/// `roleArn`: a Lambda function is invoked directly, which is why the operation documents
/// the `iam:PassRole` requirement for "any destination except a Lambda function", and a
/// logical destination created with `PutDestination` was given the role it delivers
/// through when it was created (`roleArn`: "You don't need to provide the ARN when you are
/// working with a logical destination for cross-account delivery").
pub const ROLE_ASSUMED_DESTINATION_VENDORS: [&str; 2] = ["kinesis", "firehose"];

/// The vendor of a `destinationArn` whose delivery needs the role `roleArn` names, or
/// `None` when the filter can be configured without one.
///
/// Only the vendors in [`ROLE_ASSUMED_DESTINATION_VENDORS`] need one. A vendor this
/// operation delivers to nothing in needs no role either: there is no delivery to
/// authorise, and such a `destinationArn` is refused for the resource it names instead.
pub fn subscription_role_required_vendor(arn: &str) -> Option<&str> {
    let (_partition, vendor, _account, _resource) = arn_parts(arn)?;
    ROLE_ASSUMED_DESTINATION_VENDORS
        .contains(&vendor)
        .then_some(vendor)
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
/// ([`DeliveryRecord::tags`]). The delivery source and the delivery destination are here for
/// the same reason: each carries its own tag member (`DeliverySource$tags`,
/// `DeliveryDestination$tags`) and this service stores it ([`DeliverySourceRecord::tags`],
/// [`DeliveryDestinationRecord::tags`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// One log group, by the name the store is keyed by.
    LogGroup(String),
    /// One logical destination, by the `Destination$destinationName` it is created under.
    Destination(String),
    /// One delivery, by the `Delivery$id` it was minted with in the Region that holds it.
    Delivery(String),
    /// One delivery source, by the `DeliverySource$name` it was created under in the Region
    /// that holds it.
    DeliverySource(String),
    /// One delivery destination, by the `DeliveryDestination$name` it was created under in the
    /// Region that holds it.
    DeliveryDestination(String),
    /// One lookup table, by the `LookupTable$lookupTableName` it was created under — the key
    /// the store holds it by. It is here for the same reason the delivery resources are: the
    /// model gives the resource its own tag member (`CreateLookupTable$tags`, "An optional list
    /// of key-value pairs to associate with the resource") and this service stores it
    /// ([`LookupTableRecord::tags`]).
    LookupTable(String),
}

impl TaggedResource {
    /// The resource kind a refusal names, in the words this service uses for it — what
    /// [`resource_not_found_message`] states when the ARN names no stored resource.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::LogGroup(_) => "log group",
            Self::Destination(_) => "destination",
            Self::Delivery(_) => "delivery",
            Self::DeliverySource(_) => "delivery source",
            Self::DeliveryDestination(_) => "delivery destination",
            Self::LookupTable(_) => "lookup table",
        }
    }
}

/// The ARN resource-segment prefix of a logical destination, from the ARN format
/// `TagResource$resourceArn` documents for one.
const DESTINATION_ARN_PREFIX: &str = "destination:";

/// The `Destination$destinationName` an ARN names — the name the store keys a logical
/// destination by — or `None` for an ARN that names no `destination` resource of this
/// service.
///
/// The ARN format is the one `TagResource$resourceArn` documents
/// (`arn:aws:logs:Region:account-id:destination:destination-name`), and the name is
/// whatever follows that resource prefix, separators included: `DestinationName` is
/// `^[^:*]*$`, so a name can carry slashes and dots.
pub fn destination_name_from_arn(arn: &str) -> Option<&str> {
    let (service, resource) = arn_service_and_resource(arn)?;
    if service != "logs" {
        return None;
    }
    resource
        .strip_prefix(DESTINATION_ARN_PREFIX)
        .filter(|name| !name.is_empty())
}

/// The ARN resource-segment prefix of a delivery, as [`delivery_arn`] builds one.
const DELIVERY_ARN_PREFIX: &str = "delivery:";

/// The ARN resource-segment prefix of a delivery source, as [`delivery_source_arn`] builds
/// one.
const DELIVERY_SOURCE_ARN_PREFIX: &str = "delivery-source:";

/// The ARN resource-segment prefix of a delivery destination, as
/// [`delivery_destination_arn`] builds one.
const DELIVERY_DESTINATION_ARN_PREFIX: &str = "delivery-destination:";

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
    if let Some(name) = destination_name_from_arn(arn) {
        return Some(TaggedResource::Destination(name.to_string()));
    }
    if let Some(name) = resource.strip_prefix(DELIVERY_SOURCE_ARN_PREFIX) {
        return (!name.is_empty()).then(|| TaggedResource::DeliverySource(name.to_string()));
    }
    if let Some(name) = resource.strip_prefix(DELIVERY_DESTINATION_ARN_PREFIX) {
        return (!name.is_empty()).then(|| TaggedResource::DeliveryDestination(name.to_string()));
    }
    if let Some(id) = resource.strip_prefix(DELIVERY_ARN_PREFIX) {
        return (!id.is_empty()).then(|| TaggedResource::Delivery(id.to_string()));
    }
    if let Some(name) = lookup_table_name_from_arn(arn) {
        return Some(TaggedResource::LookupTable(name.to_string()));
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

/// The refusal for a member this service parses as a KMS key's and was handed a value that
/// names something else: the message names the MEMBER and the value it would not take, so a
/// caller can fix the request from it. Shared by every operation that holds a member to
/// naming a KMS key — `kmsKeyId` of the lookup-table writes and `kmsKeyArn` of
/// `PutIntegration` — so the wording cannot drift between them.
pub fn not_a_kms_key_message(member: &str, value: &str) -> String {
    format!("{member} must name a KMS key, got '{value}'")
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

/// Whether `value` matches a term carrying `*` wildcards, each standing for any run of
/// characters (`prod*`, `*prod`, `a*b*c`); a term with no `*` matches only itself.
///
/// THE one reading of the `*` wildcard this service uses, shared by every member that
/// carries one: a `ListLogGroups` name term, and a filter pattern's `"*…*"` literal
/// ([`filter_pattern`]). Case folding is the CALLER's, because the two differ there — log
/// group name matching is documented case-INsensitive, and the filter-pattern syntax is case
/// sensitive — so this answers only the wildcard question.
pub fn wildcard_matches(value: &str, term: &str) -> bool {
    let mut literals = term.split('*');
    // The run before the first `*` is anchored at the start, the run after the last `*`
    // at the end, and every run between them is matched in order.
    let Some(first) = literals.next() else {
        return true;
    };
    let Some(mut rest) = value.strip_prefix(first) else {
        return false;
    };
    let remaining: Vec<&str> = literals.collect();
    let Some((last, middle)) = remaining.split_last() else {
        // No `*` at all: the term is literal.
        return rest.is_empty();
    };
    for literal in middle {
        match rest.find(literal) {
            Some(at) => rest = &rest[at + literal.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
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

/// The scheduled query name a `ScheduledQueryIdentifier` names: the identifier itself when it
/// is a name, or the name segment of a scheduled-query ARN ([`scheduled_query_arn`]).
///
/// A scheduled query is identified by "The ARN or name of the scheduled query", and the ARN
/// is built FROM the name, so an ARN names the query its resource segment does — the way a
/// `logGroupIdentifier` names a log group through [`log_group_name_from_identifier`].
pub fn scheduled_query_name_from_identifier(identifier: &str) -> &str {
    const MARKER: &str = ":scheduled-query:";
    match identifier.find(MARKER) {
        Some(at) => &identifier[at + MARKER.len()..],
        None => identifier,
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

/// How many milliseconds one second holds, which is what a `StartQuery` time range — "epoch
/// time, the number of SECONDS since January 1, 1970" — has to be scaled by to be compared
/// with the epoch MILLISECONDS every other timestamp of this service carries.
pub const MILLIS_PER_SECOND: i64 = 1_000;

/// The name the SDK envelope this service's refusals carry names the service by — the
/// legacy client name of CloudWatch Logs, not the model's `sdkId`.
const SDK_ENVELOPE_SERVICE: &str = "AWSLogs";

/// The HTTP status and error code of one refusal, kept PAIRED so neither can be stated in
/// an SDK envelope without the other: they are the two values a client reads back out of
/// the message text, and they must be the status and code the response itself carries.
pub type SdkEnvelopeFacts = (u16, &'static str);

/// The status and code of a refusal for a parameter "specified incorrectly", as the model
/// declares `InvalidParameterException` for this service (client error, HTTP 400).
pub const ENVELOPE_INVALID_PARAMETER: SdkEnvelopeFacts = (400, "InvalidParameterException");

/// The status and code of a refusal for a resource that "does not exist", as the model
/// declares `ResourceNotFoundException` for this service (client error, HTTP 400).
pub const ENVELOPE_RESOURCE_NOT_FOUND: SdkEnvelopeFacts = (400, "ResourceNotFoundException");

/// `body` followed by the envelope the AWS SDK appends to a CloudWatch Logs refusal:
/// ` (Service: AWSLogs; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)`.
///
/// `facts` is the status-and-code pair of the refusal being worded (one of the
/// `ENVELOPE_*` constants). The request id is the ONLY per-run field: a request id is
/// minted by the service per call — nothing in the request carries one — so it comes from
/// the emulator's one request-id mint
/// ([`service_common::query_error_envelope::new_request_id`]), a fresh v4 UUID
/// indistinguishable from the one the framework's own renderers report. Nothing of the
/// request is interpolated.
///
/// One function, so every refusal worded this way carries the envelope in the same shape.
pub fn with_sdk_error_envelope(body: &str, facts: SdkEnvelopeFacts) -> String {
    let (status_code, error_code) = facts;
    format!(
        "{body} (Service: {SDK_ENVELOPE_SERVICE}; Status Code: {status_code}; \
         Error Code: {error_code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// The refusal every paginated read of this service answers for a `nextToken` it cannot
/// resume the enumeration from — one this service never minted, one whose payload it cannot
/// decode, and one minted for a different enumeration than the request carrying it describes.
/// AWS states one fixed sentence for the whole class, carrying neither the token nor the
/// operation, so a paginating reader that keys on it restarts from the head of the stream.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The specified nextToken is invalid.";

/// The refusal `CreateExportTask` answers when the S3 bucket its `destination` names is one
/// the object store does not hold: an export that could never write anything mints no task.
/// Fixed text for the class — it carries neither the bucket name nor anything else of the
/// request.
pub const EXPORT_DESTINATION_BUCKET_MISSING_MESSAGE: &str =
    "The given bucket does not exist. Please make sure the bucket is valid.";

/// The sentence CloudWatch Logs answers when the time range a query is asked over runs
/// backwards — its end before its start — which includes no instant at all. Fixed text for
/// the condition: it names the two ends of the range and interpolates neither of their
/// values.
pub const REVERSED_TIME_RANGE_MESSAGE: &str = "End time cannot be less than Start time";

/// The whole message such a refusal carries: the fixed sentence plus the SDK envelope of
/// the `InvalidParameterException` it is refused under, so the status and code stated inside
/// the text are the ones the response answers with.
pub fn reversed_time_range_refusal() -> String {
    with_sdk_error_envelope(REVERSED_TIME_RANGE_MESSAGE, ENVELOPE_INVALID_PARAMETER)
}

/// The refusal `CreateExportTask` answers when the window it is given is not a time range:
/// its fixed text states what the service requires of a window — that the two values bound a
/// period the log groups still hold data for, and that `from` precede `to` — and its two
/// quoted slots carry the window the REQUEST sent, in the order it sent it, so the caller can
/// see which window was rejected.
pub fn export_time_range_message(from: i64, to: i64) -> String {
    format!(
        "Specified time range of '{from}' and '{to}' is not valid. Please make sure the values \
         are within the retention period of the log groups and from value is lesser than the to \
         value"
    )
}

/// The refusal `CancelExportTask` answers when the task the request names is not in one of
/// the two statuses a cancel applies to: "The task must be in the PENDING or RUNNING state."
/// The text is fixed for the condition — it interpolates neither the task id, nor the log
/// group, nor the bucket the export was going to — and the task keeps the status it reached.
pub const EXPORT_TASK_FINISHED_MESSAGE: &str = "The specified export task has already finished";

/// The revision id a resource-scoped policy carries, minted from the monotonic policy
/// revision counter so it is unique and derived from stored state. AWS documents no
/// format for it — only that a put must present the revision the policy is currently
/// at — so this renders the counter as a fixed-width hex string.
pub fn mint_revision_id(seq: u64) -> String {
    format!("{seq:032x}")
}

/// How many characters a minted revision id has, which is the width
/// [`mint_revision_id`] renders the counter at. A presented `expectedRevisionId` of any
/// other length is in no form this service ever issued
/// ([`ComAmazonawsCloudwatchlogsLogs20140328State::issued_revision_id`]).
pub const REVISION_ID_LEN: usize = 32;

/// The refusal CloudWatch Logs answers a resource-policy write or delete with when the
/// `expectedRevisionId` it presents is not a revision id the service ever issued — a value
/// in another form, or one no update has reached. That is a bad PARAMETER and not the
/// optimistic-lock conflict [`POLICY_REVISION_CONFLICT_MESSAGE`] reports: retrying it can
/// never succeed, so the text names the member at fault and interpolates nothing.
pub const INVALID_EXPECTED_REVISION_ID_MESSAGE: &str = "Invalid ExpectedRevisionId";

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

/// The id `StartQuery` hands back for the query it starts: "The unique ID of the query",
/// minted from the store's monotonic query counter so it is unique for the life of the store
/// and DERIVED FROM STORED STATE.
///
/// CloudWatch Logs renders one as a canonical hyphenated UUID, so it is minted through the
/// shared [`mint_uuid_from_seq`] every UUID-shaped id of this service comes from.
pub fn mint_query_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The `S3TableIntegrationSourceIdentifier` `AssociateSourceToS3TableIntegration` hands
/// back for the association it creates: "The unique identifier for the association between
/// the data source and S3 Table Integration", minted from the store's monotonic association
/// counter so it is unique for the life of the store and DERIVED FROM STORED STATE.
///
/// Minted through the shared [`mint_uuid_from_seq`] every id of this service AWS renders as
/// a UUID comes from, which also makes the minting order the identifier order the
/// associations are stored and enumerated in.
pub fn mint_s3_table_integration_source_identifier(seq: u64) -> String {
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

/// The refusal CloudWatch Logs answers when the missing resource is a QUERY the request
/// named by `queryId`: the service states the id it was handed rather than the resource kind,
/// and calls it INVALID rather than absent — a caller holding several ids learns which one was
/// rejected, and keeps the distinction the service draws between an id it cannot place and a
/// query that is simply no longer running ([`query_not_running_message`]).
///
/// The only variable part is the `queryId` the request carried, taken from it verbatim.
pub fn invalid_query_id_message(query_id: &str) -> String {
    format!("The provided queryId={query_id} is invalid.")
}

/// The refusal CloudWatch Logs answers when the missing resource is a QUERY DEFINITION: the
/// service states the fixed sentence for the class — it carries nothing of the request, not
/// even the id — inside the SDK envelope of the `ResourceNotFoundException` it is refused
/// under ([`with_sdk_error_envelope`]), so the status and the code stated in the text are the
/// ones the response itself answers with and the only per-run field is the request id.
pub fn query_definition_not_found_message() -> String {
    with_sdk_error_envelope(
        "Query definition id is invalid.",
        ENVELOPE_RESOURCE_NOT_FOUND,
    )
}

/// The refusal CloudWatch Logs answers when the missing resource is a SCHEDULED QUERY: the
/// service names the `ScheduledQueryIdentifier` the request asked for ("The ARN or name of the
/// scheduled query") rather than stating the resource kind alone, and states it unterminated,
/// so a caller reading the refusal sees WHICH query was not found — whichever form it named it
/// by.
///
/// Every operation that resolves a `ScheduledQueryIdentifier` refuses a query the account does
/// not hold through this one sentence, so the reads and the writes of a scheduled query agree.
pub fn scheduled_query_not_found_message(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} not found")
}

/// The refusal CloudWatch Logs answers a scheduled-query create with when the account already
/// holds a query under the `name` it asks for ("The name must be unique within your account and
/// region", refused as the conflict the operation models: "This operation attempted to create a
/// resource that already exists").
///
/// It is the same identifier phrasing the service refuses a scheduled query it cannot find with
/// ([`scheduled_query_not_found_message`]): the resource kind, then the identifier the request
/// carried, then the condition — so the only variable part is that identifier, taken from the
/// request verbatim and carrying nothing the account generated (no ARN, account id or instant).
pub fn scheduled_query_already_exists_message(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} already exists")
}

/// The refusal CloudWatch Logs answers when the missing resource is one the request
/// IDENTIFIES NOTHING OF: a log-group level policy document addressed by the identifier of
/// the log group it is attached to and by nothing else, which that group does not hold. The
/// service says nothing about the resource in that case — the text is fixed for the error
/// class and interpolates neither a log group, nor a name, nor a resource kind — unlike a
/// request naming a log group or a log stream the account does not hold, which is refused
/// by [`resource_not_found_message`] naming that resource, and unlike a policy the request
/// names, which is refused by [`policy_not_found_message`] carrying that name.
pub const UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Specified resource does not exist!";

/// The refusal CloudWatch Logs answers a resource-policy write or delete with when the
/// `expectedRevisionId` it presents is not the policy's current revision: the revision the
/// caller held was superseded, which the service reports as a conflict against the resource
/// that the caller is to re-read and retry. The text is fixed for the error class — it
/// interpolates neither the revision presented, nor the stored one, nor the policy's name
/// or resource.
pub const POLICY_REVISION_CONFLICT_MESSAGE: &str =
    "A conflicting operation is currently in progress against this resource. Please try again.";

/// The refusal CloudWatch Logs answers a delivery-destination POLICY operation with when the
/// `deliveryDestinationName` it names is not one this account holds. The delivery destination
/// policy operations state that the requested destination is not in the account rather than
/// the generic [`resource_not_found_message`] sentence, and they say it per NAME rather than
/// per request value, so every name the account does not hold is refused identically.
pub const DELIVERY_DESTINATION_NOT_FOUND_MESSAGE: &str =
    "Requested Delivery Destination does not exist in this account.";

/// The refusal CloudWatch Logs answers the DELETE of a delivery-destination policy with when
/// the destination it names holds no policy: the resource the delete addresses is the POLICY,
/// not the destination, so a destination that exists and has had none assigned has nothing for
/// the delete to remove. Like [`DELIVERY_DESTINATION_NOT_FOUND_MESSAGE`] the sentence is fixed
/// for the error class and interpolates no destination name.
pub const DELIVERY_DESTINATION_POLICY_NOT_FOUND_MESSAGE: &str =
    "Delivery Destination Policy does not exist for this Delivery Destination.";

/// The refusal `PutDeliveryDestinationPolicy` answers a `deliveryDestinationPolicy` that is
/// not a delivery destination resource policy ([`is_delivery_destination_policy`]). The
/// sentence is fixed for the error class: it states neither the destination nor anything of
/// the document it would not take.
pub const INVALID_DELIVERY_DESTINATION_POLICY_MESSAGE: &str =
    "Invalid Delivery Destination resource Policy is provided.";

/// Whether `document` is a delivery destination resource policy.
///
/// The operation "creates and assigns an IAM policy that grants permissions to CloudWatch Logs
/// to deliver logs cross-account", so the document is an IAM JSON policy in its RESOURCE-based
/// form — it states who the permission is granted to — which is the one grammar
/// [`service_common::iam_policy`] states for every service of this emulator that takes such a
/// document.
///
/// What the statements SAY is not judged: they govern a cross-account delivery nothing in this
/// store performs, and the operation documents nothing about them beyond that the policy
/// "must include two statements. One statement enables general logs delivery, and the other
/// allows delivery to the chosen destination".
pub fn is_delivery_destination_policy(document: &str) -> bool {
    ::service_common::iam_policy::resource_policy_defect(document).is_none()
}

/// The refusal CloudWatch Logs builds from the request when the identifier a
/// `DisassociateSourceFromS3TableIntegration` names is not an association of an S3 Table
/// Integration this account holds. The message interpolates the requested identifier, so a
/// caller disassociating sources in a loop can attribute the refusal to one identifier
/// instead of guessing which of its requests failed.
pub fn s3_table_integration_source_not_found_message(identifier: &str) -> String {
    format!("Association not found for identifier: {identifier}")
}

/// The refusal CloudWatch Logs builds from the request when the `destinationName` it names
/// is not a destination this account holds and the operation models no
/// `ResourceNotFoundException`, so the missing destination is reported through
/// `InvalidParameterException` — an error class documented only as "A parameter is specified
/// incorrectly", which carries no sentence of its own, so the message states WHICH parameter
/// value was rejected. The fixed text is "Destination with name " … " does not exist" and the
/// one variable part is the requested name, taken verbatim from the request, so a caller
/// managing several destinations can attribute the refusal to one request instead of guessing
/// which of them failed.
///
/// Unlike this, an operation that DOES model `ResourceNotFoundException` refuses a missing
/// destination with that error class's own documented sentence ("The specified resource does
/// not exist."), specialised per resource kind by [`resource_not_found_message`].
pub fn destination_not_found_message(destination_name: &str) -> String {
    format!("Destination with name {destination_name} does not exist")
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
