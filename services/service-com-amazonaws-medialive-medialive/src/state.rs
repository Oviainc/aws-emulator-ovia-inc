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
//! A delete never drops a record: a deleted channel, input, input security group
//! or SdiSource keeps its record in the terminal `DELETED` state its own state
//! enum models, so a reader goes on reporting it there (which is what the SDK's
//! `ChannelDeleted` / `InputDeleted` waiters poll for) instead of losing it. The
//! `DELETING` step a channel's and an input's enum models is what the clock owes
//! between the call and that terminal state, and a resource in `DELETED` is no
//! longer *currently using* anything, so it leaves the derived `Inputs`,
//! `Channels` and `AttachedChannels` lists.
//!
//! A channel's schedule lives on its own [`ChannelRecord`], as the
//! [`ChannelSchedule`] of the [`schedules`] slice: the actions
//! `BatchUpdateSchedule` created and `DeleteSchedule` clears, swept of the ones
//! whose start time is over an hour past whenever [`ComAmazonawsMedialiveMedialiveState::settle`]
//! runs.
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
//!
//! Tags are NOT a store of their own: every resource record above carries its
//! own tag map, and the tag operations reach the map of the resource their
//! `ResourceArn` names through
//! [`ComAmazonawsMedialiveMedialiveState::resource_tags`] (reading),
//! [`ComAmazonawsMedialiveMedialiveState::insert_resource_tags`] and
//! [`ComAmazonawsMedialiveMedialiveState::remove_resource_tags`] (writing), so a
//! tag written by `CreateTags` is the tag that resource's own describe reports.
//! The two writers own the per-resource tag bounds ([`MAX_RESOURCE_TAGS`],
//! [`MAX_TAG_KEY_LEN`], [`MAX_TAG_VALUE_LEN`]), which is why the map itself is
//! only ever handed out mutably inside this module.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AnywhereSettings, CdiInputSpecification, Channel, ChannelSummary,
    DescribeAnywhereSettings, DescribeFollowerChannelSettings, DescribeInferenceSettings,
    DescribeLinkedChannelSettings, DescribePrimaryChannelSettings, EncoderSettings,
    InferenceSettings, Input, InputAttachment, InputDestination, InputDestinationRequest,
    InputDestinationRoute, InputDeviceSettings, InputSecurityGroup, InputSource,
    InputSourceRequest, InputSpecification, InputVpcRequest, InputWhitelistRule,
    InputWhitelistRuleCidr,
    LinkedChannelSettings, MaintenanceStatus, MediaConnectFlow, MediaConnectFlowRequest,
    MediaResource, MediaResourceNeighbor, MonitorDeployment, MulticastSettings,
    MulticastSettingsCreateRequest, MulticastSource, OutputDestination, PipelineDetail,
    RouterDestination, RouterInputSettings, RouterSettings, SdiSource, SdiSourceSummary,
    Smpte2110ReceiverGroupSettings, SignalMapSummary, SpecialRouterSettings,
    SrtCallerDecryption, SrtCallerSource, SrtListenerDecryption, SrtListenerSettings,
    SrtSettings, SrtSettingsRequest, SuccessfulMonitorDeployment,
    VpcOutputSettings, VpcOutputSettingsDescription,
};

/// The cloudwatch alarm template group slice: its record, its bounds, its
/// refusal, the store's methods and its output projection, in one module of its
/// own so the slice can be read (and merged) on its own. What stays in this file
/// is the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the token key both group
/// resources share.
pub mod alarm_template_groups;

/// The channel schedule slice: one channel's schedule, the rules
/// `BatchUpdateSchedule` validates a request against, the clock-owed cleanup of
/// actions whose start time has passed and the start-time order a read reports.
/// Its store is the `schedule` member of [`ChannelRecord`] below, so a schedule
/// goes away with the channel that owns it.
pub mod schedules;

pub use alarm_template_groups::{
    AlarmTemplateGroupRefusal, CloudWatchAlarmTemplateGroupRecord,
    MAX_ALARM_TEMPLATE_GROUPS, MAX_ALARM_TEMPLATE_GROUP_TOKENS,
};

pub use schedules::{
    parse_action_time, AppliedSchedule, ChannelSchedule, ScheduleActionRecord, ScheduleRefusal,
    MAX_SCHEDULE_ACTIONS, SCHEDULE_ACTION_RETENTION,
};

pub(crate) use alarm_template_groups::alarm_template_group_output;

/// The multiplex slice: its record, its bounds, its refusals, the store's
/// methods and its output projection, in one module of its own for the same
/// reason as the alarm template group slice above. What stays in this file is
/// the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`].
pub mod multiplexes;

pub use multiplexes::{
    multiplex_state, MultiplexRecord, MultiplexRefusal, MAX_MULTIPLEXES, MAX_MULTIPLEX_TOKENS,
    MULTIPLEX_AVAILABILITY_ZONES,
};

pub(crate) use multiplexes::multiplex_description_output;

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
    pub const RECOVERING: &str = "RECOVERING";
    pub const STOPPING: &str = "STOPPING";
    pub const DELETING: &str = "DELETING";
    pub const DELETED: &str = "DELETED";
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
/// which a delete (`DeleteInput`, `BatchDelete`) owns: it starts `DELETING` and
/// the clock settles it into the terminal `DELETED` ([`InputRecord::settle`]),
/// which is what the SDK's `InputDeleted` waiter polls `DescribeInput` for.
pub mod input_state {
    pub const DETACHED: &str = "DETACHED";
    pub const ATTACHED: &str = "ATTACHED";
    pub const DELETING: &str = "DELETING";
    pub const DELETED: &str = "DELETED";
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
/// one due: a channel's `CREATING`, `STARTING`, `STOPPING`, `RECOVERING` and
/// `DELETING`, and a signal map's `CREATE_IN_PROGRESS` discovery.
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

/// The most `InputAttachments` entries this emulator accepts on one channel.
///
/// A ceiling, not a guess at AWS's published quota: the list is caller-chosen
/// and every read that walks it (an input's `AttachedChannels`, a signal map's
/// discovery) costs work per entry, so it is bounded here for the same reason
/// [`MAX_CHANNELS`] bounds the store. `CreateChannel` refuses a longer list with
/// its own modelled `BadRequestException`, because the request is what is out of
/// bounds.
pub const MAX_INPUT_ATTACHMENTS: usize = 32;

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

/// The most tags one resource carries.
///
/// AWS's documented tag restriction for a MediaLive resource ("Tagging AWS
/// Elemental MediaLive Resources": "Maximum number of tags per resource — 50"),
/// and at the same time the ceiling that keeps `CreateTags` from growing the
/// emulator's heap without bound: the keys it writes are caller-chosen, so
/// without a cap a create loop against one resource would accumulate forever.
/// `CreateTags` models `BadRequestException`, which is the refusal once a
/// resource is full.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// The longest tag key a resource stores, from the same AWS tag restrictions
/// ("Maximum key length — 128 Unicode characters"). The model declares no
/// constraint on `Tags` (`smithy_query(query="shape", shape="Tags")`: a plain
/// `__string` to `__string` map), so the boundary validator cannot bound it and
/// this is the service-side rule.
pub const MAX_TAG_KEY_LEN: usize = 128;

/// The longest tag value a resource stores, from the same AWS tag restrictions
/// ("Maximum value length — 256 Unicode characters"), unbounded by the model for
/// the same reason as [`MAX_TAG_KEY_LEN`].
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The page size a list operation (`ListChannels`, `ListSdiSources`) uses when
/// the request names no `MaxResults`, and the ceiling it clamps a larger
/// `MaxResults` to. Finite, so one request cannot ask the service to project an
/// unbounded number of resources in one response.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// The most resource ids ONE `BatchDelete` call may name, counting its
/// `ChannelIds`, `InputIds`, `InputSecurityGroupIds` and `MultiplexIds`
/// together.
///
/// A ceiling, not a guess at AWS's published quota: the model targets those
/// members at plain `__listOf__string` with no length constraint, so nothing
/// else bounds them, and each named id costs a derivation over the stores
/// (`attached_channels`, `inputs_using_security_group`, …) and one entry in the
/// response. Capping the batch caps that product — the work done under the
/// state guard and the size of the two result lists — in one place.
/// `BatchDelete` models `BadRequestException`, which is the refusal for a batch
/// above this bound.
pub const MAX_BATCH_DELETE_IDS: usize = 100;

/// The state a channel's in-flight transition settles into once it comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// `CREATING` and `STOPPING` settle into `IDLE`.
    Idle,
    /// `STARTING` and `RECOVERING` settle into `RUNNING`.
    Running,
    /// `DELETING` settles into the terminal `DELETED`, which the channel keeps:
    /// the record stays in the account, so `DescribeChannel` goes on reporting
    /// the channel in that state instead of losing it. This is what the SDK's
    /// `ChannelDeleted` waiter polls `DescribeChannel` for.
    Deleted,
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
    /// The pipelines that are restarting right now, as
    /// [`PipelineDetail`]`$PipelineId` numbers them (0 for `PIPELINE_0`), while
    /// the channel is `RECOVERING`. Empty in every other state: a pipeline is
    /// restarting only between the `RestartChannelPipelines` call that asked for
    /// it and the transition that brings it back up, which is what
    /// [`ChannelRecord::settle`] clears.
    restarting_pipelines: Vec<i32>,
    /// This channel's schedule: the actions `BatchUpdateSchedule` has created in
    /// it and not yet deleted. It lives on the channel because that is what owns
    /// it — `DeleteChannel` takes the schedule with it, and no operation reaches
    /// a schedule without naming its channel.
    pub schedule: ChannelSchedule,
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

    /// The pipelines of this channel that are up right now, numbered as
    /// `PipelineDetail$PipelineId` numbers them.
    ///
    /// All of the class's pipelines exactly while the channel is `RUNNING`; while
    /// it is `RECOVERING`, the ones that are not restarting, because a pipeline
    /// `RestartChannelPipelines` took down is not up until the restart completes;
    /// none at all in any other state, where the channel is not serving.
    pub fn healthy_pipelines(&self) -> Vec<i32> {
        let running = self.state == channel_state::RUNNING;
        let recovering = self.state == channel_state::RECOVERING;
        if !running && !recovering {
            return Vec::new();
        }
        (0..self.pipeline_count())
            .filter(|pipeline| !self.restarting_pipelines.contains(pipeline))
            .collect()
    }

    /// Whether `pipeline` is one of the pipelines this channel's class gives it
    /// (the `ChannelClass` doc: a `STANDARD` channel has two, a
    /// `SINGLE_PIPELINE` channel one), so a request may name it.
    pub fn has_pipeline(&self, pipeline: i32) -> bool {
        (0..self.pipeline_count()).contains(&pipeline)
    }

    /// How many of this channel's pipelines are healthy — the
    /// `PipelinesRunningCount` member's own definition ("The number of currently
    /// healthy pipelines"), i.e. how many [`Self::healthy_pipelines`] names.
    pub fn pipelines_running_count(&self) -> i32 {
        i32::try_from(self.healthy_pipelines().len()).unwrap_or(i32::MAX)
    }

    /// `PipelineDetail` is documented as "Runtime details of a pipeline when a
    /// channel is running", so there is one entry per healthy pipeline and none
    /// at all when the channel is not running. Only `PipelineId` is reported: the
    /// other members describe media actually being ingested, which this emulator
    /// does not run and therefore does not claim.
    pub fn pipeline_details(&self) -> Vec<PipelineDetail> {
        self.healthy_pipelines()
            .into_iter()
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

    /// Whether this channel has stopped: it is `IDLE`, or it failed and never
    /// started, so it is ingesting through no input. The notion `DeleteChannel`
    /// and `UpdateInput`'s conflict check share, so the two cannot disagree
    /// about which states count as settled.
    pub fn has_stopped(&self) -> bool {
        self.is_idle()
            || self.state == channel_state::CREATE_FAILED
            || self.state == channel_state::UPDATE_FAILED
    }

    /// Whether this channel is in a state `DeleteChannel` accepts: a channel is
    /// deletable once it has stopped (`IDLE`) or has failed, never while it is
    /// creating, starting, running, stopping, already deleting or deleted.
    pub fn is_deletable(&self) -> bool {
        self.has_stopped()
    }

    /// Whether this channel has reached the terminal `DELETED` state, which it
    /// then keeps: the record stays in the account, so a reader goes on
    /// reporting the channel in that state instead of losing it.
    pub fn is_deleted(&self) -> bool {
        self.state == channel_state::DELETED
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

    /// Materialise the transition the clock has already made due, and answer
    /// which one it was — `None` when nothing was owed yet, so a caller can act
    /// on the step that happened on THIS call rather than re-scanning settled
    /// records.
    fn settle(&mut self, now: SystemTime) -> Option<Settled> {
        // The schedule's own clock-owed cleanup: an action whose start time is
        // more than an hour past is no longer in the schedule. Owed whatever the
        // channel's own transition is doing, so it runs first.
        self.schedule.settle(now);
        let transition = self.transition?;
        if now < transition.due {
            return None;
        }
        // A restart that is over leaves no pipeline down, and a channel that has
        // settled — idle, running or deleted — is restarting none.
        self.restarting_pipelines.clear();
        self.state = match transition.to {
            Settled::Idle => channel_state::IDLE,
            Settled::Running => channel_state::RUNNING,
            Settled::Deleted => channel_state::DELETED,
        }
        .to_string();
        self.transition = None;
        Some(transition.to)
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
    /// Whether this group has been deleted, which is what puts it in the
    /// terminal `DELETED` state. The record stays in the account, so a reader
    /// goes on reporting the group in that state instead of losing it — the
    /// same shape [`SdiSourceRecord::is_deleted`] gives a deleted SdiSource.
    pub deleted: bool,
}

impl InputSecurityGroupRecord {
    /// Whether this group has reached the terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }
}

/// Put an input security group into the terminal `DELETED` state. There is no
/// transient step: `InputSecurityGroupState` models none between `IDLE`/`IN_USE`
/// and `DELETED`, so the delete is done with the call. Deleting a group that is
/// already `DELETED` changes nothing.
pub fn delete_input_security_group(group: &mut InputSecurityGroupRecord) {
    group.deleted = true;
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

/// The `WhitelistRules` a request asks for, validated into the CIDR strings the
/// store keeps, or the message naming the first entry that is not an IPv4 CIDR.
///
/// ONE copy for every operation that writes a group's whitelist
/// (`CreateInputSecurityGroup` and `UpdateInputSecurityGroup`), which document
/// the member identically ("List of IPv4 CIDR addresses to whitelist"), so the
/// rule the two enforce cannot drift apart. The whole list is checked before the
/// caller writes anything, so a refused request leaves the store untouched
/// rather than applying a prefix of its rules.
pub fn whitelist_rules_from_request(
    requested: Vec<InputWhitelistRuleCidr>,
) -> Result<Vec<String>, String> {
    let mut cidrs: Vec<String> = Vec::with_capacity(requested.len());
    for (index, rule) in requested.into_iter().enumerate() {
        let Some(cidr) = rule.cidr else {
            return Err(format!(
                "WhitelistRules[{index}].Cidr is required: a whitelist rule must name an IPv4 CIDR to whitelist"
            ));
        };
        if !is_ipv4_cidr(&cidr) {
            return Err(format!(
                "WhitelistRules[{index}].Cidr must be an IPv4 CIDR with a prefix length of 0 to 32 (for example 10.0.0.0/16); got {cidr:?}"
            ));
        }
        cidrs.push(cidr);
    }
    Ok(cidrs)
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
    /// (`smithy_query(query="shape", shape="SdiSource")`), so the tag
    /// operations are what report them: `ListTagsForResource` reads this map
    /// through [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
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

/// Why a monitor template GROUP's delete removed nothing. Each variant is one
/// modelled refusal, and both of them leave the store exactly as it was.
///
/// ONE enum for the two group resources
/// ([`ComAmazonawsMedialiveMedialiveState::delete_alarm_template_group`] and
/// [`ComAmazonawsMedialiveMedialiveState::delete_event_bridge_rule_template_group`]),
/// whose deletes document the SAME precondition sentence, so the two cannot
/// disagree about which refusal an unmet precondition is. A delete that documents
/// no precondition (the eventbridge rule TEMPLATE's) does not use this type: it
/// answers the record it removed or nothing at all.
#[derive(Debug)]
pub enum TemplateGroupDeleteRefusal {
    /// No group of that type in the request's region answers to the identifier,
    /// by either form `Identifier` documents (its id or its current name).
    NotFound,
    /// Something still holds the group, and its delete documents that the caller
    /// "must detach this group from all signal maps and ensure its existing
    /// templates are moved to another group or deleted" first. Each list carries
    /// the ids that still hold it, and is empty when that half of the
    /// precondition is met (or, for a group whose templates have no store yet,
    /// when there is nothing of that kind to check).
    StillHeld {
        signal_map_ids: Vec<String>,
        template_ids: Vec<String>,
    },
}

/// The message a group delete's conflict carries: the resource type, the
/// identifier the request named, what still holds it and the precondition both
/// group deletes document ("You must detach this group from all signal maps and
/// ensure its existing templates are moved to another group or deleted").
///
/// ONE builder, so the two group deletes cannot word the same refusal two ways,
/// and each clause appears only when that list is non-empty.
pub fn still_held_message(
    resource_type: &str,
    identifier: &str,
    signal_map_ids: &[String],
    template_ids: &[String],
) -> String {
    let mut held: Vec<String> = Vec::new();
    if !signal_map_ids.is_empty() {
        held.push(format!(
            "is still attached to the signal maps {}",
            signal_map_ids.join(", ")
        ));
    }
    if !template_ids.is_empty() {
        held.push(format!(
            "still holds the templates {}",
            template_ids.join(", ")
        ));
    }
    format!(
        "{resource_type} {identifier} {}; detach the group from all signal maps and move its existing templates to another group or delete them before deleting it",
        held.join(" and ")
    )
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
    /// Every multiplex that exists, keyed by its minted `Id`, in id order for
    /// the same reason as [`Self::channels`].
    pub multiplexes: BTreeMap<String, MultiplexRecord>,
    /// Mint counter behind the multiplex slice's `mint_multiplex_id`.
    pub next_multiplex_id: u64,
    /// `CreateMultiplex`'s `RequestId` (`@idempotencyToken`) -> the multiplex it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    multiplex_tokens: IdempotencyTokens,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every transition the clock had already made due as of `now`,
    /// then answer from the result.
    ///
    /// Call this at the top of every operation, reporting ones included: it only
    /// applies transitions that were owed at that instant whether or not anybody
    /// called, so it never changes what a later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        // Only a record that settles into `DELETED` on THIS call is collected,
        // so the work here does not grow with the number of records that have
        // already reached that terminal state.
        let mut just_deleted: Vec<String> = Vec::new();
        for (id, channel) in &mut self.channels {
            if channel.settle(now) == Some(Settled::Deleted) {
                just_deleted.push(id.clone());
            }
        }
        // A token never outlives the channel it minted: once that channel has
        // reached the terminal `DELETED` state its idempotency window is over, so
        // a retried create mints a new channel rather than answering with a
        // deleted one.
        for id in just_deleted {
            self.channel_tokens.forget(&id);
        }
        for input in self.inputs.values_mut() {
            input.settle(now);
        }
        for signal_map in self.signal_maps.values_mut() {
            signal_map.settle(now);
        }
        self.settle_multiplexes(now);
    }

    /// Whether the store is full, so `CreateChannel` must refuse rather than mint
    /// another channel.
    ///
    /// A `DELETED` channel still counts: its record stays in the account, so
    /// counting it is what keeps the store — and the emulator's heap — bounded
    /// by [`MAX_CHANNELS`] however many creates and deletes a caller runs.
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
    ///
    /// A channel that has reached the terminal `DELETED` state is no longer
    /// using anything, so it is not reported — the member says *currently*.
    pub fn channels_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
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
    /// for the same reason as [`Self::channels_using_security_group`], and it
    /// drops a `DELETED` input for the same reason too.
    pub fn inputs_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.inputs
            .values()
            // A deleted input is not using anything any more, which is the same
            // reading of "currently using" that `inputs_using_sdi_source`
            // applies to a deleted source.
            .filter(|input| !input.is_deleted())
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
    /// `State` is derived, not stored: a deleted group is in the terminal
    /// `DELETED` state it keeps, a group some resource in state references is
    /// `IN_USE`, and one nothing references is `IDLE` ("The current state of the
    /// Input Security Group").
    pub fn input_security_group_description(&self, id: &str) -> Option<InputSecurityGroup> {
        let group = self.input_security_groups.get(id)?;
        let inputs = self.inputs_using_security_group(id);
        let channels = self.channels_using_security_group(id);
        let state = if group.is_deleted() {
            input_security_group_state::DELETED
        } else if inputs.is_empty() && channels.is_empty() {
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
            // A deleted input has stopped using the source, exactly as a deleted
            // source has nothing using it.
            .filter(|input| !input.is_deleted())
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
    ///
    /// The ONE projection every operation that reports an SdiSource
    /// (`CreateSdiSource`, `DescribeSdiSource`, `UpdateSdiSource`,
    /// `DeleteSdiSource`) answers from, so none of them can drift from the
    /// others. `Tags` is always a map — the source's stored tag set, empty when
    /// it carries none — exactly as every other taggable MediaLive resource
    /// projection reports it ([`Self::input_description`],
    /// [`Self::channel_description`]): an absent member would make a caller that
    /// iterates the returned source's tags dereference null instead of reading
    /// an empty map.
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
            tags: Some(source.tags.clone()),
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

    /// Remove the eventbridge rule template group `identifier` names in
    /// `region`, answering the record that was removed — or refuse.
    ///
    /// `DeleteEventBridgeRuleTemplateGroup` is documented "Deletes an eventbridge
    /// rule template group. You must detach this group from all signal maps and
    /// ensure its existing templates are moved to another group or deleted", so a
    /// group a signal map still names, or one that still holds templates, is
    /// refused and nothing is removed — reporting BOTH halves at once, so a
    /// caller learns everything it must undo from one refusal. Once removed the
    /// group is GONE from the store, so every later read of either of its
    /// identifiers misses and its name is free again.
    ///
    /// The resolution, the preconditions and the removal live together here, so a
    /// handler cannot check one of them and write past another; the handler's job
    /// is only to turn a [`TemplateGroupDeleteRefusal`] into the operation's own
    /// modelled error.
    pub fn delete_event_bridge_rule_template_group(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Result<EventBridgeRuleTemplateGroupRecord, TemplateGroupDeleteRefusal> {
        let id = self
            .event_bridge_rule_template_group(region, identifier)
            .ok_or(TemplateGroupDeleteRefusal::NotFound)?
            .id
            .clone();

        let signal_map_ids = self.signal_maps_using_event_bridge_rule_template_group(&id);
        let template_ids = self.event_bridge_rule_template_ids_in_group(&id);
        if !signal_map_ids.is_empty() || !template_ids.is_empty() {
            return Err(TemplateGroupDeleteRefusal::StillHeld {
                signal_map_ids,
                template_ids,
            });
        }

        let removed = self
            .event_bridge_rule_template_groups
            .remove(&id)
            .ok_or(TemplateGroupDeleteRefusal::NotFound)?;
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted group.
        self.event_bridge_rule_template_group_tokens.forget(&id);
        Ok(removed)
    }

    /// The ids of the signal maps that currently name the monitor template group
    /// `group_id`, through whichever of a signal map's two group lists `groups`
    /// reads.
    ///
    /// ONE walk for both group resources, so the attachment both deletes refuse
    /// on is read the same way.
    fn signal_maps_naming_group(
        &self,
        group_id: &str,
        groups: fn(&SignalMapRecord) -> &[String],
    ) -> Vec<String> {
        self.signal_maps
            .values()
            .filter(|map| groups(map).iter().any(|named| named == group_id))
            .map(|map| map.id.clone())
            .collect()
    }

    /// The ids of the signal maps that name the cloudwatch alarm template group
    /// `group_id` — the attachment `DeleteCloudWatchAlarmTemplateGroup` documents
    /// the caller must undo first ("You must detach this group from all signal
    /// maps").
    pub fn signal_maps_using_alarm_template_group(&self, group_id: &str) -> Vec<String> {
        self.signal_maps_naming_group(group_id, |map| {
            map.cloud_watch_alarm_template_group_ids.as_slice()
        })
    }

    /// The ids of the signal maps that name the eventbridge rule template group
    /// `group_id`, on the same terms as the cloudwatch alarm ones beside them.
    pub fn signal_maps_using_event_bridge_rule_template_group(
        &self,
        group_id: &str,
    ) -> Vec<String> {
        self.signal_maps_naming_group(group_id, |map| {
            map.event_bridge_rule_template_group_ids.as_slice()
        })
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

    /// The channels whose `InputAttachments` name this input. One predicate, so
    /// what counts as attached cannot differ between the input's reported
    /// `AttachedChannels` and the conflict its edit answers.
    ///
    /// A channel that has reached the terminal `DELETED` state attaches nothing
    /// any more — its record stays in the account, but it is no longer one of
    /// the channels the input is *currently* attached to.
    fn channels_attaching<'a>(
        &'a self,
        input_id: &'a str,
    ) -> impl Iterator<Item = &'a ChannelRecord> {
        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
            .filter(move |channel| {
                channel
                    .input_attachments
                    .iter()
                    .any(|attachment| attachment.input_id.as_deref() == Some(input_id))
            })
    }

    /// An input's `AttachedChannels`: "A list of channel IDs that that input is
    /// attached to". Derived from the channels' own `InputAttachments`, so it
    /// tracks every channel created or deleted after this input: a channel that
    /// has reached the terminal `DELETED` state attaches nothing any more.
    pub fn attached_channels(&self, input_id: &str) -> Vec<String> {
        self.channels_attaching(input_id)
            .map(|channel| channel.id.clone())
            .collect()
    }

    /// The channel that stands in the way of editing this input: one whose
    /// `InputAttachments` names it and which has not stopped, reported as its id
    /// and its state.
    ///
    /// An attached channel that is still running ingests through exactly the
    /// settings an edit would replace, so the settings can only be changed once
    /// that channel has stopped ([`ChannelRecord::has_stopped`], the same notion
    /// `DeleteChannel` accepts) — the "in use" refusal `DeleteSdiSource` also
    /// makes for a source an input is using, and the condition `UpdateInput`'s
    /// modelled `ConflictException` answers.
    pub fn channel_blocking_input_edit(&self, input_id: &str) -> Option<(String, String)> {
        self.channels_attaching(input_id)
            .find(|channel| !channel.has_stopped())
            .map(|channel| (channel.id.clone(), channel.state.clone()))
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

    /// The `InputState` of one stored input: the deletion a delete started
    /// first — `DELETING` until the clock makes it due and the terminal
    /// `DELETED` afterwards — and otherwise `ATTACHED` exactly while some
    /// channel attaches it and `DETACHED` while none does.
    pub fn input_state(&self, input: &InputRecord) -> &'static str {
        if input.is_deleted() {
            input_state::DELETED
        } else if input.is_deleting() {
            input_state::DELETING
        } else if self.attached_channels(&input.id).is_empty() {
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
    /// from state, so it tracks every later create and delete: a channel that
    /// has reached the terminal `DELETED` state follows nothing any more.
    fn following_channel_arns(&self, primary_arn: &str) -> Vec<String> {        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
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

    /// The input whose ARN is exactly `arn`, if this store holds one — the same
    /// global-identity reading of an ARN as [`Self::channel_by_arn`].
    fn input_by_arn(&self, arn: &str) -> Option<&InputRecord> {
        self.inputs.values().find(|input| input.arn == arn)
    }

    /// Both directions of every channel-to-input attachment this store holds.
    ///
    /// Built once per discovery and then read by every node the walk visits, so
    /// a discovery costs one pass over the store's attachments rather than a
    /// re-derivation per node; the de-duplication of a channel's
    /// `InputAttachments` goes through a set, so one channel's pass is linear in
    /// the attachments it carries ([`MAX_INPUT_ATTACHMENTS`] of them at most).
    fn attachment_edges(&self) -> AttachmentEdges<'_> {
        let mut edges = AttachmentEdges::default();
        for channel in self.channels.values() {
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for id in channel
                .input_attachments
                .iter()
                .filter_map(|attachment| attachment.input_id.as_deref())
            {
                if !seen.insert(id) {
                    continue;
                }
                if let Some(input) = self.inputs.get(id).filter(|input| !input.is_deleted()) {
                    edges
                        .inputs_of
                        .entry(channel.arn.as_str())
                        .or_default()
                        .push(input);
                    edges
                        .channels_of
                        .entry(input.arn.as_str())
                        .or_default()
                        .push(channel);
                }
            }
        }
        edges
    }

    /// One discovered channel as a graph node: upstream (`Sources`) the channel
    /// it follows and the inputs attached to it, downstream (`Destinations`) the
    /// channels that follow it.
    fn channel_media_resource(
        &self,
        channel: &ChannelRecord,
        edges: &AttachmentEdges<'_>,
    ) -> MediaResource {
        let mut sources: Vec<MediaResourceNeighbor> = channel
            .linked_channel_settings
            .as_ref()
            .and_then(|linked| linked.follower_channel_settings.as_ref())
            .and_then(|follower| follower.primary_channel_arn.as_deref())
            .and_then(|primary| self.channel_by_arn(primary))
            .map(|primary| vec![neighbor(&primary.arn, primary.name.as_deref())])
            .unwrap_or_default();
        // An attached input FEEDS the channel, so it is one of the channel's
        // sources, read out of the channel's stored `InputAttachments`.
        sources.extend(
            edges
                .inputs_of
                .get(channel.arn.as_str())
                .into_iter()
                .flatten()
                .map(|input| neighbor(&input.arn, input.name.as_deref())),
        );
        MediaResource {
            name: channel.name.clone(),
            sources: Some(sources),
            destinations: Some(
                self.following_channel_arns(&channel.arn)
                    .iter()
                    .filter_map(|follower| self.channel_by_arn(follower))
                    .map(|follower| neighbor(&follower.arn, follower.name.as_deref()))
                    .collect(),
            ),
        }
    }

    /// One discovered input as a graph node: downstream (`Destinations`) the
    /// channels whose `InputAttachments` name it, which is the same edge the
    /// channel reports as a source, read from the other end.
    ///
    /// `Sources` is empty: what feeds an input is a pull URL, a push endpoint,
    /// a MediaConnect flow or a device — none of them a resource this store
    /// holds, so none of them is claimed as a neighbour.
    fn input_media_resource(
        &self,
        input: &InputRecord,
        edges: &AttachmentEdges<'_>,
    ) -> MediaResource {
        MediaResource {
            name: input.name.clone(),
            sources: Some(Vec::new()),
            destinations: Some(
                edges
                    .channels_of
                    .get(input.arn.as_str())
                    .into_iter()
                    .flatten()
                    .map(|channel| neighbor(&channel.arn, channel.name.as_deref()))
                    .collect(),
            ),
        }
    }

    /// The graph node the resource `arn` names, if this store holds one: a
    /// channel, or an input that has not been deleted.
    fn media_resource_at(&self, arn: &str, edges: &AttachmentEdges<'_>) -> Option<MediaResource> {
        if let Some(channel) = self.channel_by_arn(arn) {
            return Some(self.channel_media_resource(channel, edges));
        }
        self.input_by_arn(arn)
            .filter(|input| !input.is_deleted())
            .map(|input| self.input_media_resource(input, edges))
    }

    /// Discover the media workflow graph reachable from the resource
    /// `entry_point` names, keyed by each discovered resource's ARN.
    ///
    /// The ENTRY POINT is a channel: `DiscoveryEntryPointArn` is "A top-level
    /// supported AWS resource ARN to discovery a signal map from", and a channel
    /// is the top-level resource this store holds; an ARN naming anything else
    /// (another resource type of this service, another service, another account
    /// or region) resolves to no entry point and answers `None` — which the
    /// caller renders as the operation's modelled `NotFoundException`.
    ///
    /// The NODES are every media resource the walk reaches from there: channels
    /// and the inputs those channels attach, each keyed by its own ARN. The walk
    /// is breadth-first over the links the discovered resources themselves
    /// declare (a follower channel's primary, the followers that name it, and
    /// the inputs a channel's `InputAttachments` name), so the graph is read out
    /// of persisted state. It is the SNAPSHOT of that state at the instant
    /// discovery runs: the real service rediscovers a signal map's graph only
    /// when a discovery is asked for (`StartUpdateSignalMap`, still a scaffold
    /// here), so a resource created or deleted afterwards does not change a
    /// stored signal map's `MediaResourceMap`.
    pub fn discover_media_resource_map(
        &self,
        entry_point: &str,
    ) -> Option<BTreeMap<String, MediaResource>> {
        self.channel_by_arn(entry_point)?;

        // One pass over the store's attachments, read by every node below.
        let edges = self.attachment_edges();
        let mut discovered: BTreeMap<String, MediaResource> = BTreeMap::new();
        let mut pending: VecDeque<String> = VecDeque::new();
        pending.push_back(entry_point.to_string());

        while let Some(arn) = pending.pop_front() {
            if discovered.contains_key(&arn) {
                continue;
            }
            let Some(resource) = self.media_resource_at(&arn, &edges) else {
                continue;
            };
            for next in resource
                .sources
                .iter()
                .chain(resource.destinations.iter())
                .flatten()
            {
                pending.push_back(next.arn.clone());
            }
            discovered.insert(arn, resource);
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

    /// Which of this store's taggable resources `arn` names, if it holds one.
    ///
    /// The tag operations address a resource ONLY by its ARN, and every resource
    /// this store holds carries its own tag map, so the ARN is resolved once
    /// here and both the read ([`Self::resource_tags`]) and the write
    /// ([`Self::resource_tags_mut`]) go through the same resolution: a tag
    /// written through one of them is the tag the resource's own describe
    /// reports, because there is no second copy to write.
    fn tagged_resource(&self, arn: &str) -> Option<TaggedResource> {
        if let Some(record) = self.channel_by_arn(arn) {
            return Some(TaggedResource::Channel(record.id.clone()));
        }
        if let Some(record) = self.input_by_arn(arn) {
            return Some(TaggedResource::Input(record.id.clone()));
        }
        if let Some(record) = self
            .input_security_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::InputSecurityGroup(record.id.clone()));
        }
        if let Some(record) = self.sdi_sources.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::SdiSource(record.id.clone()));
        }
        if let Some(record) = self.signal_maps.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::SignalMap(record.id.clone()));
        }
        if let Some(record) = self.multiplexes.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Multiplex(record.id.clone()));
        }
        if let Some(record) = self
            .alarm_template_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::AlarmTemplateGroup(record.id.clone()));
        }
        if let Some(record) = self
            .event_bridge_rule_template_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::EventBridgeRuleTemplateGroup(
                record.id.clone(),
            ));
        }
        None
    }

    /// The tags stored on the resource `arn` names, or `None` when this store
    /// holds no resource with that ARN.
    pub fn resource_tags(&self, arn: &str) -> Option<&HashMap<String, String>> {
        match self.tagged_resource(arn)? {
            TaggedResource::Channel(id) => self.channels.get(&id).map(|record| &record.tags),
            TaggedResource::Input(id) => self.inputs.get(&id).map(|record| &record.tags),
            TaggedResource::InputSecurityGroup(id) => self
                .input_security_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::SdiSource(id) => self.sdi_sources.get(&id).map(|record| &record.tags),
            TaggedResource::SignalMap(id) => self.signal_maps.get(&id).map(|record| &record.tags),
            TaggedResource::Multiplex(id) => self.multiplexes.get(&id).map(|record| &record.tags),
            TaggedResource::AlarmTemplateGroup(id) => self
                .alarm_template_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::EventBridgeRuleTemplateGroup(id) => self
                .event_bridge_rule_template_groups
                .get(&id)
                .map(|record| &record.tags),
        }
    }

    /// The tag map of the resource `arn` names, to write through, or `None`
    /// when this store holds no resource with that ARN.
    ///
    /// The map handed back is the resource's OWN, so a tag written here is on
    /// the resource itself: the resource's describe and
    /// [`Self::resource_tags`] read the one map rather than two copies that
    /// could drift.
    ///
    /// PRIVATE: the per-resource tag bounds are invariants of
    /// [`Self::insert_resource_tags`], which is the only way a tag is written,
    /// so no handler can grow a resource's tag map past them.
    fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        match self.tagged_resource(arn)? {
            TaggedResource::Channel(id) => {
                self.channels.get_mut(&id).map(|record| &mut record.tags)
            }
            TaggedResource::Input(id) => self.inputs.get_mut(&id).map(|record| &mut record.tags),
            TaggedResource::InputSecurityGroup(id) => self
                .input_security_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::SdiSource(id) => self
                .sdi_sources
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::SignalMap(id) => self
                .signal_maps
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Multiplex(id) => self
                .multiplexes
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::AlarmTemplateGroup(id) => self
                .alarm_template_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::EventBridgeRuleTemplateGroup(id) => self
                .event_bridge_rule_template_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
        }
    }

    /// Add `tags` to the resource `arn` names, bounded.
    ///
    /// The bounds live here rather than in the handler, so no caller-driven
    /// write can grow a resource's tag map past them: every refusal is checked
    /// BEFORE the first insert, and a refused request leaves the stored tags
    /// exactly as they were.
    pub fn insert_resource_tags(
        &mut self,
        arn: &str,
        tags: HashMap<String, String>,
    ) -> Result<(), TagRefusal> {
        // Request shape first — a key or value past its length is refused
        // whatever resource it was aimed at.
        for (key, value) in &tags {
            if key.chars().count() > MAX_TAG_KEY_LEN {
                return Err(TagRefusal::KeyTooLong {
                    length: key.chars().count(),
                });
            }
            if value.chars().count() > MAX_TAG_VALUE_LEN {
                return Err(TagRefusal::ValueTooLong {
                    key: key.clone(),
                    length: value.chars().count(),
                });
            }
        }

        let stored = self
            .resource_tags_mut(arn)
            .ok_or(TagRefusal::NoSuchResource)?;

        // What the resource would carry afterwards: the keys it already has,
        // plus the ones this request introduces. A key being overwritten adds
        // nothing, so re-tagging a full resource is not refused.
        let added = tags
            .keys()
            .filter(|key| !stored.contains_key(*key))
            .count();
        let merged = stored.len() + added;
        if merged > MAX_RESOURCE_TAGS {
            return Err(TagRefusal::TooManyTags { merged });
        }

        for (key, value) in tags {
            stored.insert(key, value);
        }
        Ok(())
    }

    /// Remove the tags `keys` names from the resource `arn` names.
    ///
    /// Only ever shrinks a tag map, so it needs no bound of its own; a key the
    /// resource does not carry leaves it in the state the caller asked for, so
    /// naming one is not a refusal. The only refusal is an ARN this store holds
    /// no resource for.
    pub fn remove_resource_tags(
        &mut self,
        arn: &str,
        keys: &[String],
    ) -> Result<(), TagRefusal> {
        let stored = self
            .resource_tags_mut(arn)
            .ok_or(TagRefusal::NoSuchResource)?;
        for key in keys {
            stored.remove(key);
        }
        Ok(())
    }
}

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_resource_tags`] stored
/// nothing. Each variant is one refusal a tag write can answer with.
#[derive(Debug)]
pub enum TagRefusal {
    /// No resource in this store carries that ARN.
    NoSuchResource,
    /// The merged tag set would push the resource past
    /// [`MAX_RESOURCE_TAGS`]. Carries the count it would have reached.
    TooManyTags { merged: usize },
    /// A tag key is longer than [`MAX_TAG_KEY_LEN`].
    KeyTooLong { length: usize },
    /// A tag value is longer than [`MAX_TAG_VALUE_LEN`]. Carries the key it
    /// belongs to, so the refusal names the offending member.
    ValueTooLong { key: String, length: usize },
}

/// One of this store's taggable resources: which store holds it and the key it
/// is stored under.
///
/// Returned by [`ComAmazonawsMedialiveMedialiveState::tagged_resource`] as an
/// OWNED key rather than a borrow of the record, so the same resolution serves
/// the shared and the mutable accessor.
#[derive(Debug, Clone)]
enum TaggedResource {
    Channel(String),
    Input(String),
    InputSecurityGroup(String),
    SdiSource(String),
    SignalMap(String),
    Multiplex(String),
    AlarmTemplateGroup(String),
    EventBridgeRuleTemplateGroup(String),
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

/// The channel-to-input attachment edges of a whole store, in both directions:
/// each channel's attached inputs and, as the inverse, each input's attaching
/// channels.
///
/// Built by [`ComAmazonawsMedialiveMedialiveState::attachment_edges`] once per
/// discovery, so the walk reads the edges instead of re-deriving them per node.
#[derive(Debug, Default)]
struct AttachmentEdges<'state> {
    /// A channel's ARN -> the inputs it attaches, in attachment order, once
    /// each, restricted to the ones the store still holds and has not deleted —
    /// discovery never claims a resource the store does not hold.
    inputs_of: BTreeMap<&'state str, Vec<&'state InputRecord>>,
    /// An input's ARN -> the channels that attach it, in channel-id order.
    channels_of: BTreeMap<&'state str, Vec<&'state ChannelRecord>>,
}

/// One media resource as a neighbour in a discovered media workflow graph: the
/// `Arn` the model requires of a `MediaResourceNeighbor`, plus the resource's
/// own name when it has one. One helper for every resource kind the graph holds,
/// so a channel neighbour and an input neighbour cannot be built two ways.
fn neighbor(arn: &str, name: Option<&str>) -> MediaResourceNeighbor {
    MediaResourceNeighbor {
        arn: arn.to_string(),
        name: name.map(ToString::to_string),
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

/// The message a delete refused on an input a channel still attaches carries:
/// the input, the channels that attach it, and that it cannot be deleted. One
/// helper for the same reason as [`not_found_message`] — `DeleteInput` and
/// `BatchDelete` refuse the same input, so they cannot word it two ways.
pub fn attached_input_message(input_id: &str, channels: &[String]) -> String {
    format!(
        "Input {input_id} is attached to channel(s) {} and cannot be deleted",
        channels.join(", ")
    )
}

/// The message a delete refused on the resource's state carries: the resource,
/// the state it is in, and the state a delete needs. One helper for the same
/// reason as [`not_found_message`] — `DeleteChannel` and `BatchDelete` refuse
/// the same channel, so they cannot word it two ways.
pub fn not_idle_message(resource_type: &str, identifier: &str, state: &str) -> String {
    format!(
        "{resource_type} {identifier} is in state {state} and must be \
         {} before it can be deleted",
        channel_state::IDLE
    )
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
    /// The MediaConnect Router handoff settings `UpdateInput` accepts
    /// (`SpecialRouterSettings`, whose only member is the router's ARN). Kept
    /// for the same reason as `vpc`: the `Input` response shape has no member
    /// that reports a router ARN, so storing it is how the request is not
    /// silently dropped.
    pub special_router_settings: Option<SpecialRouterSettings>,
    pub tags: HashMap<String, String>,
    /// The input this one was created as a partner of (`CreatePartnerInput`),
    /// which is what makes the pair name each other in `InputPartnerIds`.
    pub partner_of: Option<String>,
    /// The instant the deletion a delete started becomes due, so `DELETING`
    /// settles into the terminal `DELETED`. `None` while no delete has been
    /// accepted for this input. Stored, never slept on.
    ///
    /// PRIVATE, so the only way in is [`begin_delete_input`] and
    /// [`InputRecord::settle`], and the only way to read it is
    /// [`InputRecord::is_deleting`].
    deletion_due: Option<SystemTime>,
    /// Whether the deletion has come due, which puts the input in the terminal
    /// `DELETED` state.
    ///
    /// A deleted input is NOT dropped from the account: `DELETED` is one of the
    /// states the model gives it (`smithy_query(query="shape",
    /// shape="InputState")`) and it is terminal, so the record stays readable in
    /// that state rather than vanishing, exactly as a deleted SdiSource does.
    deleted: bool,
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

    /// Whether `DeleteInput` has started deleting this input and the clock has
    /// not made that deletion due yet.
    pub fn is_deleting(&self) -> bool {
        self.deletion_due.is_some()
    }

    /// Whether this input's deletion has come due, which is what puts it in the
    /// terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Whether a delete has been accepted for this input, in either half of the
    /// deletion lifecycle — so a repeated `DeleteInput` is idempotent rather
    /// than restarting the clock. PRIVATE: only [`begin_delete_input`] needs it,
    /// which keeps the deletion flags' "only way in" invariant tight.
    fn is_delete_accepted(&self) -> bool {
        self.is_deleting() || self.is_deleted()
    }

    /// Materialise the deletion the clock has already made due. Owed at the
    /// stored instant whether or not anybody called, so applying it in a
    /// reporting operation changes nothing a later caller would have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.deletion_due.is_some_and(|due| now >= due) {
            self.deletion_due = None;
            self.deleted = true;
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
        special_router_settings: None,
        tags: HashMap::new(),
        partner_of: None,
        deletion_due: None,
        deleted: false,
    }
}

/// Delete an input: it moves into `DELETING` and owes the terminal `DELETED`
/// once [`TRANSIENT_STATE_SPAN`] has passed, which is the lifecycle the real
/// service runs (a caller waits on `State`, which is what the `input_deleted`
/// waiter polls for). The record then stays in the account in that terminal
/// state, so `DescribeInput` keeps reporting it rather than losing it.
///
/// Idempotent: a delete already accepted keeps the instant it was accepted at,
/// so a repeated call neither restarts the clock nor revives a deleted input.
pub fn begin_delete_input(input: &mut InputRecord, now: SystemTime) {
    if input.is_delete_accepted() {
        return;
    }
    input.deletion_due = Some(now + TRANSIENT_STATE_SPAN);
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
        restarting_pipelines: Vec::new(),
        schedule: ChannelSchedule::default(),
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

/// Move a channel into `DELETING`, owing the terminal `DELETED` state it then
/// keeps.
pub fn begin_delete(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::DELETING, Settled::Deleted, now);
}

/// Take `pipelines` down for a restart on a running channel: it moves into
/// `RECOVERING` — the `ChannelState` value for a channel whose pipelines are
/// coming back up — and owes `RUNNING` with every pipeline healthy again once
/// the restart completes.
///
/// `pipelines` are `PipelineDetail$PipelineId` numbers, each one the caller's
/// `RestartChannelPipelines` request asked for; the channel reports them as not
/// healthy for as long as it is `RECOVERING`.
pub fn begin_pipeline_restart(
    channel: &mut ChannelRecord,
    pipelines: Vec<i32>,
    now: SystemTime,
) {
    channel.restarting_pipelines = pipelines;
    channel.begin_transition(channel_state::RECOVERING, Settled::Running, now);
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
/// `StartChannelOutput`, `StopChannelOutput`, `DeleteChannelOutput`,
/// `RestartChannelPipelinesOutput`).
///
/// One macro rather than five hand-written copies of the same 23-member move, so
/// the operations cannot drift from each other. A shape that carries a member
/// beyond the 23 (`RestartChannelPipelinesOutput$MaintenanceStatus`) passes it as
/// a trailing `member: value` pair, so every member of every one of these shapes
/// is still named exactly once and adding one to the model cannot be defaulted in
/// silently.
macro_rules! channel_description_output {
    ($out:ident, $channel:expr $(, $extra:ident : $value:expr)* $(,)?) => {{
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
            $($extra: $value,)*
        }
    }};
}

pub(crate) use channel_description_output;

/// Project an `Input` onto one of the structurally identical per-operation
/// input-description output shapes (`DescribeInputOutput` today; the others when
/// their own issues land — `DeleteInputResponse` is not one of them, since the
/// model gives `DeleteInput` an output with no members at all).
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
