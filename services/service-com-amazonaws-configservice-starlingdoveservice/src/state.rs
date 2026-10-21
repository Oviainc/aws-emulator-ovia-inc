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
    ConfigRule, ConfigSnapshotDeliveryProperties, ConfigurationRecorder,
    ConfigurationRecorderStatus, DeliveryChannel, EvaluationModeConfiguration, RecordingGroup,
    RecordingMode, RecordingStrategy, Scope, Source, Tag,
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

/// How many rules one `DescribeConfigRules` page carries.
///
/// The operation models no `Limit` member, so the page size is this emulator's:
/// it matches the `ConfigRuleNames` length maximum (0..25) the same operation
/// declares.
pub const DESCRIBE_CONFIG_RULES_PAGE_SIZE: usize = 25;

/// Wording for the (unreachable in the assembled emulator) case where this
/// service's store cannot be reached, i.e. the engine is not the shared
/// `InMemoryEngine`.
pub const STATE_UNAVAILABLE: &str =
    "the Config emulator could not reach its state store for this request";

/// One persisted customer managed configuration recorder.
#[derive(Debug, Clone, Default)]
pub struct RecorderEntry {
    pub name: String,
    pub arn: String,
    pub role_arn: String,
    pub recording_group: Option<RecordingGroup>,
    pub recording_mode: Option<RecordingMode>,
    pub recording_scope: Option<String>,
    pub service_principal: Option<String>,
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
}

impl ConfigRuleEntry {
    /// The modelled `ConfigRule`, read back out of state.
    pub fn to_shape(&self) -> ConfigRule {
        ConfigRule {
            config_rule_name: Some(self.name.clone()),
            config_rule_arn: Some(self.arn.clone()),
            config_rule_id: Some(self.id.clone()),
            description: self.description.clone(),
            scope: self.scope.clone(),
            source: self.source.clone(),
            input_parameters: self.input_parameters.clone(),
            maximum_execution_frequency: self.maximum_execution_frequency.clone(),
            config_rule_state: Some(self.state.clone()),
            created_by: None,
            evaluation_modes: Some(self.evaluation_modes.clone()),
            rule_evaluation_visibility: self.rule_evaluation_visibility.clone(),
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

/// The ARN of a configuration recorder:
/// `arn:aws:config:<region>:<account>:config-recorder/<name>`.
pub fn recorder_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("config-recorder/{name}"))
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
