// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-configservice-starlingdoveservice service.
//!
//! This holds the AWS Config setup-and-rule loop: the customer managed configuration
//! recorder, its status record, the delivery channel, the Config rules, and the
//! retention configuration that governs how long history is kept. Every
//! handler under `src/ops/` computes its response from these maps; nothing is
//! hardcoded and no response echoes the request that produced it.
//!
//! One engine's store stands for ONE account in ONE Region, so the quotas Config
//! documents per account per Region are enforced over the whole store.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccountAggregationSource, AggregationAuthorization, AggregatorFilters, Compliance,
    ComplianceContributorCount, ComplianceSummary, ConfigExportDeliveryInfo, ConfigRule,
    ConfigRuleEvaluationStatus, ConfigSnapshotDeliveryProperties, ConfigStreamDeliveryInfo,
    ConfigurationAggregator, ConfigurationRecorder, ConfigurationRecorderStatus,
    ConfigurationRecorderSummary, Connector, ConnectorConfiguration, ConnectorSummary,
    DeliveryChannel, DeliveryChannelStatus, EvaluationModeConfiguration, ExclusionByResourceTypes,
    ExecutionControls, OrganizationAggregationSource, PendingAggregationRequest, RecordingGroup,
    RecordingMode, RecordingStrategy, RemediationConfiguration, RemediationParameterValue,
    ResourceDetails, ResourceEvaluation, ResourceIdentifier, RetentionConfiguration, Scope, Source,
    Tag,
};

/// The ARN namespace of this service (`awsService.arnNamespace` in the model), used
/// for every ARN this emulator mints.
pub const ARN_SERVICE: &str = "config";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region generated emulators default to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The name Config assigns when a configuration recorder or delivery channel is
/// created without one (`ConfigurationRecorder$name` and `DeliveryChannel$name`:
/// "Config automatically assigns the name of \"default\" ... if you do not specify a
/// name at creation time").
pub const DEFAULT_NAME: &str = "default";

/// The name prefix reserved for service-linked configuration recorders
/// (`InvalidConfigurationRecorderNameException`: "The prefix
/// \"AWSConfigurationRecorderFor\" is reserved for service-linked configuration
/// recorders").
pub const SERVICE_LINKED_RECORDER_NAME_PREFIX: &str = "AWSConfigurationRecorderFor";

/// `RecorderStatus` values (`com.amazonaws.configservice#RecorderStatus`).
pub const RECORDER_STATUS_PENDING: &str = "Pending";
/// See [`RECORDER_STATUS_PENDING`].
pub const RECORDER_STATUS_SUCCESS: &str = "Success";

/// `ConfigRuleState` value a freshly created rule carries
/// (`com.amazonaws.configservice#ConfigRuleState`).
pub const CONFIG_RULE_STATE_ACTIVE: &str = "ACTIVE";

/// The default `EvaluationModes` entry for a new rule (`ConfigRule$EvaluationModes`:
/// "By default, the value is Detective evaluation mode only").
pub const EVALUATION_MODE_DETECTIVE: &str = "DETECTIVE";

/// The other `EvaluationMode` value (`com.amazonaws.configservice#EvaluationMode`),
/// and the ONLY one the resource-evaluation path accepts
/// (`StartResourceEvaluationRequest$EvaluationMode`: "The only valid value for this
/// API is PROACTIVE"; `ResourceEvaluationFilters$EvaluationMode`: "Currently,
/// DECTECTIVE is not supported as a valid value").
pub const EVALUATION_MODE_PROACTIVE: &str = "PROACTIVE";

/// `RecordingStrategyType` value the default recording group uses
/// (`com.amazonaws.configservice#RecordingStrategyType`).
pub const RECORDING_STRATEGY_ALL_SUPPORTED: &str = "ALL_SUPPORTED_RESOURCE_TYPES";
/// See [`RECORDING_STRATEGY_ALL_SUPPORTED`].
pub const RECORDING_STRATEGY_EXCLUSION: &str = "EXCLUSION_BY_RESOURCE_TYPES";
/// See [`RECORDING_STRATEGY_ALL_SUPPORTED`].
pub const RECORDING_STRATEGY_INCLUSION: &str = "INCLUSION_BY_RESOURCE_TYPES";

/// The `RecordingFrequency` a configuration recorder records at unless the request
/// asks for another one (`com.amazonaws.configservice#RecordingFrequency`;
/// `RecordingMode`: "Config supports Continuous recording and Daily recording", and
/// continuous recording is the recorder's default).
pub const RECORDING_FREQUENCY_CONTINUOUS: &str = "CONTINUOUS";

/// The `RecordingScope` of a CUSTOMER MANAGED configuration recorder
/// (`com.amazonaws.configservice#RecordingScope`: `INTERNAL` when the recorded
/// `ConfigurationItems` are "recorded for free", `PAID` when recording them "impacts
/// the costs to your bill"). A recorder this account creates and pays for records in
/// the `PAID` scope; `INTERNAL` belongs to the service-linked recorders
/// `PutServiceLinkedConfigurationRecorder` creates.
pub const RECORDING_SCOPE_PAID: &str = "PAID";
/// See [`RECORDING_SCOPE_PAID`].
pub const RECORDING_SCOPE_INTERNAL: &str = "INTERNAL";

/// Every published `RecordingScope` value
/// (`com.amazonaws.configservice#RecordingScope`, whose values are `INTERNAL` and
/// `PAID`). A `recordingScope` filter value outside this list names no scope a
/// recorder can record in.
pub const RECORDING_SCOPES: [&str; 2] = [RECORDING_SCOPE_INTERNAL, RECORDING_SCOPE_PAID];

/// The only `ConfigurationRecorderFilterName` the model declares
/// (`com.amazonaws.configservice#ConfigurationRecorderFilterName`, whose single value
/// is `recordingScope`; `ConfigurationRecorderFilter$filterName`: "Currently, only
/// recordingScope is supported").
pub const CONFIGURATION_RECORDER_FILTER_RECORDING_SCOPE: &str = "recordingScope";

/// How many recorder summaries one `ListConfigurationRecorders` page carries when the
/// request asks for no fewer: the maximum of that operation's own `MaxResults` member
/// (range 0..20). A request that carries no `MaxResults`, or carries the member's own
/// default of 0, is served a page of this size — a page of zero would never report a
/// recorder the store holds.
pub const LIST_CONFIGURATION_RECORDERS_PAGE_SIZE: usize = 20;

/// The `RuleEvaluationVisibility` of a rule whose `Evaluation`s the caller can read
/// back (`ConfigRule$RuleEvaluationVisibility`: "You can get Evaluations for the
/// Amazon Web Services Config rule if this value is EXTERNAL"). Every rule
/// `PutConfigRule` creates belongs to this account, so its evaluations are
/// retrievable; `INTERNAL` is for rules whose evaluations Config keeps to itself.
pub const RULE_EVALUATION_VISIBILITY_EXTERNAL: &str = "EXTERNAL";

/// `ComplianceType` values (`com.amazonaws.configservice#ComplianceType`).
pub const COMPLIANCE_TYPE_COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANCE_TYPE_COMPLIANT`].
pub const COMPLIANCE_TYPE_NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANCE_TYPE_COMPLIANT`].
pub const COMPLIANCE_TYPE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";
/// See [`COMPLIANCE_TYPE_COMPLIANT`].
pub const COMPLIANCE_TYPE_NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// Every `ComplianceType` an evaluation-results filter may name: the published
/// `ComplianceType` values (`com.amazonaws.configservice#ComplianceType`) MINUS
/// `INSUFFICIENT_DATA`, which the filter members exclude
/// (`GetComplianceDetailsByConfigRuleRequest$ComplianceTypes`: "INSUFFICIENT_DATA is a
/// valid ComplianceType that is returned when an Config rule cannot be evaluated.
/// However, INSUFFICIENT_DATA cannot be used as a ComplianceType for filtering
/// results").
pub const FILTERABLE_COMPLIANCE_TYPES: [&str; 3] = [
    COMPLIANCE_TYPE_COMPLIANT,
    COMPLIANCE_TYPE_NON_COMPLIANT,
    COMPLIANCE_TYPE_NOT_APPLICABLE,
];

/// How many evaluation results one page carries when the request asks for no size of
/// its own (`GetComplianceDetailsByConfigRuleRequest$Limit`: "The maximum number of
/// evaluation results returned on each page. The default is 10. ... If you specify 0,
/// Config uses the default").
pub const GET_COMPLIANCE_DETAILS_PAGE_SIZE: usize = 10;

/// The cap a `ComplianceSummary` counts Config rules up to
/// (`ComplianceSummary$CompliantResourceCount`: "The number of Config rules or Amazon
/// Web Services resources that are compliant, up to a maximum of 25 for rules").
/// Reaching it is what sets `ComplianceContributorCount.CapExceeded`.
pub const COMPLIANCE_SUMMARY_RULE_CAP: i32 = 25;

/// The cap a count of contributing RESOURCES is reported up to
/// (`ComplianceSummary$NonCompliantResourceCount`: "The number of Config rules or
/// Amazon Web Services resources that are noncompliant, up to a maximum of 25 for
/// rules and 100 for resources"). A rule's `Compliance$ComplianceContributorCount`
/// counts resources, so it is reported up to this cap.
pub const COMPLIANCE_RESOURCE_CAP: i32 = 100;

/// `Owner` value for a Custom Policy rule (`com.amazonaws.configservice#Owner`).
pub const OWNER_CUSTOM_POLICY: &str = "CUSTOM_POLICY";

/// The only `ResourceConfigurationSchemaType` the model publishes
/// (`com.amazonaws.configservice#ResourceConfigurationSchemaType`), and so the schema
/// type a resource definition is read as when the request names none.
pub const RESOURCE_CONFIGURATION_SCHEMA_TYPE_CFN: &str = "CFN_RESOURCE_SCHEMA";

/// `Owner` value for a Custom Lambda rule, whose evaluation Config performs by
/// invoking the caller's Lambda function (`com.amazonaws.configservice#Owner`).
/// `DescribeConfigRuleEvaluationStatus` reports no status for a rule of this owner
/// (`ConfigRuleEvaluationStatus`: "Status information for your Config Managed rules
/// and Config Custom Policy rules. … This operation does not return status
/// information about Config Custom Lambda rules.").
pub const OWNER_CUSTOM_LAMBDA: &str = "CUSTOM_LAMBDA";

/// `DeliveryStatus` values (`com.amazonaws.configservice#DeliveryStatus`, whose
/// members are `Success`, `Failure` and `Not_Applicable`).
pub const DELIVERY_STATUS_SUCCESS: &str = "Success";
/// See [`DELIVERY_STATUS_SUCCESS`]. `ConfigStreamDeliveryInfo$lastStatus` documents
/// when this value applies: "Providing an SNS topic on a DeliveryChannel for Config
/// is optional. If the SNS delivery is turned off, the last status will be
/// Not_Applicable."
pub const DELIVERY_STATUS_NOT_APPLICABLE: &str = "Not_Applicable";

/// The `deliveryFrequency` a delivery channel delivers configuration snapshots at
/// when its `ConfigSnapshotDeliveryProperties` names none.
///
/// The model declares no default for `ConfigSnapshotDeliveryProperties$deliveryFrequency`,
/// so this is CHOSEN: the least frequent published `MaximumExecutionFrequency` value
/// (`com.amazonaws.configservice#MaximumExecutionFrequency`), which is also the
/// snapshot frequency Config offers by default when a channel is set up without one.
pub const DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY: &str = "TwentyFour_Hours";

/// How often a delivery channel delivers a configuration history file.
///
/// AWS Config Developer Guide, *Delivering configuration items*: "Config sends a new
/// configuration history file to your Amazon S3 bucket every six hours." The history
/// schedule is fixed, so it is independent of the snapshot `deliveryFrequency`.
pub const CONFIG_HISTORY_DELIVERY_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// The interval a `MaximumExecutionFrequency` value names, or `None` for a value this
/// enum does not publish.
///
/// The five values (`One_Hour`, `Three_Hours`, `Six_Hours`, `Twelve_Hours`,
/// `TwentyFour_Hours`) come from `com.amazonaws.configservice#MaximumExecutionFrequency`
/// and each spells the number of hours between two deliveries.
pub fn delivery_frequency_interval(frequency: &str) -> Option<Duration> {
    let hours: u64 = match frequency {
        "One_Hour" => 1,
        "Three_Hours" => 3,
        "Six_Hours" => 6,
        "Twelve_Hours" => 12,
        "TwentyFour_Hours" => 24,
        _ => return None,
    };
    Some(Duration::from_secs(hours * 3600))
}

/// How long after `StartConfigurationRecorder` the recorder's `Pending` recording
/// event resolves to `Success`.
///
/// The model documents no duration for this, so the span is CHOSEN: it sits inside
/// the 250–750 ms window this project uses for a transition the clock owes, which
/// keeps the transient `Pending` status observable to a caller that describes the
/// status straight after starting the recorder.
pub const RECORDER_STATUS_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most Config rules this account and Region can hold, after which
/// `PutConfigRule` refuses a new rule with `MaxNumberOfConfigRulesExceededException`.
/// 150 is the Config rules service quota (managed plus custom rules per Region).
pub const CONFIG_RULES_QUOTA: usize = 150;

/// The third-party cloud service provider a connector connects to
/// (`com.amazonaws.configservice#Provider`, whose only published value is `AZURE`;
/// `ConnectorSummary$provider`: "Currently, AZURE is supported").
pub const PROVIDER_AZURE: &str = "AZURE";

/// The only filter `ListConnectors` publishes
/// (`com.amazonaws.configservice#ConnectorFilterName`; `ConnectorFilter$filterName`:
/// "Currently, only provider is supported").
pub const CONNECTOR_FILTER_PROVIDER: &str = "provider";

/// How many connectors one `ListConnectors` page carries when the request does not
/// ask for a page size — `MaxResults` defaults to 0, which is "unspecified" rather
/// than "no results". 100 is the `ConnectorSummaries` length maximum (0..100), and
/// also `ListConnectorsMaxResults`'s own maximum, so it is the largest page the model
/// lets one response carry.
pub const LIST_CONNECTORS_PAGE_SIZE: usize = 100;

/// The most connectors this store holds, after which `PutConnector` refuses a new one
/// with `MaxNumberOfConnectorsExceededException`.
///
/// The model carries no number for this quota, so the limit is CHOSEN: it is the
/// `ConnectorSummaries` length maximum (0..100), the most connectors the model can
/// report in one page. Config documents the connector quota per account per Region
/// and one engine's store stands for ONE account in ONE Region, so — like
/// [`CONFIG_RULES_QUOTA`] and [`CONFIGURATION_AGGREGATORS_QUOTA`] — it is enforced
/// over the WHOLE store: the Region a request is scoped to is the caller's own, so
/// counting per Region would let a caller keep one bucket of connectors per Region
/// string it invents. This is also what bounds the work `ListConnectors` does to
/// filter one Region's connectors out of the store.
pub const CONNECTORS_QUOTA: usize = 100;

/// The most customer managed configuration recorders this account and Region can
/// hold. Config allows exactly ONE ("One customer managed configuration recorder per
/// account per Region ... You can have only one customer managed configuration
/// recorder for each account for each Amazon Web Services Region", the
/// `PutConfigurationRecorder` doc), after which it refuses a second with
/// `MaxNumberOfConfigurationRecordersExceededException` — which states this limit
/// (see [`max_configuration_recorders_message`]).
pub const CUSTOMER_MANAGED_RECORDERS_QUOTA: usize = 1;

/// The most SERVICE-LINKED configuration recorders this account and Region can hold,
/// after which `PutServiceLinkedConfigurationRecorder` refuses a new one with
/// `LimitExceededException` (which the model documents for exactly that API: "this
/// exception is thrown if the number of service-linked roles in the account exceeds
/// the limit"), stated by [`service_linked_recorders_quota_message`].
///
/// The model carries no number for this quota, so the limit is CHOSEN — the same way
/// [`CONNECTORS_QUOTA`] is: it is [`LIST_CONFIGURATION_RECORDERS_PAGE_SIZE`], the most
/// recorder summaries one `ListConfigurationRecorders` page can carry, so every
/// service-linked recorder the store holds stays reportable. A recorder's name is
/// derived from the `ServicePrincipal` the request carries, so without this bound a
/// caller could grow the store by inventing service principals; Config documents its
/// recorder quotas per account per Region and one engine's store stands for ONE
/// account in ONE Region, so — like [`CONFIG_RULES_QUOTA`] — it is enforced over the
/// whole store rather than per Region string a caller invents.
pub const SERVICE_LINKED_RECORDERS_QUOTA: usize = LIST_CONFIGURATION_RECORDERS_PAGE_SIZE;

/// The most configuration aggregators this account and Region can hold, after which
/// `PutConfigurationAggregator` refuses a new one with `LimitExceededException`
/// (`LimitExceededException`: "For PutConfigurationAggregator API, this exception is
/// thrown if the number of accounts and aggregators exceeds the limit"). 50 is the
/// Config "maximum number of configuration aggregators" service quota.
pub const CONFIGURATION_AGGREGATORS_QUOTA: usize = 50;

/// How many aggregators one `DescribeConfigurationAggregators` page carries when the
/// request asks for no `Limit` (`DescribeConfigurationAggregatorsRequest$Limit`: "The
/// maximum number of configuration aggregators returned on each page. The default is
/// maximum. If you specify 0, Config uses the default") — the maximum being the
/// modelled `Limit` range maximum.
pub const DESCRIBE_CONFIGURATION_AGGREGATORS_PAGE_SIZE: i32 = 100;

/// How many authorizations one `DescribeAggregationAuthorizations` page carries when
/// the request asks for no `Limit` (`DescribeAggregationAuthorizationsRequest$Limit`:
/// "The maximum number of AggregationAuthorizations returned on each page. The default
/// is maximum. If you specify 0, Config uses the default") — the maximum being the
/// modelled `Limit` range maximum.
pub const DESCRIBE_AGGREGATION_AUTHORIZATIONS_PAGE_SIZE: i32 = 100;

/// The most aggregation authorizations this account and Region can hold, after which
/// `PutAggregationAuthorization` refuses a NEW authorized pair.
///
/// Config publishes no quota for aggregation authorizations, so this is the EMULATOR's
/// own bound, not a documented AWS one: the store's keys are whole-cloth caller input
/// (an authorized account and an authorized Region), so without a cap one caller could
/// grow it one entry per distinct pair without end. It is two orders of magnitude above
/// the aggregator quota Config does publish, so no faithful workload meets it.
pub const AGGREGATION_AUTHORIZATIONS_QUOTA: usize = 5_000;

/// How many rules one `DescribeConfigRules` page carries.
///
/// The operation models no `Limit` member, so the page size is this emulator's:
/// it matches the `ConfigRuleNames` length maximum (0..25) the same operation
/// declares.
pub const DESCRIBE_CONFIG_RULES_PAGE_SIZE: usize = 25;

/// The timeout an evaluation runs under when the request asks for none
/// (`StartResourceEvaluationRequest$EvaluationTimeout`: "The timeout for an
/// evaluation. The default is 900 seconds. ... If you specify 0, Config uses the
/// default").
pub const DEFAULT_EVALUATION_TIMEOUT_SECONDS: i32 = 900;

/// How many resource evaluations one `ListResourceEvaluations` page carries when the
/// request asks for no `Limit` (`ListResourceEvaluationsRequest$Limit`: "The maximum
/// number of evaluations returned on each page. The default is 10. ... If you specify
/// 0, Config uses the default").
pub const LIST_RESOURCE_EVALUATIONS_DEFAULT_LIMIT: usize = 10;

/// The most resource evaluations this store keeps.
///
/// Config publishes no quota for started resource evaluations and
/// `StartResourceEvaluation` models no limit error, so this bound is this emulator's
/// and is CHOSEN: every record retains the caller's `ResourceDetails`, whose
/// `ResourceConfiguration` the model bounds at 51200 characters, and no operation of
/// this service deletes a resource evaluation, so the store is capped to keep this
/// process's memory bounded however many evaluations are started. Starting one past
/// the bound drops the OLDEST evaluation (and the client token that started it)
/// rather than refusing an evaluation AWS accepts, so the list path keeps reporting
/// the most recent evaluations and nothing grows without end. See
/// [`ComAmazonawsConfigserviceStarlingdoveserviceState::record_resource_evaluation`].
pub const RESOURCE_EVALUATIONS_KEPT: usize = 1_000;

/// How many rule evaluation statuses one `DescribeConfigRuleEvaluationStatus` page
/// carries when the request does not ask for a number of its own.
///
/// `RuleLimit`'s modelled default is 0 — "no stated limit" rather than "no results" —
/// so the page size for such a request is this emulator's: it matches the
/// `ConfigRuleNames` length maximum (0..25) the same operation declares.
pub const DESCRIBE_CONFIG_RULE_EVALUATION_STATUS_PAGE_SIZE: usize = 25;

/// How long after a Config rule is created or updated the evaluation Config owes that
/// rule completes.
///
/// Config evaluates a rule as soon as it is written, with no call from the caller
/// (`StartConfigRulesEvaluation`: "You don't need to call the
/// StartConfigRulesEvaluation API to run an evaluation for a new rule. When you create
/// a rule, Config evaluates your resources against the rule automatically."). The
/// model documents no duration for that evaluation, so the span is CHOSEN: it sits
/// inside the 250–750 ms window this project uses for a transition the clock owes,
/// which keeps the not-yet-evaluated rule observable to a caller that reads the
/// evaluation status straight after writing the rule.
pub const RULE_EVALUATION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The `Parameters` key naming the role Config assumes to run a remediation
/// (`PutRemediationConfigurations`: "for automatic remediation configuration, the
/// only valid assumeRole field value is AutomationAssumeRole and you need to provide
/// a value for AutomationAssumeRole to remediate your resources").
pub const AUTOMATION_ASSUME_ROLE_PARAMETER: &str = "AutomationAssumeRole";

/// The other `Parameters` key a MANUAL remediation configuration may name that role
/// with (`PutRemediationConfigurations`: "For manual remediation configuration, you
/// need to provide a value for automationAssumeRole or use a value in the assumeRole
/// field to remediate your resources. The SSM automation document can use either as
/// long as it maps to a valid parameter").
pub const ASSUME_ROLE_PARAMETER: &str = "AssumeRole";

/// The `MaximumAutomaticAttempts` an automatic remediation configuration retries up
/// to when the request names no number
/// (`RemediationConfiguration$MaximumAutomaticAttempts`: "The maximum number of
/// failed attempts for auto-remediation. If you do not select a number, the default
/// is 5").
pub const DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS: i32 = 5;

/// The `RetryAttemptSeconds` window an automatic remediation configuration retries
/// within when the request names no number
/// (`RemediationConfiguration$RetryAttemptSeconds`: "Time window to determine whether
/// or not to add a remediation exception to prevent infinite remediation attempts ...
/// If you do not select a number, the default is 60 seconds").
pub const DEFAULT_RETRY_ATTEMPT_SECONDS: i64 = 60;

/// How many entries one discovered-resource page carries when the request asks for no
/// limit of its own — resource identifiers for `ListDiscoveredResources`
/// (`ListDiscoveredResourcesRequest$limit`: "The maximum number of resource
/// identifiers returned on each page. The default is 100. ... If you specify 0, Config
/// uses the default"), `ResourceCount` objects for `GetDiscoveredResourceCounts`
/// ("By default, Config lists 100 ResourceCount objects on each page").
pub const DISCOVERED_RESOURCES_DEFAULT_LIMIT: i32 = 100;

/// The largest `limit` a discovered-resource page may ask for
/// (`com.amazonaws.configservice#Limit`, range 0..100; the same member's doc: "You
/// cannot specify a number greater than 100"). A value outside the range is what
/// `InvalidLimitException` reports.
pub const DISCOVERED_RESOURCES_MAX_LIMIT: i32 = 100;

/// The most resource IDs one discovered-resource read may narrow by
/// (`ListDiscoveredResourcesRequest$resourceIds`: "You can list a minimum of 1
/// resourceID and a maximum of 20 resourceIds").
pub const DISCOVERED_RESOURCES_MAX_RESOURCE_IDS: usize = 20;

/// How many rules one `DescribeComplianceByConfigRule` page carries.
///
/// That operation models no `Limit` member either, and it pages over the same rules
/// `DescribeConfigRules` does, so it serves them at the same page size.
pub const DESCRIBE_COMPLIANCE_BY_CONFIG_RULE_PAGE_SIZE: usize = DESCRIBE_CONFIG_RULES_PAGE_SIZE;

/// The most tags one Config resource can carry, after which `TagResource` refuses
/// the request with `TooManyTagsException` ("You have reached the limit of the number
/// of tags you can use"). 50 is the tags-per-resource limit, which the model
/// corroborates: every `Tags` member it declares, and `TagResource$Tags` itself, has a
/// `TagList` length maximum of 50, so no resource can be given more than 50 tags in
/// one request either.
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;

/// The largest `Limit` `ListTagsForResource` accepts
/// (`ListTagsForResourceRequest$Limit`: "The limit maximum is 50. You cannot specify a
/// number greater than 50"), past which it answers `InvalidLimitException`. The
/// MODELLED range of `Limit` is 0..100, so a value in 51..=100 clears boundary
/// validation and this limit is what refuses it.
pub const LIST_TAGS_MAX_LIMIT: i32 = 50;

/// How many tags one `ListTagsForResource` page carries when the request leaves
/// `Limit` at its modelled default of 0 (`ListTagsForResourceRequest$Limit`: "If you
/// specify 0, Config uses the default").
///
/// The default itself is not published, so it is this emulator's: the limit maximum
/// the same member documents, [`LIST_TAGS_MAX_LIMIT`].
pub const LIST_TAGS_DEFAULT_LIMIT: i32 = LIST_TAGS_MAX_LIMIT;

/// The most retention configurations this account and Region can hold
/// (`DescribeRetentionConfigurations` and `PutRetentionConfiguration`: "Currently,
/// Config supports only one retention configuration per region in your account").
pub const RETENTION_CONFIGURATIONS_QUOTA: usize = 1;

/// How many retention configurations one `DescribeRetentionConfigurations` page
/// carries.
///
/// The operation models no `Limit` member, so the page size is this emulator's: it
/// matches [`RETENTION_CONFIGURATIONS_QUOTA`], so one page carries every retention
/// configuration the account is allowed to hold.
pub const DESCRIBE_RETENTION_CONFIGURATIONS_PAGE_SIZE: usize = RETENTION_CONFIGURATIONS_QUOTA;

/// The most pending aggregation requests one `DescribePendingAggregationRequests`
/// page carries: the maximum of that operation's own `Limit`
/// (`DescribePendingAggregationRequestsRequest$Limit`, range 0..20, "The default is
/// maximum. If you specify 0, Config uses the default"), so a request that asks for
/// no limit is served this many.
pub const DESCRIBE_PENDING_AGGREGATION_REQUESTS_MAX_LIMIT: usize = 20;

/// Wording for the (unreachable in the assembled emulator) case where this
/// service's store cannot be reached, i.e. the engine is not the shared
/// `InMemoryEngine`.
pub const STATE_UNAVAILABLE: &str =
    "the Config emulator could not reach its state store for this request";

/// The tags on ONE resource, keyed by tag key.
///
/// A map, not the modelled `TagList`, because a resource carries each tag key at most
/// once: re-tagging a key updates its value (`TagResource`: "If existing tags are
/// specified, however, then their values will be updated"). The key ordering is also
/// the stable ordering `ListTagsForResource` pages over, so a page token survives a
/// later tag or untag.
pub type TagSet = BTreeMap<String, String>;

/// The tag set a request's `TagList` describes: the key is taken VERBATIM (so the key
/// a write stores is the key an untag or a read names), the last value wins for a key
/// the list repeats, and an entry carrying no key is dropped (the model leaves
/// `Tag$Key` optional).
pub fn tag_set(tags: &[Tag]) -> TagSet {
    tags.iter()
        .filter_map(|tag| {
            let key = tag.key.as_deref()?;
            Some((key.to_string(), tag.value.clone().unwrap_or_default()))
        })
        .collect()
}

/// A tag set as the modelled `TagList` reports it, in tag-key order.
pub fn tag_list<'a>(tags: impl IntoIterator<Item = (&'a String, &'a String)>) -> Vec<Tag> {
    tags.into_iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// Whether `arn` is a well-formed Amazon Resource Name: the `arn` prefix and six
/// colon-separated fields (`arn:<partition>:<service>:<region>:<account>:<resource>`),
/// with a partition, a service and a resource. Region and account are empty in the
/// ARNs of some services, so neither is required here.
pub fn is_amazon_resource_name(arn: &str) -> bool {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && !fields[2].is_empty()
        && !fields[5].is_empty()
}

/// How Config words `ValidationException` for a tagging request whose `ResourceArn` is
/// not an ARN at all.
///
/// BUILT FROM THE REQUEST — it quotes the value the caller sent and names the member
/// to fix — so every operation that refuses a malformed `ResourceArn` words it the
/// same way.
pub fn malformed_resource_arn_message(arn: &str) -> String {
    format!(
        "The ResourceArn '{arn}' provided in the request is invalid. Please check the \
         resource ARN."
    )
}

/// How Config words `ResourceNotFoundException` for a tagging request whose
/// `ResourceArn` names a resource this account does not hold in this Region.
///
/// BUILT FROM THE REQUEST, for the same reason as
/// [`malformed_resource_arn_message`].
pub fn resource_not_found_message(arn: &str) -> String {
    format!("The resource '{arn}' provided in the request does not exist.")
}

/// One persisted customer managed configuration recorder.
#[derive(Debug, Clone, Default)]
pub struct RecorderEntry {
    pub name: String,
    /// The id Config assigned this recorder when it was created. It is part of the
    /// recorder's ARN and never changes once minted.
    pub id: String,
    pub arn: String,
    pub role_arn: String,
    pub recording_group: Option<RecordingGroup>,
    pub recording_mode: Option<RecordingMode>,
    pub recording_scope: Option<String>,
    pub service_principal: Option<String>,
    /// The third-party cloud service provider whose resources a service-linked
    /// recorder records (`ConfigurationRecorderSummary$provider`: "For service-linked
    /// configuration recorders that record resources from a third-party cloud service
    /// provider, indicates the cloud service provider"), so it stays unset for every
    /// recorder this account creates itself. Only
    /// `PutThirdPartyServiceLinkedConfigurationRecorder` sets it.
    pub provider: Option<String>,
    /// The recorder's tags. `PutConfigurationRecorder` takes them at creation and
    /// does not update them afterwards ("Tags are added at creation and cannot be
    /// updated"); `TagResource` and `UntagResource` are what change them after that
    /// ("Use TagResource and UntagResource to update tags after creation").
    pub tags: TagSet,
}

impl RecorderEntry {
    /// The modelled `ConfigurationRecorder`, read back out of state.
    pub fn to_shape(&self) -> ConfigurationRecorder {
        ConfigurationRecorder {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            role_arn: Some(self.role_arn.clone()),
            recording_group: self.recording_group.clone(),
            recording_mode: self.recording_mode.clone(),
            recording_scope: self.recording_scope.clone(),
            service_principal: self.service_principal.clone(),
            connector_arn: None,
            scope_configuration: None,
        }
    }

    /// The modelled `ConfigurationRecorderSummary`, read back out of state: the
    /// recorder's identity and the scope it records in, without its configuration.
    ///
    /// `recordingScope` is required in the summary while state keeps it optional, so a
    /// recorder stored without one summarises with the scope
    /// [`materialised_recording_scope`] gives it.
    pub fn to_summary(&self) -> ConfigurationRecorderSummary {
        ConfigurationRecorderSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            service_principal: self.service_principal.clone(),
            recording_scope: materialised_recording_scope(self.recording_scope.clone()),
            provider: self.provider.clone(),
        }
    }
}

/// The recording status of one configuration recorder.
#[derive(Debug, Clone, Default)]
pub struct RecorderStatusEntry {
    pub name: String,
    pub arn: String,
    pub recording: bool,
    pub last_start_time: Option<SystemTime>,
    pub last_stop_time: Option<SystemTime>,
    pub last_status: Option<String>,
    pub last_status_change_time: Option<SystemTime>,
    /// When set, the instant at which the `Pending` recording event this recorder
    /// is processing becomes `Success`. Materialised by [`Self::settle`].
    pub status_settles_at: Option<SystemTime>,
}

impl RecorderStatusEntry {
    /// A status record for a recorder that has never been started.
    pub fn new(name: &str, arn: &str) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            recording: false,
            ..Self::default()
        }
    }

    /// A status record for a recorder that is ALREADY recording at `started_at`,
    /// because the service it is linked to turned it on when it created it: the
    /// caller cannot start or stop a service-linked configuration recorder
    /// (`UnmodifiableEntityException`: "For StartConfigurationRecorder and
    /// StopConfigurationRecorder ... these operations do not affect service-linked
    /// configuration recorders"), and the linked service holds it in use
    /// (`ConflictException`, for `DeleteServiceLinkedConfigurationRecorder`: "you
    /// cannot delete the service-linked recorder because it is currently in use by
    /// the linked Amazon Web Services service").
    ///
    /// Like a start the caller asked for, the recording event is `Pending` until the
    /// clock makes the move to `Success` due; [`Self::settle`] materialises it.
    pub fn started(name: &str, arn: &str, started_at: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            recording: true,
            last_start_time: Some(started_at),
            last_stop_time: None,
            last_status: Some(RECORDER_STATUS_PENDING.to_string()),
            last_status_change_time: Some(started_at),
            status_settles_at: Some(started_at + RECORDER_STATUS_SETTLE_AFTER),
        }
    }

    /// Materialise the status change the clock has already made due.
    ///
    /// This is the ONLY write a reporting operation performs: the `Pending` ->
    /// `Success` move was owed at `status_settles_at` whether or not anybody
    /// described the recorder.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.status_settles_at {
            if now >= due {
                self.last_status = Some(RECORDER_STATUS_SUCCESS.to_string());
                self.last_status_change_time = Some(due);
                self.status_settles_at = None;
            }
        }
    }

    /// The modelled `ConfigurationRecorderStatus`, read back out of state.
    pub fn to_shape(&self) -> ConfigurationRecorderStatus {
        ConfigurationRecorderStatus {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            last_start_time: self.last_start_time.map(timestamp),
            last_stop_time: self.last_stop_time.map(timestamp),
            recording: self.recording,
            last_status: self.last_status.clone(),
            last_error_code: None,
            last_error_message: None,
            last_status_change_time: self.last_status_change_time.map(timestamp),
            service_principal: None,
        }
    }
}

/// One persisted delivery channel.
#[derive(Debug, Clone)]
pub struct DeliveryChannelEntry {
    pub name: String,
    pub s3_bucket_name: Option<String>,
    pub s3_key_prefix: Option<String>,
    pub s3_kms_key_arn: Option<String>,
    pub sns_topic_arn: Option<String>,
    pub config_snapshot_delivery_properties: Option<ConfigSnapshotDeliveryProperties>,
    /// When this channel was created. It anchors the channel's periodic delivery
    /// schedule, so the first delivery of each kind falls one interval after it and
    /// `DescribeDeliveryChannelStatus` derives every reported instant from it. An
    /// update keeps the instant the channel was first created.
    pub created_at: SystemTime,
}

impl DeliveryChannelEntry {
    /// The modelled `DeliveryChannel`, read back out of state.
    pub fn to_shape(&self) -> DeliveryChannel {
        DeliveryChannel {
            name: Some(self.name.clone()),
            s3_bucket_name: self.s3_bucket_name.clone(),
            s3_key_prefix: self.s3_key_prefix.clone(),
            s3_kms_key_arn: self.s3_kms_key_arn.clone(),
            sns_topic_arn: self.sns_topic_arn.clone(),
            config_snapshot_delivery_properties: self.config_snapshot_delivery_properties.clone(),
        }
    }

    /// The `deliveryFrequency` this channel delivers configuration snapshots at: the
    /// one its `ConfigSnapshotDeliveryProperties` names, else
    /// [`DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY`].
    pub fn snapshot_delivery_frequency(&self) -> &str {
        self.config_snapshot_delivery_properties
            .as_ref()
            .and_then(|properties| properties.delivery_frequency.as_deref())
            .unwrap_or(DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY)
    }

    /// The interval between two configuration-snapshot deliveries of this channel.
    ///
    /// A `deliveryFrequency` outside the published enum cannot reach state — the
    /// modelled constraint refuses it before a handler runs — so the default interval
    /// stands in for it rather than this returning no schedule at all.
    pub fn snapshot_delivery_interval(&self) -> Duration {
        delivery_frequency_interval(self.snapshot_delivery_frequency()).unwrap_or_else(|| {
            delivery_frequency_interval(DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY)
                .expect("the default snapshot delivery frequency is a published value")
        })
    }

    /// The status of this channel as of `now`, DERIVED from the delivery schedule its
    /// creation instant anchors: nothing here is stored at write time and reading it
    /// changes no state, so two callers reading at the same instant see the same thing.
    pub fn status_at(&self, now: SystemTime) -> DeliveryChannelStatus {
        DeliveryChannelStatus {
            name: Some(self.name.clone()),
            config_snapshot_delivery_info: Some(
                self.export_delivery_info(now, self.snapshot_delivery_interval()),
            ),
            config_history_delivery_info: Some(
                self.export_delivery_info(now, CONFIG_HISTORY_DELIVERY_INTERVAL),
            ),
            config_stream_delivery_info: self.stream_delivery_info(),
        }
    }

    /// The delivery info of one periodic export (the snapshot or the configuration
    /// history) as of `now`, for a schedule that delivers every `interval`.
    ///
    /// Exactly `floor((now - created_at) / interval)` deliveries are behind us, so the
    /// next one falls one interval after the last of them — strictly after `now`, and
    /// never more than one interval after it. Before the first delivery there is no
    /// attempt to report, so only `nextDeliveryTime` carries a value.
    fn export_delivery_info(&self, now: SystemTime, interval: Duration) -> ConfigExportDeliveryInfo {
        let interval_secs = interval.as_secs().max(1);
        let elapsed = now
            .duration_since(self.created_at)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let delivered = elapsed / interval_secs;
        let last_delivery = (delivered > 0)
            .then(|| self.created_at + Duration::from_secs(delivered * interval_secs));
        let next_delivery = self.created_at + Duration::from_secs((delivered + 1) * interval_secs);

        ConfigExportDeliveryInfo {
            last_status: last_delivery.map(|_| DELIVERY_STATUS_SUCCESS.to_string()),
            last_error_code: None,
            last_error_message: None,
            last_attempt_time: last_delivery.map(timestamp),
            last_successful_time: last_delivery.map(timestamp),
            next_delivery_time: Some(timestamp(next_delivery)),
        }
    }

    /// The configuration-stream delivery info of this channel.
    ///
    /// A channel carrying no `snsTopicARN` has SNS delivery turned off, which
    /// `ConfigStreamDeliveryInfo$lastStatus` documents as `Not_Applicable`; that status
    /// was reached when the channel was created without a topic. With a topic
    /// configured the status is the one of the last notification Config streamed, and
    /// this emulator has streamed none, so there is nothing to report.
    fn stream_delivery_info(&self) -> Option<ConfigStreamDeliveryInfo> {
        if self.sns_topic_arn.is_some() {
            return None;
        }
        Some(ConfigStreamDeliveryInfo {
            last_status: Some(DELIVERY_STATUS_NOT_APPLICABLE.to_string()),
            last_error_code: None,
            last_error_message: None,
            last_status_change_time: Some(timestamp(self.created_at)),
        })
    }
}

/// What identifies one evaluation result WITHIN a rule: the resource type, the
/// resource id and the evaluation mode of its `EvaluationResultQualifier`
/// (`EvaluationResultQualifier` carries `ConfigRuleName`, `ResourceType`, `ResourceId`
/// and `EvaluationMode`; the rule name is the map this key indexes). A rule holds ONE
/// result per key — the last one recorded for that resource in that mode.
///
/// Ordering this triple is what gives [`ConfigRuleEntry::evaluation_results`] a stable
/// total order, so a page of evaluation details can resume at a key.
pub type EvaluationResultKey = (String, String, String);

/// One evaluation result recorded against a Config rule.
#[derive(Debug, Clone)]
pub struct EvaluationResultEntry {
    /// The type of the evaluated resource (`EvaluationResultQualifier$ResourceType`).
    pub resource_type: String,
    /// The id of the evaluated resource (`EvaluationResultQualifier$ResourceId`).
    pub resource_id: String,
    /// The mode the rule was evaluated in
    /// (`EvaluationResultQualifier$EvaluationMode`, `DETECTIVE` or `PROACTIVE`).
    pub evaluation_mode: String,
    /// Whether the resource complies (`EvaluationResult$ComplianceType`).
    pub compliance_type: String,
    /// The supplementary information the evaluation carried
    /// (`EvaluationResult$Annotation`), absent when it carried none.
    pub annotation: Option<String>,
    /// The time of the event that triggered the evaluation
    /// (`EvaluationResultIdentifier$OrderingTimestamp`).
    pub ordering_timestamp: SystemTime,
    /// The time the result was recorded (`EvaluationResult$ResultRecordedTime`).
    pub result_recorded_time: SystemTime,
    /// The time the rule that produced the result was invoked
    /// (`EvaluationResult$ConfigRuleInvokedTime`).
    pub config_rule_invoked_time: SystemTime,
    /// The resource evaluation this result came from
    /// (`EvaluationResultIdentifier$ResourceEvaluationId`), set only for a result a
    /// `StartResourceEvaluation` run produced.
    pub resource_evaluation_id: Option<String>,
}

impl EvaluationResultEntry {
    /// The result of evaluating the resource `resource_type`/`resource_id` in
    /// `evaluation_mode` at `at`, carrying no annotation and belonging to no resource
    /// evaluation run.
    ///
    /// One evaluation is triggered, invoked and recorded at the same instant here, so
    /// `at` is all three timestamps; a caller that needs them apart sets the fields.
    pub fn new(
        resource_type: &str,
        resource_id: &str,
        evaluation_mode: &str,
        compliance_type: &str,
        at: SystemTime,
    ) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            evaluation_mode: evaluation_mode.to_string(),
            compliance_type: compliance_type.to_string(),
            annotation: None,
            ordering_timestamp: at,
            result_recorded_time: at,
            config_rule_invoked_time: at,
            resource_evaluation_id: None,
        }
    }

    /// The key this result is stored under, which is its identity within a rule.
    pub fn key(&self) -> EvaluationResultKey {
        (
            self.resource_type.clone(),
            self.resource_id.clone(),
            self.evaluation_mode.clone(),
        )
    }

    /// The modelled `EvaluationResult` for the rule named `config_rule_name`, read
    /// back out of state.
    ///
    /// `ResultToken` stays unset: the token associates an evaluation with the
    /// `PutEvaluations` call that delivered it, and the read path does not hand one
    /// back.
    pub fn to_shape(&self, config_rule_name: &str) -> crate::ops::types::EvaluationResult {
        crate::ops::types::EvaluationResult {
            evaluation_result_identifier: Some(crate::ops::types::EvaluationResultIdentifier {
                evaluation_result_qualifier: Some(crate::ops::types::EvaluationResultQualifier {
                    config_rule_name: Some(config_rule_name.to_string()),
                    resource_type: Some(self.resource_type.clone()),
                    resource_id: Some(self.resource_id.clone()),
                    evaluation_mode: Some(self.evaluation_mode.clone()),
                }),
                ordering_timestamp: Some(timestamp(self.ordering_timestamp)),
                resource_evaluation_id: self.resource_evaluation_id.clone(),
            }),
            compliance_type: Some(self.compliance_type.clone()),
            result_recorded_time: Some(timestamp(self.result_recorded_time)),
            config_rule_invoked_time: Some(timestamp(self.config_rule_invoked_time)),
            annotation: self.annotation.clone(),
            result_token: None,
        }
    }
}

/// The invocation-and-evaluation history of one Config rule, as
/// `DescribeConfigRuleEvaluationStatus` reports it.
///
/// An evaluation pass visits the recorded configuration state of the resources in the
/// rule's scope and records one evaluation result per resource it placed
/// (`StartConfigRulesEvaluation`: it "re-runs an evaluation against the last known
/// state of your resources"). This emulator holds no recorded configuration items yet
/// — `PutResourceConfig` is still scaffolded — so a pass visits nothing, runs no rule
/// logic at all and records no evaluation result, which is why a rule's compliance
/// stays `INSUFFICIENT_DATA` ([`ConfigRuleEntry::compliance`]) while its pass
/// completes successfully.
#[derive(Debug, Clone, Default)]
pub struct RuleEvaluationEntry {
    /// When set, the instant Config invoked the evaluation pass this rule is owed.
    /// Recorded when the pass starts, so the instant the rule reports never depends
    /// on the span the pass takes.
    pub invoked_at: Option<SystemTime>,
    /// When set, the instant at which the evaluation pass this rule is owed
    /// completes. Materialised by [`Self::settle`].
    pub completes_at: Option<SystemTime>,
    /// Whether a pass has ever completed for this rule
    /// (`ConfigRuleEvaluationStatus$FirstEvaluationStarted`: "Config has evaluated
    /// your Amazon Web Services resources against the rule at least once").
    pub first_evaluation_started: bool,
    /// When Config last invoked the rule, i.e. when the last completed pass started.
    pub last_successful_invocation_at: Option<SystemTime>,
    /// When the last completed pass finished evaluating the resources in scope.
    pub last_successful_evaluation_at: Option<SystemTime>,
}

impl RuleEvaluationEntry {
    /// The evaluation history of a rule Config has just invoked an evaluation pass
    /// for, which is every rule a write creates.
    pub fn invoked_at(at: SystemTime) -> Self {
        let mut evaluation = Self::default();
        evaluation.invoke(at);
        evaluation
    }

    /// Record that Config owes this rule an evaluation pass, invoked at `at`.
    pub fn invoke(&mut self, at: SystemTime) {
        self.invoked_at = Some(at);
        self.completes_at = Some(at + RULE_EVALUATION_SETTLE_AFTER);
    }

    /// Whether the pass this rule is owed has still not completed as of `now`.
    ///
    /// Reading the pending pass is what tells a caller the rule's reported instants
    /// are the previous pass's, not this one's.
    pub fn in_progress(&self, now: SystemTime) -> bool {
        self.completes_at.is_some_and(|due| now < due)
    }

    /// Materialise the completion the clock has already made due.
    ///
    /// This is the ONLY write a reporting operation performs on a rule: the pass was
    /// owed at `completes_at` whether or not anybody read the rule's evaluation
    /// status. A pass that is still running ([`Self::in_progress`]) is left alone, so
    /// the rule keeps reporting the previous pass's instants until this one is due.
    pub fn settle(&mut self, now: SystemTime) {
        if self.in_progress(now) {
            return;
        }
        if let Some(due) = self.completes_at {
            self.first_evaluation_started = true;
            // The invocation instant is the one recorded when the pass started; a
            // pass always has one, so an absent `invoked_at` can only mean the
            // completion instant itself.
            self.last_successful_invocation_at = Some(self.invoked_at.unwrap_or(due));
            self.last_successful_evaluation_at = Some(due);
            self.invoked_at = None;
            self.completes_at = None;
        }
    }
}

/// One persisted Config rule.
#[derive(Debug, Clone)]
pub struct ConfigRuleEntry {
    pub name: String,
    pub arn: String,
    pub id: String,
    pub description: Option<String>,
    pub scope: Option<Scope>,
    pub source: Source,
    pub input_parameters: Option<String>,
    pub maximum_execution_frequency: Option<String>,
    pub state: String,
    pub evaluation_modes: Vec<EvaluationModeConfiguration>,
    pub rule_evaluation_visibility: Option<String>,
    pub created_at: SystemTime,
    /// The rule's tags, taken at creation by `PutConfigRule` and changed after that
    /// by `TagResource` and `UntagResource` (`PutConfigRule`: "Use TagResource and
    /// UntagResource to update tags after creation").
    pub tags: TagSet,
    /// The last evaluation result recorded for each resource this rule evaluated,
    /// keyed by [`EvaluationResultKey`] so the map is the single record of both what
    /// `GetComplianceDetailsByConfigRule` reports and what the rule's own compliance
    /// rolls up from ([`Self::compliance`]).
    ///
    /// No operation of this emulator can record one yet: `PutEvaluations` identifies
    /// the rule only through a `ResultToken` minted by the Lambda invocation that
    /// produced the evaluation, which this emulator does not run, so it stays
    /// scaffolded. A rule therefore carries no results — an empty page of evaluation
    /// details, and a roll-up of `INSUFFICIENT_DATA` — until an evaluation writer
    /// lands.
    pub evaluation_results: BTreeMap<EvaluationResultKey, EvaluationResultEntry>,
    /// When Config invoked this rule and when those invocations finished evaluating
    /// the resources in scope, as `DescribeConfigRuleEvaluationStatus` reports it.
    pub evaluation: RuleEvaluationEntry,
}

impl ConfigRuleEntry {
    /// The modelled `ConfigRule` as a READ reports it.
    ///
    /// `Source.CustomPolicyDetails.PolicyText` is reported EMPTY: the describe path
    /// does not hand the stored Guard policy body back, which is why the model gives
    /// `PolicyText` a zero minimum length and why the body has an operation of its
    /// own (`GetCustomRulePolicy`). The stored body is untouched — only this
    /// projection of it is blank.
    pub fn to_describe_shape(&self) -> ConfigRule {
        let mut source = self.source.clone();
        if let Some(custom_policy) = source.custom_policy_details.as_mut() {
            custom_policy.policy_text = String::new();
        }
        ConfigRule {
            config_rule_name: Some(self.name.clone()),
            config_rule_arn: Some(self.arn.clone()),
            config_rule_id: Some(self.id.clone()),
            description: self.description.clone(),
            scope: self.scope.clone(),
            source,
            input_parameters: self.input_parameters.clone(),
            maximum_execution_frequency: self.maximum_execution_frequency.clone(),
            config_rule_state: Some(self.state.clone()),
            created_by: None,
            evaluation_modes: Some(self.evaluation_modes.clone()),
            rule_evaluation_visibility: self.rule_evaluation_visibility.clone(),
        }
    }

    /// Whether `DescribeConfigRuleEvaluationStatus` reports a status for this rule.
    ///
    /// Config reports one for its managed rules and for Custom Policy rules only
    /// (`ConfigRuleEvaluationStatus`: "Status information for your Config Managed
    /// rules and Config Custom Policy rules. … This operation does not return status
    /// information about Config Custom Lambda rules.").
    pub fn reports_evaluation_status(&self) -> bool {
        self.source.owner != OWNER_CUSTOM_LAMBDA
    }

    /// The modelled `ConfigRuleEvaluationStatus` as a READ reports it.
    ///
    /// `FirstActivatedTime` is when this rule was written, which is when Config
    /// activated it. The failure members (`LastFailedInvocationTime`,
    /// `LastFailedEvaluationTime`, `LastErrorCode`, `LastErrorMessage`) are reported
    /// ABSENT because no evaluation pass of this emulator fails: there is no rule
    /// logic to fail and no resource to fail on. The debug-log members
    /// (`LastDebugLogDeliveryStatus`, `LastDebugLogDeliveryStatusReason`,
    /// `LastDebugLogDeliveryTime`) are absent for the same reason — nothing has
    /// attempted to deliver a Custom Policy debug log, which is the delivery path
    /// `DeliverConfigSnapshot` owns and which is still scaffolded. `LastDeactivatedTime`
    /// is absent because no operation of this service turns a rule off: `DeleteConfigRule`
    /// removes it outright.
    pub fn to_evaluation_status_shape(&self) -> ConfigRuleEvaluationStatus {
        ConfigRuleEvaluationStatus {
            config_rule_name: Some(self.name.clone()),
            config_rule_arn: Some(self.arn.clone()),
            config_rule_id: Some(self.id.clone()),
            first_activated_time: Some(timestamp(self.created_at)),
            last_deactivated_time: None,
            first_evaluation_started: self.evaluation.first_evaluation_started,
            last_successful_invocation_time: self
                .evaluation
                .last_successful_invocation_at
                .map(timestamp),
            last_successful_evaluation_time: self
                .evaluation
                .last_successful_evaluation_at
                .map(timestamp),
            last_failed_invocation_time: None,
            last_failed_evaluation_time: None,
            last_error_code: None,
            last_error_message: None,
            last_debug_log_delivery_status: None,
            last_debug_log_delivery_status_reason: None,
            last_debug_log_delivery_time: None,
        }
    }

    /// This rule's `ComplianceType`: the roll-up of the evaluation results recorded
    /// for it — noncompliant if any evaluated resource is noncompliant, compliant if
    /// at least one is compliant and none is not, and `INSUFFICIENT_DATA` when no
    /// result places it either way (which includes a rule nothing has evaluated).
    pub fn compliance(&self) -> &'static str {
        // One roll-up rule for the whole service: see
        // [`resource_evaluation::roll_up_compliance`].
        resource_evaluation::roll_up_compliance(
            self.evaluation_results
                .values()
                .map(|recorded| recorded.compliance_type.as_str()),
        )
    }

    /// This rule's modelled `Compliance`: the `ComplianceType` it rolls up to
    /// ([`Self::compliance`]) and how many resources contribute that result.
    ///
    /// `Compliance$ComplianceContributorCount` is "the number of Amazon Web Services
    /// resources or Config rules that cause a result of NON_COMPLIANT", so it is
    /// reported only where such resources exist — a rule no evaluation result places
    /// as noncompliant carries no contributor count — and it is counted up to
    /// [`COMPLIANCE_RESOURCE_CAP`] because the contributors counted here are
    /// resources.
    pub fn compliance_shape(&self) -> Compliance {
        let contributors = self
            .evaluation_results
            .values()
            .filter(|recorded| recorded.compliance_type == COMPLIANCE_TYPE_NON_COMPLIANT)
            .count();
        Compliance {
            compliance_type: Some(self.compliance().to_string()),
            compliance_contributor_count: (contributors > 0).then(|| {
                capped_count(
                    i64::try_from(contributors).unwrap_or(i64::MAX),
                    COMPLIANCE_RESOURCE_CAP,
                )
            }),
        }
    }
}

/// One persisted connector to a third-party cloud service provider.
#[derive(Debug, Clone)]
pub struct ConnectorEntry {
    /// The connector's name. `PutConnector` takes no name, so Config derives it from
    /// the configuration the connector was created with ([`connector_name`]).
    pub name: String,
    pub arn: String,
    /// The Region this connector belongs to — the Region of the request that created
    /// it, and the Region segment of [`Self::arn`]. A connector is a REGIONAL
    /// resource, so every operation serves only the connectors of the Region its
    /// request is scoped to.
    pub region: String,
    /// The `Provider` this connector's configuration names.
    pub provider: String,
    /// The provider-specific configuration the connector was created with. Connectors
    /// cannot be updated (`PutConnector`: "To update the connector configuration, you
    /// must delete all associated configuration recorders, delete the connector, and
    /// recreate it"), so this never changes once stored.
    pub connector_configuration: ConnectorConfiguration,
    pub created_at: SystemTime,
    /// Tags are taken at creation (`PutConnector`: "Tags are added at creation and
    /// cannot be updated with this operation").
    pub tags: Vec<Tag>,
}

impl ConnectorEntry {
    /// The identifier of the tenant this connector's configuration names, which is
    /// the identifier of the provider configuration it carries.
    pub fn tenant_identifier(&self) -> &str {
        self.connector_configuration
            .azure
            .as_ref()
            .map(|azure| azure.tenant_identifier.as_str())
            .unwrap_or_default()
    }

    /// The modelled `Connector`, read back out of state.
    pub fn to_shape(&self) -> Connector {
        Connector {
            name: self.name.clone(),
            arn: self.arn.clone(),
            connector_configuration: self.connector_configuration.clone(),
            created_time: timestamp(self.created_at),
        }
    }

    /// The modelled `ConnectorSummary` a listing reports for this connector.
    pub fn to_summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            provider: self.provider.clone(),
            tenant_identifier: self.tenant_identifier().to_string(),
            created_time: timestamp(self.created_at),
        }
    }
}

/// One persisted remediation configuration: the remediation action Config runs for
/// one Config rule.
///
/// Config holds at most one remediation configuration per Config rule — a second
/// `PutRemediationConfigurations` for the same rule UPDATES the first, and
/// `DeleteRemediationConfiguration` names only the rule — so the store keys them by
/// `ConfigRuleName`.
#[derive(Debug, Clone, Default)]
pub struct RemediationConfigurationEntry {
    pub config_rule_name: String,
    /// The id Config assigned this remediation configuration when it was created. It
    /// is part of the configuration's ARN and never changes once minted, so an update
    /// keeps the ARN the create reported.
    pub id: String,
    pub arn: String,
    pub target_type: String,
    pub target_id: String,
    pub target_version: Option<String>,
    /// The remediation parameters, keyed by parameter name. A `BTreeMap` so the
    /// stored configuration has one stable ordering however the request spelled it.
    pub parameters: BTreeMap<String, RemediationParameterValue>,
    pub resource_type: Option<String>,
    pub automatic: bool,
    pub execution_controls: Option<ExecutionControls>,
    pub maximum_automatic_attempts: Option<i32>,
    pub retry_attempt_seconds: Option<i64>,
}

impl RemediationConfigurationEntry {
    /// The remediation configuration `requested` creates, carrying the id and ARN
    /// Config assigned it.
    pub fn new(id: String, arn: String, requested: &RemediationConfiguration) -> Self {
        let mut entry = Self {
            config_rule_name: requested.config_rule_name.trim().to_string(),
            id,
            arn,
            ..Self::default()
        };
        entry.update_from(requested);
        entry
    }

    /// Apply `requested` to a remediation configuration that already exists, which
    /// keeps the id and ARN minted when it was created.
    pub fn update_from(&mut self, requested: &RemediationConfiguration) {
        self.target_type = requested.target_type.clone();
        self.target_id = requested.target_id.clone();
        self.target_version = requested.target_version.clone();
        self.parameters = requested
            .parameters
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        self.resource_type = requested.resource_type.clone();
        self.automatic = requested.automatic;
        self.execution_controls = requested.execution_controls.clone();
        // Both retry members govern AUTO-remediation, so Config's documented defaults
        // apply to a configuration that remediates automatically; a manual
        // configuration carries only what the request named.
        self.maximum_automatic_attempts = requested
            .maximum_automatic_attempts
            .or_else(|| requested.automatic.then_some(DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS));
        self.retry_attempt_seconds = requested
            .retry_attempt_seconds
            .or_else(|| requested.automatic.then_some(DEFAULT_RETRY_ATTEMPT_SECONDS));
    }

    /// The modelled `RemediationConfiguration`, read back out of state.
    ///
    /// `CreatedByService` is reported ABSENT: it names the service that owns a
    /// service-linked rule, and `PutRemediationConfigurations` "does not support
    /// adding remediation configurations for service-linked Config Rules", so a
    /// configuration created through that API belongs to the account, not a service.
    pub fn to_shape(&self) -> RemediationConfiguration {
        RemediationConfiguration {
            config_rule_name: self.config_rule_name.clone(),
            target_type: self.target_type.clone(),
            target_id: self.target_id.clone(),
            target_version: self.target_version.clone(),
            parameters: if self.parameters.is_empty() {
                None
            } else {
                Some(
                    self.parameters
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect(),
                )
            },
            resource_type: self.resource_type.clone(),
            automatic: self.automatic,
            execution_controls: self.execution_controls.clone(),
            maximum_automatic_attempts: self.maximum_automatic_attempts,
            retry_attempt_seconds: self.retry_attempt_seconds,
            arn: Some(self.arn.clone()),
            created_by_service: None,
        }
    }
}

/// One resource Config has discovered, as its configuration recorder captured it.
///
/// A discovered resource SURVIVES its own deletion in Config's inventory —
/// `ListDiscoveredResources` can be asked to include deleted resources, and reports
/// `resourceDeletionTime` for each one — so a deletion is recorded as the instant the
/// resource went away rather than by dropping the entry.
#[derive(Debug, Clone, Default)]
pub struct DiscoveredResourceEntry {
    /// The resource's type, a `com.amazonaws.configservice#ResourceType` value.
    pub resource_type: String,
    /// The id of the resource (`ResourceIdentifier$resourceId`).
    pub resource_id: String,
    /// The resource's custom name, when it has one
    /// (`ResourceIdentifier$resourceName`: "The custom name of the resource (if
    /// available)").
    pub resource_name: Option<String>,
    /// When set, the instant the resource was deleted
    /// (`ResourceIdentifier$resourceDeletionTime`: "The time that the resource was
    /// deleted").
    pub deleted_at: Option<SystemTime>,
    /// The `SchemaVersionId` the resource's configuration item was recorded under,
    /// for a resource whose configuration state `PutResourceConfig` recorded.
    pub schema_version_id: Option<String>,
    /// The `Configuration` document as it was recorded, byte for byte.
    pub configuration: Option<String>,
    /// The tags the configuration item was recorded with
    /// (`PutResourceConfigRequest$Tags`).
    pub tags: BTreeMap<String, String>,
    /// When Config first recorded a configuration item for this resource, and when it
    /// last did. The first instant is kept across a re-record of the same resource,
    /// which only moves the last one. Both are `None` for a resource no configuration
    /// item has been recorded for.
    pub first_recorded_at: Option<SystemTime>,
    pub last_recorded_at: Option<SystemTime>,
}

impl DiscoveredResourceEntry {
    /// The modelled `ResourceIdentifier`, read back out of state.
    pub fn to_identifier(&self) -> ResourceIdentifier {
        ResourceIdentifier {
            resource_type: Some(self.resource_type.clone()),
            resource_id: Some(self.resource_id.clone()),
            resource_name: self.resource_name.clone(),
            resource_deletion_time: self.deleted_at.map(timestamp),
        }
    }
}

/// One persisted retention configuration: the name Config gave the object and the
/// number of days it keeps historical information for.
#[derive(Debug, Clone, Default)]
pub struct RetentionConfigurationEntry {
    pub name: String,
    pub retention_period_in_days: i32,
}

impl RetentionConfigurationEntry {
    /// The modelled `RetentionConfiguration`, read back out of state.
    pub fn to_shape(&self) -> RetentionConfiguration {
        RetentionConfiguration {
            name: self.name.clone(),
            retention_period_in_days: self.retention_period_in_days,
        }
    }
}

/// One pending aggregation request this account holds.
///
/// A pending aggregation request is an aggregator account that has asked to aggregate
/// this account's data and has not been authorized yet
/// (`PendingAggregationRequest`: "An object that represents the account ID and region
/// of an aggregator account that is requesting authorization but is not yet
/// authorized"). The REQUESTER is therefore another account's aggregator, and the
/// call that creates the entry — that account's `PutConfigurationAggregator`, naming
/// this account as a source — reaches that account's Config, not this one: one
/// engine's store stands for ONE account in ONE Region, so no operation served here
/// mints one. `DeletePendingAggregationRequest` is the modelled operation that
/// removes one, and `DescribePendingAggregationRequests` reports what the store
/// holds, which for an account nobody has asked to aggregate is nothing.
#[derive(Debug, Clone, Default)]
pub struct PendingAggregationRequestEntry {
    /// The 12-digit account id of the account requesting to aggregate data
    /// (`PendingAggregationRequest$RequesterAccountId`).
    pub requester_account_id: String,
    /// The Region requesting to aggregate data
    /// (`PendingAggregationRequest$RequesterAwsRegion`).
    pub requester_aws_region: String,
}

impl PendingAggregationRequestEntry {
    /// A pending request from the aggregator account `requester_account_id` in
    /// `requester_aws_region`.
    pub fn new(requester_account_id: &str, requester_aws_region: &str) -> Self {
        Self {
            requester_account_id: requester_account_id.to_string(),
            requester_aws_region: requester_aws_region.to_string(),
        }
    }

    /// The key this entry is stored under: the (account, Region) pair that identifies
    /// the requesting aggregator, which is exactly what
    /// `DeletePendingAggregationRequest` names.
    pub fn key(&self) -> (String, String) {
        (
            self.requester_account_id.clone(),
            self.requester_aws_region.clone(),
        )
    }

    /// The modelled `PendingAggregationRequest`, read back out of state.
    pub fn to_shape(&self) -> PendingAggregationRequest {
        PendingAggregationRequest {
            requester_account_id: Some(self.requester_account_id.clone()),
            requester_aws_region: Some(self.requester_aws_region.clone()),
        }
    }
}

/// One persisted configuration aggregator.
///
/// An aggregator aggregates EITHER individual source accounts
/// ([`Self::account_aggregation_sources`]) OR an organization
/// ([`Self::organization_aggregation_source`]) — never both, which is what
/// `PutConfigurationAggregator` refuses.
#[derive(Debug, Clone)]
pub struct ConfigurationAggregatorEntry {
    pub name: String,
    /// The id Config assigned this aggregator when it was created
    /// (`config-aggregator-<base-36>`). It is the resource segment of the
    /// aggregator's ARN and never changes once minted.
    pub id: String,
    pub arn: String,
    pub account_aggregation_sources: Vec<AccountAggregationSource>,
    pub organization_aggregation_source: Option<OrganizationAggregationSource>,
    pub aggregator_filters: Option<AggregatorFilters>,
    pub created_at: SystemTime,
    /// When the aggregator's sources or filters were last replaced. Equals
    /// [`Self::created_at`] until the first update.
    pub last_updated_at: SystemTime,
    /// Tags are taken at creation and not updated afterwards
    /// (`PutConfigurationAggregator`: "Tags are added at creation and cannot be
    /// updated with this operation ... tags will not be updated, even if they are
    /// different. Use TagResource and UntagResource to update tags after creation"),
    /// which is what changes them from then on.
    pub tags: TagSet,
}

impl ConfigurationAggregatorEntry {
    /// The modelled `ConfigurationAggregator`, read back out of state.
    ///
    /// `CreatedBy` names the Amazon Web Services service that created the aggregator,
    /// so it stays unset for one a caller created.
    pub fn to_shape(&self) -> ConfigurationAggregator {
        ConfigurationAggregator {
            configuration_aggregator_name: Some(self.name.clone()),
            configuration_aggregator_arn: Some(self.arn.clone()),
            account_aggregation_sources: (!self.account_aggregation_sources.is_empty())
                .then(|| self.account_aggregation_sources.clone()),
            organization_aggregation_source: self.organization_aggregation_source.clone(),
            creation_time: Some(timestamp(self.created_at)),
            last_updated_time: Some(timestamp(self.last_updated_at)),
            created_by: None,
            aggregator_filters: self.aggregator_filters.clone(),
        }
    }
}

/// The key one resource evaluation is listed at: the instant its execution started,
/// MOST RECENT FIRST, with the id breaking ties so the ordering is total and one page
/// token names exactly one evaluation.
pub type ResourceEvaluationPageKey = (::std::cmp::Reverse<i128>, String);

/// One resource evaluation `StartResourceEvaluation` started.
///
/// The evaluation's OUTCOME is not part of this record: deciding whether the resource
/// details comply means running every proactive Config rule against them, which this
/// emulator has no rule engine for — the same reason
/// [`ConfigRuleEntry::evaluation_results`] holds only the results something recorded.
/// `GetResourceEvaluationSummary` therefore DERIVES the status and the compliance it
/// reports from this record, the rules in state and the clock (see
/// [`resource_evaluation`]), and an execution that would have to run a rule's
/// implementation is reported as the modelled failure rather than as a synthesised
/// verdict. What is persisted here is what the control plane owns and what the list
/// path reports: the id Config minted, the mode the evaluation runs in, when its
/// execution started, the deployment context it belongs to, the resource details it
/// was handed and the timeout it runs under.
#[derive(Debug, Clone)]
pub struct ResourceEvaluationEntry {
    /// The `ResourceEvaluationId` Config minted for this execution.
    pub id: String,
    pub evaluation_mode: String,
    pub started_at: SystemTime,
    /// The `EvaluationContext$EvaluationContextIdentifier` the request carried, which
    /// is what `ResourceEvaluationFilters$EvaluationContextIdentifier` filters on.
    pub evaluation_context_identifier: Option<String>,
    pub resource_details: ResourceDetails,
    pub evaluation_timeout_seconds: i32,
    /// The `ClientToken` this evaluation was started with, if any; the token index
    /// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::resource_evaluation_tokens`])
    /// points back at this entry.
    pub client_token: Option<String>,
}

impl ResourceEvaluationEntry {
    /// The modelled `ResourceEvaluation`, read back out of state.
    pub fn to_shape(&self) -> ResourceEvaluation {
        ResourceEvaluation {
            resource_evaluation_id: Some(self.id.clone()),
            evaluation_mode: Some(self.evaluation_mode.clone()),
            evaluation_start_timestamp: Some(timestamp(self.started_at)),
        }
    }

    /// The instant this evaluation's execution started, as nanoseconds since the Unix
    /// epoch.
    pub fn started_at_nanos(&self) -> i128 {
        epoch_nanos(timestamp(self.started_at))
    }

    /// See [`ResourceEvaluationPageKey`].
    pub fn page_key(&self) -> ResourceEvaluationPageKey {
        (::std::cmp::Reverse(self.started_at_nanos()), self.id.clone())
    }

    /// Whether this evaluation was started with exactly the parameters `other` carries
    /// — what decides between a retry carrying the same `ClientToken` succeeding
    /// idempotently and `IdempotentParameterMismatch`.
    pub fn started_with(
        &self,
        evaluation_mode: &str,
        resource_details: &ResourceDetails,
        evaluation_context_identifier: Option<&str>,
        evaluation_timeout_seconds: i32,
    ) -> bool {
        let details = &self.resource_details;
        self.evaluation_mode == evaluation_mode
            && self.evaluation_timeout_seconds == evaluation_timeout_seconds
            && self.evaluation_context_identifier.as_deref() == evaluation_context_identifier
            && details.resource_id == resource_details.resource_id
            && details.resource_type == resource_details.resource_type
            && details.resource_configuration == resource_details.resource_configuration
            && details.resource_configuration_schema_type
                == resource_details.resource_configuration_schema_type
    }
}

/// One persisted aggregation authorization.
///
/// An authorization is identified by the PAIR it authorizes — the aggregator account
/// and the aggregator Region (`AggregationAuthorization`: "the authorizations granted
/// to aggregator accounts and regions") — which is also what its ARN carries, so the
/// pair is the key [`ComAmazonawsConfigserviceStarlingdoveserviceState::aggregation_authorizations`]
/// holds it under.
#[derive(Debug, Clone)]
pub struct AggregationAuthorizationEntry {
    pub authorized_account_id: String,
    pub authorized_aws_region: String,
    pub arn: String,
    pub created_at: SystemTime,
    /// Tags are taken at creation and not updated afterwards
    /// (`PutAggregationAuthorization`: "Tags are added at creation and cannot be
    /// updated with this operation ... tags will not be updated, even if they are
    /// different. Use TagResource and UntagResource to update tags after creation").
    pub tags: Vec<Tag>,
}

impl AggregationAuthorizationEntry {
    /// The modelled `AggregationAuthorization`, read back out of state.
    pub fn to_shape(&self) -> AggregationAuthorization {
        AggregationAuthorization {
            aggregation_authorization_arn: Some(self.arn.clone()),
            authorized_account_id: Some(self.authorized_account_id.clone()),
            authorized_aws_region: Some(self.authorized_aws_region.clone()),
            creation_time: Some(timestamp(self.created_at)),
        }
    }
}

/// Per-service state for com-amazonaws-configservice-starlingdoveservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Every configuration recorder of this account and Region, keyed by name: the
    /// customer managed one `PutConfigurationRecorder` creates and the service-linked
    /// ones `PutServiceLinkedConfigurationRecorder` creates, which carry a
    /// `service_principal`.
    ///
    /// BOUNDED at [`CUSTOMER_MANAGED_RECORDERS_QUOTA`] customer managed entries plus
    /// [`SERVICE_LINKED_RECORDERS_QUOTA`] service-linked ones — one engine's store
    /// stands for one account in one Region, and both per-Region quotas are enforced
    /// over the whole store, so no sequence of calls can grow this map further.
    pub recorders: HashMap<String, RecorderEntry>,
    /// The status record of each recorder, keyed by recorder name. Holds at most one
    /// entry per recorder in [`Self::recorders`], so the same bound holds.
    pub recorder_status: HashMap<String, RecorderStatusEntry>,
    /// Delivery channels, keyed by name. Config allows one per account per Region.
    pub delivery_channels: HashMap<String, DeliveryChannelEntry>,
    /// Config rules, keyed by `ConfigRuleName`. A `BTreeMap` so `DescribeConfigRules`
    /// pages over a stable ordering and a page token survives inserts and deletes.
    pub config_rules: BTreeMap<String, ConfigRuleEntry>,
    /// Remediation configurations, keyed by the `ConfigRuleName` they remediate —
    /// Config holds at most one per rule.
    pub remediation_configurations: BTreeMap<String, RemediationConfigurationEntry>,
    /// Source of the next remediation configuration id, so every remediation
    /// configuration Config creates here gets a distinct one.
    pub next_remediation_configuration_seq: u64,
    /// Source of the next `ConfigRuleId` suffix, so every rule gets a distinct id.
    pub next_rule_seq: u64,
    /// Source of the next configuration recorder id, so every recorder Config
    /// creates here gets a distinct one.
    pub next_recorder_seq: u64,
    /// Connectors to third-party cloud service providers, keyed by connector ARN. The
    /// ARN carries both the Region the connector belongs to and its name, so the key
    /// is what identifies one connector of one account in one Region. A `BTreeMap` so
    /// `ListConnectors` pages over a stable ordering — which, within one Region, is
    /// the connectors' name ordering — and a page token survives connectors created
    /// and deleted between pages.
    pub connectors: BTreeMap<String, ConnectorEntry>,
    /// Resource evaluations started by `StartResourceEvaluation`, keyed by
    /// `ResourceEvaluationId`. A `BTreeMap` so the store's iteration order is stable;
    /// `ListResourceEvaluations` reports them in [`ResourceEvaluationPageKey`] order.
    ///
    /// BOUNDED at [`RESOURCE_EVALUATIONS_KEPT`] entries: write it through
    /// [`Self::record_resource_evaluation`], which drops the oldest evaluation past
    /// that bound, so repeated starts cannot grow this process's state without end.
    pub resource_evaluations: BTreeMap<String, ResourceEvaluationEntry>,
    /// The `ResourceEvaluationId` each `ClientToken` already started an evaluation
    /// for, so a retry with the same token is idempotent rather than a second
    /// execution.
    ///
    /// Holds at most one entry per evaluation in [`Self::resource_evaluations`], so it
    /// is bounded by the same [`RESOURCE_EVALUATIONS_KEPT`]: dropping an evaluation
    /// drops the token that started it, and a caller varying only the token cannot
    /// grow this index on its own.
    pub resource_evaluation_tokens: HashMap<String, String>,
    /// Source of the next `ResourceEvaluationId`, so every evaluation gets a distinct
    /// one.
    pub next_resource_evaluation_seq: u64,
    /// The resources Config has discovered in this account and Region, keyed by
    /// `(resourceType, resourceId)` — the pair that identifies a resource in Config,
    /// which is why a resource read takes both. A `BTreeMap` so a discovered-resource
    /// read pages over a stable ordering (by type, then by resource id) and a page
    /// token survives resources recorded or deleted between pages.
    ///
    /// A deleted resource stays in the inventory carrying its deletion time, because a
    /// discovered-resource read can be asked to include deleted resources.
    ///
    /// `PutResourceConfig` is what WRITES the inventory, recording the configuration
    /// state of one custom resource; `DeleteResourceConfig`, which records a
    /// resource's deletion, is still a scaffold with a stub issue of its own, so an
    /// account that has recorded nothing has discovered nothing and a
    /// discovered-resource read reports an empty page for every type.
    pub discovered_resources: BTreeMap<(String, String), DiscoveredResourceEntry>,
    /// Retention configurations, keyed by `RetentionConfigurationName`. A `BTreeMap`
    /// so `DescribeRetentionConfigurations` pages over a stable ordering; it holds at
    /// most [`RETENTION_CONFIGURATIONS_QUOTA`] entries, because one engine's store
    /// stands for one account in one Region.
    pub retention_configurations: BTreeMap<String, RetentionConfigurationEntry>,
    /// Aggregator accounts that have asked to aggregate this account's data and are
    /// not authorized yet, keyed by the (requester account, requester Region) pair
    /// that identifies one. A `BTreeMap` so `DescribePendingAggregationRequests`
    /// pages over a stable ordering and a page token survives entries added or
    /// removed between pages. See [`PendingAggregationRequestEntry`] for which call
    /// creates one.
    pub pending_aggregation_requests: BTreeMap<(String, String), PendingAggregationRequestEntry>,
    /// Configuration aggregators, keyed by `ConfigurationAggregatorName`. A
    /// `BTreeMap` so `DescribeConfigurationAggregators` pages over a stable ordering
    /// and a page token survives aggregators created and deleted between pages.
    pub configuration_aggregators: BTreeMap<String, ConfigurationAggregatorEntry>,
    /// Source of the next configuration aggregator id, so every aggregator gets a
    /// distinct one.
    pub next_aggregator_seq: u64,
    /// Aggregation authorizations, keyed by
    /// [`aggregation_authorization_key`]. A `BTreeMap` so
    /// `DescribeAggregationAuthorizations` pages over a stable ordering and a page
    /// token survives authorizations granted and deleted between pages.
    pub aggregation_authorizations: BTreeMap<String, AggregationAuthorizationEntry>,
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Record `entry` as a started resource evaluation, with the `ClientToken` that
    /// started it indexed back to it.
    ///
    /// This is the ONE place [`Self::resource_evaluations`] and
    /// [`Self::resource_evaluation_tokens`] are written, so the two cannot drift: past
    /// [`RESOURCE_EVALUATIONS_KEPT`] entries it drops the OLDEST evaluation together
    /// with the token that started it, keeping both stores bounded whatever a caller
    /// does.
    pub fn record_resource_evaluation(&mut self, entry: ResourceEvaluationEntry) {
        if let Some(token) = entry.client_token.clone() {
            self.resource_evaluation_tokens
                .insert(token, entry.id.clone());
        }
        self.resource_evaluations.insert(entry.id.clone(), entry);

        while self.resource_evaluations.len() > RESOURCE_EVALUATIONS_KEPT {
            // One insert can take the store at most one entry past the bound, so this
            // runs once; it is a loop so the bound holds however the store was filled.
            // The oldest execution, by the instant it started and then by id so the
            // choice is deterministic when two started in the same instant.
            let oldest = self
                .resource_evaluations
                .values()
                .min_by(|left, right| {
                    (left.started_at_nanos(), left.id.as_str())
                        .cmp(&(right.started_at_nanos(), right.id.as_str()))
                })
                .map(|entry| (entry.id.clone(), entry.client_token.clone()));
            let Some((id, client_token)) = oldest else {
                break;
            };
            self.resource_evaluations.remove(&id);
            if let Some(token) = client_token {
                self.resource_evaluation_tokens.remove(&token);
            }
        }
    }

    /// The configuration recorder that is RECORDING right now, if any: Config
    /// discovers resources only while a recorder is turned on.
    pub fn recording_recorder(&self) -> Option<&RecorderEntry> {
        self.recorder_status
            .values()
            .filter(|status| status.recording)
            .find_map(|status| self.recorders.get(&status.name))
    }

    /// How many discovered resources Config holds of each resource type it is
    /// RECORDING, in the order `GetDiscoveredResourceCounts` reports them: descending
    /// by the number of resources (`ResourceCounts`: "Each object is listed in
    /// descending order by the number of resources"), and by resource type among the
    /// types with equally many, so the ordering — and therefore a page token — is
    /// total.
    ///
    /// A type the recording recorder does not record contributes nothing, and no type
    /// at all is reported while no recorder is recording.
    pub fn discovered_resource_counts(&self) -> Vec<(String, i64)> {
        let mut per_type: BTreeMap<&str, i64> = BTreeMap::new();
        for entry in self.discovered_resources.values() {
            // A resource Config recorded the deletion of is no longer one of the
            // resources it is recording, so it contributes to no count.
            if entry.deleted_at.is_some() {
                continue;
            }
            *per_type.entry(entry.resource_type.as_str()).or_default() += 1;
        }
        // Which recorder is recording is one question for the whole call, so resolve
        // it once rather than per resource type.
        let recording_group = self
            .recording_recorder()
            .map(|recorder| recorder.recording_group.as_ref());
        let records = |resource_type: &str| match recording_group {
            None => false,
            Some(None) => true,
            Some(Some(group)) => group_records_resource_type(group, resource_type),
        };
        let mut counts: Vec<(String, i64)> = per_type
            .into_iter()
            .filter(|(resource_type, _)| records(resource_type))
            .map(|(resource_type, count)| (resource_type.to_string(), count))
            .collect();
        counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        counts
    }

    /// The tags of the resource `arn` identifies, or `None` when this store holds no
    /// resource with that ARN.
    ///
    /// This is the ONE place an ARN is resolved to a tag set, so the tagging
    /// operations agree with each other and with the resource's own read path: the
    /// tags live ON the resource, which is also why deleting the resource deletes its
    /// tags (`TagResource`: "When a resource is deleted, the tags associated with that
    /// resource are deleted as well"). Every taggable resource this emulator persists
    /// is reached here; a resource type whose write operation is still scaffolded
    /// cannot be in the store at all, so its ARN resolves to `None` exactly as an
    /// ARN of a deleted resource does.
    pub fn tags_of(&self, arn: &str) -> Option<&TagSet> {
        if let Some(recorder) = self.recorders.values().find(|entry| entry.arn == arn) {
            return Some(&recorder.tags);
        }
        if let Some(rule) = self.config_rules.values().find(|entry| entry.arn == arn) {
            return Some(&rule.tags);
        }
        if let Some(aggregator) = self
            .configuration_aggregators
            .values()
            .find(|entry| entry.arn == arn)
        {
            return Some(&aggregator.tags);
        }
        None
    }

    /// The tags of the resource `arn` identifies, for a request that changes them.
    /// See [`Self::tags_of`].
    pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut TagSet> {
        if let Some(recorder) = self.recorders.values_mut().find(|entry| entry.arn == arn) {
            return Some(&mut recorder.tags);
        }
        if let Some(rule) = self.config_rules.values_mut().find(|entry| entry.arn == arn) {
            return Some(&mut rule.tags);
        }
        if let Some(aggregator) = self
            .configuration_aggregators
            .values_mut()
            .find(|entry| entry.arn == arn)
        {
            return Some(&mut aggregator.tags);
        }
        None
    }
}

// The part of the resource-evaluation surface the SUMMARY derives — the status an
// execution has reached and the compliance it places the resource at — lives in
// `state/resource_evaluation.rs` and is re-exported here, so a handler reaches its
// names as `crate::state::<name>`. Declared beside the store accessor, away from the
// import list, the constants and the test module where other fixes land.
pub mod resource_evaluation;
pub use resource_evaluation::*;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConfigserviceStarlingdoveserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConfigserviceStarlingdoveserviceState>)
}

/// The region this request is scoped to: the region of its SigV4 credential scope,
/// or [`DEFAULT_REGION`] when the request carries none.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN resource type of a configuration recorder, as Config spells it in the
/// ARNs it mints.
pub const RECORDER_ARN_RESOURCE_TYPE: &str = "configuration-recorder";

/// The ARN of a configuration recorder:
/// `arn:aws:config:<region>:<account>:configuration-recorder/<name>/<id>`. Both the
/// recorder's name and the id Config assigned it are part of the ARN.
pub fn recorder_arn(region: &str, name: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RECORDER_ARN_RESOURCE_TYPE}/{name}/{id}"),
    )
}

/// The id Config assigns the `seq`-th configuration recorder created in this
/// emulator: sixteen base-36 characters, the shape Config's own recorder ids take.
///
/// [`splitmix64`] is a bijection on `u64`, so distinct sequence numbers give distinct
/// ids; it is only there to spread the digits, because an id is opaque and a caller
/// must not be able to read a creation order out of it.
pub fn mint_recorder_id(seq: u64) -> String {
    base36(splitmix64(seq), 16)
}

/// SplitMix64's finalising mix: a bijection on `u64`.
fn splitmix64(seq: u64) -> u64 {
    let mut z = seq.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The DNS suffix every Amazon Web Services service principal ends with, which is
/// what makes a value a service principal rather than an arbitrary string the
/// modelled `ServicePrincipal` pattern (`^[\w+=,.@-]+$`, which also admits `@`, `+`,
/// `=` and `,`) would accept.
pub const SERVICE_PRINCIPAL_SUFFIX: &str = ".amazonaws.com";

/// Config's own service principal, which is also the path segment IAM files Config's
/// service-linked role under.
pub const CONFIG_SERVICE_PRINCIPAL: &str = "config.amazonaws.com";

/// The IAM service-linked role Config creates in the account the first time a
/// service-linked configuration recorder is created, and the role every
/// service-linked configuration recorder then records with
/// (`PutServiceLinkedConfigurationRecorder`: "This API creates a service-linked role
/// AWSServiceRoleForConfig in your account. The service-linked role is created only
/// when the role does not exist in your account").
pub const CONFIG_SERVICE_LINKED_ROLE_NAME: &str = "AWSServiceRoleForConfig";

/// The ARN of [`CONFIG_SERVICE_LINKED_ROLE_NAME`]:
/// `arn:aws:iam::<account>:role/aws-service-role/config.amazonaws.com/AWSServiceRoleForConfig`,
/// the path IAM gives a service-linked role. IAM is global, so the ARN carries no
/// Region.
///
/// The role is created once per account and reused, so every service-linked
/// configuration recorder of one account records with the SAME role ARN.
pub fn config_service_linked_role_arn() -> String {
    ::emulator_server::emulator_arn(
        "iam",
        "",
        &format!(
            "role/aws-service-role/{CONFIG_SERVICE_PRINCIPAL}/{CONFIG_SERVICE_LINKED_ROLE_NAME}"
        ),
    )
}

/// The name Config assigns the service-linked configuration recorder of
/// `service_principal`, or `None` when the value names no Amazon Web Services service
/// principal.
///
/// The name is NOT chosen by the caller: Config assigns one carrying the reserved
/// prefix ([`SERVICE_LINKED_RECORDER_NAME_PREFIX`];
/// `PutServiceLinkedConfigurationRecorderResponse$Name`: "Config automatically
/// assigns a name that has the prefix \"AWSConfigurationRecorderFor\" to the new
/// service-linked configuration recorder"). The model documents no further structure,
/// so the suffix is DERIVED FROM THE REQUEST and from nothing else: the leading DNS
/// label of the service principal — the service's own name, the part that
/// distinguishes `ssm.amazonaws.com` from `securityhub.amazonaws.com` — with its
/// `-`/`_`-separated words upper-camel-cased, so one service principal always gets
/// one name and two different services never share one.
///
/// A value that is no service principal gets no name: it must end in
/// [`SERVICE_PRINCIPAL_SUFFIX`] and every label before that must be a DNS label.
pub fn service_linked_recorder_name(service_principal: &str) -> Option<String> {
    let host = service_principal.strip_suffix(SERVICE_PRINCIPAL_SUFFIX)?;
    if host.is_empty() || host.split('.').any(|label| !is_dns_label(label)) {
        return None;
    }
    let service = host.split('.').next()?;
    let suffix: String = service
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(capitalise)
        .collect();
    if suffix.is_empty() {
        return None;
    }
    Some(format!("{SERVICE_LINKED_RECORDER_NAME_PREFIX}{suffix}"))
}

/// Whether `label` is one DNS label of a service principal: ASCII alphanumerics,
/// `-` and `_`, neither empty nor hyphen-bounded.
fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `word` with its first character upper-cased and the rest left as it came.
fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
    }
}

/// How Config words `ValidationException` for a request whose `ServicePrincipal`
/// names no Amazon Web Services service (`ValidationException`: "You have provided a
/// service principal for service-linked configuration recorder that is not valid").
///
/// BUILT FROM THE REQUEST — it quotes the value the caller sent and names the member
/// to fix — so every operation that takes a service principal refuses it the same
/// way.
pub fn invalid_service_principal_message(service_principal: &str) -> String {
    format!(
        "The ServicePrincipal '{service_principal}' provided in the request is not a valid \
         service principal for a service-linked configuration recorder."
    )
}

/// How Config words `ConflictException` when the account already holds a
/// service-linked configuration recorder for the service a request names
/// (`ConflictException`: "For PutServiceLinkedConfigurationRecorder, you cannot
/// create a service-linked recorder because a service-linked recorder already exists
/// for the specified service").
///
/// BUILT FROM THE REQUEST, for the same reason as
/// [`invalid_service_principal_message`].
pub fn service_linked_recorder_conflict_message(service_principal: &str) -> String {
    format!(
        "A service-linked configuration recorder already exists for the service principal \
         '{service_principal}'."
    )
}

/// How Config words `LimitExceededException` when creating one more service-linked
/// configuration recorder would take the account past
/// [`SERVICE_LINKED_RECORDERS_QUOTA`].
///
/// The message states THE SERVICE'S OWN LIMIT, not anything from the request — the
/// same shape as [`max_configuration_recorders_message`] — so it is the same sentence
/// however the caller reached the limit.
pub fn service_linked_recorders_quota_message() -> String {
    format!(
        "Failed to create a service-linked configuration recorder because this account \
         already holds the maximum of {SERVICE_LINKED_RECORDERS_QUOTA} service-linked \
         configuration recorder(s) in this region"
    )
}

/// The ARN resource type of a connector, as Config spells it in the ARNs it mints.
pub const CONNECTOR_ARN_RESOURCE_TYPE: &str = "connector";

/// The `ConnectorName` length maximum (`com.amazonaws.configservice#ConnectorName`,
/// length 1..256), which is also the length of the resource portion of a connector's
/// ARN, because that portion is exactly the connector's name.
pub const CONNECTOR_NAME_MAX_LEN: usize = 256;

/// The ARN of a connector: `arn:aws:config:<region>:<account>:connector/<name>`.
pub fn connector_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONNECTOR_ARN_RESOURCE_TYPE}/{name}"),
    )
}

/// The name of the connector a configuration describes: the provider it connects to
/// and the identifier of the tenant within that provider.
///
/// `PutConnector` carries no name, so the name is DERIVED from the configuration —
/// which is also why one tenant can have only one connector: a second connector for
/// the same provider configuration would need the same name.
pub fn connector_name(provider: &str, tenant_identifier: &str) -> String {
    format!("{}-{tenant_identifier}", provider.to_lowercase())
}

/// The Region and the connector name an `Arn` names, or `None` when the value is not
/// a well-formed connector ARN of this service.
///
/// A well-formed ARN of another partition, Region or account parses here and simply
/// names no connector this store holds — "not found" rather than "not valid".
pub fn connector_arn_parts(arn: &str) -> Option<(&str, &str)> {
    let mut segments = arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    let partition = segments.next()?;
    let service = segments.next()?;
    let region = segments.next()?;
    let account = segments.next()?;
    let resource = segments.next()?;
    // A connector is a regional resource, so its ARN names a Region.
    if partition.is_empty() || service != ARN_SERVICE || region.is_empty() || account.is_empty() {
        return None;
    }
    let name = resource
        .strip_prefix(CONNECTOR_ARN_RESOURCE_TYPE)?
        .strip_prefix('/')?;
    // `ConnectorName` is 1..256 characters, and the resource portion of a connector
    // ARN is exactly that name.
    if name.is_empty() || name.len() > CONNECTOR_NAME_MAX_LEN || name.contains('/') {
        return None;
    }
    Some((region, name))
}

/// Why `arn` is not a well-formed connector ARN. BUILT FROM THE REQUEST: it quotes
/// the value the caller sent and names the form the member takes, so every operation
/// that takes a connector ARN refuses a malformed one the same way.
pub fn invalid_connector_arn_message(arn: &str) -> String {
    format!(
        "The Arn '{arn}' provided in the request is not a valid connector ARN. Please \
         provide an ARN of the form \
         arn:aws:{ARN_SERVICE}:<region>:<account>:{CONNECTOR_ARN_RESOURCE_TYPE}/<name>."
    )
}

/// How this service words a connector that no longer, or never did, exist. Built
/// from the request for the same reason as [`invalid_connector_arn_message`].
pub fn no_such_connector_message(arn: &str) -> String {
    format!("The connector '{arn}' provided in the request does not exist.")
}

/// The ARN resource type of a configuration aggregator, as Config spells it in the
/// ARNs it mints (`ConfigurationAggregatorArn`'s pattern:
/// `config-aggregator/config-aggregator-[a-z\d]+`).
pub const AGGREGATOR_ARN_RESOURCE_TYPE: &str = "config-aggregator";

/// The ARN of a configuration aggregator:
/// `arn:aws:config:<region>:<account>:config-aggregator/config-aggregator-<id>`.
pub fn configuration_aggregator_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{AGGREGATOR_ARN_RESOURCE_TYPE}/{id}"),
    )
}

/// The id Config assigns the `seq`-th configuration aggregator created in this
/// emulator: `config-aggregator-` plus eight base-36 characters, the shape the
/// `ConfigurationAggregatorArn` pattern allows (`config-aggregator-[a-z\d]+`).
///
/// [`splitmix64`] spreads the digits so a caller cannot read a creation order out of
/// an id; being a bijection on `u64`, distinct sequence numbers still give distinct
/// ids.
pub fn mint_configuration_aggregator_id(seq: u64) -> String {
    format!("{AGGREGATOR_ARN_RESOURCE_TYPE}-{}", base36(splitmix64(seq), 8))
}

/// The ARN resource type of an aggregation authorization, as Config spells it in the
/// ARNs it mints: an aggregation authorization's ARN is
/// `arn:aws:config:<region>:<account>:aggregation-authorization/`
/// `<authorizedAccountId>/<authorizedAwsRegion>` (the AWS Config resource ARN format
/// for `AggregationAuthorization`).
pub const AGGREGATION_AUTHORIZATION_ARN_RESOURCE_TYPE: &str = "aggregation-authorization";

/// The ARN of the aggregation authorization granted to `authorized_account_id` in
/// `authorized_aws_region`, held by the account and Region the request was signed for.
///
/// The authorized pair is what identifies the authorization, so it is what the ARN
/// carries: nothing is minted, and the same pair always names the same ARN.
pub fn aggregation_authorization_arn(
    region: &str,
    authorized_account_id: &str,
    authorized_aws_region: &str,
) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{AGGREGATION_AUTHORIZATION_ARN_RESOURCE_TYPE}/{authorized_account_id}/\
             {authorized_aws_region}"
        ),
    )
}

/// The key one aggregation authorization is held under: the authorized account id and
/// the authorized Region, in that order, so `DescribeAggregationAuthorizations` pages
/// by account and then by Region. `AccountId`'s `^\d{12}$` pattern leaves no room for
/// the separator in the account id, so distinct pairs give distinct keys.
pub fn aggregation_authorization_key(
    authorized_account_id: &str,
    authorized_aws_region: &str,
) -> String {
    format!("{authorized_account_id}/{authorized_aws_region}")
}

/// The ARN of a Config rule: `arn:aws:config:<region>:<account>:config-rule/<id>`.
pub fn config_rule_arn(region: &str, rule_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("config-rule/{rule_id}"))
}

/// The ARN resource type of a remediation configuration, as Config spells it in the
/// ARNs it mints.
pub const REMEDIATION_CONFIGURATION_ARN_RESOURCE_TYPE: &str = "remediation-configuration";

/// The ARN of a remediation configuration:
/// `arn:aws:config:<region>:<account>:remediation-configuration/<rule-name>/<id>`.
/// Both the remediated rule's name and the id Config assigned the configuration are
/// part of the ARN.
pub fn remediation_configuration_arn(region: &str, config_rule_name: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{REMEDIATION_CONFIGURATION_ARN_RESOURCE_TYPE}/{config_rule_name}/{id}"),
    )
}

/// The id Config assigns the `seq`-th remediation configuration created in this
/// emulator: sixteen base-36 characters, minted the same opaque way a configuration
/// recorder id is (see [`mint_recorder_id`]).
pub fn mint_remediation_configuration_id(seq: u64) -> String {
    base36(splitmix64(seq ^ 0x5245_4d45_4449_4154), 16)
}

/// Whether `parameter` carries a value at all: a static value with at least one
/// non-blank entry, or a dynamic (resource) value.
fn parameter_carries_value(parameter: &RemediationParameterValue) -> bool {
    parameter
        .static_value
        .as_ref()
        .is_some_and(|static_value| {
            static_value
                .values
                .iter()
                .any(|value| !value.trim().is_empty())
        })
        || parameter
            .resource_value
            .as_ref()
            .is_some_and(|resource_value| !resource_value.value.trim().is_empty())
}

/// The `Parameters` keys that can carry the role a remediation runs as: an AUTOMATIC
/// configuration remediates only through [`AUTOMATION_ASSUME_ROLE_PARAMETER`] ("the
/// only valid assumeRole field value is AutomationAssumeRole"), a MANUAL one through
/// either key, as the SSM document's own parameter names decide.
pub fn assume_role_parameter_names(automatic: bool) -> &'static [&'static str] {
    if automatic {
        &[AUTOMATION_ASSUME_ROLE_PARAMETER]
    } else {
        &[AUTOMATION_ASSUME_ROLE_PARAMETER, ASSUME_ROLE_PARAMETER]
    }
}

/// Whether `requested` provides the role Config assumes to remediate a resource,
/// under one of the keys [`assume_role_parameter_names`] accepts for it. Every
/// remediation configuration needs one: it is what the remediation runs as.
pub fn has_assume_role(requested: &RemediationConfiguration) -> bool {
    let Some(parameters) = requested.parameters.as_ref() else {
        return false;
    };
    assume_role_parameter_names(requested.automatic)
        .iter()
        .any(|name| parameters.get(*name).is_some_and(parameter_carries_value))
}

/// The name of a parameter of `requested` that does not select exactly ONE kind of
/// value, with what is wrong with it.
///
/// `RemediationParameterValue` is "either a dynamic (resource) value or a static
/// value. You must select either a dynamic value or a static value" — so a parameter
/// that selects both, or neither, is not a value Config can resolve at remediation
/// time. The modelled `Parameters` map is a `HashMap`, so the names are sorted before
/// they are visited: a request with several bad parameters is then always refused for
/// the same one.
pub fn ill_formed_parameter(requested: &RemediationConfiguration) -> Option<(String, &'static str)> {
    let parameters = requested.parameters.as_ref()?;
    let mut names: Vec<&String> = parameters.keys().collect();
    names.sort();
    names.into_iter().find_map(|name| {
        let parameter = parameters.get(name)?;
        match (
            parameter.static_value.is_some(),
            parameter.resource_value.is_some(),
        ) {
            (true, true) => Some((name.clone(), "both a StaticValue and a ResourceValue")),
            (false, false) => Some((name.clone(), "neither a StaticValue nor a ResourceValue")),
            _ => None,
        }
    })
}

/// The `ConfigRuleId` for the `seq`-th rule created in this emulator:
/// `config-rule-<six base-36 characters>`, the shape Config's own rule ids take.
pub fn mint_config_rule_id(seq: u64) -> String {
    format!("config-rule-{}", base36(seq, 6))
}

/// `value` in base 36 (digits then lowercase letters), left-padded to `width`.
fn base36(mut value: u64, width: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::with_capacity(width);
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

/// The recording group Config records with when a recorder is put without one
/// (`PutConfigurationRecorder`: "Default is to record all supported resource types,
/// excluding the global IAM resource types").
pub fn default_recording_group() -> RecordingGroup {
    RecordingGroup {
        all_supported: true,
        include_global_resource_types: false,
        resource_types: None,
        exclusion_by_resource_types: None,
        recording_strategy: Some(RecordingStrategy {
            use_only: Some(RECORDING_STRATEGY_ALL_SUPPORTED.to_string()),
        }),
    }
}

/// The recording strategy a recording group RECORDS UNDER: the one the group states
/// itself, or the one its resource type lists imply ([`implied_recording_strategy`]).
pub fn recording_strategy_of(group: &RecordingGroup) -> &str {
    group
        .recording_strategy
        .as_ref()
        .and_then(|strategy| strategy.use_only.as_deref())
        .filter(|use_only| !use_only.is_empty())
        .unwrap_or_else(|| implied_recording_strategy(group))
}

/// Whether a recording group covers `resource_type`: only the types an inclusion
/// group enumerates, every type an exclusion group does not exclude, and every type
/// under `ALL_SUPPORTED_RESOURCE_TYPES`.
pub fn group_records_resource_type(group: &RecordingGroup, resource_type: &str) -> bool {
    let listed = |resource_types: Option<&[String]>| {
        resource_types
            .unwrap_or_default()
            .iter()
            .any(|listed| listed == resource_type)
    };
    match recording_strategy_of(group) {
        RECORDING_STRATEGY_INCLUSION => listed(group.resource_types.as_deref()),
        RECORDING_STRATEGY_EXCLUSION => !listed(
            group
                .exclusion_by_resource_types
                .as_ref()
                .and_then(|exclusion| exclusion.resource_types.as_deref()),
        ),
        // `ALL_SUPPORTED_RESOURCE_TYPES`, and any strategy this emulator does not
        // know: the recorder records every type it discovers.
        _ => true,
    }
}

/// The recording strategy a group's own resource type lists imply: the exclusion
/// strategy when the group excludes resource types, the inclusion strategy when it
/// enumerates them, and `ALL_SUPPORTED_RESOURCE_TYPES` otherwise.
fn implied_recording_strategy(group: &RecordingGroup) -> &'static str {
    let excludes = group
        .exclusion_by_resource_types
        .as_ref()
        .and_then(|exclusion| exclusion.resource_types.as_deref())
        .is_some_and(|resource_types| !resource_types.is_empty());
    let includes = group
        .resource_types
        .as_deref()
        .is_some_and(|resource_types| !resource_types.is_empty());
    if excludes {
        RECORDING_STRATEGY_EXCLUSION
    } else if includes {
        RECORDING_STRATEGY_INCLUSION
    } else {
        RECORDING_STRATEGY_ALL_SUPPORTED
    }
}

/// The recording group a recorder RECORDS WITH, derived from the one a request
/// carried (or its absence).
///
/// Config stores the recorder as a complete recording group rather than as the
/// subset the caller sent, so a read reports concrete values: the recording strategy
/// implied by the group — the exclusion strategy when the group excludes resource
/// types, the inclusion strategy when it enumerates them, and
/// `ALL_SUPPORTED_RESOURCE_TYPES` otherwise — and an empty exclusion list when the
/// group excludes nothing. A value the request carried is kept as it was sent.
pub fn materialised_recording_group(requested: Option<RecordingGroup>) -> RecordingGroup {
    let mut group = requested.unwrap_or_else(default_recording_group);

    if group.recording_strategy.is_none() {
        group.recording_strategy = Some(RecordingStrategy {
            use_only: Some(implied_recording_strategy(&group).to_string()),
        });
    }
    if group.exclusion_by_resource_types.is_none() {
        group.exclusion_by_resource_types = Some(ExclusionByResourceTypes {
            resource_types: Some(Vec::new()),
        });
    }
    group
}

/// The recording mode a recorder RECORDS WITH, derived from the one a request
/// carried (or its absence): continuous recording with no per-resource-type
/// overrides unless the request asked for another frequency or listed overrides.
pub fn materialised_recording_mode(requested: Option<RecordingMode>) -> RecordingMode {
    let mut mode = requested.unwrap_or_else(|| RecordingMode {
        recording_frequency: RECORDING_FREQUENCY_CONTINUOUS.to_string(),
        recording_mode_overrides: None,
    });
    if mode.recording_frequency.is_empty() {
        mode.recording_frequency = RECORDING_FREQUENCY_CONTINUOUS.to_string();
    }
    if mode.recording_mode_overrides.is_none() {
        mode.recording_mode_overrides = Some(Vec::new());
    }
    mode
}

/// The recording scope a CUSTOMER MANAGED recorder records in: the one the request
/// carried, or [`RECORDING_SCOPE_PAID`].
pub fn materialised_recording_scope(requested: Option<String>) -> String {
    requested
        .filter(|scope| !scope.is_empty())
        .unwrap_or_else(|| RECORDING_SCOPE_PAID.to_string())
}

/// The compliance roll-up of `rules`: how many are compliant and how many are
/// noncompliant, each counted up to [`COMPLIANCE_SUMMARY_RULE_CAP`], as of `at`.
pub fn summarise_rule_compliance<'a>(
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
    at: SystemTime,
) -> ComplianceSummary {
    let mut compliant = 0i64;
    let mut non_compliant = 0i64;
    for rule in rules {
        match rule.compliance() {
            COMPLIANCE_TYPE_COMPLIANT => compliant += 1,
            COMPLIANCE_TYPE_NON_COMPLIANT => non_compliant += 1,
            // A rule no evaluation result places either way contributes to neither
            // count.
            _ => {}
        }
    }
    ComplianceSummary {
        compliant_resource_count: Some(capped_count(compliant, COMPLIANCE_SUMMARY_RULE_CAP)),
        non_compliant_resource_count: Some(capped_count(
            non_compliant,
            COMPLIANCE_SUMMARY_RULE_CAP,
        )),
        compliance_summary_timestamp: Some(timestamp(at)),
    }
}

/// `count` contributors reported as a `ComplianceContributorCount` against `cap`: the
/// count itself while it is under the cap, the cap once it is past it, and
/// `CapExceeded` set exactly then.
///
/// The cap is a parameter because Config reports a count of rules and a count of
/// resources up to different maxima ([`COMPLIANCE_SUMMARY_RULE_CAP`] and
/// [`COMPLIANCE_RESOURCE_CAP`]), while the capping rule itself is the same one.
fn capped_count(count: i64, cap: i32) -> ComplianceContributorCount {
    let limit = i64::from(cap);
    ComplianceContributorCount {
        capped_count: i32::try_from(count.min(limit)).unwrap_or(cap),
        cap_exceeded: count > limit,
    }
}

/// The evaluation modes a rule carries when it is put without any
/// (`ConfigRule$EvaluationModes`: "By default, the value is Detective evaluation
/// mode only").
pub fn default_evaluation_modes() -> Vec<EvaluationModeConfiguration> {
    vec![EvaluationModeConfiguration {
        mode: Some(EVALUATION_MODE_DETECTIVE.to_string()),
    }]
}

/// A modelled timestamp for a wall-clock instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// Encode the key the next page of a paged read should resume at as an opaque
/// `NextToken`, with the base64 codec the framework already carries. Every paged read
/// of this service pages over its store's own key ordering, so one codec serves them
/// all.
pub fn encode_page_token(resume_at: &str) -> String {
    ::aws_smithy_types::base64::encode(resume_at)
}

/// The key a `NextToken` resumes at, or `None` when the token is malformed — i.e.
/// not something [`encode_page_token`] produced.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::aws_smithy_types::base64::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// The `ResourceEvaluationId` Config assigns the `seq`-th resource evaluation started
/// in this emulator.
///
/// The model gives `ResourceEvaluationId` no pattern (only a 1..128 length), so the id
/// takes the opaque sixteen-base-36-character form this service already mints ids in
/// (see [`mint_recorder_id`]). [`splitmix64`] is a bijection on `u64`, so distinct
/// sequence numbers give distinct ids; it spreads the digits so a caller cannot read a
/// start order out of an id.
pub fn mint_resource_evaluation_id(seq: u64) -> String {
    // A different mixing input from `mint_recorder_id`'s, so the n-th evaluation and
    // the n-th recorder do not carry the same opaque id.
    base36(splitmix64(seq ^ 0x5265_736f_7572_6345), 16)
}

/// `at` as nanoseconds since the Unix epoch: one totally ordered integer for an
/// instant, so a time-window filter and a page ordering compare instants the same way,
/// at the full resolution a modelled timestamp carries.
pub fn epoch_nanos(at: ::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}

/// Encode the [`ResourceEvaluationPageKey`] a `ListResourceEvaluations` page should
/// resume at as an opaque `NextToken`.
pub fn encode_resource_evaluation_token(resume_at: &ResourceEvaluationPageKey) -> String {
    encode_page_token(&format!("{}:{}", resume_at.0 .0, resume_at.1))
}

/// The [`ResourceEvaluationPageKey`] a `NextToken` resumes at, or `None` when the
/// token is malformed — i.e. not something [`encode_resource_evaluation_token`]
/// produced.
pub fn decode_resource_evaluation_token(token: &str) -> Option<ResourceEvaluationPageKey> {
    let decoded = decode_page_token(token)?;
    let (nanos, id) = decoded.split_once(':')?;
    let nanos: i128 = nanos.parse().ok()?;
    if id.is_empty() {
        return None;
    }
    Some((::std::cmp::Reverse(nanos), id.to_string()))
}

/// How Config words `NoAvailableConfigurationRecorderException` on an operation for
/// which no per-operation text is known: a paraphrase of the shape's own doc ("There
/// are no customer managed configuration recorders available to record your
/// resources. Use the PutConfigurationRecorder operation to create the customer
/// managed configuration recorder").
///
/// `PutConfigRule` and `PutDeliveryChannel` do NOT use this: each carries the fixed
/// message AWS emits for its own precondition
/// ([`NO_RECORDER_TO_PUT_CONFIG_RULE`], [`NO_RECORDER_TO_PUT_DELIVERY_CHANNEL`]), so
/// a client can tell from the message which precondition failed.
pub const NO_AVAILABLE_CONFIGURATION_RECORDER_MESSAGE: &str =
    "there are no customer managed configuration recorders available; use \
     PutConfigurationRecorder to create one";

/// The separator between the two fields of the key a pending aggregation request page
/// resumes at. An `AccountId` is twelve digits (`PendingAggregationRequest$
/// RequesterAccountId`, pattern `^\d{12}$`), so it never contains this character and
/// the key splits unambiguously.
const PENDING_AGGREGATION_REQUEST_KEY_SEPARATOR: char = '#';

/// Encode the (account, Region) key a `DescribePendingAggregationRequests` page should
/// resume at as an opaque `NextToken`.
pub fn encode_pending_aggregation_request_token(key: &(String, String)) -> String {
    encode_page_token(&format!(
        "{}{PENDING_AGGREGATION_REQUEST_KEY_SEPARATOR}{}",
        key.0, key.1
    ))
}

/// The (account, Region) key a `NextToken` resumes at, or `None` when the token is
/// malformed — i.e. not something [`encode_pending_aggregation_request_token`]
/// produced, so it does not decode to an account id of twelve digits and a non-empty
/// Region.
pub fn decode_pending_aggregation_request_token(token: &str) -> Option<(String, String)> {
    let key = decode_page_token(token)?;
    let (account, region) = key.split_once(PENDING_AGGREGATION_REQUEST_KEY_SEPARATOR)?;
    let well_formed = account.len() == 12
        && account.chars().all(|digit| digit.is_ascii_digit())
        && !region.is_empty();
    well_formed.then(|| (account.to_string(), region.to_string()))
}

/// Encode the [`EvaluationResultKey`] a `GetComplianceDetailsByConfigRule` page should
/// resume at as an opaque `NextToken`.
///
/// The three fields are LENGTH-PREFIXED rather than joined on a separator:
/// `ResourceType` (`StringWithCharLimit256`) and `ResourceId` (`BaseResourceId`) are
/// unconstrained strings, so no character can be claimed as one without a resource id
/// that contains it splitting the key wrongly.
pub fn encode_evaluation_result_token(key: &EvaluationResultKey) -> String {
    let mut encoded = String::new();
    for field in [&key.0, &key.1, &key.2] {
        encoded.push_str(&format!("{}:{field}", field.len()));
    }
    encode_page_token(&encoded)
}

/// The [`EvaluationResultKey`] a `NextToken` resumes at, or `None` when the token is
/// malformed — i.e. not something [`encode_evaluation_result_token`] produced, so its
/// three length-prefixed fields do not account for exactly its decoded bytes.
pub fn decode_evaluation_result_token(token: &str) -> Option<EvaluationResultKey> {
    let decoded = decode_page_token(token)?;
    let mut rest = decoded.as_str();
    let mut fields: Vec<String> = Vec::with_capacity(3);
    for _ in 0..3 {
        let (length, tail) = rest.split_once(':')?;
        let length: usize = length.parse().ok()?;
        fields.push(tail.get(..length)?.to_string());
        rest = tail.get(length..)?;
    }
    if !rest.is_empty() {
        return None;
    }
    let mut fields = fields.into_iter();
    Some((fields.next()?, fields.next()?, fields.next()?))
}

/// How Config words `InvalidParameterValueException` for a results filter that named a
/// `ComplianceType` no filter may name.
///
/// The message is BUILT FROM THE REQUEST — it quotes the value the caller sent, names
/// the member and lists the values that member accepts
/// ([`FILTERABLE_COMPLIANCE_TYPES`]).
pub fn unfilterable_compliance_type_message(member: &str, value: &str) -> String {
    format!(
        "'{value}' is not a valid value for {member}; the valid values are {}.",
        FILTERABLE_COMPLIANCE_TYPES.join(", ")
    )
}

/// How Config words `NoSuchConfigRuleException` for a request that named ONE rule
/// this account does not hold, through a single-name member (`ConfigRuleName`).
///
/// The message is BUILT FROM THE REQUEST — it quotes the name the caller sent and
/// names the member to check — so every operation that looks a rule up by a single
/// name words it the same way. An operation that looks rules up through a LIST
/// member words it as [`NO_SUCH_CONFIG_RULES`] instead: the message follows
/// the shape of the member that named the rule, and a list cannot be quoted back as
/// one name.
pub fn no_such_config_rule_message(name: &str) -> String {
    format!(
        "The ConfigRule '{name}' provided in the request is invalid. Please check the \
         configRule name."
    )
}

/// How the resource-evaluation path words `InvalidParameterValueException` for a
/// request that asked for an `EvaluationMode` it does not accept.
///
/// The message is BUILT FROM THE REQUEST — it names the mode the caller asked for and
/// the modes this path supports (`PROACTIVE` alone, per
/// `StartResourceEvaluationRequest$EvaluationMode` and
/// `ResourceEvaluationFilters$EvaluationMode`) — so every operation whose
/// `EvaluationMode` is documented as `PROACTIVE`-only words the refusal the same way.
pub fn unsupported_evaluation_mode_message(requested: &str) -> String {
    format!(
        "{requested} Evaluation Mode is not supported. The supported Evaluation Modes \
         are: [{EVALUATION_MODE_PROACTIVE}]."
    )
}

/// How Config words `NoSuchConfigRuleException` for a request that named rules
/// through a LIST member (`ConfigRuleNames`), one of which this account does not
/// hold.
///
/// A list-shaped lookup reports the plural form and quotes no name: the refusal is
/// about the list as a whole, so it names the member to check and leaves the caller
/// to compare the list it sent against the rules it holds. See
/// [`no_such_config_rule_message`] for the single-name form.
pub const NO_SUCH_CONFIG_RULES: &str = "One or more ConfigRules provided in the request are \
                                        invalid. Please check the configRule names.";

/// How Config words `NoSuchConfigurationRecorderException`.
///
/// A FIXED message for the class — it quotes nothing from the request — so every
/// operation that fails to find a configuration recorder answers with exactly this
/// text, whether the request named the recorder by name or by ARN.
pub const NO_SUCH_CONFIGURATION_RECORDER: &str =
    "The name of the configuration recorder you entered cannot be found. Verify the \
     configuration recorder name, and try again.";

/// How `PutConfigRule` words `NoAvailableConfigurationRecorderException`.
///
/// A FIXED message for this operation's precondition: the class is shared with
/// `PutDeliveryChannel` (see [`NO_RECORDER_TO_PUT_DELIVERY_CHANNEL`]), and each
/// operation states the precondition that failed rather than the class, so a client
/// can tell the two apart from the message alone.
pub const NO_RECORDER_TO_PUT_CONFIG_RULE: &str =
    "You must create a configuration recorder before you can create or update a Config rule.";

/// How `PutDeliveryChannel` words `NoAvailableConfigurationRecorderException`. See
/// [`NO_RECORDER_TO_PUT_CONFIG_RULE`] for why the two differ.
pub const NO_RECORDER_TO_PUT_DELIVERY_CHANNEL: &str =
    "Configuration recorder is not available to put delivery channel.";

/// How `StartConfigurationRecorder` words `NoAvailableDeliveryChannelException`: a
/// FIXED message naming the precondition that failed.
pub const NO_DELIVERY_CHANNEL_TO_START_RECORDER: &str =
    "Delivery channel is not available to start configuration recorder.";

/// How Config words `NoSuchDeliveryChannelException` for a request that named a
/// delivery channel this account does not hold.
///
/// BUILT FROM THE REQUEST — it quotes the channel name the caller sent — so every
/// operation that refuses an absent channel words it the same way.
pub fn no_such_delivery_channel_message(name: &str) -> String {
    format!("Cannot find delivery channel with specified name '{name}'.")
}

/// How Config words `LastDeliveryChannelDeleteFailedException`: the delete of the
/// account's last delivery channel while its configuration recorder is running.
///
/// BUILT FROM THE REQUEST — it quotes the channel name the caller asked to delete.
pub fn last_delivery_channel_delete_failed_message(name: &str) -> String {
    format!(
        "Failed to delete last specified delivery channel with name '{name}', because there \
         is a running configuration recorder."
    )
}

/// How Config words `MaxNumberOfConfigurationRecordersExceededException`.
///
/// The message states THE SERVICE'S OWN LIMIT ([`CUSTOMER_MANAGED_RECORDERS_QUOTA`])
/// and nothing of the request or the environment: naming the Region would make one
/// account's refusal read differently from another's for the same cause. The two
/// spaces after "recorder" are the service's own rendering (it interpolates an empty
/// recorder designation there), not a typo.
pub fn max_configuration_recorders_message() -> String {
    format!(
        "Failed to put configuration recorder  because you have reached the limit for the \
         maximum number of customer managed configuration records: \
         ({CUSTOMER_MANAGED_RECORDERS_QUOTA})"
    )
}

/// How Config words `NoSuchRemediationConfigurationException` for a request naming a
/// Config rule that holds no remediation configuration — none at all, or none for the
/// `resource_type` the request named.
///
/// The message is BUILT FROM THE REQUEST — it quotes the `ConfigRuleName` the caller
/// sent, and the `ResourceType` when the caller named one, so the two refusals are
/// told apart — for the case the modelled error documents ("You specified an Config
/// rule without a remediation configuration").
pub fn no_such_remediation_configuration_message(
    name: &str,
    resource_type: Option<&str>,
) -> String {
    match resource_type {
        None => format!("No RemediationConfiguration for rule '{name}' exists."),
        Some(resource_type) => format!(
            "No RemediationConfiguration for rule '{name}' and resource type \
             '{resource_type}' exists."
        ),
    }
}

/// How Config words `NoSuchRetentionConfigurationException` for a request that named
/// a retention configuration this account does not hold.
///
/// The message is BUILT FROM THE REQUEST — it quotes the name the caller sent and
/// names the member to check — so every operation that refuses an absent retention
/// configuration words it the same way.
pub fn no_such_retention_configuration_message(name: &str) -> String {
    format!(
        "Cannot find retention configuration with the specified name '{name}'. Please \
         check the retentionConfigurationName."
    )
}

/// How Config words `MaxNumberOfRetentionConfigurationsExceededException`: the write
/// would take the account past [`RETENTION_CONFIGURATIONS_QUOTA`] retention
/// configurations in this Region.
pub fn retention_configurations_quota_message() -> String {
    format!(
        "Failed to add the retention configuration because this account already holds \
         the maximum of {RETENTION_CONFIGURATIONS_QUOTA} retention configuration(s) in \
         this region."
    )
}

/// How Config words `NoSuchConfigurationAggregatorException` for a request that named
/// an aggregator this account does not hold.
///
/// The message is BUILT FROM THE REQUEST — it quotes the
/// `ConfigurationAggregatorName` the caller sent and names the member to check — so
/// every operation that refuses an absent aggregator words it the same way.
pub fn no_such_configuration_aggregator_message(name: &str) -> String {
    format!(
        "The configuration aggregator '{name}' provided in the request does not exist. \
         Please check the ConfigurationAggregatorName and try again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule named `name` whose evaluated resources carry `compliance_types`, one
    /// per resource.
    fn rule_with(name: &str, compliance_types: &[&str]) -> ConfigRuleEntry {
        ConfigRuleEntry {
            name: name.to_string(),
            arn: format!("arn:aws:config:us-east-1:000000000000:config-rule/{name}"),
            id: name.to_string(),
            description: None,
            scope: None,
            source: Source::default(),
            input_parameters: None,
            maximum_execution_frequency: None,
            state: CONFIG_RULE_STATE_ACTIVE.to_string(),
            evaluation_modes: default_evaluation_modes(),
            rule_evaluation_visibility: Some(RULE_EVALUATION_VISIBILITY_EXTERNAL.to_string()),
            created_at: SystemTime::UNIX_EPOCH,
            tags: TagSet::new(),
            evaluation_results: compliance_types
                .iter()
                .enumerate()
                .map(|(index, compliance)| {
                    let entry = EvaluationResultEntry::new(
                        "AWS::EC2::Volume",
                        &format!("vol-{index}"),
                        EVALUATION_MODE_DETECTIVE,
                        compliance,
                        SystemTime::UNIX_EPOCH,
                    );
                    (entry.key(), entry)
                })
                .collect(),
            evaluation: RuleEvaluationEntry::default(),
        }
    }

    #[test]
    fn a_rule_rolls_up_the_compliance_of_the_resources_it_evaluated() {
        let cases: [(&str, &[&str], &str); 6] = [
            ("nothing evaluated", &[], COMPLIANCE_TYPE_INSUFFICIENT_DATA),
            (
                "every resource compliant",
                &[COMPLIANCE_TYPE_COMPLIANT, COMPLIANCE_TYPE_COMPLIANT],
                COMPLIANCE_TYPE_COMPLIANT,
            ),
            (
                "one resource noncompliant",
                &[COMPLIANCE_TYPE_COMPLIANT, COMPLIANCE_TYPE_NON_COMPLIANT],
                COMPLIANCE_TYPE_NON_COMPLIANT,
            ),
            (
                "every resource noncompliant",
                &[COMPLIANCE_TYPE_NON_COMPLIANT],
                COMPLIANCE_TYPE_NON_COMPLIANT,
            ),
            (
                "only not-applicable results",
                &["NOT_APPLICABLE"],
                COMPLIANCE_TYPE_INSUFFICIENT_DATA,
            ),
            (
                "not-applicable beside a compliant result",
                &["NOT_APPLICABLE", COMPLIANCE_TYPE_COMPLIANT],
                COMPLIANCE_TYPE_COMPLIANT,
            ),
        ];

        for (case, evaluated, expected) in cases {
            let actual = rule_with("rule", evaluated).compliance();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the rule to roll up to {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn the_summary_counts_rules_up_to_the_published_cap() {
        // Rows differ along how many rules land in each count, including each side of
        // the published cap of 25 rules.
        let cap = usize::try_from(COMPLIANCE_SUMMARY_RULE_CAP).expect("the cap is positive");
        let cases: [(&str, usize, usize, usize); 6] = [
            ("no rules at all", 0, 0, 0),
            ("one of each", 1, 1, 0),
            ("one short of the cap", cap - 1, 0, 0),
            ("exactly the cap", cap, cap, 0),
            ("one past the cap", cap + 1, 0, 0),
            ("far past the cap", cap * 3, cap * 3, 0),
        ];

        for (case, compliant, non_compliant, insufficient) in cases {
            let mut rules: Vec<ConfigRuleEntry> = Vec::new();
            for index in 0..compliant {
                rules.push(rule_with(
                    &format!("c-{index}"),
                    &[COMPLIANCE_TYPE_COMPLIANT],
                ));
            }
            for index in 0..non_compliant {
                rules.push(rule_with(
                    &format!("n-{index}"),
                    &[COMPLIANCE_TYPE_NON_COMPLIANT],
                ));
            }
            for index in 0..insufficient {
                rules.push(rule_with(&format!("i-{index}"), &[]));
            }

            let summary = summarise_rule_compliance(rules.iter(), SystemTime::UNIX_EPOCH);
            let expect = |count: usize| ComplianceContributorCount {
                capped_count: i32::try_from(count.min(cap)).expect("a capped count fits"),
                cap_exceeded: count > cap,
            };
            for (member, actual, count) in [
                (
                    "CompliantResourceCount",
                    summary.compliant_resource_count.clone(),
                    compliant,
                ),
                (
                    "NonCompliantResourceCount",
                    summary.non_compliant_resource_count.clone(),
                    non_compliant,
                ),
            ] {
                let actual = actual
                    .unwrap_or_else(|| panic!("case {case:?}: expected a {member}"));
                let expected = expect(count);
                assert_eq!(
                    (actual.capped_count, actual.cap_exceeded),
                    (expected.capped_count, expected.cap_exceeded),
                    "case {case:?}: expected {member} {{CappedCount: {}, CapExceeded: {}}} for \
                     {count} rules, got {{CappedCount: {}, CapExceeded: {}}}",
                    expected.capped_count,
                    expected.cap_exceeded,
                    actual.capped_count,
                    actual.cap_exceeded
                );
            }
            assert!(
                summary.compliance_summary_timestamp.is_some(),
                "case {case:?}: expected a ComplianceSummaryTimestamp"
            );
        }
    }

    #[test]
    fn an_evaluation_result_page_token_round_trips_every_key_it_is_given() {
        // Rows differ along what the key's fields hold: ordinary values, the characters
        // a separator-joined token would split on, empty and multi-byte fields, and a
        // resource id at the `BaseResourceId` length maximum of 768.
        let cases: [(&str, EvaluationResultKey); 6] = [
            (
                "an ordinary key",
                (
                    "AWS::EC2::Volume".to_string(),
                    "vol-0123456789abcdef0".to_string(),
                    EVALUATION_MODE_DETECTIVE.to_string(),
                ),
            ),
            (
                "a resource id carrying the separators a joined token would use",
                (
                    "AWS::S3::Bucket".to_string(),
                    "bucket#with:odd/characters|inside".to_string(),
                    EVALUATION_MODE_DETECTIVE.to_string(),
                ),
            ),
            (
                "a resource id that is itself a length prefix",
                (
                    "AWS::S3::Bucket".to_string(),
                    "3:abc".to_string(),
                    "PROACTIVE".to_string(),
                ),
            ),
            (
                "empty fields",
                (String::new(), String::new(), String::new()),
            ),
            (
                "multi-byte characters",
                (
                    "AWS::S3::Bucket".to_string(),
                    "caf\u{e9}-b\u{fc}cket-\u{1f512}".to_string(),
                    "PROACTIVE".to_string(),
                ),
            ),
            (
                "a resource id at the published BaseResourceId maximum of 768",
                (
                    "AWS::EC2::Instance".to_string(),
                    "i".repeat(768),
                    EVALUATION_MODE_DETECTIVE.to_string(),
                ),
            ),
        ];

        for (case, key) in cases {
            let token = encode_evaluation_result_token(&key);
            let decoded = decode_evaluation_result_token(&token);
            assert_eq!(
                decoded.as_ref(),
                Some(&key),
                "case {case:?}: expected the token {token:?} to decode back to {key:?}, \
                 got {decoded:?}"
            );
        }

        // A token this emulator did not issue decodes to nothing, whatever it is.
        for token in [
            "",
            "not-base64-at-all!!",
            &::aws_smithy_types::base64::encode("AWS::EC2::Volume/vol-0"),
            &::aws_smithy_types::base64::encode("3:abc"),
            &::aws_smithy_types::base64::encode("3:abc3:def3:ghi-trailing"),
            &::aws_smithy_types::base64::encode("99:abc0:0:"),
        ] {
            let decoded = decode_evaluation_result_token(token);
            assert!(
                decoded.is_none(),
                "case {token:?}: a token this operation did not issue is malformed, got \
                 {decoded:?}"
            );
        }
    }

    #[test]
    fn an_evaluation_pass_completes_only_once_the_clock_has_made_it_due() {
        // Rows differ along how far past the invocation the clock has moved: before
        // the pass is due, exactly when it is due, and well past it.
        let invoked_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let cases: [(&str, Duration, bool); 4] = [
            ("at the invocation itself", Duration::ZERO, false),
            (
                "one millisecond before the pass is due",
                RULE_EVALUATION_SETTLE_AFTER - Duration::from_millis(1),
                false,
            ),
            ("exactly when it is due", RULE_EVALUATION_SETTLE_AFTER, true),
            ("a minute past it", Duration::from_secs(60), true),
        ];

        assert!(
            RULE_EVALUATION_SETTLE_AFTER >= Duration::from_millis(250)
                && RULE_EVALUATION_SETTLE_AFTER <= Duration::from_millis(750),
            "the span the clock owes an evaluation must stay inside 250-750 ms, got \
             {RULE_EVALUATION_SETTLE_AFTER:?}"
        );

        for (case, elapsed, completed) in cases {
            let mut evaluation = RuleEvaluationEntry::default();
            evaluation.invoke(invoked_at);
            let now = invoked_at + elapsed;
            assert_eq!(
                evaluation.in_progress(now),
                !completed,
                "case {case:?}: expected in_progress {}, got {}",
                !completed,
                evaluation.in_progress(now)
            );

            evaluation.settle(now);
            assert_eq!(
                evaluation.first_evaluation_started, completed,
                "case {case:?}: expected FirstEvaluationStarted {completed}, got {}",
                evaluation.first_evaluation_started
            );
            let expected_invocation = completed.then_some(invoked_at);
            assert_eq!(
                evaluation.last_successful_invocation_at, expected_invocation,
                "case {case:?}: expected the last successful invocation at \
                 {expected_invocation:?}, got {:?}",
                evaluation.last_successful_invocation_at
            );
            let expected_evaluation = completed.then_some(invoked_at + RULE_EVALUATION_SETTLE_AFTER);
            assert_eq!(
                evaluation.last_successful_evaluation_at, expected_evaluation,
                "case {case:?}: expected the last successful evaluation at \
                 {expected_evaluation:?}, got {:?}",
                evaluation.last_successful_evaluation_at
            );

            // Settling again at the same instant changes nothing: the completion is
            // materialised once.
            let before = (
                evaluation.last_successful_invocation_at,
                evaluation.last_successful_evaluation_at,
            );
            evaluation.settle(now);
            assert_eq!(
                (
                    evaluation.last_successful_invocation_at,
                    evaluation.last_successful_evaluation_at
                ),
                before,
                "case {case:?}: a second settle must not move the recorded instants"
            );
        }
    }

    #[test]
    fn a_recorder_id_is_distinct_for_every_recorder_and_is_part_of_the_arn() {
        let mut ids: Vec<String> = (0..1_000).map(mint_recorder_id).collect();
        let minted = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            minted,
            "every configuration recorder must get a distinct id, got {} distinct of {minted}",
            ids.len()
        );
        for id in &ids {
            assert!(
                id.len() == 16
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "a recorder id is sixteen base-36 characters, got {id:?}"
            );
        }

        let arn = recorder_arn("us-west-2", "probe", &mint_recorder_id(7));
        assert!(
            arn.contains(&format!(
                ":{RECORDER_ARN_RESOURCE_TYPE}/probe/{}",
                mint_recorder_id(7)
            )),
            "the recorder ARN carries the resource type, the name and the id, got {arn:?}"
        );
    }

    #[test]
    fn a_resource_evaluation_id_is_distinct_for_every_evaluation() {
        let mut ids: Vec<String> = (0..1_000).map(mint_resource_evaluation_id).collect();
        let minted = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            minted,
            "every resource evaluation must get a distinct id, got {} distinct of {minted}",
            ids.len()
        );
        for id in &ids {
            assert!(
                (1..=128).contains(&id.len())
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "a ResourceEvaluationId is 1..128 opaque base-36 characters, got {id:?}"
            );
        }
        assert_ne!(
            mint_resource_evaluation_id(0),
            mint_recorder_id(0),
            "the n-th evaluation and the n-th recorder must not carry the same id"
        );
    }

    #[test]
    fn a_resource_evaluation_page_token_round_trips_its_key() {
        // Rows differ along the instant the key names (the epoch itself, before it,
        // after it, and a sub-second offset) and along the id beside it.
        let cases: [(&str, i128, &str); 5] = [
            ("the epoch itself", 0, "0000000000000000"),
            ("one nanosecond past the epoch", 1, "abcdefgh12345678"),
            ("before the epoch", -1_500_000_000, "zzzzzzzzzzzzzzzz"),
            ("a whole second", 1_700_000_000_000_000_000, "7f3k9q1m5x8c2v0b"),
            (
                "a sub-second offset",
                1_700_000_000_123_456_789,
                "0123456789abcdef",
            ),
        ];

        for (case, nanos, id) in cases {
            let key = (::std::cmp::Reverse(nanos), id.to_string());
            let token = encode_resource_evaluation_token(&key);
            let decoded = decode_resource_evaluation_token(&token)
                .unwrap_or_else(|| panic!("case {case:?}: {token:?} must decode"));
            assert_eq!(
                decoded, key,
                "case {case:?}: expected the token to round trip, got {decoded:?}"
            );
        }

        // Rows differ along how the token is malformed; none of them is a token this
        // service issued, so none of them decodes.
        let malformed: [(&str, &str); 4] = [
            ("not base64", "**not base64**"),
            ("base64 with no separator", "MTIzNDU2"),
            ("base64 whose instant is not a number", "d2hlbjppZA=="),
            ("base64 with no id", "MTIzNDU2Nzg5MDo="),
        ];
        for (case, token) in malformed {
            let decoded = decode_resource_evaluation_token(token);
            assert!(
                decoded.is_none(),
                "case {case:?}: expected {token:?} to be refused, got {decoded:?}"
            );
        }
    }

    #[test]
    fn the_resource_evaluation_store_stays_bounded_and_drops_the_oldest() {
        // Rows differ along how far the starts go past the bound: one short of it,
        // exactly it, one past it and well past it.
        let cases: [(&str, usize); 4] = [
            ("one short of the bound", RESOURCE_EVALUATIONS_KEPT - 1),
            ("exactly the bound", RESOURCE_EVALUATIONS_KEPT),
            ("one past the bound", RESOURCE_EVALUATIONS_KEPT + 1),
            ("well past the bound", RESOURCE_EVALUATIONS_KEPT + 25),
        ];

        for (case, started) in cases {
            let mut store = ComAmazonawsConfigserviceStarlingdoveserviceState::default();
            let mut ids: Vec<String> = Vec::new();
            for index in 0..started {
                let id = mint_resource_evaluation_id(index as u64);
                ids.push(id.clone());
                store.record_resource_evaluation(ResourceEvaluationEntry {
                    id,
                    evaluation_mode: EVALUATION_MODE_PROACTIVE.to_string(),
                    // One millisecond apart, so the oldest is the first one started.
                    started_at: SystemTime::UNIX_EPOCH + Duration::from_millis(index as u64),
                    evaluation_context_identifier: None,
                    resource_details: ResourceDetails::default(),
                    evaluation_timeout_seconds: DEFAULT_EVALUATION_TIMEOUT_SECONDS,
                    client_token: Some(format!("token-{index}")),
                });
            }

            let kept = started.min(RESOURCE_EVALUATIONS_KEPT);
            assert_eq!(
                store.resource_evaluations.len(),
                kept,
                "case {case:?}: expected the store to hold {kept} evaluations after \
                 {started} starts, got {}",
                store.resource_evaluations.len()
            );
            assert_eq!(
                store.resource_evaluation_tokens.len(),
                kept,
                "case {case:?}: expected the token index to hold one entry per kept \
                 evaluation, got {}",
                store.resource_evaluation_tokens.len()
            );

            // What is kept is the MOST RECENT window: the last `kept` evaluations
            // started, each with the token that started it.
            for (index, id) in ids.iter().enumerate() {
                let expected = index >= started - kept;
                assert_eq!(
                    store.resource_evaluations.contains_key(id),
                    expected,
                    "case {case:?}: evaluation {index} ({id:?}) kept should be {expected}"
                );
                assert_eq!(
                    store
                        .resource_evaluation_tokens
                        .contains_key(&format!("token-{index}")),
                    expected,
                    "case {case:?}: the token of evaluation {index} indexed should be \
                     {expected}"
                );
            }
        }
    }
}
