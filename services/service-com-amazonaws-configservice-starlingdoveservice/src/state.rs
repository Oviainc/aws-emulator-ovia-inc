// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-configservice-starlingdoveservice service.
//!
//! This holds the AWS Config setup-and-rule loop: the customer managed configuration
//! recorder, its status record, the delivery channel, and the Config rules. Every
//! handler under `src/ops/` computes its response from these maps; nothing is
//! hardcoded and no response echoes the request that produced it.
//!
//! One engine's store stands for ONE account in ONE Region, so the quotas Config
//! documents per account per Region are enforced over the whole store.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccountAggregationSource, AggregatorFilters, ComplianceContributorCount, ComplianceSummary,
    ConfigRule, ConfigSnapshotDeliveryProperties, ConfigurationAggregator, ConfigurationRecorder,
    ConfigurationRecorderStatus, ConfigurationRecorderSummary, DeliveryChannel,
    EvaluationModeConfiguration,
    ExclusionByResourceTypes, OrganizationAggregationSource, PendingAggregationRequest,
    RecordingGroup, RecordingMode, RecordingStrategy, Scope, Source, Tag,
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

/// The cap a `ComplianceSummary` counts Config rules up to
/// (`ComplianceSummary$CompliantResourceCount`: "The number of Config rules or Amazon
/// Web Services resources that are compliant, up to a maximum of 25 for rules").
/// Reaching it is what sets `ComplianceContributorCount.CapExceeded`.
pub const COMPLIANCE_SUMMARY_RULE_CAP: i32 = 25;

/// `Owner` value for a Custom Policy rule (`com.amazonaws.configservice#Owner`).
pub const OWNER_CUSTOM_POLICY: &str = "CUSTOM_POLICY";

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

/// How many rules one `DescribeConfigRules` page carries.
///
/// The operation models no `Limit` member, so the page size is this emulator's:
/// it matches the `ConfigRuleNames` length maximum (0..25) the same operation
/// declares.
pub const DESCRIBE_CONFIG_RULES_PAGE_SIZE: usize = 25;

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
    /// Tags are taken at creation and not updated afterwards
    /// (`PutConfigurationRecorder`: "Tags are added at creation and cannot be
    /// updated").
    pub tags: Vec<Tag>,
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
#[derive(Debug, Clone, Default)]
pub struct DeliveryChannelEntry {
    pub name: String,
    pub s3_bucket_name: Option<String>,
    pub s3_key_prefix: Option<String>,
    pub s3_kms_key_arn: Option<String>,
    pub sns_topic_arn: Option<String>,
    pub config_snapshot_delivery_properties: Option<ConfigSnapshotDeliveryProperties>,
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
    pub tags: Vec<Tag>,
    /// The `ComplianceType` of the last evaluation result recorded for each resource
    /// this rule evaluated, keyed by `<ComplianceResourceType>/<ComplianceResourceId>`.
    ///
    /// A rule's own compliance is the roll-up of these ([`Self::compliance`]). No
    /// operation of this emulator can record one yet: `PutEvaluations` identifies the
    /// rule only through a `ResultToken` minted by the Lambda invocation that produced
    /// the evaluation, which this emulator does not run, so it stays scaffolded. A rule
    /// therefore carries no results and rolls up to `INSUFFICIENT_DATA` until an
    /// evaluation writer lands.
    pub resource_compliance: BTreeMap<String, String>,
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

    /// This rule's `ComplianceType`: the roll-up of the evaluation results recorded
    /// for it — noncompliant if any evaluated resource is noncompliant, compliant if
    /// at least one is compliant and none is not, and `INSUFFICIENT_DATA` when no
    /// result places it either way (which includes a rule nothing has evaluated).
    pub fn compliance(&self) -> &'static str {
        let has = |compliance_type: &str| {
            self.resource_compliance
                .values()
                .any(|recorded| recorded == compliance_type)
        };
        if has(COMPLIANCE_TYPE_NON_COMPLIANT) {
            COMPLIANCE_TYPE_NON_COMPLIANT
        } else if has(COMPLIANCE_TYPE_COMPLIANT) {
            COMPLIANCE_TYPE_COMPLIANT
        } else {
            COMPLIANCE_TYPE_INSUFFICIENT_DATA
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
    /// different. Use TagResource and UntagResource to update tags after creation").
    pub tags: Vec<Tag>,
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

/// Per-service state for com-amazonaws-configservice-starlingdoveservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Customer managed configuration recorders, keyed by name. Config allows one
    /// per account per Region, so this holds at most one entry: one engine's store
    /// stands for one account in one Region, and the per-Region quotas below are
    /// enforced over the whole store.
    pub recorders: HashMap<String, RecorderEntry>,
    /// The status record of each recorder, keyed by recorder name.
    pub recorder_status: HashMap<String, RecorderStatusEntry>,
    /// Delivery channels, keyed by name. Config allows one per account per Region.
    pub delivery_channels: HashMap<String, DeliveryChannelEntry>,
    /// Config rules, keyed by `ConfigRuleName`. A `BTreeMap` so `DescribeConfigRules`
    /// pages over a stable ordering and a page token survives inserts and deletes.
    pub config_rules: BTreeMap<String, ConfigRuleEntry>,
    /// Source of the next `ConfigRuleId` suffix, so every rule gets a distinct id.
    pub next_rule_seq: u64,
    /// Source of the next configuration recorder id, so every recorder Config
    /// creates here gets a distinct one.
    pub next_recorder_seq: u64,
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
}

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

/// The ARN of a Config rule: `arn:aws:config:<region>:<account>:config-rule/<id>`.
pub fn config_rule_arn(region: &str, rule_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("config-rule/{rule_id}"))
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
    let excludes = group
        .exclusion_by_resource_types
        .as_ref()
        .and_then(|exclusion| exclusion.resource_types.as_deref())
        .is_some_and(|resource_types| !resource_types.is_empty());
    let includes = group
        .resource_types
        .as_deref()
        .is_some_and(|resource_types| !resource_types.is_empty());

    if group.recording_strategy.is_none() {
        let use_only = if excludes {
            RECORDING_STRATEGY_EXCLUSION
        } else if includes {
            RECORDING_STRATEGY_INCLUSION
        } else {
            RECORDING_STRATEGY_ALL_SUPPORTED
        };
        group.recording_strategy = Some(RecordingStrategy {
            use_only: Some(use_only.to_string()),
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
        compliant_resource_count: Some(capped_count(compliant)),
        non_compliant_resource_count: Some(capped_count(non_compliant)),
        compliance_summary_timestamp: Some(timestamp(at)),
    }
}

/// `count` reported as a `ComplianceContributorCount`: capped at
/// [`COMPLIANCE_SUMMARY_RULE_CAP`], with `CapExceeded` set once the cap is reached.
fn capped_count(count: i64) -> ComplianceContributorCount {
    let cap = i64::from(COMPLIANCE_SUMMARY_RULE_CAP);
    ComplianceContributorCount {
        capped_count: i32::try_from(count.min(cap)).unwrap_or(COMPLIANCE_SUMMARY_RULE_CAP),
        cap_exceeded: count > cap,
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

/// Encode the key a `DescribeConfigRules` page should resume at as an opaque
/// `NextToken`, with the base64 codec the framework already carries.
pub fn encode_page_token(resume_at: &str) -> String {
    ::aws_smithy_types::base64::encode(resume_at)
}

/// The key a `NextToken` resumes at, or `None` when the token is malformed — i.e.
/// not something [`encode_page_token`] produced.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::aws_smithy_types::base64::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

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

/// How Config words `NoSuchConfigRuleException` for a request that named a rule this
/// account does not hold.
///
/// The message is BUILT FROM THE REQUEST — it quotes the `ConfigRuleName` the caller
/// sent and names the member to check — so every operation that refuses an absent
/// rule words it the same way.
pub fn no_such_config_rule_message(name: &str) -> String {
    format!(
        "The ConfigRule '{name}' provided in the request is invalid. Please check the \
         configRule name."
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
            tags: Vec::new(),
            resource_compliance: compliance_types
                .iter()
                .enumerate()
                .map(|(index, compliance)| {
                    (
                        format!("AWS::EC2::Volume/vol-{index}"),
                        (*compliance).to_string(),
                    )
                })
                .collect(),
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
}
