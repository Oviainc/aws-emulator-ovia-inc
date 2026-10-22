// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-medialive-medialive service.
//!
//! The channel lifecycle lives here: one [`ChannelRecord`] per created channel,
//! the monotonic id counter that mints its `Id`, and the idempotency-token map
//! that makes a retried `CreateChannel` return the channel it already created.
//! The input lifecycle lives beside it: one [`InputRecord`] per created input,
//! its own id counter and its own token map. The SDI-source resource lives here
//! too: one [`SdiSourceRecord`] per created source, its own id counter and its
//! own token map. So do signal maps: one [`SignalMapRecord`] per created signal
//! map, carrying the media-resource graph discovery found by walking the
//! channels this store already holds.
//!
//! The input security group lifecycle lives here too: one
//! [`InputSecurityGroupRecord`] per created group and the counter that mints its
//! `Id`. A group's `State`, `Inputs` and `Channels` are derived from the rest of
//! the state on every read, never recorded at create time.
//!
//! The monitor side of the service lives here as well: one
//! [`CloudWatchAlarmTemplateGroupRecord`] per created cloudwatch alarm template
//! group and one [`EventBridgeRuleTemplateGroupRecord`] per created eventbridge
//! rule template group, each keyed by its minted `Id` and carrying the region it
//! was created in, so a group's name is unique "within the scope of a resource
//! type in a specific region" (the `Name` member's own documentation). The
//! eventbridge rule TEMPLATES those groups hold live here too: one
//! [`EventBridgeRuleTemplateRecord`] per created template, carrying the id of
//! the group its `GroupIdentifier` resolved to, so a template reports the group
//! it belongs to out of state rather than out of the request.
//!
//! Every value a channel-, input-, input-security-group-, SdiSource-,
//! signal-map- or template-group-shaped response carries is either stored
//! verbatim from the request that created the resource or derived from the stored
//! state ([`ComAmazonawsMedialiveMedialiveState::channel_description`],
//! [`ComAmazonawsMedialiveMedialiveState::input_description`],
//! [`ComAmazonawsMedialiveMedialiveState::sdi_source_description`],
//! [`ComAmazonawsMedialiveMedialiveState::signal_map_description`],
//! [`alarm_template_group_output`],
//! [`event_bridge_rule_template_group_output`]) - no operation builds a response
//! out of its own request.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AnywhereSettings, CdiInputSpecification, Channel, ChannelSummary,
    DescribeAnywhereSettings, DescribeFollowerChannelSettings, DescribeInferenceSettings,
    DescribeLinkedChannelSettings, DescribePrimaryChannelSettings, EncoderSettings,
    InferenceSettings, Input, InputAttachment, InputDestination, InputDestinationRequest,
    InputDestinationRoute, InputDeviceSettings, InputSecurityGroup, InputSource,
    InputSourceRequest, InputSpecification, InputVpcRequest, InputWhitelistRule,
    LinkedChannelSettings, MaintenanceStatus, MediaConnectFlow, MediaConnectFlowRequest,
    MediaResource, MediaResourceNeighbor, MonitorDeployment, MulticastSettings,
    MulticastSettingsCreateRequest, MulticastSource, OutputDestination, PipelineDetail,
    RouterDestination, RouterInputSettings, RouterSettings, SdiSource, SdiSourceSummary,
    Smpte2110ReceiverGroupSettings, SrtCallerDecryption, SrtCallerSource,
    SignalMapSummary, SrtListenerDecryption, SrtListenerSettings, SrtSettings,
    SrtSettingsRequest, SuccessfulMonitorDeployment,
    VpcOutputSettings, VpcOutputSettingsDescription,
};

/// The cloudwatch alarm template group slice: its record, its bounds, its
/// refusal, the store's methods and its output projection, in one module of its
/// own so the slice can be read (and merged) on its own. What stays in this file
/// is the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the token key both group
/// resources share.
pub mod alarm_template_groups;

pub use alarm_template_groups::{
    AlarmTemplateGroupRefusal, CloudWatchAlarmTemplateGroupRecord,
    MAX_ALARM_TEMPLATE_GROUPS, MAX_ALARM_TEMPLATE_GROUP_TOKENS,
};

pub(crate) use alarm_template_groups::alarm_template_group_output;

/// The eventbridge rule TEMPLATE slice: its record, its bounds, its refusal, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the per-region token key every
/// monitor template resource shares.
pub mod event_bridge_rule_templates;

pub use event_bridge_rule_templates::{
    is_event_target_arn, is_event_type, EventBridgeRuleTemplateRecord,
    EventBridgeRuleTemplateRefusal, NewEventBridgeRuleTemplate,
    EVENT_BRIDGE_RULE_TEMPLATE_EVENT_TYPES, MAX_EVENT_BRIDGE_RULE_TEMPLATES,
    MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS, MAX_EVENT_BRIDGE_RULE_TEMPLATE_TOKENS,
    MAX_EVENT_TARGET_ARN_LEN,
};

pub(crate) use event_bridge_rule_templates::event_bridge_rule_template_output;

/// The `ChannelState` enum values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="ChannelState")`:
/// CREATING | CREATE_FAILED | IDLE | STARTING | RUNNING | RECOVERING | STOPPING |
/// DELETING | DELETED | UPDATING | UPDATE_FAILED).
pub mod channel_state {
    pub const CREATING: &str = "CREATING";
    pub const CREATE_FAILED: &str = "CREATE_FAILED";
    pub const IDLE: &str = "IDLE";
    pub const STARTING: &str = "STARTING";
    pub const RUNNING: &str = "RUNNING";
    pub const STOPPING: &str = "STOPPING";
    pub const DELETING: &str = "DELETING";
    pub const UPDATE_FAILED: &str = "UPDATE_FAILED";
}

/// The `SdiSourceState` enum values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="SdiSourceState")`: IDLE | IN_USE |
/// DELETED).
pub mod sdi_source_state {
    pub const IDLE: &str = "IDLE";
    pub const IN_USE: &str = "IN_USE";
    pub const DELETED: &str = "DELETED";
}

/// The `ChannelClass` enum values (`smithy_query(query="shape",
/// shape="ChannelClass")`: STANDARD | SINGLE_PIPELINE).
pub mod channel_class {
    pub const STANDARD: &str = "STANDARD";
    pub const SINGLE_PIPELINE: &str = "SINGLE_PIPELINE";
}

/// The `SignalMapStatus` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="SignalMapStatus")`:
/// CREATE_IN_PROGRESS | CREATE_COMPLETE | CREATE_FAILED | UPDATE_IN_PROGRESS |
/// UPDATE_COMPLETE | UPDATE_REVERTED | UPDATE_FAILED | READY | NOT_READY).
pub mod signal_map_status {
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
}

/// The `SignalMapMonitorDeploymentStatus` value this slice uses, spelled exactly
/// as the model declares it (`smithy_query(query="shape",
/// shape="SignalMapMonitorDeploymentStatus")`: NOT_DEPLOYED |
/// DRY_RUN_DEPLOYMENT_COMPLETE | DRY_RUN_DEPLOYMENT_FAILED |
/// DRY_RUN_DEPLOYMENT_IN_PROGRESS | DEPLOYMENT_COMPLETE | DEPLOYMENT_FAILED |
/// DEPLOYMENT_IN_PROGRESS | DELETE_COMPLETE | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
///
/// Only `NOT_DEPLOYED` is reachable here: a monitor deployment is started by
/// `StartMonitorDeployment`, which is still a scaffold, so no signal map this
/// store holds has ever had one.
pub mod signal_map_monitor_deployment_status {
    pub const NOT_DEPLOYED: &str = "NOT_DEPLOYED";
}

/// The `InputSecurityGroupState` enum values, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="InputSecurityGroupState")`:
/// IDLE | IN_USE | UPDATING | DELETED).
pub mod input_security_group_state {
    pub const IDLE: &str = "IDLE";
    pub const IN_USE: &str = "IN_USE";
    pub const UPDATING: &str = "UPDATING";
    pub const DELETED: &str = "DELETED";
}

/// The `InputState` enum values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="InputState")`: CREATING |
/// DETACHED | ATTACHED | DELETING | DELETED).
///
/// `CREATING` is NOT one of them: the real service answers `CreateInput` with an
/// input that is already `DETACHED` (the recorded create carries
/// `State=DETACHED`), so an input is returned ready and owes the clock no
/// transition. `DELETING` and `DELETED` are the deletion half of the lifecycle,
/// which `DeleteInput` owns (still a scaffold stub).
pub mod input_state {
    pub const DETACHED: &str = "DETACHED";
    pub const ATTACHED: &str = "ATTACHED";
}

/// The `InputClass` enum values (`smithy_query(query="shape",
/// shape="InputClass")`: STANDARD | SINGLE_PIPELINE).
pub mod input_class {
    pub const STANDARD: &str = "STANDARD";
    pub const SINGLE_PIPELINE: &str = "SINGLE_PIPELINE";
}

/// The `InputSourceType` enum values (`smithy_query(query="shape",
/// shape="InputSourceType")`: STATIC | DYNAMIC).
pub mod input_source_type {
    pub const STATIC: &str = "STATIC";
    pub const DYNAMIC: &str = "DYNAMIC";
}

/// The `InputNetworkLocation` an input that names none gets: the
/// `InputNetworkLocation` doc says "By default all inputs will continue to be
/// AWS inputs" (`smithy_query(query="shape", shape="InputNetworkLocation")`).
pub const DEFAULT_INPUT_NETWORK_LOCATION: &str = "AWS";

/// The `InputType` values whose sources can carry a dynamic URL: the
/// `Input$InputSourceType` doc — "Certain pull input sources can be dynamic,
/// meaning that they can have their URL's dynamically changes during input switch
/// actions. Presently, this functionality only works with MP4_FILE and TS_FILE
/// inputs."
const DYNAMIC_CAPABLE_INPUT_TYPES: [&str; 2] = ["MP4_FILE", "TS_FILE"];

/// The variable a source URL carries to make a pull input dynamic, per the
/// MediaLive user guide's "Creating a dynamic input" ("in the URL, include the
/// string `$urlPath$` in place of the part of the path that will change").
const DYNAMIC_URL_VARIABLE: &str = "$urlPath$";

/// How long a resource stays in a transient state before the clock makes the next
/// one due: a channel's `CREATING`, `STARTING`, `STOPPING` and `DELETING`, and a
/// signal map's `CREATE_IN_PROGRESS` discovery.
///
/// CHOSEN, not documented by AWS: the real service takes minutes, and no doc
/// sentence pins a number, so this is the emulator's own span. It is deliberately
/// non-zero so a caller that acts immediately after `CreateChannel` /
/// `StartChannel` / `CreateSignalMap` still observes the transient state and the
/// `ConflictException` it produces, exactly as it would against the real service.
pub const TRANSIENT_STATE_SPAN: Duration = Duration::from_millis(500);

/// The id the first channel of a process gets. AWS channel ids are numeric
/// strings (the issue's example is `1234567`); minting them from a counter at a
/// fixed seven-digit width keeps the `BTreeMap` key order equal to the mint
/// order, which is what makes `ListChannels` pagination deterministic.
const FIRST_CHANNEL_ID: u64 = 1_000_000;

/// The most channels this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateChannel` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_CHANNELS: usize = 100;

/// The most `RequestId` entries kept for `CreateChannel` idempotency.
///
/// At most one token is recorded per stored channel and a token is dropped with
/// the channel it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the channels they name.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_CHANNELS;

/// The id the first signal map of a process gets. AWS signal map ids are short
/// strings (`__stringMin7Max11PatternAws097`, i.e. 7 to 11 characters); minting
/// them from a counter at a fixed seven-digit width satisfies that length and
/// keeps the `BTreeMap` key order equal to the mint order. A range of its own,
/// for the same reason as [`FIRST_INPUT_SECURITY_GROUP_ID`]: a caller that
/// passes a channel's or an input's id where a signal map's belongs gets a
/// `NotFoundException` rather than a coincidental hit.
const FIRST_SIGNAL_MAP_ID: u64 = 4_000_000;

/// The most signal maps this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateSignalMap` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_SIGNAL_MAPS: usize = 100;

/// The id the first SdiSource of a process gets, and the most SDI sources this
/// emulator stores for one account.
///
/// The id is a numeric string, as MediaLive resource ids are, minted from a
/// counter at a fixed seven-digit width so the `BTreeMap` key order equals the
/// mint order and `ListSdiSources` pagination is deterministic. A range of its
/// own keeps the id spaces apart. The ceiling is not a guess at an AWS quota: it
/// exists so a create loop cannot grow the emulator's heap without bound, and
/// `CreateSdiSource`'s own modelled `TooManyRequestsException` is the refusal
/// once the store is full.
const FIRST_SDI_SOURCE_ID: u64 = 7_000_000;

/// See [`FIRST_SDI_SOURCE_ID`].
pub const MAX_SDI_SOURCES: usize = 100;

/// The id the first input security group of a process gets. AWS input security
/// group ids are numeric strings, the same shape as channel and input ids; a
/// range of its own keeps the id spaces apart, so a caller (or a test) that
/// passes a channel's or an input's id where a group id belongs gets a
/// `NotFoundException` rather than a coincidental hit.
const FIRST_INPUT_SECURITY_GROUP_ID: u64 = 3_000_000;

/// The most input security groups this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateInputSecurityGroup`
/// models `TooManyRequestsException`, which is the refusal once the store is
/// full.
pub const MAX_INPUT_SECURITY_GROUPS: usize = 100;

/// The id the first input of a process gets. AWS input ids are numeric strings,
/// so they are minted from a counter at a fixed seven-digit width, which keeps
/// the `BTreeMap` key order equal to the mint order.
const FIRST_INPUT_ID: u64 = 1_000_000;

/// The most inputs this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateInput` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_INPUTS: usize = 100;

/// The most `RequestId` entries kept for `CreateInput` idempotency - the same
/// bound, for the same reason, as [`MAX_IDEMPOTENCY_TOKENS`] on channels.
pub const MAX_INPUT_IDEMPOTENCY_TOKENS: usize = MAX_INPUTS;

/// The most eventbridge rule template groups this emulator stores for one
/// account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound.
/// `CreateEventBridgeRuleTemplateGroup` models `TooManyRequestsException`, which
/// is the refusal once the store is full.
pub const MAX_EVENT_BRIDGE_RULE_TEMPLATE_GROUPS: usize = 100;

/// The id the first eventbridge rule template group of a process gets.
///
/// The `Id` member targets `__stringMin7Max11PatternAws097` and documents "AWS
/// provided template groups have ids that start with `aws-`", so a
/// caller-created group's id is seven digits; minting from a counter keeps every
/// minted id seven digits wide and the `BTreeMap` key order equal to the mint
/// order. A range of its own keeps the id spaces of the service's resource types
/// apart, as [`FIRST_SDI_SOURCE_ID`] and [`FIRST_INPUT_SECURITY_GROUP_ID`] do.
const FIRST_EVENT_BRIDGE_RULE_TEMPLATE_GROUP_ID: u64 = 5_000_000;

/// The longest `Name` `CreateEventBridgeRuleTemplateGroup` accepts, from its
/// target shape `__stringMin1Max255PatternS`.
pub const MAX_RESOURCE_NAME_LEN: usize = 255;

/// The longest `Description` a monitor resource accepts, from its target shape
/// `__stringMin0Max1024`.
pub const MAX_RESOURCE_DESCRIPTION_LEN: usize = 1024;

/// The longest `RequestId` (`@idempotencyToken`) a create accepts, from its
/// target shape `__stringMin1Max256PatternS`.
pub const MAX_REQUEST_ID_LEN: usize = 256;

/// The page size a list operation (`ListChannels`, `ListSdiSources`) uses when
/// the request names no `MaxResults`, and the ceiling it clamps a larger
/// `MaxResults` to. Finite, so one request cannot ask the service to project an
/// unbounded number of resources in one response.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// The state a channel's in-flight transition settles into once it comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// `CREATING` and `STOPPING` settle into `IDLE`.
    Idle,
    /// `STARTING` settles into `RUNNING`.
    Running,
    /// `DELETING` settles into the channel no longer existing.
    Gone,
}

/// A transition the clock owes: the instant it becomes due and what it settles
/// into. Stored, never slept on.
#[derive(Debug, Clone, Copy)]
struct Transition {
    due: SystemTime,
    to: Settled,
}

/// One created channel, as `CreateChannel` stored it plus its lifecycle state.
///
/// The members mirror the model's `Channel` shape. Members the real service
/// computes from infrastructure it owns (a pipeline's egress IP, a VPC network
/// interface id, an engine-version expiry) are NOT invented here: they stay
/// absent, because this emulator has no such infrastructure to report.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub id: String,
    pub arn: String,
    pub state: String,
    pub channel_class: String,
    pub name: Option<String>,
    pub cdi_input_specification: Option<CdiInputSpecification>,
    pub destinations: Vec<OutputDestination>,
    pub encoder_settings: Option<EncoderSettings>,
    pub input_attachments: Vec<InputAttachment>,
    pub input_specification: Option<InputSpecification>,
    pub log_level: Option<String>,
    pub maintenance: Option<MaintenanceStatus>,
    pub role_arn: Option<String>,
    pub tags: HashMap<String, String>,
    pub channel_security_groups: Vec<String>,
    pub anywhere_settings: Option<AnywhereSettings>,
    pub linked_channel_settings: Option<LinkedChannelSettings>,
    pub inference_settings: Option<InferenceSettings>,
    pub vpc: Option<VpcOutputSettings>,
    pub channel_engine_version: Option<String>,
    /// The transition the clock owes this channel, if any.
    transition: Option<Transition>,
}

impl ChannelRecord {
    /// The number of encoding pipelines this channel's class gives it: two for
    /// `STANDARD`, one for `SINGLE_PIPELINE` (the `ChannelClass` doc: "A standard
    /// channel has two encoding pipelines and a single pipeline channel only has
    /// one").
    pub fn pipeline_count(&self) -> i32 {
        if self.channel_class == channel_class::SINGLE_PIPELINE {
            1
        } else {
            2
        }
    }

    /// How many of this channel's pipelines are running: all of them exactly
    /// while the channel is `RUNNING`, none in any other state.
    pub fn pipelines_running_count(&self) -> i32 {
        if self.state == channel_state::RUNNING {
            self.pipeline_count()
        } else {
            0
        }
    }

    /// `PipelineDetail` is documented as "Runtime details of a pipeline when a
    /// channel is running", so there is one entry per running pipeline and none
    /// at all when the channel is not running. Only `PipelineId` is reported: the
    /// other members describe media actually being ingested, which this emulator
    /// does not run and therefore does not claim.
    pub fn pipeline_details(&self) -> Vec<PipelineDetail> {
        (0..self.pipelines_running_count())
            .map(|pipeline| PipelineDetail {
                pipeline_id: Some(pipeline.to_string()),
                ..PipelineDetail::default()
            })
            .collect()
    }

    /// Whether this channel may be started, stopped or deleted right now.
    pub fn is_idle(&self) -> bool {
        self.state == channel_state::IDLE
    }

    /// Whether this channel is in a state `DeleteChannel` accepts: a channel is
    /// deletable once it has stopped (`IDLE`) or has failed, never while it is
    /// creating, starting, running, stopping or already deleting.
    pub fn is_deletable(&self) -> bool {
        self.is_idle()
            || self.state == channel_state::CREATE_FAILED
            || self.state == channel_state::UPDATE_FAILED
    }

    /// Put the channel into `state` as of `now` and owe `to` once
    /// [`TRANSIENT_STATE_SPAN`] has passed.
    fn begin_transition(&mut self, state: &str, to: Settled, now: SystemTime) {
        self.state = state.to_string();
        self.transition = Some(Transition {
            due: now + TRANSIENT_STATE_SPAN,
            to,
        });
    }

    /// Materialise the transition the clock has already made due. Returns false
    /// when the channel is now gone (a settled `DELETING`), so the caller drops
    /// the record.
    fn settle(&mut self, now: SystemTime) -> bool {
        match self.transition {
            Some(transition) if now >= transition.due => match transition.to {
                Settled::Idle => {
                    self.state = channel_state::IDLE.to_string();
                    self.transition = None;
                    true
                }
                Settled::Running => {
                    self.state = channel_state::RUNNING.to_string();
                    self.transition = None;
                    true
                }
                Settled::Gone => false,
            },
            _ => true,
        }
    }
}

/// One created eventbridge rule template group, as
/// `CreateEventBridgeRuleTemplateGroup` stored it.
///
/// `Description` is `Option` because the member is documented "A resource's
/// optional description": a group created without one has none, rather than an
/// empty one. `created_at` / `modified_at` are the only computed members, and
/// `region` is what scopes the group's name uniqueness and the identifier
/// lookups.
#[derive(Debug, Clone)]
pub struct EventBridgeRuleTemplateGroupRecord {
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for, which is also the region
    /// [`Self::arn`] names.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

/// One created input security group, as `CreateInputSecurityGroup` stored it.
///
/// Only the members the request supplies are stored. `State`, `Inputs` and
/// `Channels` are NOT stored: the model documents them as what currently uses
/// the group, so they are derived from the rest of the state on every read
/// ([`ComAmazonawsMedialiveMedialiveState::input_security_group_description`]).
#[derive(Debug, Clone)]
pub struct InputSecurityGroupRecord {
    pub id: String,
    pub arn: String,
    /// The whitelisted IPv4 CIDRs, in the order the request listed them.
    pub whitelist_rules: Vec<String>,
    pub tags: HashMap<String, String>,
}

/// Whether `cidr` is an IPv4 CIDR block, which is what
/// `CreateInputSecurityGroup`'s `WhitelistRules` member documents it accepts
/// ("List of IPv4 CIDR addresses to whitelist",
/// `smithy_query(query="doc", shape="InputWhitelistRuleCidr", member="Cidr")`:
/// "The IPv4 CIDR to whitelist").
///
/// Dotted-quad address, `/`, then a prefix length of 0..=32 — the published
/// bounds of an IPv4 prefix. The model declares no `@pattern` for it, so this is
/// the service-side rule the boundary validator cannot express.
pub fn is_ipv4_cidr(cidr: &str) -> bool {
    let Some((address, prefix)) = cidr.split_once('/') else {
        return false;
    };
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Ok(prefix_length) = prefix.parse::<u32>() else {
        return false;
    };
    prefix_length <= 32 && address.parse::<::std::net::Ipv4Addr>().is_ok()
}

/// A bounded map from a caller-chosen `@idempotencyToken` to the id of the
/// resource that token created, so a retried create returns the resource the
/// first attempt made instead of a second one.
///
/// One implementation for every resource kind that models an idempotency token
/// (channels, inputs, SDI sources, signal maps, template groups), so the cap and
/// the eviction order cannot drift between them, and a token used on two
/// different creates cannot make one of them answer with the other's resource.
/// The fields are PRIVATE: the only way in is [`Self::record`], which enforces
/// the cap by evicting the oldest token.
#[derive(Debug, Default)]
pub struct IdempotencyTokens {
    /// token -> the id of the resource it created.
    tokens: HashMap<String, String>,
    /// The order [`Self::tokens`] was written in, so the cap evicts the oldest
    /// token rather than an arbitrary one.
    order: VecDeque<String>,
}

impl IdempotencyTokens {
    /// Record `token` as having created `resource_id`, evicting the oldest token
    /// first if that would push the map past `cap`.
    fn record(&mut self, token: String, resource_id: &str, cap: usize) {
        while self.tokens.len() >= cap {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap
                // an invariant even if they ever drift.
                None => {
                    self.tokens.clear();
                    break;
                }
            }
        }
        if self
            .tokens
            .insert(token.clone(), resource_id.to_string())
            .is_none()
        {
            self.order.push_back(token);
        }
    }

    /// The id `token` created, whether or not that resource still exists.
    fn created(&self, token: &str) -> Option<&str> {
        self.tokens.get(token).map(String::as_str)
    }

    /// Drop every token that created `resource_id`, so a token never outlives
    /// the resource it minted.
    fn forget(&mut self, resource_id: &str) {
        self.tokens.retain(|_, created| created != resource_id);
        let live = &self.tokens;
        self.order.retain(|token| live.contains_key(token));
    }
}

/// One created SdiSource, as `CreateSdiSource` stored it and `UpdateSdiSource`
/// last changed it.
///
/// The members mirror the model's `SdiSource` shape, minus the two it derives:
/// `Inputs` and `State` come from what else is in the store (see
/// [`ComAmazonawsMedialiveMedialiveState::inputs_using_sdi_source`]) and from
/// whether the source has been deleted, never from a field written at create
/// time.
#[derive(Debug, Clone)]
pub struct SdiSourceRecord {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub r#type: Option<String>,
    /// The request's `Tags`. `CreateSdiSource` accepts them but neither
    /// `SdiSource` nor `SdiSourceSummary` carries a `Tags` member
    /// (`smithy_query(query="shape", shape="SdiSource")`), so nothing reports
    /// them until `ListTagsForResource` stops being a scaffold stub; they are
    /// kept rather than dropped so that operation reads them from the store.
    pub tags: HashMap<String, String>,
    /// Whether `DeleteSdiSource` has deleted this source.
    ///
    /// A deleted SdiSource is NOT dropped from the account: `DELETED` is one of
    /// the three states the model gives it (`smithy_query(query="shape",
    /// shape="SdiSourceState")`: IDLE | IN_USE | DELETED), and it is terminal,
    /// so the record stays readable in that state rather than vanishing. PRIVATE,
    /// so the only way into it is [`delete_sdi_source`] and the only way to read
    /// it is [`SdiSourceRecord::is_deleted`].
    deleted: bool,
}

impl SdiSourceRecord {
    /// Whether this source has been deleted, which is what puts it in the
    /// terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }
}

/// Per-service state for com-amazonaws-medialive-medialive. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMedialiveMedialiveState {
    /// Every channel that exists, keyed by its minted `Id`. A `BTreeMap` so
    /// `ListChannels` walks it in id order on every call and its `NextToken`
    /// means the same thing on the next one.
    pub channels: BTreeMap<String, ChannelRecord>,
    /// Mint counter behind [`Self::mint_channel_id`].
    pub next_channel_id: u64,
    /// Every input security group that exists, keyed by its minted `Id`. A
    /// `BTreeMap` for the same reason as [`Self::channels`]: a reader walks it in
    /// id order on every call.
    pub input_security_groups: BTreeMap<String, InputSecurityGroupRecord>,
    /// Mint counter behind [`Self::mint_input_security_group_id`].
    pub next_input_security_group_id: u64,
    /// `CreateChannel`'s `RequestId` (`@idempotencyToken`) -> the channel it
    /// created, so a retry returns that channel instead of a second one.
    ///
    /// PRIVATE, so the cap and the eviction order are invariants the type
    /// enforces: it is written only by [`Self::record_idempotency_token`] and
    /// read only through [`Self::channel_for_token`].
    channel_tokens: IdempotencyTokens,
    /// Every input that exists, keyed by its minted `Id`, in id order for the
    /// same reason as [`Self::channels`].
    pub inputs: BTreeMap<String, InputRecord>,
    /// Mint counter behind [`Self::mint_input_id`].
    pub next_input_id: u64,
    /// `CreateInput`'s `RequestId` (`@idempotencyToken`) -> the input it created.
    input_tokens: IdempotencyTokens,
    /// Every cloudwatch alarm template group that exists, keyed by its minted
    /// `Id`. A `BTreeMap` so a later `ListCloudWatchAlarmTemplateGroups` walks it
    /// in id order on every call.
    ///
    /// PRIVATE, so the name-uniqueness rule, the cap and the mint stay invariants
    /// of [`Self::insert_alarm_template_group`] rather than of each handler.
    alarm_template_groups: BTreeMap<String, CloudWatchAlarmTemplateGroupRecord>,
    /// Mint counter behind [`Self::mint_alarm_template_group_id`].
    next_alarm_template_group_id: u64,
    /// `CreateCloudWatchAlarmTemplateGroup`'s `RequestId` (`@idempotencyToken`),
    /// keyed per region by [`template_group_token_key`] -> the group it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    alarm_template_group_tokens: IdempotencyTokens,
    /// Every SdiSource that exists, keyed by its minted `Id`, in a `BTreeMap`
    /// for the same reason as [`Self::channels`].
    pub sdi_sources: BTreeMap<String, SdiSourceRecord>,
    /// Mint counter behind [`Self::mint_sdi_source_id`].
    pub next_sdi_source_id: u64,
    /// `CreateSdiSource`'s `RequestId` (`@idempotencyToken`) -> the SdiSource it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    sdi_source_tokens: IdempotencyTokens,
    /// Every eventbridge rule template group that exists, keyed by its minted
    /// `Id`. A `BTreeMap` so a walk over the groups is in id order on every
    /// call.
    pub event_bridge_rule_template_groups: BTreeMap<String, EventBridgeRuleTemplateGroupRecord>,
    /// Mint counter behind [`Self::mint_event_bridge_rule_template_group_id`].
    pub next_event_bridge_rule_template_group_id: u64,
    /// `CreateEventBridgeRuleTemplateGroup`'s `RequestId` (`@idempotencyToken`)
    /// -> the group it created, so a retry returns that group instead of a
    /// second one. PRIVATE for the same reason as [`Self::channel_tokens`].
    event_bridge_rule_template_group_tokens: IdempotencyTokens,
    /// Every eventbridge rule template that exists, keyed by its minted `Id`. A
    /// `BTreeMap` so a later `ListEventBridgeRuleTemplates` walks it in id order
    /// on every call.
    ///
    /// PRIVATE, so the group resolution, the name-uniqueness rule, the cap and
    /// the mint stay invariants of
    /// [`Self::insert_event_bridge_rule_template`] rather than of each handler.
    event_bridge_rule_templates: BTreeMap<String, EventBridgeRuleTemplateRecord>,
    /// Mint counter behind [`Self::mint_event_bridge_rule_template_id`].
    next_event_bridge_rule_template_id: u64,
    /// `CreateEventBridgeRuleTemplate`'s `RequestId` (`@idempotencyToken`), keyed
    /// per region by [`template_group_token_key`] -> the template it created.
    /// PRIVATE for the same reason as [`Self::channel_tokens`].
    event_bridge_rule_template_tokens: IdempotencyTokens,
    /// Every signal map that exists, keyed by its minted `Id`, in mint order for
    /// the same reason as [`Self::channels`].
    pub signal_maps: BTreeMap<String, SignalMapRecord>,
    /// Mint counter behind [`Self::mint_signal_map_id`].
    pub next_signal_map_id: u64,
    /// `CreateSignalMap`'s `RequestId` (`@idempotencyToken`) -> the signal map it
    /// created.
    signal_map_tokens: IdempotencyTokens,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every transition the clock had already made due as of `now`,
    /// then answer from the result.
    ///
    /// Call this at the top of every operation, reporting ones included: it only
    /// applies transitions that were owed at that instant whether or not anybody
    /// called, so it never changes what a later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        let mut removed: Vec<String> = Vec::new();
        self.channels.retain(|id, channel| {
            let survives = channel.settle(now);
            if !survives {
                removed.push(id.clone());
            }
            survives
        });
        // A token never outlives the channel it minted.
        for id in removed {
            self.channel_tokens.forget(&id);
        }
        for signal_map in self.signal_maps.values_mut() {
            signal_map.settle(now);
        }
    }

    /// Whether the store is full, so `CreateChannel` must refuse rather than mint
    /// another channel.
    pub fn is_full(&self) -> bool {
        self.channels.len() >= MAX_CHANNELS
    }

    /// Record `token` as having created `channel_id`, evicting the oldest token
    /// first if that would push the map past [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record_idempotency_token(&mut self, token: String, channel_id: &str) {
        self.channel_tokens
            .record(token, channel_id, MAX_IDEMPOTENCY_TOKENS);
    }

    /// The next channel id, as AWS shapes them: a numeric string.
    pub fn mint_channel_id(&mut self) -> String {
        let id = FIRST_CHANNEL_ID + self.next_channel_id;
        self.next_channel_id += 1;
        id.to_string()
    }

    /// The next input security group id, as AWS shapes them: a numeric string.
    pub fn mint_input_security_group_id(&mut self) -> String {
        let id = FIRST_INPUT_SECURITY_GROUP_ID + self.next_input_security_group_id;
        self.next_input_security_group_id += 1;
        id.to_string()
    }

    /// Whether the input security group store is full, so
    /// `CreateInputSecurityGroup` must refuse rather than mint another group.
    pub fn input_security_groups_are_full(&self) -> bool {
        self.input_security_groups.len() >= MAX_INPUT_SECURITY_GROUPS
    }

    /// The ids of the channels currently using `group_id` as one of their
    /// channel security groups — `InputSecurityGroup$Channels` ("The list of
    /// channels currently using this Input Security Group as their channel
    /// security group"). Derived from the channel store, so it tracks every
    /// later create and delete rather than being recorded at create time.
    pub fn channels_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| {
                channel
                    .channel_security_groups
                    .iter()
                    .any(|attached| attached == group_id)
            })
            .map(|channel| channel.id.clone())
            .collect()
    }

    /// The ids of the inputs currently using `group_id` as one of their input
    /// security groups — `InputSecurityGroup$Inputs` ("The list of inputs
    /// currently using this Input Security Group"). Derived from the input store
    /// for the same reason as [`Self::channels_using_security_group`].
    pub fn inputs_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.inputs
            .values()
            .filter(|input| {
                input
                    .security_groups
                    .iter()
                    .any(|attached| attached == group_id)
            })
            .map(|input| input.id.clone())
            .collect()
    }

    /// The full `InputSecurityGroup` description of one stored group, or `None`
    /// when no group has that id.
    ///
    /// `State` is derived, not stored: a group is `IN_USE` exactly while some
    /// resource in state references it, and `IDLE` otherwise ("The current state
    /// of the Input Security Group").
    pub fn input_security_group_description(&self, id: &str) -> Option<InputSecurityGroup> {
        let group = self.input_security_groups.get(id)?;
        let inputs = self.inputs_using_security_group(id);
        let channels = self.channels_using_security_group(id);
        let state = if inputs.is_empty() && channels.is_empty() {
            input_security_group_state::IDLE
        } else {
            input_security_group_state::IN_USE
        };
        Some(InputSecurityGroup {
            arn: Some(group.arn.clone()),
            id: Some(group.id.clone()),
            inputs: Some(inputs),
            state: Some(state.to_string()),
            tags: Some(group.tags.clone()),
            whitelist_rules: Some(
                group
                    .whitelist_rules
                    .iter()
                    .map(|cidr| InputWhitelistRule {
                        cidr: Some(cidr.clone()),
                    })
                    .collect(),
            ),
            channels: Some(channels),
        })
    }

    /// The channel a stored `RequestId` created, if that channel still exists.
    pub fn channel_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.channel_tokens.created(request_id)?;
        self.channels.get(id).map(|channel| channel.id.as_str())
    }

    /// Whether the SdiSource store is full, so `CreateSdiSource` must refuse
    /// rather than mint another source.
    pub fn sdi_sources_full(&self) -> bool {
        self.sdi_sources.len() >= MAX_SDI_SOURCES
    }

    /// The next SdiSource id, as AWS shapes MediaLive resource ids: a numeric
    /// string.
    pub fn mint_sdi_source_id(&mut self) -> String {
        let id = FIRST_SDI_SOURCE_ID + self.next_sdi_source_id;
        self.next_sdi_source_id += 1;
        id.to_string()
    }

    /// Record `token` as having created `sdi_source_id`.
    pub fn record_sdi_source_token(&mut self, token: String, sdi_source_id: &str) {
        self.sdi_source_tokens
            .record(token, sdi_source_id, MAX_SDI_SOURCES);
    }

    /// The SdiSource a stored `RequestId` created, if that source still exists.
    pub fn sdi_source_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.sdi_source_tokens.created(request_id)?;
        self.sdi_sources.get(id).map(|source| source.id.as_str())
    }

    /// The inputs that are currently using the SdiSource `id` - the model's
    /// `Inputs` member ("The list of inputs that are currently using this SDI
    /// source. This list will be empty if the SdiSource has just been deleted").
    ///
    /// Derived from the input store: an input names the sources it ingests from
    /// in `CreateInput`'s `SdiSources` (`InputSdiSources`, "SDI Sources for this
    /// Input"), whose entries are SdiSource ids, so this is every input id whose
    /// list names `id`, in input-id order. A deleted source has nothing using it
    /// any more, which is what makes its `Inputs` empty.
    fn inputs_using_sdi_source(&self, id: &str) -> Vec<String> {
        if self
            .sdi_sources
            .get(id)
            .is_some_and(SdiSourceRecord::is_deleted)
        {
            return Vec::new();
        }
        self.inputs
            .values()
            .filter(|input| input.sdi_sources.iter().any(|source| source == id))
            .map(|input| input.id.clone())
            .collect()
    }

    /// Whether an input is attached to the SdiSource `id`, which is what
    /// `DeleteSdiSource` refuses on ("The SdiSource must ... not be attached to
    /// any input").
    pub fn sdi_source_is_in_use(&self, id: &str) -> bool {
        !self.inputs_using_sdi_source(id).is_empty()
    }

    /// The full `SdiSource` description of one stored source, or `None` when no
    /// source has that id.
    pub fn sdi_source_description(&self, id: &str) -> Option<SdiSource> {
        let source = self.sdi_sources.get(id)?;
        let inputs = self.inputs_using_sdi_source(id);
        Some(SdiSource {
            arn: Some(source.arn.clone()),
            id: Some(source.id.clone()),
            state: Some(sdi_source_state_for(source, &inputs).to_string()),
            inputs: Some(inputs),
            mode: source.mode.clone(),
            name: source.name.clone(),
            r#type: source.r#type.clone(),
        })
    }

    /// The `SdiSourceSummary` projection of one stored source. The summary shape
    /// carries exactly the `SdiSource` members
    /// (`smithy_query(query="shape", shape="SdiSourceSummary")`), so it is the
    /// same stored record read through the list shape.
    pub fn sdi_source_summary(&self, source: &SdiSourceRecord) -> SdiSourceSummary {
        let inputs = self.inputs_using_sdi_source(&source.id);
        SdiSourceSummary {
            arn: Some(source.arn.clone()),
            id: Some(source.id.clone()),
            state: Some(sdi_source_state_for(source, &inputs).to_string()),
            inputs: Some(inputs),
            mode: source.mode.clone(),
            name: source.name.clone(),
            r#type: source.r#type.clone(),
        }
    }

    /// Whether the group store is full, so
    /// `CreateEventBridgeRuleTemplateGroup` must refuse rather than mint
    /// another group.
    pub fn event_bridge_rule_template_groups_full(&self) -> bool {
        self.event_bridge_rule_template_groups.len() >= MAX_EVENT_BRIDGE_RULE_TEMPLATE_GROUPS
    }

    /// The next eventbridge rule template group id: seven digits, so it matches
    /// the `Id` member's `__stringMin7Max11PatternAws097` target without the
    /// `aws-` prefix AWS reserves for the groups it provides itself.
    pub fn mint_event_bridge_rule_template_group_id(&mut self) -> String {
        let id = FIRST_EVENT_BRIDGE_RULE_TEMPLATE_GROUP_ID
            + self.next_event_bridge_rule_template_group_id;
        self.next_event_bridge_rule_template_group_id += 1;
        id.to_string()
    }

    /// Record `token`, as presented in `region`, as having created the group
    /// `group_id`.
    pub fn record_event_bridge_rule_template_group_token(
        &mut self,
        region: &str,
        token: &str,
        group_id: &str,
    ) {
        self.event_bridge_rule_template_group_tokens.record(
            template_group_token_key(region, token),
            group_id,
            MAX_EVENT_BRIDGE_RULE_TEMPLATE_GROUPS,
        );
    }

    /// The eventbridge rule template group a `RequestId` presented in `region`
    /// created, if that group still exists and belongs to that region.
    ///
    /// An `@idempotencyToken` is idempotent for the regional endpoint it was
    /// presented to, so the region is part of the key AND re-checked on the
    /// record: one token used in two regions creates (and resolves) a group of
    /// its own in each.
    pub fn event_bridge_rule_template_group_for_token(
        &self,
        region: &str,
        request_id: &str,
    ) -> Option<&str> {
        let id = self
            .event_bridge_rule_template_group_tokens
            .created(&template_group_token_key(region, request_id))?;
        self.event_bridge_rule_template_groups
            .get(id)
            .filter(|group| group.region == region)
            .map(|group| group.id.as_str())
    }

    /// The group called `name` in `region`, if one exists. The `Name` member is
    /// documented "Names must be unique within the scope of a resource type in a
    /// specific region", so the scope of the search is one region.
    pub fn event_bridge_rule_template_group_named(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&EventBridgeRuleTemplateGroupRecord> {
        self.event_bridge_rule_template_groups
            .values()
            .find(|group| group.region == region && group.name == name)
    }

    /// The group an `Identifier` names in `region`: "Can be either be its id or
    /// current name" (the `Identifier` member's documentation), so the id is
    /// tried first and the name second.
    pub fn event_bridge_rule_template_group(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&EventBridgeRuleTemplateGroupRecord> {
        self.event_bridge_rule_template_groups
            .get(identifier)
            .filter(|group| group.region == region)
            .or_else(|| self.event_bridge_rule_template_group_named(region, identifier))
    }

    /// The full `Channel` description of one stored channel, or `None` when no
    /// channel has that id.
    pub fn channel_description(&self, id: &str) -> Option<Channel> {
        let channel = self.channels.get(id)?;
        Some(Channel {
            arn: Some(channel.arn.clone()),
            cdi_input_specification: channel.cdi_input_specification.clone(),
            channel_class: Some(channel.channel_class.clone()),
            destinations: Some(channel.destinations.clone()),
            // ABSENT, not empty: `EgressEndpoints` reports the public IP each
            // pipeline's output comes from, which the real service allocates and
            // this emulator does not. An empty list would claim the channel
            // egresses from nowhere; absent says it is not reported.
            egress_endpoints: None,
            encoder_settings: channel.encoder_settings.clone(),
            id: Some(channel.id.clone()),
            input_attachments: Some(channel.input_attachments.clone()),
            input_specification: channel.input_specification.clone(),
            log_level: channel.log_level.clone(),
            maintenance: channel.maintenance.clone(),
            name: channel.name.clone(),
            pipeline_details: Some(channel.pipeline_details()),
            pipelines_running_count: Some(channel.pipelines_running_count()),
            role_arn: channel.role_arn.clone(),
            state: Some(channel.state.clone()),
            tags: Some(channel.tags.clone()),
            vpc: channel.vpc.as_ref().map(described_vpc),
            anywhere_settings: channel.anywhere_settings.as_ref().map(described_anywhere),
            channel_engine_version: channel
                .channel_engine_version
                .as_deref()
                .map(described_engine_version),
            linked_channel_settings: channel
                .linked_channel_settings
                .as_ref()
                .map(|linked| self.described_linked(&channel.arn, linked)),
            channel_security_groups: Some(channel.channel_security_groups.clone()),
            inference_settings: channel.inference_settings.as_ref().map(described_inference),
        })
    }

    /// The `ChannelSummary` projection of one stored channel — the same stored
    /// record as [`Self::channel_description`], minus the members the summary
    /// shape does not carry (`EncoderSettings`, `PipelineDetails`).
    pub fn channel_summary(&self, channel: &ChannelRecord) -> ChannelSummary {
        ChannelSummary {
            arn: Some(channel.arn.clone()),
            cdi_input_specification: channel.cdi_input_specification.clone(),
            channel_class: Some(channel.channel_class.clone()),
            destinations: Some(channel.destinations.clone()),
            // ABSENT for the same reason as in `channel_description`.
            egress_endpoints: None,
            id: Some(channel.id.clone()),
            input_attachments: Some(channel.input_attachments.clone()),
            input_specification: channel.input_specification.clone(),
            log_level: channel.log_level.clone(),
            maintenance: channel.maintenance.clone(),
            name: channel.name.clone(),
            pipelines_running_count: Some(channel.pipelines_running_count()),
            role_arn: channel.role_arn.clone(),
            state: Some(channel.state.clone()),
            tags: Some(channel.tags.clone()),
            vpc: channel.vpc.as_ref().map(described_vpc),
            anywhere_settings: channel.anywhere_settings.as_ref().map(described_anywhere),
            channel_engine_version: channel
                .channel_engine_version
                .as_deref()
                .map(described_engine_version),
            used_channel_engine_versions: None,
            linked_channel_settings: channel
                .linked_channel_settings
                .as_ref()
                .map(|linked| self.described_linked(&channel.arn, linked)),
            channel_security_groups: Some(channel.channel_security_groups.clone()),
            inference_settings: channel.inference_settings.as_ref().map(described_inference),
        }
    }

    /// Whether the input store is full, so `CreateInput` must refuse rather than
    /// mint another input.
    pub fn inputs_are_full(&self) -> bool {
        self.inputs.len() >= MAX_INPUTS
    }

    /// The next input id, as AWS shapes them: a numeric string.
    pub fn mint_input_id(&mut self) -> String {
        let id = FIRST_INPUT_ID + self.next_input_id;
        self.next_input_id += 1;
        id.to_string()
    }

    /// Record `token` as having created `input_id`, under the same bounded,
    /// evicting discipline as the channel tokens.
    pub fn record_input_idempotency_token(&mut self, token: String, input_id: &str) {
        self.input_tokens
            .record(token, input_id, MAX_INPUT_IDEMPOTENCY_TOKENS);
    }

    /// The input a stored `RequestId` created, if that input still exists.
    pub fn input_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.input_tokens.created(request_id)?;
        self.inputs.get(id).map(|input| input.id.as_str())
    }

    /// An input's `AttachedChannels`: "A list of channel IDs that that input is
    /// attached to". Derived from the channels' own `InputAttachments`, so it
    /// tracks every channel created or deleted after this input.
    pub fn attached_channels(&self, input_id: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| {
                channel.input_attachments.iter().any(|attachment| {
                    attachment.input_id.as_deref() == Some(input_id)
                })
            })
            .map(|channel| channel.id.clone())
            .collect()
    }

    /// An input's `InputPartnerIds`: "A list of IDs for all Inputs which are
    /// partners of this one". A partner pair names each other, so this is both
    /// the inputs created as a partner OF this one and, if this input is itself
    /// a partner, the input it was created for.
    fn input_partner_ids(&self, input_id: &str) -> Vec<String> {
        let mut partners: Vec<String> = self
            .inputs
            .values()
            .filter(|input| input.partner_of.as_deref() == Some(input_id))
            .map(|input| input.id.clone())
            .collect();
        if let Some(primary) = self
            .inputs
            .get(input_id)
            .and_then(|input| input.partner_of.clone())
            .filter(|primary| self.inputs.contains_key(primary))
        {
            partners.push(primary);
        }
        partners.sort();
        partners
    }

    /// The `InputState` of one stored input: `ATTACHED` exactly while some
    /// channel attaches it, and `DETACHED` while none does.
    pub fn input_state(&self, input: &InputRecord) -> &'static str {
        if self.attached_channels(&input.id).is_empty() {
            input_state::DETACHED
        } else {
            input_state::ATTACHED
        }
    }

    /// The full `Input` description of one stored input, or `None` when no input
    /// has that id.
    pub fn input_description(&self, id: &str) -> Option<Input> {
        let input = self.inputs.get(id)?;
        Some(Input {
            arn: Some(input.arn.clone()),
            attached_channels: Some(self.attached_channels(id)),
            destinations: Some(
                input
                    .destinations
                    .iter()
                    .map(described_input_destination)
                    .collect(),
            ),
            id: Some(input.id.clone()),
            input_class: Some(input.input_class().to_string()),
            input_devices: Some(input.input_devices.clone()),
            input_partner_ids: Some(self.input_partner_ids(id)),
            input_source_type: Some(input.input_source_type().to_string()),
            media_connect_flows: Some(
                input
                    .media_connect_flows
                    .iter()
                    .map(|flow| MediaConnectFlow {
                        flow_arn: flow.flow_arn.clone(),
                    })
                    .collect(),
            ),
            name: input.name.clone(),
            role_arn: input.role_arn.clone(),
            security_groups: Some(input.security_groups.clone()),
            sources: Some(input.sources.iter().map(described_input_source).collect()),
            state: Some(self.input_state(input).to_string()),
            tags: Some(input.tags.clone()),
            r#type: input.input_type.clone(),
            srt_settings: input.srt_settings.as_ref().map(described_srt_settings),
            input_network_location: Some(input.input_network_location.clone()),
            multicast_settings: input
                .multicast_settings
                .as_ref()
                .map(described_multicast_settings),
            smpte2110_receiver_group_settings: input.smpte2110_receiver_group_settings.clone(),
            sdi_sources: Some(input.sdi_sources.clone()),
            router_settings: input.router_settings.as_ref().map(described_router_settings),
        })
    }

    /// A primary channel's `FollowingChannelArns`: the ARNs of the channels
    /// whose own follower settings name this channel as their primary. Derived
    /// from state, so it tracks every later create and delete.
    fn following_channel_arns(&self, primary_arn: &str) -> Vec<String> {        self.channels
            .values()
            .filter(|channel| {
                channel
                    .linked_channel_settings
                    .as_ref()
                    .and_then(|linked| linked.follower_channel_settings.as_ref())
                    .and_then(|follower| follower.primary_channel_arn.as_deref())
                    == Some(primary_arn)
            })
            .map(|channel| channel.arn.clone())
            .collect()
    }

    fn described_linked(
        &self,
        arn: &str,
        linked: &LinkedChannelSettings,
    ) -> DescribeLinkedChannelSettings {
        DescribeLinkedChannelSettings {
            follower_channel_settings: linked.follower_channel_settings.as_ref().map(|follower| {
                DescribeFollowerChannelSettings {
                    linked_channel_type: follower.linked_channel_type.clone(),
                    primary_channel_arn: follower.primary_channel_arn.clone(),
                }
            }),
            primary_channel_settings: linked.primary_channel_settings.as_ref().map(|primary| {
                DescribePrimaryChannelSettings {
                    following_channel_arns: Some(self.following_channel_arns(arn)),
                    linked_channel_type: primary.linked_channel_type.clone(),
                }
            }),
        }
    }

    /// The channel whose ARN is exactly `arn`, if this store holds one. An ARN
    /// identifies a resource globally, so a partition, region or account that
    /// differs names a resource this emulator does not hold.
    fn channel_by_arn(&self, arn: &str) -> Option<&ChannelRecord> {
        self.channels.values().find(|channel| channel.arn == arn)
    }

    /// Whether the signal-map store is full, so `CreateSignalMap` must refuse
    /// rather than mint another signal map.
    pub fn signal_maps_full(&self) -> bool {
        self.signal_maps.len() >= MAX_SIGNAL_MAPS
    }

    /// The next signal map id: a numeric string of the length the model's
    /// `__stringMin7Max11PatternAws097` allows.
    pub fn mint_signal_map_id(&mut self) -> String {
        let id = FIRST_SIGNAL_MAP_ID + self.next_signal_map_id;
        self.next_signal_map_id += 1;
        id.to_string()
    }

    /// Record `token` as having created `signal_map_id`.
    pub fn record_signal_map_token(&mut self, token: String, signal_map_id: &str) {
        self.signal_map_tokens
            .record(token, signal_map_id, MAX_SIGNAL_MAPS);
    }

    /// The signal map a stored `RequestId` created, if it still exists.
    pub fn signal_map_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.signal_map_tokens.created(request_id)?;
        self.signal_maps.get(id).map(|map| map.id.as_str())
    }

    /// The id of the signal map `identifier` names: `GetSignalMap`'s
    /// `Identifier` is documented as "A signal map's identifier. Can be either
    /// be its id or current name", so an id is tried first and then a name.
    pub fn signal_map_id_for_identifier(&self, identifier: &str) -> Option<&str> {
        if let Some(map) = self.signal_maps.get(identifier) {
            return Some(map.id.as_str());
        }
        self.signal_map_named(identifier)
    }

    /// The id of the signal map called `name`, if one exists. A signal map's
    /// `Name` is documented as unique "within the scope of a resource type in a
    /// specific region", which is what makes this lookup single-valued.
    pub fn signal_map_named(&self, name: &str) -> Option<&str> {
        self.signal_maps
            .values()
            .find(|map| map.name == name)
            .map(|map| map.id.as_str())
    }

    /// Discover the media workflow graph reachable from the resource
    /// `entry_point` names, keyed by each discovered resource's ARN.
    ///
    /// The graph's resources are CHANNELS: a channel is the only resource kind
    /// this store holds that carries links to other media resources, so an ARN
    /// naming anything else (another resource type of this service, another
    /// service, another account or region) resolves to no graph resource and
    /// answers `None` — which the caller renders as the operation's modelled
    /// `NotFoundException`. An input or an input security group this store does
    /// hold is a member of no workflow graph here: nothing records which inputs
    /// a signal map's workflow reaches, so claiming one would be fabrication.
    ///
    /// The walk is breadth-first over the links the discovered resources
    /// themselves declare (a follower channel's primary, and the followers that
    /// name it), so the graph is read out of persisted state. It is the SNAPSHOT
    /// of that state at the instant discovery runs: the real service rediscovers
    /// a signal map's graph only when a discovery is asked for
    /// (`StartUpdateSignalMap`, still a scaffold here), so a channel created or
    /// deleted afterwards does not change a stored signal map's
    /// `MediaResourceMap`.
    pub fn discover_media_resource_map(
        &self,
        entry_point: &str,
    ) -> Option<BTreeMap<String, MediaResource>> {
        self.channel_by_arn(entry_point)?;

        let mut discovered: BTreeMap<String, MediaResource> = BTreeMap::new();
        let mut pending: VecDeque<String> = VecDeque::new();
        pending.push_back(entry_point.to_string());

        while let Some(arn) = pending.pop_front() {
            if discovered.contains_key(&arn) {
                continue;
            }
            let Some(channel) = self.channel_by_arn(&arn) else {
                continue;
            };
            // Upstream: the primary this channel follows, reported only when
            // that channel exists here — discovery never claims a resource the
            // store does not hold.
            let sources: Vec<MediaResourceNeighbor> = channel
                .linked_channel_settings
                .as_ref()
                .and_then(|linked| linked.follower_channel_settings.as_ref())
                .and_then(|follower| follower.primary_channel_arn.as_deref())
                .and_then(|primary| self.channel_by_arn(primary))
                .map(|primary| vec![neighbor(primary)])
                .unwrap_or_default();
            // Downstream: the channels that follow this one.
            let destinations: Vec<MediaResourceNeighbor> = self
                .following_channel_arns(&arn)
                .iter()
                .filter_map(|follower| self.channel_by_arn(follower))
                .map(neighbor)
                .collect();

            for next in sources.iter().chain(destinations.iter()) {
                pending.push_back(next.arn.clone());
            }
            discovered.insert(
                arn,
                MediaResource {
                    name: channel.name.clone(),
                    sources: Some(sources),
                    destinations: Some(destinations),
                },
            );
        }
        Some(discovered)
    }

    /// The full description of one stored signal map, or `None` when no signal
    /// map has that id.
    pub fn signal_map_description(&self, id: &str) -> Option<SignalMapDescription> {
        let map = self.signal_maps.get(id)?;
        let discovered = map.status == signal_map_status::CREATE_COMPLETE;
        Some(SignalMapDescription {
            arn: Some(map.arn.clone()),
            // The ids of the cloudwatch alarm template groups the create
            // resolved, or ABSENT when the request named none — absent rather
            // than empty, because `CloudWatchAlarmTemplateGroupIds` reports the
            // groups the signal map is associated with and an association the
            // request never asked for is not an empty list of associations.
            cloud_watch_alarm_template_group_ids: (!map
                .cloud_watch_alarm_template_group_ids
                .is_empty())
                .then(|| map.cloud_watch_alarm_template_group_ids.clone()),
            // The ids of the eventbridge rule template groups the create
            // resolved, or ABSENT when the request named none.
            event_bridge_rule_template_group_ids: (!map
                .event_bridge_rule_template_group_ids
                .is_empty())
                .then(|| map.event_bridge_rule_template_group_ids.clone()),
            created_at: Some(timestamp(map.created_at)),
            description: map.description.clone(),
            discovery_entry_point_arn: Some(map.discovery_entry_point_arn.clone()),
            // ABSENT while nothing failed: `ErrorMessage` is "Error message
            // associated with a failed creation or failed update attempt", and a
            // stored signal map's discovery resolved its entry point.
            error_message: None,
            failed_media_resource_map: None,
            id: Some(map.id.clone()),
            // The discovered graph and the instant it was discovered are
            // reported only once the clock has made discovery due; while the
            // signal map is CREATE_IN_PROGRESS there is nothing discovered yet.
            last_discovered_at: map.last_discovered_at.map(timestamp),
            media_resource_map: discovered.then(|| map.media_resource_map.clone()),
            modified_at: Some(timestamp(map.modified_at)),
            // No monitor has ever been deployed for a signal map in this
            // emulator (`StartMonitorDeployment` is scaffolded), so there is
            // neither a deployment to report nor a pending change to one.
            monitor_changes_pending_deployment: Some(false),
            monitor_deployment: None,
            last_successful_monitor_deployment: None,
            name: Some(map.name.clone()),
            status: Some(map.status.clone()),
            tags: Some(map.tags.clone()),
        })
    }

    /// The `SignalMapSummary` projection of one stored signal map, as
    /// `ListSignalMaps` reports it.
    ///
    /// Every member is read off the stored record or derived from it, exactly as
    /// [`Self::signal_map_description`] reads the same record for the flat
    /// outputs: the summary is one view of one signal map, so the two cannot
    /// disagree about its `Status`, its timestamps or its tags.
    pub fn signal_map_summary(&self, map: &SignalMapRecord) -> SignalMapSummary {
        SignalMapSummary {
            arn: map.arn.clone(),
            created_at: timestamp(map.created_at),
            description: map.description.clone(),
            id: map.id.clone(),
            modified_at: Some(timestamp(map.modified_at)),
            // No monitor has ever been deployed for a signal map in this
            // emulator (`StartMonitorDeployment` is still a scaffold), which is
            // what `NOT_DEPLOYED` reports.
            monitor_deployment_status: signal_map_monitor_deployment_status::NOT_DEPLOYED
                .to_string(),
            name: map.name.clone(),
            status: map.status.clone(),
            tags: Some(map.tags.clone()),
        }
    }
}

/// One created signal map, as `CreateSignalMap` stored it plus the graph its
/// discovery found and the lifecycle state of that discovery.
#[derive(Debug, Clone)]
pub struct SignalMapRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub discovery_entry_point_arn: String,
    /// The ids of the eventbridge rule template groups the create resolved from
    /// the identifiers the request named — ids this store holds, never the
    /// identifiers themselves.
    pub event_bridge_rule_template_group_ids: Vec<String>,
    /// The ids of the cloudwatch alarm template groups the create resolved, on
    /// the same terms as the eventbridge ones beside them.
    pub cloud_watch_alarm_template_group_ids: Vec<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Updated when the signal map changes — which, for a signal map nothing
    /// has updated yet, is when its discovery completed.
    modified_at: SystemTime,
    /// The graph discovery found, keyed by resource ARN. Reported only once
    /// discovery has come due.
    media_resource_map: BTreeMap<String, MediaResource>,
    /// The `SignalMapStatus` this signal map is in.
    status: String,
    /// The instant discovery becomes due, so `CREATE_IN_PROGRESS` settles into
    /// `CREATE_COMPLETE`. Stored, never slept on.
    discovery_due: SystemTime,
    last_discovered_at: Option<SystemTime>,
}

impl SignalMapRecord {
    /// Materialise the discovery the clock has already made due. Owed at
    /// `discovery_due` whether or not anybody called, so applying it in a
    /// reporting operation changes nothing a later caller would have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.status == signal_map_status::CREATE_IN_PROGRESS && now >= self.discovery_due {
            self.status = signal_map_status::CREATE_COMPLETE.to_string();
            self.last_discovered_at = Some(self.discovery_due);
            self.modified_at = self.discovery_due;
        }
    }
}

/// The description of one signal map, as `CreateSignalMap` and `GetSignalMap`
/// both report it: the model gives those operations structurally identical flat
/// outputs, so one projection feeds both through
/// [`signal_map_description_output`].
#[derive(Debug, Clone)]
pub struct SignalMapDescription {
    pub arn: Option<String>,
    pub cloud_watch_alarm_template_group_ids: Option<Vec<String>>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub description: Option<String>,
    pub discovery_entry_point_arn: Option<String>,
    pub error_message: Option<String>,
    pub event_bridge_rule_template_group_ids: Option<Vec<String>>,
    pub failed_media_resource_map: Option<BTreeMap<String, MediaResource>>,
    pub id: Option<String>,
    pub last_discovered_at: Option<::aws_smithy_types::DateTime>,
    pub last_successful_monitor_deployment: Option<SuccessfulMonitorDeployment>,
    pub media_resource_map: Option<BTreeMap<String, MediaResource>>,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub monitor_changes_pending_deployment: Option<bool>,
    pub monitor_deployment: Option<MonitorDeployment>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub tags: Option<HashMap<String, String>>,
}

/// Project a [`SignalMapDescription`] onto one of the structurally identical
/// per-operation signal-map output shapes (`CreateSignalMapOutput`,
/// `GetSignalMapOutput`).
///
/// One macro rather than a hand-written copy per operation, so the operations
/// cannot drift from each other. The modelled maps are `HashMap`s, so the
/// `BTreeMap`s the state keeps (ordered, for deterministic iteration) are
/// converted here.
macro_rules! signal_map_description_output {
    ($out:ident, $map:expr) => {{
        let map: $crate::state::SignalMapDescription = $map;
        $out {
            arn: map.arn,
            cloud_watch_alarm_template_group_ids: map.cloud_watch_alarm_template_group_ids,
            created_at: map.created_at,
            description: map.description,
            discovery_entry_point_arn: map.discovery_entry_point_arn,
            error_message: map.error_message,
            event_bridge_rule_template_group_ids: map.event_bridge_rule_template_group_ids,
            failed_media_resource_map: map
                .failed_media_resource_map
                .map(|resources| resources.into_iter().collect()),
            id: map.id,
            last_discovered_at: map.last_discovered_at,
            last_successful_monitor_deployment: map.last_successful_monitor_deployment,
            media_resource_map: map
                .media_resource_map
                .map(|resources| resources.into_iter().collect()),
            modified_at: map.modified_at,
            monitor_changes_pending_deployment: map.monitor_changes_pending_deployment,
            monitor_deployment: map.monitor_deployment,
            name: map.name,
            status: map.status,
            tags: map.tags,
        }
    }};
}

pub(crate) use signal_map_description_output;

/// Build a new signal map record from a `CreateSignalMap` request's values and
/// the graph discovery found for its entry point.
///
/// The signal map starts in `CREATE_IN_PROGRESS` and owes `CREATE_COMPLETE`
/// once [`TRANSIENT_STATE_SPAN`] has passed, which is what the real service
/// does: creation is asynchronous and a caller waits on `Status`.
pub fn new_signal_map(
    id: String,
    arn: String,
    name: String,
    discovery_entry_point_arn: String,
    media_resource_map: BTreeMap<String, MediaResource>,
    now: SystemTime,
) -> SignalMapRecord {
    SignalMapRecord {
        id,
        arn,
        name,
        description: None,
        discovery_entry_point_arn,
        event_bridge_rule_template_group_ids: Vec::new(),
        cloud_watch_alarm_template_group_ids: Vec::new(),
        tags: HashMap::new(),
        created_at: now,
        modified_at: now,
        media_resource_map,
        status: signal_map_status::CREATE_IN_PROGRESS.to_string(),
        discovery_due: now + TRANSIENT_STATE_SPAN,
        last_discovered_at: None,
    }
}

/// One channel as a neighbour in a discovered media workflow graph.
fn neighbor(channel: &ChannelRecord) -> MediaResourceNeighbor {
    MediaResourceNeighbor {
        arn: channel.arn.clone(),
        name: channel.name.clone(),
    }
}

/// A stored instant as the model's `__timestampIso8601` type.
fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// Whether `arn` is syntactically an AWS ARN:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`, six
/// colon-separated segments whose partition, service and resource are non-empty
/// (region and account are empty for a global service's ARNs).
pub fn is_resource_arn(arn: &str) -> bool {
    let mut segments = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The message a not-found refusal carries: the resource type, the identifier
/// the request named, then `not found` — the phrasing the real service composes
/// from the request. One helper, so no two operations word it two ways.
pub fn not_found_message(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} {identifier} not found")
}

/// One created input, as `CreateInput` stored it plus its lifecycle state.
///
/// The REQUEST shapes are what is stored (`InputSourceRequest`,
/// `InputDestinationRequest`, …), and the `Input` response shape is projected
/// out of them by [`ComAmazonawsMedialiveMedialiveState::input_description`], so
/// nothing a request supplied is dropped and no response is built from the
/// request that happens to be in flight. Members the real service computes from
/// infrastructure it owns (a push endpoint's IP, port and URL, a VPC network
/// interface, a MediaConnect router output ARN) are NOT invented here.
#[derive(Debug, Clone)]
pub struct InputRecord {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    /// The `InputType` the request named, stored under a non-reserved name.
    pub input_type: Option<String>,
    pub input_network_location: String,
    pub role_arn: Option<String>,
    /// The `InputSecurityGroups` the request attached, reported as the input's
    /// `SecurityGroups`.
    pub security_groups: Vec<String>,
    pub sources: Vec<InputSourceRequest>,
    pub destinations: Vec<InputDestinationRequest>,
    pub media_connect_flows: Vec<MediaConnectFlowRequest>,
    pub input_devices: Vec<InputDeviceSettings>,
    pub sdi_sources: Vec<String>,
    pub srt_settings: Option<SrtSettingsRequest>,
    pub multicast_settings: Option<MulticastSettingsCreateRequest>,
    pub smpte2110_receiver_group_settings: Option<Smpte2110ReceiverGroupSettings>,
    pub router_settings: Option<RouterSettings>,
    /// The private-VPC settings the request asked for. The `Input` shape has no
    /// member that reports them back, but they are what makes this a VPC input,
    /// so they are kept with the record rather than discarded.
    pub vpc: Option<InputVpcRequest>,
    pub tags: HashMap<String, String>,
    /// The input this one was created as a partner of (`CreatePartnerInput`),
    /// which is what makes the pair name each other in `InputPartnerIds`.
    pub partner_of: Option<String>,
}

impl InputRecord {
    /// How many ingest endpoints the create request named: the widest of the
    /// per-pipeline lists, since one entry in any of them is one pipeline's
    /// worth of ingest.
    fn ingest_endpoint_count(&self) -> usize {
        let multicast_sources = self
            .multicast_settings
            .as_ref()
            .and_then(|settings| settings.sources.as_ref())
            .map_or(0, Vec::len);
        let receiver_groups = self
            .smpte2110_receiver_group_settings
            .as_ref()
            .and_then(|settings| settings.smpte2110_receiver_groups.as_ref())
            .map_or(0, Vec::len);
        let srt_caller_sources = self
            .srt_settings
            .as_ref()
            .and_then(|settings| settings.srt_caller_sources.as_ref())
            .map_or(0, Vec::len);
        [
            self.sources.len(),
            self.destinations.len(),
            self.media_connect_flows.len(),
            self.input_devices.len(),
            self.sdi_sources.len(),
            multicast_sources,
            receiver_groups,
            srt_caller_sources,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }

    /// This input's `InputClass`, per the `InputClass` doc: "A standard input has
    /// two sources and a single pipeline input only has one" — so an input whose
    /// create request named exactly one ingest endpoint is `SINGLE_PIPELINE`,
    /// and any other is the two-pipeline `STANDARD` class MediaLive defaults to.
    pub fn input_class(&self) -> &'static str {
        if self.ingest_endpoint_count() == 1 {
            input_class::SINGLE_PIPELINE
        } else {
            input_class::STANDARD
        }
    }

    /// This input's `InputSourceType`: `DYNAMIC` when it is one of the types
    /// whose pull sources may carry a dynamic URL and a source URL carries the
    /// `$urlPath$` variable, otherwise `STATIC` ("By default all input sources
    /// are static", `InputSourceType`'s doc).
    pub fn input_source_type(&self) -> &'static str {
        let dynamic_capable = self
            .input_type
            .as_deref()
            .is_some_and(|input_type| DYNAMIC_CAPABLE_INPUT_TYPES.contains(&input_type));
        let dynamic_url = self.sources.iter().any(|source| {
            source
                .url
                .as_deref()
                .is_some_and(|url| url.contains(DYNAMIC_URL_VARIABLE))
        });
        if dynamic_capable && dynamic_url {
            input_source_type::DYNAMIC
        } else {
            input_source_type::STATIC
        }
    }
}

/// Build a new input record from the ids. Every modelled member starts at the
/// empty value the request then fills in.
///
/// There is no transient state to owe: the real service answers `CreateInput`
/// with an input that is already `DETACHED`, so this one is usable from the
/// instant it is stored.
pub fn new_input(id: String, arn: String) -> InputRecord {
    InputRecord {
        id,
        arn,
        name: None,
        input_type: None,
        input_network_location: DEFAULT_INPUT_NETWORK_LOCATION.to_string(),
        role_arn: None,
        security_groups: Vec::new(),
        sources: Vec::new(),
        destinations: Vec::new(),
        media_connect_flows: Vec::new(),
        input_devices: Vec::new(),
        sdi_sources: Vec::new(),
        srt_settings: None,
        multicast_settings: None,
        smpte2110_receiver_group_settings: None,
        router_settings: None,
        vpc: None,
        tags: HashMap::new(),
        partner_of: None,
    }
}

/// `InputSourceRequest` -> `InputSource`: the same three members, so a pull
/// input reports the sources it was created with.
fn described_input_source(source: &InputSourceRequest) -> InputSource {
    InputSource {
        password_param: source.password_param.clone(),
        url: source.url.clone(),
        username: source.username.clone(),
    }
}

/// `InputDestinationRequest` -> `InputDestination`.
///
/// `Ip` is the request's `StaticIpAddress` when it named one (the address an
/// ON-PREM push input takes on the customer network); `Port`, `Url` and `Vpc`
/// describe the push endpoint and the VPC network interface the real service
/// provisions, which this emulator does not, so it claims none of them.
fn described_input_destination(destination: &InputDestinationRequest) -> InputDestination {
    InputDestination {
        ip: destination.static_ip_address.clone(),
        port: None,
        url: None,
        vpc: None,
        network: destination.network.clone(),
        network_routes: destination.network_routes.as_ref().map(|routes| {
            routes
                .iter()
                .map(|route| InputDestinationRoute {
                    cidr: route.cidr.clone(),
                    gateway: route.gateway.clone(),
                })
                .collect()
        }),
    }
}

/// `SrtSettingsRequest` -> `SrtSettings`.
fn described_srt_settings(settings: &SrtSettingsRequest) -> SrtSettings {
    SrtSettings {
        srt_caller_sources: settings.srt_caller_sources.as_ref().map(|sources| {
            sources
                .iter()
                .map(|source| SrtCallerSource {
                    decryption: source.decryption.as_ref().map(|decryption| {
                        SrtCallerDecryption {
                            algorithm: decryption.algorithm.clone(),
                            passphrase_secret_arn: decryption.passphrase_secret_arn.clone(),
                        }
                    }),
                    minimum_latency: source.minimum_latency,
                    srt_listener_address: source.srt_listener_address.clone(),
                    srt_listener_port: source.srt_listener_port.clone(),
                    stream_id: source.stream_id.clone(),
                })
                .collect()
        }),
        srt_listener_settings: settings.srt_listener_settings.as_ref().map(|listener| {
            SrtListenerSettings {
                decryption: Some(SrtListenerDecryption {
                    algorithm: listener.decryption.algorithm.clone(),
                    passphrase_secret_arn: listener.decryption.passphrase_secret_arn.clone(),
                }),
                minimum_latency: Some(listener.minimum_latency),
                stream_id: listener.stream_id.clone(),
            }
        }),
    }
}

/// `MulticastSettingsCreateRequest` -> `MulticastSettings`.
fn described_multicast_settings(
    settings: &MulticastSettingsCreateRequest,
) -> MulticastSettings {
    MulticastSettings {
        sources: settings.sources.as_ref().map(|sources| {
            sources
                .iter()
                .map(|source| MulticastSource {
                    source_ip: source.source_ip.clone(),
                    url: source.url.clone(),
                })
                .collect()
        }),
    }
}

/// `RouterSettings` (request) -> `RouterInputSettings` (response).
///
/// `RouterOutputArn` names the MediaConnect router output the real service
/// associates with the input, which this emulator does not create, so it stays
/// absent rather than being invented.
fn described_router_settings(settings: &RouterSettings) -> RouterInputSettings {
    RouterInputSettings {
        destinations: settings.destinations.as_ref().map(|destinations| {
            destinations
                .iter()
                .map(|destination| RouterDestination {
                    availability_zone_name: Some(destination.availability_zone_name.clone()),
                    router_output_arn: None,
                })
                .collect()
        }),
        encryption_type: settings.encryption_type.clone(),
        secret_arn: settings.secret_arn.clone(),
    }
}

/// `VpcOutputSettings` (request) -> `VpcOutputSettingsDescription` (response).
///
/// `AvailabilityZones` and `NetworkInterfaceIds` are left absent: the real
/// service fills them from the ENIs it allocates, which this emulator does not
/// create, so reporting values for them would be fabrication.
fn described_vpc(vpc: &VpcOutputSettings) -> VpcOutputSettingsDescription {
    VpcOutputSettingsDescription {
        availability_zones: None,
        network_interface_ids: None,
        security_group_ids: vpc.security_group_ids.clone(),
        subnet_ids: Some(vpc.subnet_ids.clone()),
    }
}

fn described_anywhere(anywhere: &AnywhereSettings) -> DescribeAnywhereSettings {
    DescribeAnywhereSettings {
        channel_placement_group_id: anywhere.channel_placement_group_id.clone(),
        cluster_id: anywhere.cluster_id.clone(),
    }
}

fn described_inference(inference: &InferenceSettings) -> DescribeInferenceSettings {
    DescribeInferenceSettings {
        feed_arn: inference.feed_arn.clone(),
        audio_feed_inputs: inference.audio_feed_inputs.clone(),
        enrichment_methods: inference.enrichment_methods.clone(),
    }
}

/// `ChannelEngineVersionRequest` (just a `Version`) -> the response shape.
/// `ExpirationDate` is "The UTC time when the version expires" — an AWS-side
/// release fact, not something this emulator knows, so it stays absent.
fn described_engine_version(version: &str) -> crate::ops::types::ChannelEngineVersionResponse {
    crate::ops::types::ChannelEngineVersionResponse {
        expiration_date: None,
        version: Some(version.to_string()),
    }
}

/// Build a new channel record from a `CreateChannel` request's values.
///
/// Every member is the value the request supplied; the lifecycle members are the
/// only computed ones. The channel starts in `CREATING` and owes `IDLE`, which is
/// what the real service does.
pub fn new_channel(
    id: String,
    arn: String,
    channel_class: String,
    now: SystemTime,
) -> ChannelRecord {
    let mut channel = ChannelRecord {
        id,
        arn,
        state: String::new(),
        channel_class,
        name: None,
        cdi_input_specification: None,
        destinations: Vec::new(),
        encoder_settings: None,
        input_attachments: Vec::new(),
        input_specification: None,
        log_level: None,
        maintenance: None,
        role_arn: None,
        tags: HashMap::new(),
        channel_security_groups: Vec::new(),
        anywhere_settings: None,
        linked_channel_settings: None,
        inference_settings: None,
        vpc: None,
        channel_engine_version: None,
        transition: None,
    };
    channel.begin_transition(channel_state::CREATING, Settled::Idle, now);
    channel
}

/// Move a channel from `IDLE` into `STARTING`, owing `RUNNING`.
pub fn begin_start(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::STARTING, Settled::Running, now);
}

/// Move a running channel into `STOPPING`, owing `IDLE`.
pub fn begin_stop(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::STOPPING, Settled::Idle, now);
}

/// Move a channel into `DELETING`, owing its removal from state.
pub fn begin_delete(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::DELETING, Settled::Gone, now);
}

/// The `State` of `source`, whose attached inputs are `inputs`.
///
/// `DELETED` once `DeleteSdiSource` has deleted it — the terminal state, which a
/// deleted source keeps. Otherwise `IN_USE` when an input is attached to it and
/// `IDLE` when none is, which is the `State` member's own definition ("whether
/// the SDI source is attached to an SDI input (IN_USE) or not (IDLE)"). Derived
/// from the store on every read, never written at create time.
fn sdi_source_state_for(source: &SdiSourceRecord, inputs: &[String]) -> &'static str {
    if source.is_deleted() {
        sdi_source_state::DELETED
    } else if inputs.is_empty() {
        sdi_source_state::IDLE
    } else {
        sdi_source_state::IN_USE
    }
}

/// Build a new SdiSource record from a `CreateSdiSource` request's id and ARN.
///
/// The caller stores the request's `Name`, `Mode`, `Type` and `Tags` on the
/// record it gets back. There is no lifecycle transition to owe: a created
/// SdiSource is `IDLE` immediately, because nothing is attached to it yet.
pub fn new_sdi_source(id: String, arn: String) -> SdiSourceRecord {
    SdiSourceRecord {
        id,
        arn,
        name: None,
        mode: None,
        r#type: None,
        tags: HashMap::new(),
        deleted: false,
    }
}

/// Delete an SdiSource: it moves to the terminal `DELETED` state and stays in
/// the account, so `DescribeSdiSource` and `ListSdiSources` keep reporting it
/// there rather than losing it. Idempotent — deleting an already deleted source
/// changes nothing.
pub fn delete_sdi_source(source: &mut SdiSourceRecord) {
    source.deleted = true;
}

/// Build a new eventbridge rule template group record.
///
/// `name` is the value the request supplied; `created_at` and `modified_at` are
/// the create instant, because a group has not been modified since the call
/// that created it. The caller then stores the request's optional `Description`
/// and `Tags` on the record. There is no lifecycle transition to owe: AWS
/// returns a created group ready.
pub fn new_event_bridge_rule_template_group(
    id: String,
    arn: String,
    region: String,
    name: String,
    now: SystemTime,
) -> EventBridgeRuleTemplateGroupRecord {
    EventBridgeRuleTemplateGroupRecord {
        id,
        arn,
        region,
        name,
        description: None,
        tags: HashMap::new(),
        created_at: now,
        modified_at: now,
    }
}

/// The region this request was signed for, which is the region every ARN this
/// request mints names. Falls back to the emulator's default region when the
/// request carries no SigV4 credential scope, or an empty region in it.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region a request that names none is treated as being in (`emulator_arn`'s
/// documented default).
const DEFAULT_REGION: &str = "us-east-1";

/// The message for the one failure that is not about the request: this service
/// was mounted on an engine that holds no store for it. One constant, so the
/// service's operations cannot word it several ways.
pub const NO_STORE: &str = "medialive service state is unavailable";

/// Project a `Channel` onto one of the structurally identical per-operation
/// channel-description output shapes (`DescribeChannelOutput`,
/// `StartChannelOutput`, `StopChannelOutput`, `DeleteChannelOutput`).
///
/// One macro rather than four hand-written copies of the same 23-member move, so
/// the four operations cannot drift from each other.
macro_rules! channel_description_output {
    ($out:ident, $channel:expr) => {{
        let channel: $crate::ops::types::Channel = $channel;
        $out {
            arn: channel.arn,
            cdi_input_specification: channel.cdi_input_specification,
            channel_class: channel.channel_class,
            destinations: channel.destinations,
            egress_endpoints: channel.egress_endpoints,
            encoder_settings: channel.encoder_settings,
            id: channel.id,
            input_attachments: channel.input_attachments,
            input_specification: channel.input_specification,
            log_level: channel.log_level,
            maintenance: channel.maintenance,
            name: channel.name,
            pipeline_details: channel.pipeline_details,
            pipelines_running_count: channel.pipelines_running_count,
            role_arn: channel.role_arn,
            state: channel.state,
            tags: channel.tags,
            vpc: channel.vpc,
            anywhere_settings: channel.anywhere_settings,
            channel_engine_version: channel.channel_engine_version,
            linked_channel_settings: channel.linked_channel_settings,
            channel_security_groups: channel.channel_security_groups,
            inference_settings: channel.inference_settings,
        }
    }};
}

pub(crate) use channel_description_output;

/// Project an `Input` onto one of the structurally identical per-operation
/// input-description output shapes (`DescribeInputOutput` today;
/// `DeleteInputOutput` and the rest when their own issues land).
///
/// One macro rather than a hand-written copy of the same 22-member move per
/// operation, so those operations cannot drift from each other.
macro_rules! input_description_output {
    ($out:ident, $input:expr) => {{
        let input: $crate::ops::types::Input = $input;
        $out {
            arn: input.arn,
            attached_channels: input.attached_channels,
            destinations: input.destinations,
            id: input.id,
            input_class: input.input_class,
            input_devices: input.input_devices,
            input_partner_ids: input.input_partner_ids,
            input_source_type: input.input_source_type,
            media_connect_flows: input.media_connect_flows,
            name: input.name,
            role_arn: input.role_arn,
            security_groups: input.security_groups,
            sources: input.sources,
            state: input.state,
            tags: input.tags,
            r#type: input.r#type,
            srt_settings: input.srt_settings,
            input_network_location: input.input_network_location,
            multicast_settings: input.multicast_settings,
            smpte2110_receiver_group_settings: input.smpte2110_receiver_group_settings,
            sdi_sources: input.sdi_sources,
            router_settings: input.router_settings,
        }
    }};
}

pub(crate) use input_description_output;

/// Project a stored [`EventBridgeRuleTemplateGroupRecord`] onto one of the
/// structurally identical per-operation group output shapes
/// (`CreateEventBridgeRuleTemplateGroupOutput` and
/// `GetEventBridgeRuleTemplateGroupOutput` today;
/// `UpdateEventBridgeRuleTemplateGroupOutput` when its own issue lands).
///
/// One macro rather than a hand-written copy per operation, so the operations
/// that report a group cannot drift from each other. `Tags` is reported only
/// when the group carries tags: an empty map would claim a tag set the caller
/// never supplied.
macro_rules! event_bridge_rule_template_group_output {
    ($out:ident, $group:expr) => {
        $crate::state::template_group_output!(
            $out,
            $crate::state::EventBridgeRuleTemplateGroupRecord,
            $group
        )
    };
}

pub(crate) use event_bridge_rule_template_group_output;

/// The ONE projection of a stored template-group record onto a group output
/// shape, parameterised by the record type.
///
/// Every monitor template group the service models carries the same members, so
/// the per-resource macros above and below delegate here rather than each
/// carrying its own copy of the projection, which could then drift. `Tags` is
/// reported only when the group carries tags: an empty map would claim a tag set
/// the caller never supplied.
macro_rules! template_group_output {
    ($out:ident, $record:ty, $group:expr) => {{
        let group: &$record = $group;
        $out {
            arn: Some(group.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(group.created_at)),
            description: group.description.clone(),
            id: Some(group.id.clone()),
            modified_at: Some(::aws_smithy_types::DateTime::from(group.modified_at)),
            name: Some(group.name.clone()),
            tags: if group.tags.is_empty() {
                None
            } else {
                Some(group.tags.clone())
            },
        }
    }};
}

pub(crate) use template_group_output;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMedialiveMedialiveState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMedialiveMedialiveState>)
}

// ---------------------------------------------------------------------------
// What the cloudwatch alarm template group slice leaves in this file: the token
// key below, which both group resources share. The slice itself is the
// `alarm_template_groups` module declared at the top of this file, and its store
// is the `alarm_template_groups` member of
// `ComAmazonawsMedialiveMedialiveState` above.
// ---------------------------------------------------------------------------

/// The key a monitor template token map uses — both group resources and the
/// eventbridge rule templates inside them.
///
/// A `RequestId` is idempotent for the regional endpoint it was presented to, so
/// the region is part of the key: two regions using the same token each keep
/// their own entry instead of overwriting one another's. The region is re-checked
/// on the record the token resolves to, so the key's shape carries no invariant
/// of its own.
fn template_group_token_key(region: &str, token: &str) -> String {
    format!("{region}/{token}")
}
