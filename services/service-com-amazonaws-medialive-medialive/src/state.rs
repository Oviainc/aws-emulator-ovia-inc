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
//! channels this store already holds, and the latest monitor deployment
//! `StartMonitorDeployment` started for it ([`MonitorDeploymentRecord`]) beside
//! the latest one that succeeded, which is how both the signal map's
//! `MonitorDeployment` and its summary's `MonitorDeploymentStatus` are reported
//! out of state.
//!
//! The MediaLive Anywhere network lives here too: one [`NetworkRecord`] per
//! created network, its own id counter and its own token map. A network's
//! `State` is the one lifecycle member it owes the clock (`CREATING` until the
//! creation comes due, then `ACTIVE`) and its `AssociatedClusterIds` are derived
//! from the rest of the state on every read, never recorded at create time.
//!
//! The INPUT DEVICES this account has claimed live here as well: one
//! [`InputDeviceRecord`] per claimed device, keyed by the device id the claim
//! named rather than by an id this service mints, because a Link unit's id is
//! its own. A device's `DeviceUpdateStatus` comes out of the maintenance window
//! `StartInputDeviceMaintenanceWindow` opens on it, and its `OutputType` and
//! `MedialiveInputArns` out of the inputs this store holds — see
//! [`input_devices`].
//!
//! The input security group lifecycle lives here too: one
//! [`InputSecurityGroupRecord`] per created group and the counter that mints its
//! `Id`. A group's `Inputs` and `Channels` are derived from the rest of the
//! state on every read, never recorded at create time, and so is its `State` —
//! out of what uses the group and out of whether `DeleteInputSecurityGroup` has
//! deleted it.
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
//! A kept record is READABLE, not writable: once a delete has been accepted for
//! an id, that id names no resource a caller may still change. Every MUTATION
//! therefore resolves its id through the live lookups
//! ([`ComAmazonawsMedialiveMedialiveState::live_input`],
//! [`ComAmazonawsMedialiveMedialiveState::live_channel_mut`],
//! [`ComAmazonawsMedialiveMedialiveState::live_input_security_group`], and the
//! tag writers' own private `resource_tags_mut`) and answers a dead id with its
//! own modelled not-found error, while a read goes on reporting the record in
//! its terminal state.
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
//! TEMPLATES those groups hold live here too: one
//! [`CloudWatchAlarmTemplateRecord`] per created cloudwatch alarm template and
//! one [`EventBridgeRuleTemplateRecord`] per created eventbridge rule template,
//! each carrying the id of the group its `GroupIdentifier` resolved to, so a
//! template reports the group it belongs to out of state rather than out of the
//! request.
//!
//! Every value a channel-, input-, input-security-group-, SdiSource-,
//! signal-map- or template-group-shaped response carries is either stored
//! verbatim from the request that created the resource or derived from the stored
//! state ([`ComAmazonawsMedialiveMedialiveState::channel_description`],
//! [`ComAmazonawsMedialiveMedialiveState::input_description`],
//! [`ComAmazonawsMedialiveMedialiveState::sdi_source_description`],
//! [`ComAmazonawsMedialiveMedialiveState::signal_map_description`],
//! [`alarm_template_group_output`],
//! [`alarm_template_output`],
//! [`event_bridge_rule_template_group_output`]) - no operation builds a response
//! out of its own request.
//!
//! MediaLive Anywhere's CLUSTER lives here too: one [`ClusterRecord`] per
//! created cluster, its own id counter and its own token map, carrying the
//! alerts raised against that cluster. A cluster's `ChannelIds` is derived from
//! the channels this store already holds, never recorded at create time, and so
//! is its `State` — out of the `CREATING` -> `ACTIVE` transition the clock owes
//! it.
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
    DescribeLinkedChannelSettings, DescribeNetworkSummary, DescribePrimaryChannelSettings,
    EncoderSettings,
    InferenceSettings, Input, InputAttachment, InputDestination, InputDestinationRequest,
    InputDestinationRoute, InputDeviceSettings, InputSecurityGroup, InputSource,
    InputSourceRequest, InputSpecification, InputVpcRequest, InputWhitelistRule,
    InputWhitelistRuleCidr, IpPool,
    LinkedChannelSettings, MaintenanceStatus, MediaConnectFlow, MediaConnectFlowRequest,
    MediaResource, MediaResourceNeighbor, MonitorDeployment, MulticastSettings,
    MulticastSettingsCreateRequest, MulticastSource, OutputDestination, PipelineDetail,
    Route,
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

/// The cloudwatch alarm TEMPLATE slice: its record, its bounds, its refusal, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the per-region token key every
/// monitor template resource shares.
pub mod alarm_templates;

pub use alarm_templates::{
    AlarmTemplateRefusal, AlarmTemplateUpdateRefusal, CloudWatchAlarmTemplateChanges,
    CloudWatchAlarmTemplateRecord, NewCloudWatchAlarmTemplate, MAX_ALARM_PERIOD_SECONDS,
    MAX_ALARM_TEMPLATES, MAX_ALARM_TEMPLATE_TOKENS, MAX_METRIC_NAME_LEN, MIN_ALARM_DATAPOINTS,
    MIN_ALARM_PERIOD_SECONDS,
};

pub(crate) use alarm_templates::alarm_template_output;

/// The multiplex slice: its record, its bounds, its refusals, the store's
/// methods and its output projection, in one module of its own for the same
/// reason as the alarm template group slice above. The PROGRAMS a multiplex
/// holds are part of that slice: they live on the multiplex record that owns
/// them. What stays in this file is the part the rest of the state needs: the
/// store members on [`ComAmazonawsMedialiveMedialiveState`].
pub mod multiplexes;

pub use multiplexes::{
    multiplex_state, MultiplexProgramRecord, MultiplexProgramRefusal,
    MultiplexProgramSettingsRefusal, MultiplexProgramUpdateRefusal, MultiplexRecord,
    MultiplexRefusal, MultiplexStartRefusal, MultiplexStopRefusal, MAX_MULTIPLEXES,
    MAX_MULTIPLEX_PROGRAMS,
    MAX_MULTIPLEX_PROGRAM_TOKENS,
    MAX_MULTIPLEX_TOKENS, MAX_SERVICE_DESCRIPTOR_NAME_LEN, MULTIPLEX_AVAILABILITY_ZONES,
};

pub(crate) use multiplexes::{multiplex_description_output, multiplex_program_description_output};

/// The eventbridge rule TEMPLATE slice: its record, its bounds, its refusal, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the per-region token key every
/// monitor template resource shares.
pub mod event_bridge_rule_templates;

pub use event_bridge_rule_templates::{
    event_bridge_rule_template_summary, is_event_target_arn, is_event_type,
    EventBridgeRuleTemplateChanges, EventBridgeRuleTemplateRecord,
    EventBridgeRuleTemplateRefusal, EventBridgeRuleTemplateUpdateRefusal,
    NewEventBridgeRuleTemplate, EVENT_BRIDGE_RULE_TEMPLATE_EVENT_TYPES,
    MAX_EVENT_BRIDGE_RULE_TEMPLATES, MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS,
    MAX_EVENT_BRIDGE_RULE_TEMPLATE_TOKENS, MAX_EVENT_TARGET_ARN_LEN,
};

pub(crate) use event_bridge_rule_templates::event_bridge_rule_template_output;

/// The input device slice: the record `ClaimDevice` registers, the device-id
/// form it accepts, the maintenance window
/// `StartInputDeviceMaintenanceWindow` opens and the projection every operation
/// that reports a device reads, in one module of its own for the same reason as
/// the slices above. What stays in this file is the part the rest of the state
/// needs: the store member on [`ComAmazonawsMedialiveMedialiveState`] and the
/// ARN resolution the tag operations share.
pub mod input_devices;

pub use input_devices::{
    input_device_type_for_id, not_a_device_id_message, InputDeviceRecord, MAX_INPUT_DEVICES,
};

pub(crate) use input_devices::input_device_output;

/// The cluster slice: MediaLive Anywhere's `Cluster` record, its bounds, its
/// refusals, the store's methods and its output projections, in one module of
/// its own for the same reason as the multiplex slice above. The ALERTS raised
/// against a cluster are part of that slice: they live on the cluster record
/// that owns them. What stays in this file is the part the rest of the state
/// needs: the store members on [`ComAmazonawsMedialiveMedialiveState`].
pub mod clusters;

pub use clusters::{
    cluster_alert_output, cluster_alert_state, cluster_state, described_cluster_network_settings,
    ClusterAlertRecord, ClusterRecord, ClusterRefusal, NewCluster, CLUSTER_ALERT_FILTERS,
    CLUSTER_ALERT_FILTER_ALL, MAX_CLUSTERS, MAX_CLUSTER_ALERTS, MAX_CLUSTER_TOKENS,
};

pub(crate) use clusters::cluster_description_output;

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

/// The `NetworkState` enum values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="NetworkState")`: CREATING |
/// CREATE_FAILED | ACTIVE | DELETING | IDLE | IN_USE | UPDATING | DELETE_FAILED |
/// DELETED).
///
/// Only the two the create lifecycle reaches are named here. `CreateNetwork` is
/// asynchronous — `State` is documented "The current state of the Network. Only
/// MediaLive Anywhere can change the state" — so a network starts `CREATING` and
/// the clock owes it `ACTIVE`, the one state the enum names for a network that
/// exists, has not failed and nothing has deleted. The rest belong to operations
/// no handler implements yet: `UPDATING` to `UpdateNetwork`, `DELETING` and
/// `DELETED` to `DeleteNetwork`, `IDLE` and `IN_USE` to the cluster attachments
/// this store has none of (`CreateCluster` is still a scaffold), and the two
/// `*_FAILED` states to failures this emulator does not inject.
pub mod network_state {
    pub const CREATING: &str = "CREATING";
    pub const ACTIVE: &str = "ACTIVE";
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

/// The `SignalMapMonitorDeploymentStatus` values this slice uses, spelled
/// exactly as the model declares them (`smithy_query(query="shape",
/// shape="SignalMapMonitorDeploymentStatus")`: NOT_DEPLOYED |
/// DRY_RUN_DEPLOYMENT_COMPLETE | DRY_RUN_DEPLOYMENT_FAILED |
/// DRY_RUN_DEPLOYMENT_IN_PROGRESS | DEPLOYMENT_COMPLETE | DEPLOYMENT_FAILED |
/// DEPLOYMENT_IN_PROGRESS | DELETE_COMPLETE | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
///
/// `NOT_DEPLOYED` is where every signal map starts; `StartMonitorDeployment`
/// moves it into the IN_PROGRESS status of the kind of deployment the request
/// asked for — the dry-run one when `DryRun` is set, the real one otherwise —
/// and the clock settles that into the matching COMPLETE status
/// ([`MonitorDeploymentRecord`]).
///
/// The `*_FAILED` statuses are deliberately absent: nothing in this emulator
/// makes a deployment fail, so no code may report one as failed. So are the
/// `DELETE_*` ones, which belong to `StartDeleteMonitorDeployment` — still a
/// scaffold, and its own issue.
pub mod signal_map_monitor_deployment_status {
    pub const NOT_DEPLOYED: &str = "NOT_DEPLOYED";
    pub const DEPLOYMENT_IN_PROGRESS: &str = "DEPLOYMENT_IN_PROGRESS";
    pub const DEPLOYMENT_COMPLETE: &str = "DEPLOYMENT_COMPLETE";
    pub const DRY_RUN_DEPLOYMENT_IN_PROGRESS: &str = "DRY_RUN_DEPLOYMENT_IN_PROGRESS";
    pub const DRY_RUN_DEPLOYMENT_COMPLETE: &str = "DRY_RUN_DEPLOYMENT_COMPLETE";
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

/// One PUSH-type input's ingest protocol: the URL scheme the upstream system
/// pushes with, the port the service opens for it, and whether the
/// destination's `StreamName` is part of the endpoint URL's path.
#[derive(Debug, Clone, Copy)]
struct PushIngestProtocol {
    scheme: &'static str,
    port: u16,
    carries_stream_name: bool,
}

impl PushIngestProtocol {
    /// The endpoint URL this protocol's upstream system pushes to -
    /// `InputDestination$Url`, "the endpoint that the customer stream will be
    /// pushed to": the scheme, the endpoint's address and the port, plus the
    /// destination's stream name as the path for the protocol that names one.
    fn endpoint_url(&self, ip: &str, stream_name: Option<&str>) -> String {
        let mut url = format!("{}://{ip}:{}", self.scheme, self.port);
        if self.carries_stream_name {
            if let Some(stream_name) = stream_name.filter(|name| !name.is_empty()) {
                url.push('/');
                url.push_str(stream_name.trim_start_matches('/'));
            }
        }
        url
    }
}

/// The ingest protocol of every `InputType` the service allocates a push
/// endpoint for, keyed by the type name.
///
/// Source: the `InputType` enum (`smithy_query(query="shape",
/// shape="InputType")`) names the types; the push ports are the ones MediaLive
/// opens for a pushed stream - RTMP's registered 1935, and 5000 for the RTP and
/// UDP push inputs. Only RTMP's URL carries the destination's stream name,
/// per `InputDestinationRequest$StreamName`: "A unique name for the location
/// the RTMP stream is being pushed to."
///
/// Every other `InputType` is deliberately absent: the PULL and file types are
/// not pushed to at all, and for the rest (MEDIACONNECT, MEDIACONNECT_ROUTER,
/// INPUT_DEVICE, AWS_CDI, MULTICAST, SDI, SMPTE_2110_RECEIVER_GROUP and the two
/// SRT types) no documented rule gives the `<scheme>://<ip>:<port>` endpoint a
/// form, so none is invented for them.
const PUSH_INGEST_PROTOCOLS: [(&str, PushIngestProtocol); 3] = [
    (
        "RTMP_PUSH",
        PushIngestProtocol {
            scheme: "rtmp",
            port: 1935,
            carries_stream_name: true,
        },
    ),
    (
        "RTP_PUSH",
        PushIngestProtocol {
            scheme: "rtp",
            port: 5000,
            carries_stream_name: false,
        },
    ),
    (
        "UDP_PUSH",
        PushIngestProtocol {
            scheme: "udp",
            port: 5000,
            carries_stream_name: false,
        },
    ),
];

/// The ingest protocol of `input_type`, or `None` when the service allocates no
/// push endpoint for an input of that type.
fn push_ingest_protocol(input_type: Option<&str>) -> Option<PushIngestProtocol> {
    let input_type = input_type?;
    PUSH_INGEST_PROTOCOLS
        .iter()
        .find(|(name, _)| *name == input_type)
        .map(|(_, protocol)| *protocol)
}

/// Base of the pool a push input's ingest addresses come from: `198.18.0.0`,
/// the first address of `198.18.0.0/15`, the RFC 2544 benchmarking range.
/// Non-routable, so an emulated push endpoint never hands a caller an address
/// that belongs to somebody.
const INGEST_IP_POOL_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

/// `198.18.0.0/15` holds 2^17 addresses; allocation wraps inside it.
const INGEST_IP_POOL_LEN: u32 = 1 << 17;

/// How long a resource stays in a transient state before the clock makes the next
/// one due: a channel's `CREATING`, `STARTING`, `STOPPING`, `RECOVERING` and
/// `DELETING`, a signal map's `CREATE_IN_PROGRESS` discovery, and a signal map's
/// monitor deployment, which `StartMonitorDeployment` starts in an IN_PROGRESS
/// status.
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

/// The id the first Network of a process gets, and the most networks this
/// emulator stores for one account.
///
/// The id is a numeric string, as MediaLive resource ids are, minted from a
/// counter at a fixed seven-digit width so the `BTreeMap` key order equals the
/// mint order and `ListNetworks` pagination is deterministic; `Id` is documented
/// as "the resource-id portion of the ARN", so the ARN is built from it. A range
/// of its own keeps the id spaces apart, for the same reason as
/// [`FIRST_SDI_SOURCE_ID`]. The ceiling is not a guess at an AWS quota: it exists
/// so a create loop cannot grow the emulator's heap without bound, and
/// `CreateNetwork`'s own modelled `TooManyRequestsException` is the refusal once
/// the store is full.
const FIRST_NETWORK_ID: u64 = 8_000_000;

/// See [`FIRST_NETWORK_ID`].
pub const MAX_NETWORKS: usize = 100;

/// The most `IpPools` (or `Routes`) entries this emulator accepts on one
/// network.
///
/// A ceiling, not a guess at AWS's published quota: both lists are caller-chosen
/// and the model targets them at plain lists with no length constraint, so
/// nothing else bounds them, and every entry costs heap on a stored record and a
/// member in every response that reports the network. `CreateNetwork` refuses a
/// longer list with its own modelled `BadRequestException`, because the request
/// is what is out of bounds — the same treatment [`MAX_INPUT_ATTACHMENTS`] gets.
pub const MAX_NETWORK_CIDR_ENTRIES: usize = 32;

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

    /// Whether a delete has already been accepted for this channel — it is in
    /// the transient `DELETING` state the clock owes the terminal one, or in
    /// that terminal `DELETED` state itself.
    ///
    /// The notion a MUTATION of the channel resolves against: an id whose
    /// delete the service has accepted names no channel a caller may still
    /// change, exactly as [`InputRecord::is_delete_accepted`] does for an
    /// input.
    pub fn is_delete_accepted(&self) -> bool {
        self.is_deleted() || self.state == channel_state::DELETING
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

/// Which `State` an input security group projection reports — the one member
/// [`ComAmazonawsMedialiveMedialiveState::input_security_group_description`] and
/// [`ComAmazonawsMedialiveMedialiveState::input_security_group_usage_description`]
/// differ by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupStateView {
    /// The terminal `DELETED` once the group has been deleted, and otherwise the
    /// state its users imply: what every READ of the group reports.
    Terminal,
    /// The state the group's users imply, whether or not the group has been
    /// deleted: what the recorded `DeleteInputSecurityGroup` response carries.
    ///
    /// Both recorded rows this view answers, from replaying the
    /// `medialive-input-security-group-lifecycle` fixture — the only evidence
    /// for it, so it is recorded here rather than left to be rediscovered:
    ///
    /// * `#task#6`, the delete of a group nothing uses: reporting the terminal
    ///   `DELETED` diverged at `$.state`, and reporting the users' state (here
    ///   `IDLE`) matches. Computing the description BEFORE marking the group
    ///   deleted would also answer this row.
    /// * `#task#8`, a SECOND delete of that same, already deleted group: it too
    ///   diverged at `$.state` on `DELETED`, and matches on the users' state —
    ///   which a pre-mutation snapshot could NOT produce, since the group was
    ///   already deleted when the call arrived. That row is why the view exists
    ///   rather than an ordering of the write.
    ///
    /// `#task#7` and `#task#9`, the reads between and after those deletes, both
    /// want the terminal `DELETED`, which is [`Self::Terminal`].
    Usage,
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

/// The message a request whose whitelist rules are not all valid IPv4 CIDRs is
/// refused with — the fixed sentence of that rejection class, which states the
/// rule the whole list must meet and interpolates nothing: the service refuses
/// the request as a whole rather than offering a per-member error contract, so
/// neither the index of the offending rule nor the value it carried is part of
/// the message.
pub const INVALID_WHITELIST_CIDR_MESSAGE: &str = "Every whitelist needs to have a valid IPv4 CIDR.";

/// The `WhitelistRules` a request asks for, validated into the CIDR strings the
/// store keeps, or the message a list carrying anything that is not a valid IPv4
/// CIDR is refused with.
///
/// ONE copy for every operation that writes a group's whitelist
/// (`CreateInputSecurityGroup` and `UpdateInputSecurityGroup`), which document
/// the member identically ("List of IPv4 CIDR addresses to whitelist"), so the
/// rule the two enforce cannot drift apart. The whole list is checked before the
/// caller writes anything, so a refused request leaves the store untouched
/// rather than applying a prefix of its rules.
///
/// A rule that names no CIDR at all fails the same rule and so carries the same
/// message: every whitelist needs a valid IPv4 CIDR, and a missing one is not a
/// valid one.
pub fn whitelist_rules_from_request(
    requested: Vec<InputWhitelistRuleCidr>,
) -> Result<Vec<String>, String> {
    let mut cidrs: Vec<String> = Vec::with_capacity(requested.len());
    for rule in requested {
        let cidr = rule.cidr.filter(|cidr| is_ipv4_cidr(cidr));
        let Some(cidr) = cidr else {
            return Err(INVALID_WHITELIST_CIDR_MESSAGE.to_string());
        };
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

/// One created Network, as `CreateNetwork` stored it plus the lifecycle state of
/// that creation.
///
/// The members mirror the model's network response shapes (`CreateNetworkResponse`,
/// `DescribeNetworkResponse` and the `DescribeNetworkSummary` a listing reports,
/// which carry the same seven members), minus the one they derive:
/// `AssociatedClusterIds` comes from what else is in the store
/// ([`ComAmazonawsMedialiveMedialiveState::clusters_using_network`]), never from a
/// field written at create time.
#[derive(Debug, Clone)]
pub struct NetworkRecord {
    pub id: String,
    pub arn: String,
    /// The request's `Name`. `Option`, because the model declares the member
    /// optional (`smithy_query(query="operation", operation="CreateNetwork")`: no
    /// `@required`), so a network created without one has none rather than an
    /// empty name.
    pub name: Option<String>,
    /// The CIDR blocks the request reserved, stored as the `IpPool` shape a
    /// response reports, because `IpPoolCreateRequest` and `IpPool` carry the
    /// same one member (`Cidr`).
    pub ip_pools: Vec<IpPool>,
    /// The routes the request named, stored as the `Route` shape a response
    /// reports, for the same reason as [`Self::ip_pools`].
    pub routes: Vec<Route>,
    /// The request's `Tags`. No network response shape carries a `Tags` member
    /// (`smithy_query(query="operation", operation="DescribeNetwork")`), so the
    /// tag operations are what report them: `ListTagsForResource` reads this map
    /// through [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
    /// The `NetworkState` this network is in.
    state: String,
    /// The instant the creation becomes due, so `CREATING` settles into
    /// `ACTIVE`. Stored, never slept on.
    active_due: SystemTime,
}

impl NetworkRecord {
    /// Materialise the creation the clock has already made due. Owed at
    /// `active_due` whether or not anybody called, so applying it in a reporting
    /// operation changes nothing a later caller would have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.state == network_state::CREATING && now >= self.active_due {
            self.state = network_state::ACTIVE.to_string();
        }
    }
}

/// The message a request whose `IpPools` are not all valid IPv4 CIDR blocks is
/// refused with \u2014 the fixed sentence of that rejection class, which states the
/// rule the whole list must meet and interpolates nothing, exactly as
/// [`INVALID_WHITELIST_CIDR_MESSAGE`] does for a security group's rules.
pub const INVALID_IP_POOL_CIDR_MESSAGE: &str = "Every IP pool needs to have a valid IPv4 CIDR.";

/// The message a request whose `Routes` are not all valid is refused with: a
/// route's `Cidr` is "A CIDR block for one Route" and its optional `Gateway` is
/// "The IP address of the Gateway for this route", so a route missing the former
/// or carrying a gateway that is not an IP address fails this one rule.
pub const INVALID_ROUTE_MESSAGE: &str =
    "Every route needs to have a valid IPv4 CIDR and, if any, a valid IPv4 gateway address.";

/// The message a request naming more `IpPools` or `Routes` than the emulator
/// stores on one network is refused with.
pub fn too_many_network_entries_message(member: &str) -> String {
    format!("A network takes at most {MAX_NETWORK_CIDR_ENTRIES} {member} entries.")
}

/// The `IpPools` a request asks for, validated into the pools the store keeps,
/// or the message a list that is not all valid IPv4 CIDR blocks is refused with.
///
/// Takes the CIDRs rather than the request shape, so the one rule serves every
/// operation that writes a network's pools (`CreateNetwork` today,
/// `UpdateNetwork` when its own issue lands) even though the model gives each of
/// them its own request shape (`IpPoolCreateRequest`, `IpPoolUpdateRequest`)
/// carrying the same single member. The whole list is checked before the caller
/// writes anything, so a refused request leaves the store untouched rather than
/// applying a prefix of its pools.
pub fn network_ip_pools(
    requested: impl IntoIterator<Item = Option<String>>,
) -> Result<Vec<IpPool>, String> {
    let mut pools: Vec<IpPool> = Vec::new();
    for cidr in requested {
        if pools.len() >= MAX_NETWORK_CIDR_ENTRIES {
            return Err(too_many_network_entries_message("ipPools"));
        }
        let cidr = cidr.filter(|cidr| is_ipv4_cidr(cidr));
        let Some(cidr) = cidr else {
            return Err(INVALID_IP_POOL_CIDR_MESSAGE.to_string());
        };
        pools.push(IpPool { cidr: Some(cidr) });
    }
    Ok(pools)
}

/// The `Routes` a request asks for, validated into the routes the store keeps,
/// or the message a list carrying an invalid route is refused with. Takes the
/// `(Cidr, Gateway)` pairs for the same reason [`network_ip_pools`] takes the
/// CIDRs.
///
/// `Gateway` is documented "The IP address of the Gateway for this route, if
/// applicable", so a route may omit it; one that names it must name an IP
/// address.
pub fn network_routes(
    requested: impl IntoIterator<Item = (Option<String>, Option<String>)>,
) -> Result<Vec<Route>, String> {
    let mut routes: Vec<Route> = Vec::new();
    for (cidr, gateway) in requested {
        if routes.len() >= MAX_NETWORK_CIDR_ENTRIES {
            return Err(too_many_network_entries_message("routes"));
        }
        let cidr = cidr.filter(|cidr| is_ipv4_cidr(cidr));
        let gateway_is_address = gateway
            .as_deref()
            .is_none_or(|gateway| gateway.parse::<::std::net::Ipv4Addr>().is_ok());
        let Some(cidr) = cidr.filter(|_| gateway_is_address) else {
            return Err(INVALID_ROUTE_MESSAGE.to_string());
        };
        routes.push(Route {
            cidr: Some(cidr),
            gateway,
        });
    }
    Ok(routes)
}

/// Build a new network record from a `CreateNetwork` request's id and ARN.
///
/// The network starts `CREATING` and owes `ACTIVE` once [`TRANSIENT_STATE_SPAN`]
/// has passed: `State` is documented "The current state of the Network. Only
/// MediaLive Anywhere can change the state", so creation is asynchronous and a
/// caller waits on that member. The caller then stores the request's `Name`,
/// `IpPools`, `Routes` and `Tags` on the record.
pub fn new_network(id: String, arn: String, now: SystemTime) -> NetworkRecord {
    NetworkRecord {
        id,
        arn,
        name: None,
        ip_pools: Vec::new(),
        routes: Vec::new(),
        tags: HashMap::new(),
        state: network_state::CREATING.to_string(),
        active_due: now + TRANSIENT_STATE_SPAN,
    }
}

/// Project a [`DescribeNetworkSummary`] onto one of the structurally identical
/// per-operation network output shapes (`CreateNetworkOutput`,
/// `DescribeNetworkOutput`; `UpdateNetworkOutput` and `DeleteNetworkOutput`
/// carry the same seven members, for when their own issues land).
///
/// One macro rather than a hand-written copy of the same seven-member move per
/// operation, so the operations that report a network cannot drift from each
/// other. `DescribeNetworkSummary` is the shape a LISTING reports, which makes
/// the listed summary and the described network one projection rather than two.
macro_rules! network_description_output {
    ($out:ident, $network:expr) => {{
        let network: $crate::ops::types::DescribeNetworkSummary = $network;
        $out {
            arn: network.arn,
            associated_cluster_ids: network.associated_cluster_ids,
            id: network.id,
            ip_pools: network.ip_pools,
            name: network.name,
            routes: network.routes,
            state: network.state,
        }
    }};
}

pub(crate) use network_description_output;

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

/// The message a group delete refused because the group still holds templates
/// carries — the fixed sentence of that rejection class, which states the
/// precondition both group deletes document ("ensure its existing templates are
/// moved to another group or deleted") and interpolates nothing: the group's
/// identifier is in the request's own URI and the ids of the templates it still
/// holds are state the caller did not supply.
pub const TEMPLATE_GROUP_NOT_EMPTY_MESSAGE: &str = "Template group must be empty before deletion.";

/// The message a group delete refused because a signal map still names the
/// group carries: the other half of the precondition both group deletes
/// document ("You must detach this group from all signal maps"), in the same
/// fixed form as [`TEMPLATE_GROUP_NOT_EMPTY_MESSAGE`] and interpolating nothing
/// for the same reason.
pub const TEMPLATE_GROUP_STILL_ATTACHED_MESSAGE: &str =
    "Template group must be detached from all signal maps before deletion.";

/// The message a group delete's conflict carries: the unmet half of the
/// precondition both group deletes document ("You must detach this group from
/// all signal maps and ensure its existing templates are moved to another group
/// or deleted"), as the fixed sentence of that rejection class.
///
/// ONE builder, so the two group deletes cannot word the same refusal two ways.
/// The halves are reported in the order the documentation states them: a group a
/// signal map still names must be detached first, and only then does its
/// remaining content matter.
pub fn still_held_message(signal_map_ids: &[String], template_ids: &[String]) -> String {
    if signal_map_ids.is_empty() && !template_ids.is_empty() {
        TEMPLATE_GROUP_NOT_EMPTY_MESSAGE.to_string()
    } else {
        TEMPLATE_GROUP_STILL_ATTACHED_MESSAGE.to_string()
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
    /// Next offset into the ingest-address pool, so no two push destinations
    /// report the same endpoint address. PRIVATE: the only way in is
    /// [`Self::allocate_input_ingest_addresses`], which owns the pool.
    next_ingest_ip: u32,
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
    /// Every cloudwatch alarm template that exists, keyed by its minted `Id`. A
    /// `BTreeMap` so a later `ListCloudWatchAlarmTemplates` walks it in id order
    /// on every call.
    ///
    /// PRIVATE, so the group resolution, the name-uniqueness rule, the cap and
    /// the mint stay invariants of [`Self::insert_alarm_template`] rather than of
    /// each handler.
    alarm_templates: BTreeMap<String, CloudWatchAlarmTemplateRecord>,
    /// Mint counter behind [`Self::mint_alarm_template_id`].
    next_alarm_template_id: u64,
    /// `CreateCloudWatchAlarmTemplate`'s `RequestId` (`@idempotencyToken`), keyed
    /// per region by [`template_group_token_key`] -> the template it created.
    /// PRIVATE for the same reason as [`Self::channel_tokens`].
    alarm_template_tokens: IdempotencyTokens,
    /// Every SdiSource that exists, keyed by its minted `Id`, in a `BTreeMap`
    /// for the same reason as [`Self::channels`].
    pub sdi_sources: BTreeMap<String, SdiSourceRecord>,
    /// Mint counter behind [`Self::mint_sdi_source_id`].
    pub next_sdi_source_id: u64,
    /// `CreateSdiSource`'s `RequestId` (`@idempotencyToken`) -> the SdiSource it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    sdi_source_tokens: IdempotencyTokens,
    /// Every Network that exists, keyed by its minted `Id`, in a `BTreeMap` for
    /// the same reason as [`Self::channels`]: `ListNetworks` walks it in id
    /// order on every call and its `NextToken` means the same thing on the next
    /// one.
    pub networks: BTreeMap<String, NetworkRecord>,
    /// Mint counter behind [`Self::mint_network_id`].
    pub next_network_id: u64,
    /// `CreateNetwork`'s `RequestId` (`@idempotencyToken`) -> the network it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    network_tokens: IdempotencyTokens,
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
    /// `CreateMultiplexProgram`'s `RequestId` (`@idempotencyToken`) -> the
    /// program it created, keyed by the multiplex slice's
    /// `multiplex_program_key` because a program is named by its multiplex and
    /// its own name rather than by an id. PRIVATE for the same reason as
    /// [`Self::channel_tokens`].
    multiplex_program_tokens: IdempotencyTokens,
    /// Every input device this account has claimed, keyed by the device id the
    /// claim named. A `BTreeMap` for the same reason as [`Self::channels`]: a
    /// reader walks it in id order on every call. There is no mint counter
    /// beside it, because a device id is the unit's own, not one this service
    /// assigns.
    pub input_devices: BTreeMap<String, InputDeviceRecord>,
    /// Every cluster that exists, keyed by its minted `Id`, in id order for the
    /// same reason as [`Self::channels`]: a reader walks it in id order on every
    /// call. The alerts raised against a cluster live on its own record.
    pub clusters: BTreeMap<String, ClusterRecord>,
    /// Mint counter behind the cluster slice's `mint_cluster_id`.
    pub next_cluster_id: u64,
    /// `CreateCluster`'s `RequestId` (`@idempotencyToken`) -> the cluster it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    cluster_tokens: IdempotencyTokens,
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
        for network in self.networks.values_mut() {
            network.settle(now);
        }
        self.settle_multiplexes(now);
        self.settle_input_devices(now);
        self.settle_clusters(now);
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
            // An input whose deletion has been accepted is on its way out of the
            // account and no longer "currently using" anything — the same
            // reading of "currently using" that `inputs_using_sdi_source`
            // applies to the source side.
            .filter(|input| !input.is_delete_accepted())
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
        self.described_input_security_group(id, GroupStateView::Terminal)
    }

    /// The same description with `State` derived from the group's users ALONE —
    /// `IN_USE` while some resource references it and `IDLE` while none does —
    /// ignoring whether the group has been deleted.
    ///
    /// This is what `DeleteInputSecurityGroup` answers with: the recorded delete
    /// response reports that usage state both for the group it has just deleted
    /// and for one an earlier delete already deleted, while the terminal
    /// `DELETED` is what every `DescribeInputSecurityGroup` after the delete
    /// reports.
    pub(crate) fn input_security_group_usage_description(
        &self,
        id: &str,
    ) -> Option<InputSecurityGroup> {
        self.described_input_security_group(id, GroupStateView::Usage)
    }

    /// One projection behind both readers above, so the two cannot drift in any
    /// member but the `State` that `view` selects.
    fn described_input_security_group(
        &self,
        id: &str,
        view: GroupStateView,
    ) -> Option<InputSecurityGroup> {
        let group = self.input_security_groups.get(id)?;
        let inputs = self.inputs_using_security_group(id);
        let channels = self.channels_using_security_group(id);
        let state = match view {
            GroupStateView::Terminal => {
                input_security_group_state_for(group, &inputs, &channels)
            }
            GroupStateView::Usage => input_security_group_usage_state(&inputs, &channels),
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
            // An input whose deletion has been accepted has stopped using the
            // source, exactly as a deleted source has nothing using it.
            .filter(|input| !input.is_delete_accepted())
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

    /// Whether the network store is full, so `CreateNetwork` must refuse rather
    /// than mint another network.
    pub fn networks_are_full(&self) -> bool {
        self.networks.len() >= MAX_NETWORKS
    }

    /// The next network id, as AWS shapes MediaLive resource ids: a numeric
    /// string, which `Id` documents as "the resource-id portion of the ARN".
    pub fn mint_network_id(&mut self) -> String {
        let id = FIRST_NETWORK_ID + self.next_network_id;
        self.next_network_id += 1;
        id.to_string()
    }

    /// Record `token` as having created `network_id`.
    pub fn record_network_token(&mut self, token: String, network_id: &str) {
        self.network_tokens.record(token, network_id, MAX_NETWORKS);
    }

    /// The network a stored `RequestId` created, if that network still exists.
    pub fn network_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.network_tokens.created(request_id)?;
        self.networks.get(id).map(|network| network.id.as_str())
    }

    /// The id of the network whose `Name` is `name`, if the store holds one.
    ///
    /// `CreateNetwork`'s `Name` is documented "Specify a name that is unique in
    /// the AWS account… Names are case-sensitive", so the lookup is exact and
    /// account-wide (not per region), and it is what makes a second create under
    /// a name already taken the modelled `ConflictException` rather than a second
    /// network.
    pub fn network_named(&self, name: &str) -> Option<&str> {
        self.networks
            .values()
            .find(|network| network.name.as_deref() == Some(name))
            .map(|network| network.id.as_str())
    }

    /// The ids of the clusters currently attached to the network `id` — what
    /// `AssociatedClusterIds` reports. Derived from the store on every read, so
    /// it tracks every later cluster create and delete rather than being recorded
    /// at create time.
    ///
    /// Empty for every network today: a cluster names its networks through
    /// `CreateCluster`, which is still a scaffold, so this store holds no cluster
    /// that could be attached to one. The derivation lives here so the cluster
    /// slice has one place to fill in when it lands.
    pub fn clusters_using_network(&self, _network_id: &str) -> Vec<String> {
        Vec::new()
    }

    /// The `DescribeNetworkSummary` projection of one stored network, or `None`
    /// when no network has that id.
    ///
    /// The ONE projection every operation that reports a network
    /// (`CreateNetwork`, `DescribeNetwork`, `ListNetworks`) answers from, so none
    /// of them can drift from the others: the summary shape a listing reports
    /// carries exactly the members the flat describe output does
    /// (`smithy_query(query="shape", shape="DescribeNetworkSummary")`), and
    /// [`network_description_output`] projects it onto those outputs.
    pub fn network_description(&self, id: &str) -> Option<DescribeNetworkSummary> {
        let network = self.networks.get(id)?;
        Some(DescribeNetworkSummary {
            arn: Some(network.arn.clone()),
            associated_cluster_ids: Some(self.clusters_using_network(id)),
            id: Some(network.id.clone()),
            ip_pools: Some(network.ip_pools.clone()),
            name: network.name.clone(),
            routes: Some(network.routes.clone()),
            state: Some(network.state.clone()),
        })
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

    /// Apply the members an `UpdateEventBridgeRuleTemplateGroup` request
    /// supplied onto the group `identifier` names in `region`, and answer that
    /// group's id — or `None` when this region holds no such group.
    ///
    /// "Updates the specified eventbridge rule template group": a member the
    /// request carries replaces the stored one, a member it omits leaves that
    /// value as it was, and `ModifiedAt` becomes the instant of the call, which
    /// is when the record was last written. The identifier resolves through
    /// [`Self::event_bridge_rule_template_group`], so an update reaches its
    /// group by either form `Identifier` documents.
    pub fn update_event_bridge_rule_template_group(
        &mut self,
        region: &str,
        identifier: &str,
        description: Option<String>,
        now: SystemTime,
    ) -> Option<String> {
        let id = self
            .event_bridge_rule_template_group(region, identifier)?
            .id
            .clone();
        let group = self.event_bridge_rule_template_groups.get_mut(&id)?;
        if let Some(description) = description {
            group.description = Some(description);
        }
        group.modified_at = now;
        Some(id)
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

    /// Allocate the system-generated ingest address of every push destination
    /// of `id` that does not hold one yet, so the input reports the endpoint an
    /// upstream system pushes its stream to.
    ///
    /// Call this from the operations that WRITE an input's destinations
    /// (`CreateInput`, `UpdateInput`): allocating an address is a write, so no
    /// reporting operation does it.
    ///
    /// Top-up only, and only for an input whose type has a push ingest protocol
    /// ([`PUSH_INGEST_PROTOCOLS`]): an address already allocated for a
    /// destination slot stays there, which is what keeps
    /// `InputDestination$Ip` - "the system-generated static IP address of
    /// endpoint. It remains fixed for the lifetime of the input" - fixed across
    /// an update that rewrites the destinations.
    ///
    /// Every destination slot of a push input gets one, including a slot whose
    /// request named its own ON-PREM `StaticIpAddress` (which is what that slot
    /// then reports): the slot keeps an endpoint of its own, so a later update
    /// that drops the static address does not leave it without one.
    ///
    /// An address belongs to the destination SLOT, not to the stream name that
    /// occupies it: an update that shrinks the list keeps the trailing addresses,
    /// so a later update that grows it again puts the same address back on the
    /// same ingest position rather than spending a new one.
    pub fn allocate_input_ingest_addresses(&mut self, id: &str) {
        let Some(input) = self.inputs.get(id) else {
            return;
        };
        if push_ingest_protocol(input.input_type.as_deref()).is_none() {
            return;
        }
        let allocated = input.ingest_ips.len();
        let wanted = input.destinations.len();
        let fresh: Vec<String> = (allocated..wanted)
            .map(|_| self.allocate_ingest_address())
            .collect();
        if let Some(input) = self.inputs.get_mut(id) {
            input.ingest_ips.extend(fresh);
        }
    }

    /// The next address out of the ingest pool.
    ///
    /// The counter wraps at [`INGEST_IP_POOL_LEN`], so an address could in
    /// principle be reissued - but only after 2^17 allocations, and the store
    /// holds at most [`MAX_INPUTS`] inputs, each with a caller-bounded
    /// destination list, so the wrap is the pool's last-resort bound rather than
    /// a path a client can reach.
    fn allocate_ingest_address(&mut self) -> String {
        let offset = self.next_ingest_ip % INGEST_IP_POOL_LEN;
        self.next_ingest_ip = self.next_ingest_ip.wrapping_add(1);
        ::std::net::Ipv4Addr::from(INGEST_IP_POOL_BASE + offset).to_string()
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

    /// The input `id` names while it is still LIVE: a record whose delete this
    /// service has not accepted.
    ///
    /// The lookup a MUTATION of an input goes through. A delete never drops the
    /// record — it keeps it in `DELETING` and then in the terminal `DELETED`, so
    /// a reader goes on reporting it — but that record is no longer a resource a
    /// caller may change, so a mutation resolves the id through here and refuses
    /// a dead one with its own modelled not-found error. Reads keep using
    /// [`Self::inputs`] / [`Self::input_description`] directly.
    pub fn live_input(&self, id: &str) -> Option<&InputRecord> {
        self.inputs
            .get(id)
            .filter(|input| !input.is_delete_accepted())
    }

    /// The channel `id` names while it is still LIVE, for the same reason as
    /// [`Self::live_input`]: a channel whose delete has been accepted is no
    /// longer a channel a caller may change (its schedule included).
    pub fn live_channel_mut(&mut self, id: &str) -> Option<&mut ChannelRecord> {
        self.channels
            .get_mut(id)
            .filter(|channel| !channel.is_delete_accepted())
    }

    /// The input security group `id` names while it is still LIVE, for the same
    /// reason as [`Self::live_input`]: a group `DeleteInputSecurityGroup` has
    /// put in the terminal `DELETED` state is no longer a group a caller may
    /// change.
    pub fn live_input_security_group(&self, id: &str) -> Option<&InputSecurityGroupRecord> {
        self.input_security_groups
            .get(id)
            .filter(|group| !group.is_deleted())
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
    ///
    /// Every member is DERIVED from the store as it stands at the moment of the
    /// read rather than echoed from whatever the create request happened to
    /// carry: `AttachedChannels` and `State` follow the channels that attach it,
    /// and `Destinations` follows the ingest endpoints the service still holds
    /// for it ([`described_input_destinations`]), so an input whose delete has
    /// been accepted stops reporting them.
    pub fn input_description(&self, id: &str) -> Option<Input> {
        let input = self.inputs.get(id)?;
        Some(Input {
            arn: Some(input.arn.clone()),
            attached_channels: Some(self.attached_channels(id)),
            destinations: Some(described_input_destinations(input)),
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
            // Reported on every Input read, with its (possibly empty) group
            // list: the structure is how a caller tells a receiver-group input's
            // streams apart, so an input configured with no receiver group
            // answers with an empty list rather than an unset member.
            smpte2110_receiver_group_settings: Some(Smpte2110ReceiverGroupSettings {
                smpte2110_receiver_groups: Some(
                    input
                        .smpte2110_receiver_group_settings
                        .as_ref()
                        .and_then(|settings| settings.smpte2110_receiver_groups.clone())
                        .unwrap_or_default(),
                ),
            }),
            sdi_sources: Some(input.sdi_sources.clone()),
            router_settings: input.router_settings.as_ref().map(described_router_settings),
        })
    }

    /// Accept the deletion of the input `id` names and answer with the input it
    /// deleted, or `None` when no input has that id.
    ///
    /// `DeleteInput` answers with the input it just deleted: the representation
    /// that input HAD when the delete was accepted, carrying the `State` the
    /// acceptance put it in. Both halves come out of the store — the snapshot is
    /// taken from the record before the deletion is written and the state is
    /// read back from the record after it — so nothing is assembled out of the
    /// request.
    ///
    /// That is why the snapshot is taken first: accepting the delete is what
    /// RELEASES the input's ingest endpoints ([`described_input_destinations`]),
    /// and a caller is told which endpoints the delete took down. Every later
    /// read of the input answers from the record itself, so it reports none of
    /// them.
    ///
    /// Idempotent, like [`begin_delete_input`]: a delete already accepted keeps
    /// the instant the first call set, and its snapshot is the torn-down input's
    /// own — endpoints already released and all.
    pub fn delete_input_description(&mut self, id: &str, now: SystemTime) -> Option<Input> {
        let mut deleted = self.input_description(id)?;
        let record = self.inputs.get_mut(id)?;
        begin_delete_input(record, now);
        let record = self.inputs.get(id)?;
        deleted.state = Some(self.input_state(record).to_string());
        Some(deleted)
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

    /// The signal map `identifier` names, for a MUTATION of it — resolved by id
    /// or by current name, exactly as [`Self::signal_map_id_for_identifier`]
    /// resolves it for a read, so the two cannot disagree about which signal map
    /// an identifier names.
    pub fn signal_map_for_identifier_mut(
        &mut self,
        identifier: &str,
    ) -> Option<&mut SignalMapRecord> {
        let id = self.signal_map_id_for_identifier(identifier)?.to_string();
        self.signal_maps.get_mut(&id)
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
            // The monitor side of the signal map, out of the record: the latest
            // deployment `StartMonitorDeployment` started and the latest one
            // that succeeded, both ABSENT while there is none.
            //
            // `MonitorChangesPendingDeployment` is "if true, there are pending
            // monitor changes for this signal map that can be deployed": no
            // operation in this emulator changes a signal map's monitor, so
            // there is never a pending change to deploy.
            monitor_changes_pending_deployment: Some(false),
            monitor_deployment: map
                .monitor_deployment
                .as_ref()
                .map(MonitorDeploymentRecord::described),
            last_successful_monitor_deployment: map.last_successful_monitor_deployment.clone(),
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
            // The status of the signal map's latest monitor deployment, read off
            // the same record `signal_map_description` reports
            // `MonitorDeployment` from, so the summary and the full description
            // cannot disagree about it.
            monitor_deployment_status: map.monitor_deployment_status().to_string(),
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
        if let Some(record) = self.networks.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Network(record.id.clone()));
        }
        if let Some(record) = self.signal_maps.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::SignalMap(record.id.clone()));
        }
        if let Some(record) = self.multiplexes.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Multiplex(record.id.clone()));
        }
        if let Some(record) = self.input_devices.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::InputDevice(record.id.clone()));
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
        if let Some(record) = self.clusters.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Cluster(record.id.clone()));
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
            TaggedResource::Network(id) => self.networks.get(&id).map(|record| &record.tags),
            TaggedResource::SignalMap(id) => self.signal_maps.get(&id).map(|record| &record.tags),
            TaggedResource::Multiplex(id) => self.multiplexes.get(&id).map(|record| &record.tags),
            TaggedResource::InputDevice(id) => {
                self.input_devices.get(&id).map(|record| &record.tags)
            }
            TaggedResource::AlarmTemplateGroup(id) => self
                .alarm_template_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::EventBridgeRuleTemplateGroup(id) => self
                .event_bridge_rule_template_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::Cluster(id) => self.clusters.get(&id).map(|record| &record.tags),
        }
    }

    /// The tag map of the resource `arn` names, to write through, or `None`
    /// when this store holds no resource with that ARN that is still live.
    ///
    /// The map handed back is the resource's OWN, so a tag written here is on
    /// the resource itself: the resource's describe and
    /// [`Self::resource_tags`] read the one map rather than two copies that
    /// could drift.
    ///
    /// A resource whose delete has been accepted is NOT handed out: tagging and
    /// untagging change a resource's configuration, so they resolve the ARN
    /// through the same live lookup every other mutation uses
    /// ([`Self::live_input`]) and the tag writers answer a dead ARN with their
    /// modelled not-found error. The READ path ([`Self::resource_tags`]) keeps
    /// resolving a deleted resource, whose record stays in the account.
    ///
    /// PRIVATE: the per-resource tag bounds are invariants of
    /// [`Self::insert_resource_tags`], which is the only way a tag is written,
    /// so no handler can grow a resource's tag map past them.
    fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        match self.tagged_resource(arn)? {
            TaggedResource::Channel(id) => self
                .channels
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::Input(id) => self
                .inputs
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::InputSecurityGroup(id) => self
                .input_security_groups
                .get_mut(&id)
                .filter(|record| !record.is_deleted())
                .map(|record| &mut record.tags),
            TaggedResource::SdiSource(id) => self
                .sdi_sources
                .get_mut(&id)
                .filter(|record| !record.is_deleted())
                .map(|record| &mut record.tags),
            // No delete filter: `DeleteNetwork` is still a scaffold, so no
            // network in this store has had a delete accepted for it.
            TaggedResource::Network(id) => {
                self.networks.get_mut(&id).map(|record| &mut record.tags)
            }
            TaggedResource::SignalMap(id) => self
                .signal_maps
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Multiplex(id) => self
                .multiplexes
                .get_mut(&id)
                .map(|record| &mut record.tags),
            // A claimed device is never deleted — nothing in this service
            // un-claims one — so there is no dead id to filter out here.
            TaggedResource::InputDevice(id) => self
                .input_devices
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
            TaggedResource::Cluster(id) => self
                .clusters
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
    Network(String),
    SignalMap(String),
    Multiplex(String),
    InputDevice(String),
    AlarmTemplateGroup(String),
    EventBridgeRuleTemplateGroup(String),
    Cluster(String),
}

/// One monitor deployment of one signal map, as `StartMonitorDeployment`
/// started it.
///
/// A deployment is ASYNCHRONOUS — the operation answers 202 and the caller polls
/// the signal map's `MonitorDeployment$Status` — so the record carries the status
/// it is in now and the instant the clock makes its outcome due. The instant is
/// STORED and never slept on.
#[derive(Debug, Clone)]
pub struct MonitorDeploymentRecord {
    /// Whether the request asked for a dry run, which is what decides both the
    /// pair of statuses this deployment moves through and whether completing it
    /// leaves a successful deployment behind.
    dry_run: bool,
    /// The `SignalMapMonitorDeploymentStatus` this deployment is in.
    status: String,
    /// `MonitorDeployment$DetailsUri`, built once when the deployment started
    /// ([`monitor_deployment_details_uri`]) so every later read reports the same
    /// URI.
    details_uri: String,
    /// The instant this deployment's outcome becomes due.
    due: SystemTime,
}

impl MonitorDeploymentRecord {
    /// Whether this deployment is still running, in either of the two
    /// IN_PROGRESS statuses a deployment can be running in.
    fn in_progress(&self) -> bool {
        self.status == signal_map_monitor_deployment_status::DEPLOYMENT_IN_PROGRESS
            || self.status == signal_map_monitor_deployment_status::DRY_RUN_DEPLOYMENT_IN_PROGRESS
    }

    /// The status this deployment completes in: a dry run reports the dry-run
    /// outcome, a real deployment the deployed one.
    fn complete_status(&self) -> &'static str {
        if self.dry_run {
            signal_map_monitor_deployment_status::DRY_RUN_DEPLOYMENT_COMPLETE
        } else {
            signal_map_monitor_deployment_status::DEPLOYMENT_COMPLETE
        }
    }

    /// This deployment as the modelled `MonitorDeployment`, "the latest monitor
    /// deployment of a signal map".
    fn described(&self) -> MonitorDeployment {
        MonitorDeployment {
            details_uri: Some(self.details_uri.clone()),
            // ABSENT while nothing failed: `MonitorDeployment$ErrorMessage` is
            // the "error message associated with a FAILED monitor deployment",
            // and no deployment fails in this emulator.
            error_message: None,
            status: self.status.clone(),
        }
    }
}

/// One created signal map, as `CreateSignalMap` stored it plus the graph its
/// discovery found, the lifecycle state of that discovery and the monitor
/// deployments `StartMonitorDeployment` has run for it.
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
    /// The latest monitor deployment `StartMonitorDeployment` started for this
    /// signal map, or `None` while none ever was — which is the `NOT_DEPLOYED`
    /// status [`Self::monitor_deployment_status`] reports.
    monitor_deployment: Option<MonitorDeploymentRecord>,
    /// The latest monitor deployment that COMPLETED, which only a real
    /// (non-dry-run) deployment leaves behind: a dry run deploys nothing, so it
    /// is not a successful monitor deployment OF the signal map.
    last_successful_monitor_deployment: Option<SuccessfulMonitorDeployment>,
}

impl SignalMapRecord {
    /// Materialise what the clock has already made due: the discovery, owed at
    /// `discovery_due`, and the outcome of a monitor deployment, owed at its own
    /// `due`. Both were owed at those instants whether or not anybody called, so
    /// applying them in a reporting operation changes nothing a later caller
    /// would have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.status == signal_map_status::CREATE_IN_PROGRESS && now >= self.discovery_due {
            self.status = signal_map_status::CREATE_COMPLETE.to_string();
            self.last_discovered_at = Some(self.discovery_due);
            self.modified_at = self.discovery_due;
        }
        // Completed on THIS call: the due instant to stamp as the signal map's
        // `ModifiedAt`, and the successful deployment to record when the
        // deployment was a real one. Read out of the deployment before the
        // signal map's own fields are written, so the record is borrowed once.
        let settled = self.monitor_deployment.as_mut().and_then(|deployment| {
            if !deployment.in_progress() || now < deployment.due {
                return None;
            }
            deployment.status = deployment.complete_status().to_string();
            let successful = (!deployment.dry_run).then(|| SuccessfulMonitorDeployment {
                details_uri: deployment.details_uri.clone(),
                status: deployment.status.clone(),
            });
            Some((deployment.due, successful))
        });
        if let Some((due, successful)) = settled {
            self.modified_at = due;
            if let Some(successful) = successful {
                self.last_successful_monitor_deployment = Some(successful);
            }
        }
    }

    /// The `SignalMapStatus` this signal map is in.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Whether this signal map has a monitor to deploy at all: deploying "the
    /// latest monitor of the specified signal map" needs the media resource map
    /// its discovery found, so a signal map whose discovery has not completed
    /// has nothing to deploy yet.
    pub fn discovery_complete(&self) -> bool {
        self.status == signal_map_status::CREATE_COMPLETE
    }

    /// Whether a monitor deployment of this signal map is already running, so a
    /// second one cannot be started on top of it.
    pub fn monitor_deployment_in_progress(&self) -> bool {
        self.monitor_deployment
            .as_ref()
            .is_some_and(MonitorDeploymentRecord::in_progress)
    }

    /// The status of this signal map's latest monitor deployment, as
    /// `SignalMapSummary$MonitorDeploymentStatus` reports it: `NOT_DEPLOYED`
    /// while no deployment was ever started for it.
    pub fn monitor_deployment_status(&self) -> &str {
        self.monitor_deployment.as_ref().map_or(
            signal_map_monitor_deployment_status::NOT_DEPLOYED,
            |deployment| deployment.status.as_str(),
        )
    }

    /// Start a monitor deployment of this signal map, replacing the latest one:
    /// `StartMonitorDeployment` "initiates a deployment to deploy the LATEST
    /// monitor", so a signal map whose monitor was deployed before is deployed
    /// again rather than refused.
    ///
    /// The deployment starts in the IN_PROGRESS status of the kind the request
    /// asked for and comes due [`TRANSIENT_STATE_SPAN`] later, so a caller that
    /// polls straight after the call still observes it running. A deployment
    /// that has already completed is left in place until this one completes:
    /// `LastSuccessfulMonitorDeployment` is the latest SUCCESSFUL deployment,
    /// which a redeployment does not undo.
    pub fn begin_monitor_deployment(&mut self, dry_run: bool, now: SystemTime) {
        let status = if dry_run {
            signal_map_monitor_deployment_status::DRY_RUN_DEPLOYMENT_IN_PROGRESS
        } else {
            signal_map_monitor_deployment_status::DEPLOYMENT_IN_PROGRESS
        };
        self.monitor_deployment = Some(MonitorDeploymentRecord {
            dry_run,
            status: status.to_string(),
            details_uri: monitor_deployment_details_uri(&self.arn, &self.id),
            due: now + TRANSIENT_STATE_SPAN,
        });
        self.modified_at = now;
    }
}

/// The URI one signal map's monitor deployment reports as its `DetailsUri`: the
/// MediaLive console's page for that signal map, in the region the signal map's
/// own ARN names.
///
/// CHOSEN, not documented by AWS: `MonitorDeployment$DetailsUri` is only "URI
/// associated with a signal map's monitor deployment" and no AWS document gives
/// that URI a form, so this emulator serves one deterministic form of its own.
/// It is derived from STORED state — the signal map's ARN and its id — so every
/// read of a deployment reports the same URI, and it is never built out of the
/// request in flight.
fn monitor_deployment_details_uri(arn: &str, id: &str) -> String {
    let region = arn_region(arn);
    format!(
        "https://{region}.console.aws.amazon.com/medialive/home\
         ?region={region}#!/signal-maps/{id}/monitor"
    )
}

/// The region segment of an ARN
/// (`arn:<partition>:<service>:<region>:<account-id>:<resource>`), empty for a
/// global service's ARNs, which carry no region.
fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
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
        // No monitor has been deployed for a signal map nobody has deployed one
        // for: `StartMonitorDeployment` is what starts the first one.
        monitor_deployment: None,
        last_successful_monitor_deployment: None,
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

/// The `message` a not-found refusal carries when what the identifier failed to
/// resolve to is a resource of the EVENTBRIDGE RULE-TEMPLATE hierarchy — a rule
/// template or a rule-template group: NONE, the member is left unset.
///
/// `NotFoundException` is a structure whose single member `Message` (jsonName
/// `message`) is OPTIONAL and whose documentation is a placeholder
/// (`smithy_query(query="shape",shape="NotFoundException")`), so the model
/// states no message rule for it and the member may legitimately be absent. The
/// recorded behaviour of this hierarchy is that it IS absent: a 404 answered for
/// a rule template or a rule-template group an identifier does not resolve to
/// carries the status and the error code and no `message` member at all. So this
/// is a `None` and not an empty string — an empty string is a present member
/// whose value is empty, which is a different error document.
///
/// Every OTHER MediaLive resource family keeps composing its message with
/// [`not_found_message`]: the recorded 404s of the classic channel/input API
/// ("Channel <id> not found") and of the signal map's discovery entry point do
/// carry one, so the absence is this hierarchy's, not the service's.
///
/// ONE constant, so the ten places that report such a resource missing — its
/// own get/update/delete at both levels, the template create's and the
/// listing's group resolution, and the signal-map operations that resolve a rule
/// template group — cannot answer it two ways.
pub const EVENT_BRIDGE_RULE_TEMPLATE_NOT_FOUND_MESSAGE: Option<String> = None;

/// The message a mutation refused on an id whose delete has already been
/// accepted carries: the subject the request named, then `already deleted` —
/// the distinction the real service draws between an id that was deleted and
/// one that never existed (both answered 404). One helper for the same reason
/// as [`not_found_message`].
///
/// `subject` is how the refusal names what was deleted, which is the wording
/// each resource's recorded refusal uses: the input refusal recorded in the
/// `medialive.input-lifecycle-url-pull` behavioural check names the REQUEST
/// MEMBER and its value (`inputId=<id>`), while this service's other refusals
/// name the resource type and its id.
pub fn already_deleted_message(subject: &str) -> String {
    format!("{subject} already deleted")
}

/// How a refusal on a deleted input names it: the `InputId` member as the wire
/// spells it, then the id the request carried — composed from the request, like
/// every other message this service writes.
pub fn deleted_input_subject(input_id: &str) -> String {
    format!("inputId={input_id}")
}

/// The message a delete refused on an input a channel still attaches carries:
/// the input the REQUEST named, and that it is busy and cannot be deleted.
///
/// The template is the service's own for this rejection class, and the only
/// value it interpolates is the request's own `InputId`: which channels attach
/// the input is state the caller did not name, so it is not part of the message
/// (the attaching channels are what `DescribeInput`/`ListChannels` report).
///
/// One helper for the same reason as [`not_found_message`] — `DeleteInput` and
/// `BatchDelete` refuse the same input, so they cannot word it two ways.
pub fn busy_input_message(input_id: &str) -> String {
    format!("Input {input_id} is busy, it cannot be deleted")
}

/// The message a channel delete refused because the channel has not stopped
/// carries — the fixed sentence of that rejection class, naming the state the
/// caller must reach rather than interpolating the channel's id or its current
/// state (neither is something the caller supplied, and both are what
/// `DescribeChannel` reports).
///
/// ONE constant, so `DeleteChannel` and `BatchDelete` — which refuse the same
/// channel — cannot word the refusal two ways.
pub const CHANNEL_NOT_STOPPED_MESSAGE: &str =
    "Cannot delete a channel that has not been stopped yet. \
     Please stop the channel and try again.";

/// The message a delete refused on the resource's state carries: the resource,
/// the state it is in, and the state a delete needs.
///
/// This is the generic, request-composed wording, used for a resource whose
/// class message this service does not know — the multiplex, whose
/// not-stopped refusal only `BatchDelete` reaches. The CHANNEL delete has a
/// known fixed message for the class and uses [`CHANNEL_NOT_STOPPED_MESSAGE`]
/// instead.
pub fn not_idle_message(resource_type: &str, identifier: &str, state: &str) -> String {
    format!(
        "{resource_type} {identifier} is in state {state} and must be \
         {} before it can be deleted",
        channel_state::IDLE
    )
}

/// The message a create refused because the region already holds a monitor
/// resource of that type under the requested `Name` carries.
///
/// Every monitor resource — the cloudwatch alarm template and its group, the
/// eventbridge rule template and its group, and the signal map — documents the
/// same uniqueness rule for `Name` ("Names must be unique within the scope of a
/// resource type in a specific region"), so the name collision is ONE rejection
/// class across all of them and this is the fixed sentence it carries. Nothing
/// is interpolated: the colliding resource's internal id and the region are
/// state the caller did not supply, and a caller matching on the message would
/// be matching on this emulator's environment.
pub const NAME_ALREADY_IN_USE_MESSAGE: &str = "Name already in use";

/// One created input, as `CreateInput` stored it plus its lifecycle state.
///
/// The REQUEST shapes are what is stored (`InputSourceRequest`,
/// `InputDestinationRequest`, …), and the `Input` response shape is projected
/// out of them by [`ComAmazonawsMedialiveMedialiveState::input_description`], so
/// nothing a request supplied is dropped and no response is built from the
/// request that happens to be in flight. A push input's ingest endpoint is
/// allocated into [`InputRecord::ingest_ips`] by
/// [`ComAmazonawsMedialiveMedialiveState::allocate_input_ingest_addresses`] when
/// its destinations are written, so the endpoint a response reports comes out of
/// the store too. Members the real service computes from infrastructure it owns
/// that this emulator does not provision (a VPC network interface, a
/// MediaConnect router output ARN) are NOT invented here.
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
    /// The system-generated static ingest address this input holds for each of
    /// its destination slots - `InputDestination$Ip`, which "remains fixed for
    /// the lifetime of the input".
    ///
    /// PRIVATE: the only way in is
    /// [`ComAmazonawsMedialiveMedialiveState::allocate_input_ingest_addresses`],
    /// which owns the pool and never reassigns a slot's address.
    ingest_ips: Vec<String>,
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
    /// than restarting the clock, and an input on its way out is no longer a
    /// current user of the resources it referenced
    /// ([`ComAmazonawsMedialiveMedialiveState::inputs_using_security_group`]).
    pub fn is_delete_accepted(&self) -> bool {
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
        ingest_ips: Vec::new(),
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
/// Accepting the delete also RELEASES the ingest endpoints the input held, which
/// is why no read of it reports a destination afterwards
/// ([`described_input_destinations`]).
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

/// An input's `Destinations` — "A list of the destinations of the input
/// (PUSH-type)" — as they stand at the moment of the read.
///
/// A destination is an ingest ENDPOINT the service HOLDS for the input, not
/// merely configuration the caller handed it: the address behind it is
/// system-allocated and "remains fixed for the lifetime of the input"
/// (`InputDestination$Ip`), and something listens on it for the stream an
/// upstream system pushes. An input whose delete this service has accepted is
/// being torn down — `DELETING` on its way to the terminal `DELETED` — so those
/// endpoints are released and no read reports any of them: nothing ingests
/// through them any more, and a client polling `DescribeInput` to learn the
/// input has been released must not be handed an endpoint to keep pushing to.
///
/// Caller-supplied configuration (`Sources`, `SecurityGroups`, `Tags`, …) is NOT
/// released this way: it is what the record says the input was, and the deleted
/// record goes on reporting it.
///
/// The one projection that reports a released endpoint is the delete's own
/// response, which answers with the input it deleted — see
/// [`ComAmazonawsMedialiveMedialiveState::delete_input_description`].
fn described_input_destinations(input: &InputRecord) -> Vec<InputDestination> {
    if input.is_delete_accepted() {
        return Vec::new();
    }
    input
        .destinations
        .iter()
        .enumerate()
        .map(|(index, destination)| described_input_destination(input, index, destination))
        .collect()
}

/// `InputDestinationRequest` -> `InputDestination`, for the destination at
/// `index` of `input`'s destination list.
///
/// `Ip` is the request's `StaticIpAddress` when it named one (the address an
/// ON-PREM push input takes on the customer network), otherwise the
/// system-generated address allocated for that destination slot. `Port` and
/// `Url` are the ingest endpoint of the input type's push protocol, so a client
/// creating a push input learns where to push the stream; an input type with no
/// push protocol ([`PUSH_INGEST_PROTOCOLS`]) reports neither.
///
/// The three endpoint members are reported together or not at all: without an
/// address there is no endpoint to announce, so no port is claimed for one
/// either.
///
/// `Vpc` describes the network interface the real service provisions in the
/// customer VPC, which this emulator does not, so it claims none of it.
fn described_input_destination(
    input: &InputRecord,
    index: usize,
    destination: &InputDestinationRequest,
) -> InputDestination {
    let ip = destination
        .static_ip_address
        .clone()
        .filter(|address| !address.is_empty())
        .or_else(|| input.ingest_ips.get(index).cloned());
    let endpoint = push_ingest_protocol(input.input_type.as_deref()).zip(ip.as_deref());
    InputDestination {
        port: endpoint.map(|(protocol, _)| protocol.port.to_string()),
        url: endpoint
            .map(|(protocol, ip)| protocol.endpoint_url(ip, destination.stream_name.as_deref())),
        ip,
        vpc: None,
        network: destination.network.clone(),
        // A list member of the projection is reported as present and empty
        // rather than unset when the input has no routes, like every other list
        // in the `Input` shape.
        network_routes: Some(
            destination
                .network_routes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|route| InputDestinationRoute {
                    cidr: route.cidr.clone(),
                    gateway: route.gateway.clone(),
                })
                .collect(),
        ),
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

/// The `State` of `group`, whose current users are `inputs` and `channels`.
///
/// `DELETED` once `DeleteInputSecurityGroup` has deleted it — the terminal
/// state, which a deleted group keeps, so a client polling for it sees it and
/// one that treats `IDLE` as "available" cannot attach a deleted group.
/// Otherwise the state the group's users imply
/// ([`input_security_group_usage_state`]). Derived from the store on every read,
/// never written at create time.
fn input_security_group_state_for(
    group: &InputSecurityGroupRecord,
    inputs: &[String],
    channels: &[String],
) -> &'static str {
    if group.is_deleted() {
        input_security_group_state::DELETED
    } else {
        input_security_group_usage_state(inputs, channels)
    }
}

/// The `State` the group's users alone imply: `IN_USE` while some resource
/// references it and `IDLE` while none does, which is what
/// `InputSecurityGroup$State` reports ("The current state of the Input Security
/// Group").
fn input_security_group_usage_state(inputs: &[String], channels: &[String]) -> &'static str {
    if inputs.is_empty() && channels.is_empty() {
        input_security_group_state::IDLE
    } else {
        input_security_group_state::IN_USE
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
/// their own issues land).
///
/// `DeleteInput` serialises through `DescribeInputOutput` itself — see the note
/// on that handler — so the deleted input it answers with is the same projection
/// a later `DescribeInput` reports.
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

/// Project an `InputSecurityGroup` onto one of the structurally identical
/// per-operation FLAT group output shapes (`DescribeInputSecurityGroupOutput`;
/// `DeleteInputSecurityGroup` serialises through that same shape — see the note
/// on that handler).
///
/// One macro rather than a hand-written copy of the same seven-member move per
/// operation, so the operations that report a group flat cannot drift from each
/// other. The operations whose model WRAPS the group in a `SecurityGroup` member
/// (`CreateInputSecurityGroup`, `UpdateInputSecurityGroup`) use the
/// `InputSecurityGroup` shape itself and so need no projection.
macro_rules! input_security_group_description_output {
    ($out:ident, $group:expr) => {{
        let group: $crate::ops::types::InputSecurityGroup = $group;
        $out {
            arn: group.arn,
            id: group.id,
            inputs: group.inputs,
            state: group.state,
            tags: group.tags,
            whitelist_rules: group.whitelist_rules,
            channels: group.channels,
        }
    }};
}

pub(crate) use input_security_group_description_output;

/// Project a stored [`EventBridgeRuleTemplateGroupRecord`] onto one of the
/// structurally identical per-operation group output shapes
/// (`CreateEventBridgeRuleTemplateGroupOutput`,
/// `GetEventBridgeRuleTemplateGroupOutput` and
/// `UpdateEventBridgeRuleTemplateGroupOutput`).
///
/// One macro rather than a hand-written copy per operation, so the operations
/// that report a group cannot drift from each other. `Tags` is always reported,
/// as the map it is modelled as; see [`template_group_output`].
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
/// carrying its own copy of the projection, which could then drift.
///
/// `Tags` reports the group's tag set WHATEVER it holds — an untagged group
/// reports the empty map, not an absent member. The member is modelled as a
/// `TagMap`, and a resource that carries no tags carries zero tags rather than
/// an unknown number of them, so a caller that iterates the map sees an empty
/// map instead of a null it has to special-case. It is also how this service's
/// own signal maps already report their tags
/// ([`ComAmazonawsMedialiveMedialiveState::signal_map_description`]), so one
/// resource of this service cannot answer "no tags" differently from another.
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
            tags: Some(group.tags.clone()),
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
