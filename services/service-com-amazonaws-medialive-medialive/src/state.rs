// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-medialive-medialive service.
//!
//! The store is PARTITIONED BY REGION
//! ([`ComAmazonawsMedialiveMedialiveStore`]): MediaLive is a regional service,
//! so every resource it mints belongs to the region whose endpoint created it —
//! the region its ARN names — and the state an operation is handed
//! ([`service_state`]) is the partition of the region ITS request was signed
//! for. Everything below therefore describes ONE region's resources, and a
//! reader, a mutation or a reference that resolves an id resolves it in that
//! region alone.
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
//! ([`MonitorDeploymentRecord`]) beside the latest one that succeeded, which is
//! how both the signal map's `MonitorDeployment` and its summary's
//! `MonitorDeploymentStatus` are reported out of state. A deployment is of one
//! of the three kinds its [`MonitorDeploymentKind`] names: the real deployment
//! and the dry run `StartMonitorDeployment` starts, and the deletion of the
//! monitor `StartDeleteMonitorDeployment` starts — one record either way, so a
//! signal map has exactly one latest monitor deployment whatever started it.
//!
//! The MediaLive Anywhere network lives here too: one [`NetworkRecord`] per
//! created network, its own id counter and its own token map. A network's
//! `State` is the one lifecycle member it owes the clock (`CREATING` until the
//! creation comes due, then `ACTIVE`; `UPDATING` until an accepted settings
//! change comes due, then `ACTIVE` again; `DELETING` until an accepted delete
//! comes due, then the terminal `DELETED`) and its `AssociatedClusterIds` are derived
//! from the rest of the state on every read, never recorded at create time.
//!
//! The INPUT DEVICES this account has claimed live here as well: one
//! [`InputDeviceRecord`] per claimed device, keyed by the device id the claim
//! named rather than by an id this service mints, because a Link unit's id is
//! its own. A device's `DeviceUpdateStatus` comes out of the maintenance window
//! `StartInputDeviceMaintenanceWindow` opens on it, its `InputDeviceState` out
//! of the `StartInputDevice` / `StopInputDevice` commands it has accepted, and
//! its `OutputType` and
//! `MedialiveInputArns` out of the inputs this store holds — see
//! [`input_devices`]. A device also carries the TRANSFER pending on it, which
//! `TransferInputDevice` records, `ListInputDeviceTransfers` reports in the
//! direction the reader asks for, `AcceptInputDeviceTransfer` ends for the
//! account the transfer names as its recipient, `RejectInputDeviceTransfer`
//! ends for that same recipient when it refuses the device instead, and
//! `CancelInputDeviceTransfer`
//! ends for the account that requested it, and the latest THUMBNAIL it has
//! sent, which `DescribeInputDeviceThumbnail` reports — one only a connected
//! Link unit sends, so a device claimed through this emulator's API has none.
//!
//! The input security group lifecycle lives here too: one
//! [`InputSecurityGroupRecord`] per created group and the counter that mints its
//! `Id`. A group's `Inputs` and `Channels` are derived from the rest of the
//! state on every read, never recorded at create time, and so is its `State` —
//! out of what uses the group and out of whether `DeleteInputSecurityGroup` has
//! deleted it.
//!
//! The MediaLive Anywhere NODE lives in its own module ([`nodes`]): one
//! [`NodeRecord`] per created node, keyed by its minted `Id` and carrying the
//! cluster the create was addressed to, because a node's id is "Unique in the
//! Cluster" and every node operation names both. Its deletion is a
//! DEREGISTRATION — the state its enum models — so a deleted node ends in
//! `DEREGISTERED` rather than in a `DELETED` the model does not declare.
//!
//! A delete never drops a record: a deleted channel, cluster, input, input
//! security group, network or SdiSource keeps its record in the terminal
//! `DELETED` state its own
//! state enum models, so a reader goes on reporting it there (which is what the
//! SDK's `ChannelDeleted` / `InputDeleted` waiters poll for) instead of losing
//! it. The `DELETING` step a channel's, a cluster's, an input's and a network's
//! enum models is
//! what the clock owes between the call and that terminal state, and a resource in
//! `DELETED` is no longer *currently using* anything, so it leaves the derived
//! `Inputs`, `Channels`, `AttachedChannels`, `MedialiveInputArns` and
//! `AssociatedClusterIds` lists. That terminal state is the ONE cut point every
//! one of those lists applies: a resource the delete has only been ACCEPTED for
//! is still a resource this account holds, so it goes on being reported as a
//! current user until the clock settles it.
//!
//! A kept record is READABLE, not writable: once a delete has been accepted for
//! an id, that id names no resource a caller may still change. Every MUTATION
//! therefore resolves its id through the live lookups
//! ([`ComAmazonawsMedialiveMedialiveState::live_input`],
//! [`ComAmazonawsMedialiveMedialiveState::live_cluster`],
//! [`ComAmazonawsMedialiveMedialiveState::mutable_channel`] over
//! [`ComAmazonawsMedialiveMedialiveState::live_channel_mut`], and the tag
//! writers' own private `resource_tags_mut`) and answers a dead id with its own
//! modelled not-found error. That resolution comes FIRST, before the mutation
//! looks at the resource's state at all, so a tombstone is answered as not found
//! and never with the state conflict a resource that still exists gets, while a
//! read goes on reporting the record in its terminal state.
//!
//! The INPUT SECURITY GROUP is the one resource here whose delete is a SOFT
//! delete: it only drops the group from `ListInputSecurityGroups`, leaving the
//! group individually addressable AND still mutable. So a mutation of a group
//! resolves its id across the whole
//! [`ComAmazonawsMedialiveMedialiveState::input_security_groups`] store and
//! refuses only an id this account never held.
//! [`ComAmazonawsMedialiveMedialiveState::live_input_security_group`] is what a
//! request REFERENCING a group resolves through instead: changing a deleted
//! group is allowed, taking a new dependency on one is not.
//!
//! A channel's schedule lives on its own [`ChannelRecord`], as the
//! [`ChannelSchedule`] of the [`schedules`] slice: the actions
//! `BatchUpdateSchedule` created and `DeleteSchedule` clears, swept of the ones
//! whose start time is over an hour past whenever [`ComAmazonawsMedialiveMedialiveState::settle`]
//! runs.
//!
//! A channel's ALERTS live on its own [`ChannelRecord`] too, as the
//! [`ChannelAlerts`] log of the [`channel_alerts`] slice: the alerts the service
//! has observed on that channel, which `ListAlerts` reports. Nothing in this
//! emulator ingests media, so no operation observes an alert condition and the
//! log stays empty unless [`ChannelRecord::raise_alert`] is called — a read
//! reports what the log holds for the pipelines the channel's class gives it
//! ([`ChannelRecord::reportable_alerts`]) and never invents an alert for a
//! channel that is merely running.
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
//! The ACCOUNT's own MediaLive settings live here too, beside the region
//! partitions rather than inside one:
//! one [`AccountConfigurationRecord`] per region an `UpdateAccountConfiguration`
//! has written one in. That is not a resource a caller creates — the account
//! always has a configuration — so a region nothing has configured yet reads as
//! the unset configuration rather than as missing. See the
//! [`account_configuration`] slice.
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
//! The CHANNEL PLACEMENT GROUPS those clusters hold live here as well: one
//! [`ChannelPlacementGroupRecord`] per created group, its own id counter and its
//! own token map, each carrying the id of the cluster it was created in, so the
//! (`ClusterId`, `ChannelPlacementGroupId`) pair every one of its operations
//! names it by resolves out of state. A group's `Channels` is derived from the
//! channels this store already holds — the ones whose
//! `AnywhereSettings$ChannelPlacementGroupId` names the group — and its `State`
//! from the node assignment the clock owes it
//! (`ASSIGNING` -> `ASSIGNED`, `UNASSIGNING` -> `UNASSIGNED`).
//!
//! Tags are NOT a store of their own: every resource record above carries its
//! own tag map, and the tag operations reach the map of the resource their
//! `ResourceArn` names through
//! [`ComAmazonawsMedialiveMedialiveState::resource_tags`] (reading),
//! [`ComAmazonawsMedialiveMedialiveState::insert_resource_tags`] and
//! [`ComAmazonawsMedialiveMedialiveState::remove_resource_tags`] (writing), so a
//! tag written by `CreateTags` is the tag that resource's own describe reports.
//! The two writers own the per-resource tag bounds ([`MAX_RESOURCE_TAGS`],
//! [`MAX_TAG_KEY_LEN`], [`MAX_TAG_VALUE_LEN`]) and the reserved key namespace
//! ([`RESERVED_TAG_KEY_PREFIX`]), which is why the map itself is only ever
//! handed out mutably inside this module. An operation that stores a tag map a
//! request carried without going through them (a create, an update) refuses the
//! reserved namespace with [`reserved_tag_key_in`] before it writes.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AnywhereSettings, CdiInputSpecification, Channel, ChannelEgressEndpoint, ChannelSummary,
    DescribeAnywhereSettings, DescribeFollowerChannelSettings, DescribeInferenceSettings,
    DescribeLinkedChannelSettings, DescribeNetworkSummary, DescribePrimaryChannelSettings,
    EncoderSettings, EventBridgeRuleTemplateGroupSummary,
    InferenceSettings, Input, InputAttachment, InputDestination, InputDestinationRequest,
    InputDestinationRoute, InputDeviceSettings, InputSecurityGroup, InputSource,
    InputSourceRequest, InputSpecification, InputVpcRequest, InputWhitelistRule,
    InputWhitelistRuleCidr, IpPool,
    LinkedChannelSettings, MaintenanceCreateSettings, MaintenanceStatus, MediaConnectFlow,
    MediaConnectFlowRequest,
    MediaResource, MediaResourceNeighbor, MonitorDeployment, MulticastSettings,
    MulticastSettingsCreateRequest, MulticastSource, OutputDestination, PipelineDetail,
    Route,
    RouterDestination, RouterInputSettings, RouterSettings, SdiSource, SdiSourceSummary,
    Smpte2110ReceiverGroupSettings, SignalMapSummary, SpecialRouterSettings,
    SrtCallerDecryption, SrtCallerSource, SrtListenerDecryption, SrtListenerSettings,
    SrtSettings, SrtSettingsRequest, SuccessfulMonitorDeployment,
    ValidationError, VpcOutputSettings, VpcOutputSettingsDescription,
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

/// The channel CONFIGURATION slice: the one rule the operations that WRITE a
/// channel's configuration (`CreateChannel`, `UpdateChannel`) validate it
/// against before anything is stored, and the per-element problems a
/// configuration this service could not run is refused with.
pub mod channel_configurations;

pub use channel_configurations::ChannelConfiguration;

/// The channel ALERT slice: one channel's alert log, the state an alert is in,
/// the `StateFilter` a read narrows it with and the set-time order a read
/// reports it in. Its store is the `alerts` member of [`ChannelRecord`] below,
/// for the same reason as the schedule: an alert belongs to exactly one channel.
pub mod channel_alerts;

pub use channel_alerts::{
    channel_alert_output, channel_alert_state, ChannelAlertRecord, ChannelAlerts,
    CHANNEL_ALERT_FILTERS, CHANNEL_ALERT_FILTER_ALL, MAX_CHANNEL_ALERTS,
};

pub use alarm_template_groups::{
    AlarmTemplateGroupRefusal, CloudWatchAlarmTemplateGroupRecord,
    MAX_ALARM_TEMPLATE_GROUPS, MAX_ALARM_TEMPLATE_GROUP_TOKENS,
};

/// The ACCOUNT CONFIGURATION slice: the per-region record this account's
/// `UpdateAccountConfiguration` writes and `DescribeAccountConfiguration` reads,
/// the shape of KMS key ARN it accepts and the bound on the store, in one module
/// of its own for the same reason as [`alarm_template_groups`]. What stays in
/// this file is the part the rest of the state needs: the
/// `account_configurations` store member on
/// [`ComAmazonawsMedialiveMedialiveStore`].
pub mod account_configuration;

pub use account_configuration::{
    is_kms_key_arn, medialive_managed_kms_key_arn, AccountConfigurationRecord,
    MAX_ACCOUNT_CONFIGURATION_REGIONS,
};

pub use schedules::{
    parse_action_time, AppliedSchedule, ChannelSchedule, ChannelScheduleScope, ScheduleActionRecord,
    ScheduleRefusal, MAX_SCHEDULE_ACTIONS, SCHEDULE_ACTION_RETENTION,
};

pub(crate) use alarm_template_groups::alarm_template_group_output;

/// The channel placement group slice: its record, its bounds, its refusals, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`].
pub mod channel_placement_groups;

pub use channel_placement_groups::{
    channel_placement_group_state, too_many_nodes_message, unassociable_node_message,
    ChannelPlacementGroupChanges, ChannelPlacementGroupDescription, ChannelPlacementGroupRecord,
    ChannelPlacementGroupRefusal, ChannelPlacementGroupUpdateRefusal,
    MAX_CHANNEL_PLACEMENT_GROUPS, MAX_CHANNEL_PLACEMENT_GROUP_NODES,
    MAX_CHANNEL_PLACEMENT_GROUP_TOKENS,
};

pub(crate) use channel_placement_groups::channel_placement_group_output;

/// The cloudwatch alarm TEMPLATE slice: its record, its bounds, its refusal, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the per-region token key every
/// monitor template resource shares.
pub mod alarm_templates;

pub use alarm_templates::{
    alarm_template_summary, check_alarm_template_settings, template_scope, AlarmTemplateRefusal,
    AlarmTemplateSettings, AlarmTemplateUpdateRefusal, CloudWatchAlarmTemplateChanges,
    CloudWatchAlarmTemplateRecord, NewCloudWatchAlarmTemplate, MAX_ALARM_PERIOD_SECONDS,
    MAX_ALARM_TEMPLATES, MAX_ALARM_TEMPLATE_TOKENS, MAX_METRIC_NAME_LEN, MIN_ALARM_DATAPOINTS,
    MIN_ALARM_PERIOD_SECONDS,
};

pub(crate) use alarm_templates::alarm_template_output;

/// The AWS-PROVIDED monitor template catalog slice: the cloudwatch alarm
/// template groups the service publishes, the templates inside them and the
/// records a region's partition is seeded with, in one module of its own for the
/// same reason as the two slices above. What stays in this file is the part the
/// rest of the state needs: the seeding of a partition
/// ([`ComAmazonawsMedialiveMedialiveState::with_mints`]) and the predicate every
/// writer checks before it modifies a monitor template resource
/// ([`is_published_id`]).
pub mod managed_monitor_templates;

pub use managed_monitor_templates::{
    is_published_id, listing_position, published_groups, published_resource_message,
    published_templates, service_owned_arn, PublishedWriteRefusal, CATALOG_PUBLISHED_AT,
};

/// The multiplex slice: its record, its bounds, its refusals, the store's
/// methods and its output projection, in one module of its own for the same
/// reason as the alarm template group slice above. The PROGRAMS a multiplex
/// holds are part of that slice, and so are the ALERTS raised on it: both live
/// on the multiplex record that owns them. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`].
pub mod multiplexes;

pub use multiplexes::{
    multiplex_alert_state, multiplex_alert_type, multiplex_program_not_found_message,
    multiplex_state,
    MultiplexProgramDeleteRefusal, MultiplexProgramRecord, MultiplexProgramRefusal,
    MultiplexProgramSettingsRefusal, MultiplexProgramUpdateRefusal, MultiplexRecord,
    MultiplexRefusal, MultiplexSettingOutOfRange, MultiplexStartRefusal, MultiplexStopRefusal,
    MultiplexUpdateRefusal, MAX_MULTIPLEXES,
    MAX_MULTIPLEX_ALERTS,
    MAX_MULTIPLEX_PROGRAMS,
    MAX_MULTIPLEX_PROGRAM_TOKENS,
    MAX_MULTIPLEX_TOKENS, MAX_SERVICE_DESCRIPTOR_NAME_LEN, MULTIPLEX_ALERT_STATE_FILTERS,
    MULTIPLEX_ALERT_STATE_FILTER_ALL, MULTIPLEX_AVAILABILITY_ZONES,
};

pub(crate) use multiplexes::{multiplex_description_output, multiplex_program_description_output};

/// The NODE slice — the MediaLive Anywhere Node: its record, its bounds, its
/// refusals, the store's methods and its output projection, in one module of its
/// own for the same reason as the multiplex slice above. What stays in this file
/// is the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the node's entry in the ARN
/// resolution the tag operations share ([`TaggedResource`]).
pub mod nodes;

pub use nodes::{
    card_number_out_of_range_message, node_not_idle_message, node_state,
    too_many_interface_mappings_message, too_many_sdi_source_mappings_message, NodeChanges,
    NodeDeleteRefusal, NodeRecord, NodeRefusal, NodeStateRefusal, NodeUpdateRefusal,
    FIRST_SDI_CARD_NUMBER, MAX_NODES, MAX_NODE_INTERFACE_MAPPINGS, MAX_NODE_SDI_SOURCE_MAPPINGS,
    MAX_NODE_TOKENS, NODE_CONNECTION_STATE, SDI_SOURCE_MAPPING_MEMBERS_MESSAGE,
};

pub(crate) use nodes::node_description_output;

/// The node REGISTRATION SCRIPT slice — the artifact
/// `CreateNodeRegistrationScript` issues for a cluster, in a module of its own
/// beside the [`nodes`] slice it registers nodes into. What stays in this file is
/// the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`].
pub mod node_registration_scripts;

pub use node_registration_scripts::{
    NewNodeRegistrationScript, NodeRegistrationScriptRecord, NodeRegistrationScriptRefusal,
    MAX_NODE_REGISTRATION_SCRIPTS, MAX_NODE_REGISTRATION_SCRIPT_TOKENS,
};


/// The eventbridge rule TEMPLATE slice: its record, its bounds, its refusal, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the per-region token key every
/// monitor template resource shares.
pub mod event_bridge_rule_templates;

pub use event_bridge_rule_templates::{
    event_bridge_rule_template_summary, event_targets_from_request, is_event_target_arn,
    is_event_type,
    EventBridgeRuleTemplateChanges, EventBridgeRuleTemplateRecord,
    EventBridgeRuleTemplateRefusal, EventBridgeRuleTemplateUpdateRefusal,
    NewEventBridgeRuleTemplate, EVENT_BRIDGE_RULE_TEMPLATE_EVENT_TYPES,
    MAX_EVENT_BRIDGE_RULE_TEMPLATES, MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS,
    MAX_EVENT_BRIDGE_RULE_TEMPLATE_TOKENS, MAX_EVENT_TARGET_ARN_LEN,
};

pub(crate) use event_bridge_rule_templates::event_bridge_rule_template_output;

/// The alert vocabulary every alert surface of this service shares: the two
/// states an alert is in, the `StateFilter` values each listing documents, the
/// membership rule that refuses a value outside them and the sentence that
/// refusal carries, and the two derivations (`State` out of the cleared instant,
/// and which alerts a filter admits) the cluster slice and the multiplex slice
/// both delegate to.
pub mod alerts;

pub use alerts::{
    alert_admitted_by, alert_state, alert_state_filter, alert_state_of,
    undocumented_alert_state_filter_message, ALERT_STATE_FILTERS, ALERT_STATE_FILTER_ALL,
};

/// The input device slice: the record `ClaimDevice` registers, the device-id
/// form it accepts, the maintenance window
/// `StartInputDeviceMaintenanceWindow` opens and the projection every operation
/// that reports a device reads, in one module of its own for the same reason as
/// the slices above. What stays in this file is the part the rest of the state
/// needs: the store member on [`ComAmazonawsMedialiveMedialiveState`] and the
/// ARN resolution the tag operations share.
pub mod input_devices;

pub use input_devices::{
    configurable_settings_error, input_device_transfer_type, input_device_type_for_id,
    is_account_id, not_a_device_id_message, not_an_account_id_message, reboot_input_device_force,
    repeated_audio_channel_pair_message, settings_member_mismatch_message,
    InputDeviceConfiguration, InputDeviceRecord,
    InputDeviceThumbnailRecord, InputDeviceTransferRecord, TransferAcceptance, TransferCancellation,
    TransferRejection,
    FIRST_AUDIO_CHANNEL_PAIR_ID, LAST_AUDIO_CHANNEL_PAIR_ID, MAX_AUDIO_CHANNEL_PAIRS,
    MAX_INPUT_DEVICES,
    MAX_INPUT_DEVICE_THUMBNAIL_BYTES, MAX_TRANSFER_MESSAGE_CHARS,
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
    cluster_alert_output, cluster_alert_state, cluster_not_idle_message, cluster_state,
    described_cluster_network_settings, mapped_network_member,
    too_many_cluster_interface_mappings_message, ClusterAlertRecord, ClusterChanges,
    ClusterDeleteRefusal, ClusterRecord, ClusterRefusal, ClusterUpdateRefusal, NewCluster,
    CLUSTER_ALERT_FILTERS, CLUSTER_ALERT_FILTER_ALL, MAX_CLUSTERS, MAX_CLUSTER_ALERTS,
    MAX_CLUSTER_INTERFACE_MAPPINGS, MAX_CLUSTER_TOKENS,
};

pub(crate) use clusters::cluster_description_output;

/// The thumbnail slice: the frames a channel's encoder pipelines captured, the
/// bound on that log, the derived reader `DescribeThumbnails` reports through
/// and the store's one writer, in one module of its own for the same reason as
/// the cluster slice above. Its store is the `thumbnails` member of
/// [`ChannelRecord`] below, so a capture log goes away with the channel that
/// owns it.
pub mod thumbnails;

pub use thumbnails::{
    thumbnail_detail_output, thumbnail_state, ThumbnailRecord, MAX_CHANNEL_THUMBNAILS,
};

/// The OFFERING slice: the catalog of reserved resources available for purchase
/// that `ListOfferings`, `DescribeOffering` and `PurchaseOffering` all answer
/// from, in one module of its own for the same reason as the slices above. It is
/// a catalog rather than a store — an offering is not a resource a caller
/// creates — so it adds no member to [`ComAmazonawsMedialiveMedialiveState`];
/// what it takes FROM the state is the configuration of a stored channel, which
/// is what the `ChannelConfiguration` filter matches offerings against.
pub mod offerings;

pub use offerings::{
    catalog, offering_by_id, offering_id, offering_index, offering_page,
    reservation_resource_type, specification_filter_refusal, ChannelOfferingConfiguration,
    OfferingFilters, OfferingSpec, OFFERING_CURRENCY_CODE, OFFERING_DURATION,
    OFFERING_DURATION_UNITS, OFFERING_FIXED_PRICE, OFFERING_TYPE,
};

/// The RESERVATION slice: the reserved resources the account has bought against
/// the [`offerings`] catalog, in one module of its own for the same reason as the
/// slices above. Its store is the [`ComAmazonawsMedialiveMedialiveState::reservations`]
/// member below, and every member a reservation reports about what it reserves is
/// read from the catalog entry the purchase named.
pub mod reservations;

pub use reservations::{
    reservation_not_expired_message, reservation_start, reservation_state, reservation_time,
    NewReservation, ReservationDeleteRefusal, ReservationRecord, ReservationRefusal,
    ReservationStartRefusal, ReservationUpdate, DEFAULT_AUTOMATIC_RENEWAL, MAX_RESERVATIONS,
};

/// The ENGINE VERSION slice: the encoder engine versions available to this
/// account, which `ListVersions` reports, in one module of its own for the same
/// reason as the slices above. Its store is the
/// [`ComAmazonawsMedialiveMedialiveState::channel_engine_versions`] member below,
/// seeded when the store comes up because an engine version is not a resource a
/// caller creates, and read through at every call because which of those versions
/// is still available is a question for the clock.
pub mod engine_versions;

pub use engine_versions::{
    channel_engine_version_label, requested_engine_version_pin, ChannelEngineVersionCatalog,
    ChannelEngineVersionRecord, CHANNEL_ENGINE_VERSION_COUNT, CHANNEL_ENGINE_VERSION_INTERVAL,
    CHANNEL_ENGINE_VERSION_LIFETIME, DEFAULT_ENGINE_VERSION,
};

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
    pub const UPDATING: &str = "UPDATING";
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
/// Only the states the create, the update and the delete lifecycles reach are
/// named here.
/// `CreateNetwork` is asynchronous — `State` is documented "The current state of
/// the Network. Only MediaLive Anywhere can change the state" — so a network
/// starts `CREATING` and the clock owes it `ACTIVE`, the one state the enum names
/// for a network that exists, has not failed and nothing has deleted. Changing a
/// network's settings is asynchronous for the same reason, so an accepted
/// `UpdateNetwork` puts the network in `UPDATING` and the clock owes it `ACTIVE`
/// again ([`NetworkRecord::begin_update`]). The delete is asynchronous too, so an
/// accepted `DeleteNetwork` puts the network in `DELETING` and the clock owes it
/// the terminal `DELETED` the record then keeps ([`begin_delete_network`]). The
/// two `*_FAILED` states belong to failures this emulator does not inject.
/// `IDLE` and `IN_USE` are the
/// two the model documents nothing about for a network (`State` carries the one
/// sentence above and no member says what puts a network in either), so this
/// emulator never claims one: a network's attachments are reported through its
/// `AssociatedClusterIds`
/// ([`ComAmazonawsMedialiveMedialiveState::clusters_using_network`]) rather than
/// guessed at through its state.
pub mod network_state {
    pub const CREATING: &str = "CREATING";
    pub const ACTIVE: &str = "ACTIVE";
    pub const UPDATING: &str = "UPDATING";
    pub const DELETING: &str = "DELETING";
    pub const DELETED: &str = "DELETED";
}

/// The `ChannelClass` enum values (`smithy_query(query="shape",
/// shape="ChannelClass")`: STANDARD | SINGLE_PIPELINE).
pub mod channel_class {
    pub const STANDARD: &str = "STANDARD";
    pub const SINGLE_PIPELINE: &str = "SINGLE_PIPELINE";
}

/// The pipelines a channel of `channel_class` has, numbered as
/// [`PipelineDetail`]`$PipelineId` numbers them.
///
/// The ONE place the class-to-pipeline rule lives (the `ChannelClass` doc: "A
/// standard channel has two encoding pipelines and a single pipeline channel
/// only has one"): [`ChannelRecord::pipelines`] is this for the class a channel
/// HAS, and a write that would give it another class asks this for the class it
/// WOULD have.
pub fn class_pipelines(channel_class: &str) -> ::std::ops::Range<i32> {
    let count = if channel_class == channel_class::SINGLE_PIPELINE {
        1
    } else {
        2
    };
    0..count
}

/// The pipeline a `PipelineId` / `ChannelPipelineIdToRestart` value names, as
/// [`PipelineDetail`]`$PipelineId` numbers it.
///
/// Both enums declare exactly `PIPELINE_0` and `PIPELINE_1`
/// (`smithy_query(query="shape", shape="PipelineId")`, and the `PipelineIds`
/// member's own doc: "Format PIPELINE_0 or PIPELINE_1"), so they are matched
/// exactly rather than by parsing the suffix: a spelling the enums do not
/// declare names no pipeline. `None` for anything else, which every caller
/// refuses. Whether the pipeline it names is one the CHANNEL has is
/// [`ChannelRecord::has_pipeline`], since that depends on the channel's class.
pub fn pipeline_index(id: &str) -> Option<i32> {
    match id {
        "PIPELINE_0" => Some(0),
        "PIPELINE_1" => Some(1),
        _ => None,
    }
}

/// The `SignalMapStatus` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="SignalMapStatus")`:
/// CREATE_IN_PROGRESS | CREATE_COMPLETE | CREATE_FAILED | UPDATE_IN_PROGRESS |
/// UPDATE_COMPLETE | UPDATE_REVERTED | UPDATE_FAILED | READY | NOT_READY).
pub mod signal_map_status {
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    /// Where `StartUpdateSignalMap` leaves a signal map: the update "is
    /// initiated" and answers 202, so the change runs asynchronously and the
    /// clock settles it into [`UPDATE_COMPLETE`].
    pub const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
    pub const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
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
/// The `DELETE_*` pair belongs to `StartDeleteMonitorDeployment`, which
/// "initiates a deployment to delete the monitor of the specified signal map":
/// that deployment runs in `DELETE_IN_PROGRESS` and the clock settles it into
/// `DELETE_COMPLETE`, on the same terms as the deploying kinds above it.
///
/// The `*_FAILED` statuses are deliberately absent: nothing in this emulator
/// makes a deployment fail, so no code may report one as failed.
pub mod signal_map_monitor_deployment_status {
    pub const NOT_DEPLOYED: &str = "NOT_DEPLOYED";
    pub const DEPLOYMENT_IN_PROGRESS: &str = "DEPLOYMENT_IN_PROGRESS";
    pub const DEPLOYMENT_COMPLETE: &str = "DEPLOYMENT_COMPLETE";
    pub const DRY_RUN_DEPLOYMENT_IN_PROGRESS: &str = "DRY_RUN_DEPLOYMENT_IN_PROGRESS";
    pub const DRY_RUN_DEPLOYMENT_COMPLETE: &str = "DRY_RUN_DEPLOYMENT_COMPLETE";
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
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

/// The `InputNetworkLocation` value of an input that lives in the AWS Cloud —
/// "AWS, for an input existing in the AWS Cloud"
/// (`smithy_query(query="shape", shape="InputNetworkLocation")`, whose other
/// value is the on-premises one). This is the location whose ingest addresses
/// are this service's own public ones.
pub const AWS_INPUT_NETWORK_LOCATION: &str = "AWS";

/// The `InputNetworkLocation` an input that names none gets: the
/// `InputNetworkLocation` doc says "By default all inputs will continue to be
/// AWS inputs" (`smithy_query(query="shape", shape="InputNetworkLocation")`).
pub const DEFAULT_INPUT_NETWORK_LOCATION: &str = AWS_INPUT_NETWORK_LOCATION;

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

/// The `InputType` values whose ingest IS the `Sources` URL list — the PULL
/// types: `Sources` is "The source URLs for a PULL-type input. Every PULL type
/// input needs exactly two source URLs for redundancy. Only specify sources for
/// PULL type Inputs" (`smithy_query(query="operation", operation="CreateInput")`),
/// so an input of one of these types has nothing to ingest from until it carries
/// at least one source URL.
///
/// The two file types are the same pair [`DYNAMIC_CAPABLE_INPUT_TYPES`] names,
/// because what a dynamic URL varies is exactly such a source. Every other
/// `InputType` ingests through a list of its own (`Destinations` for a pushed-to
/// type, `MediaConnectFlows`, `InputDevices`, `SdiSources`, the multicast,
/// receiver-group and SRT settings), so none of them is in this table.
const PULL_SOURCE_INPUT_TYPES: [&str; 4] = ["URL_PULL", "RTMP_PULL", "MP4_FILE", "TS_FILE"];

/// Whether an input of `input_type` ingests through its `Sources`
/// ([`PULL_SOURCE_INPUT_TYPES`]).
fn pulls_from_sources(input_type: Option<&str>) -> bool {
    input_type.is_some_and(|input_type| PULL_SOURCE_INPUT_TYPES.contains(&input_type))
}

/// Base of the pool this service's emulated public addresses come from — a push
/// input's ingest address and a channel pipeline's egress address alike:
/// `198.18.0.0`, the first address of `198.18.0.0/15`, the RFC 2544
/// benchmarking range. Non-routable, so an emulated endpoint never hands a
/// caller an address that belongs to somebody.
const IP_POOL_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

/// `198.18.0.0/15` holds 2^17 addresses; allocation wraps inside it.
const IP_POOL_LEN: u32 = 1 << 17;

/// How long a resource stays in a transient state before the clock makes the next
/// one due: a channel's `CREATING`, `STARTING`, `STOPPING`, `RECOVERING` and
/// `DELETING`, a signal map's `CREATE_IN_PROGRESS` discovery, and a signal map's
/// monitor deployment, which `StartMonitorDeployment` starts in an IN_PROGRESS
/// status.
///
/// CHOSEN, not documented by AWS: the real service takes minutes, and no doc
/// sentence pins a number, so this is the emulator's own span. It is deliberately
/// non-zero so a caller that acts immediately after `CreateChannel` /
/// `StartChannel` / `CreateSignalMap` / `StartUpdateSignalMap` still observes
/// the transient state and the
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

/// The most `WhitelistRules` entries this emulator accepts on one input security
/// group.
///
/// The same ceiling as [`MAX_NETWORK_CIDR_ENTRIES`] and for the same reason: the
/// list is caller-chosen and the model targets it at a plain list with no length
/// constraint, so nothing else bounds it, and every entry costs heap on a stored
/// record that outlives the request plus a member in every response that reports
/// the group. Both operations that write a whitelist model
/// `BadRequestException`, which is the refusal for a longer list — the request is
/// what is out of bounds.
pub const MAX_WHITELIST_RULES: usize = MAX_NETWORK_CIDR_ENTRIES;

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

/// The most `Destinations` entries this emulator accepts on one channel.
///
/// A ceiling, not a guess at AWS's published quota, and the same kind of bound
/// as [`MAX_INPUT_ATTACHMENTS`] above: the list is caller-chosen and every read
/// that walks it costs work per entry — a signal map's discovery, and the
/// multiplex slice's feeder index, which `settle` builds out of these
/// destinations on every request that could raise a multiplex alert. Bounding it
/// keeps that per-request work a function of the emulator's own ceilings rather
/// than of one caller's request body. `CreateChannel` refuses a longer list with
/// its own modelled `BadRequestException`, because the request is what is out of
/// bounds. Enforced where a caller's destinations are STORED — `CreateChannel`,
/// `UpdateChannel` and `UpdateChannelClass` — because a handler that stores them
/// without applying this bound would stop the per-request cost the multiplex
/// slice's feeder index pays being bounded by this constant. Each words the
/// refusal with [`too_many_channel_entries_message`].
pub const MAX_CHANNEL_DESTINATIONS: usize = 32;

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

/// Check a string member against the length bounds its target shape names,
/// answering the message that refusal carries: the member, the bound and the
/// length that broke it.
///
/// ONE copy of that length rule and of its wording, so the operations that
/// check the same member of the same resource against the same bound — a
/// create and the update beside it — cannot drift into two readings of one
/// constraint. Length is counted in characters, which is the unit a Smithy
/// `@length` constraint counts in.
///
/// A member the request did not carry is `None` and asks for no change, so it
/// breaks no bound.
pub fn bounded_length(
    member: &str,
    value: Option<&str>,
    min: usize,
    max: usize,
) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let len = value.chars().count();
    if len >= min && len <= max {
        return Ok(());
    }
    Err(format!(
        "{member} must be between {min} and {max} characters long, but was {len}"
    ))
}

/// Check a numeric member against the range its target shape names, answering
/// the message that refusal carries: the member, the bound and the value that
/// broke it.
///
/// The counterpart of [`bounded_length`] for the integer members, and ONE copy
/// of that range rule and of its wording for the same reason: a create and the
/// update beside it check the same member of the same resource against the same
/// range, so neither can drift into its own reading of it.
///
/// A member the request did not carry is `None` and asks for no change, so it
/// breaks no bound.
pub fn bounded_integer(member: &str, value: Option<i32>, min: i32, max: i32) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if value >= min && value <= max {
        return Ok(());
    }
    Err(format!(
        "{member} must be between {min} and {max}, but was {value}"
    ))
}

/// Check a member against the closed set of values the model declares for it,
/// answering the message that refusal carries: the member, the catalog and the
/// value that is not in it.
///
/// ONE copy of that closed-set rule and of its wording, for the reason
/// [`bounded_length`] states: an enum member names a set the service can act on,
/// and every operation that writes that member is held to the same set.
///
/// A member the request did not carry is `None` and asks for no change, so it
/// names no value outside the set.
pub fn one_of(member: &str, values: &[&str], value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if values.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "{member} must be one of {}, but was {value}",
        values.join(", ")
    ))
}

/// Check a caller-chosen name that IS the identity of the resource it creates,
/// answering the message that refusal carries: the member and the rule it broke.
///
/// A name of this kind is what a later request uses to address the resource —
/// the key it is stored at, and the path segment the operations that name one
/// resource carry — so an empty value names no resource at all and nothing can
/// be stored under it: the request is refused with the operation's own modelled
/// client error instead of creating something no request can name again.
///
/// ONE copy of that rule and of its wording, for the reason [`bounded_length`]
/// states: every operation that keys a resource by a name the caller supplied is
/// held to it, so none can drift into its own reading of it.
pub fn non_empty_identity(member: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{member} must not be empty"));
    }
    Ok(())
}

/// Check a count against the floor the model's `__integerMin1` sets for it,
/// answering the message that refusal carries: the member and the count that
/// broke it.
///
/// ONE copy of that floor and of its wording, for the reason [`bounded_length`]
/// states: a purchase and the update beside it hold the same counts to the same
/// minimum. A member the request did not carry is not a count at all, so it is
/// not refused.
pub fn at_least_one(member: &str, value: Option<i32>) -> Result<(), String> {
    match value {
        Some(count) if count < 1 => Err(format!("{member} must be at least 1, got {count}")),
        _ => Ok(()),
    }
}

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

/// The tag-key prefix AWS keeps for its own, system-applied tags.
///
/// The same AWS tag restrictions reserve it ("Do not use the aws: prefix in your
/// tag names or values because it is reserved for AWS use. You can't edit or
/// delete tag names or values with this prefix"), so a tag key carrying it is
/// not a key a caller owns: a request asking to create, change or remove one is
/// refused and the resource's stored tags are left as they were.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The prefix is matched without regard to case, so the namespace cannot be
/// reached through a differently cased spelling of it.
pub fn is_reserved_tag_key(key: &str) -> bool {
    let mut characters = key.chars();
    RESERVED_TAG_KEY_PREFIX.chars().all(|expected| {
        characters
            .next()
            .is_some_and(|got| got.eq_ignore_ascii_case(&expected))
    })
}

/// The reserved key among `keys`, when they carry one.
///
/// The SMALLEST such key, so a request naming several is refused with the same
/// message however the map it came out of happened to be iterated.
pub fn first_reserved_tag_key<'k, I>(keys: I) -> Option<&'k str>
where
    I: IntoIterator<Item = &'k String>,
{
    keys.into_iter()
        .map(String::as_str)
        .filter(|key| is_reserved_tag_key(key))
        .min()
}

/// The reserved key a request's `Tags` member carries, when it carries one.
///
/// What an operation that stores a caller's whole tag map itself — a create, an
/// update — checks before it writes, so the reserved namespace is refused
/// wherever a tag key reaches the store and not only on the tag operations.
pub fn reserved_tag_key_in(tags: Option<&HashMap<String, String>>) -> Option<&str> {
    first_reserved_tag_key(tags.into_iter().flat_map(HashMap::keys))
}

/// What to tell a caller that named a key in the reserved namespace.
///
/// One wording for every operation that can be handed such a key, so the tag
/// operations and the creates cannot drift into several readings of one
/// restriction.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!(
        "Tag key '{key}' cannot start with '{RESERVED_TAG_KEY_PREFIX}'. \
         That prefix is reserved for AWS use."
    )
}

/// The largest page a list operation (`ListChannels`, `ListSdiSources`,
/// `DescribeSchedule`, …) answers with: the upper bound of the `MaxResults`
/// range AWS publishes for this service's list operations (1 to 1000). A
/// `MaxResults` within that range is the page size verbatim, a larger one is
/// clamped here, and a request that names none uses this bound — so a caller
/// that asks for more items than this emulator's stores can hold is never
/// handed a short page plus a continuation token. Finite, so one request cannot
/// ask the service to project an unbounded number of resources in one response.
pub const MAX_LIST_PAGE_SIZE: usize = 1000;

/// The page size one paginated request is served at, from the `MaxResults` it
/// carried — or the message its refusal carries when that member asks for no
/// items at all.
///
/// ONE copy of this page-size rule and of its wording, for the reason
/// [`bounded_length`] states: every paginated operation of this service cuts its
/// page by the same rule, so none of them can drift into its own reading of the
/// floor, of the ceiling or of what an absent member asks for. A `MaxResults`
/// below one asks for nothing and is refused by the floor [`at_least_one`]
/// states for a count; a larger one is the page size verbatim up to
/// [`MAX_LIST_PAGE_SIZE`], which is also what a request naming no `MaxResults`
/// is served at — so no single request can ask the service to project an
/// unbounded number of resources in one response.
///
/// The caller maps the message onto the modelled client error its own operation
/// declares, exactly as the operations wrapping [`bounded_length`] and
/// [`at_least_one`] do.
pub fn list_page_size(max_results: Option<i32>) -> Result<usize, String> {
    at_least_one("maxResults", max_results)?;
    Ok(max_results.map_or(MAX_LIST_PAGE_SIZE, |asked_for| {
        (asked_for as usize).min(MAX_LIST_PAGE_SIZE)
    }))
}

/// The number a value spells when it is written the ONE way this service renders
/// a number it minted — plain decimal digits, no sign and no leading zero — or
/// `None` when it is spelled any other way.
///
/// ONE copy of this rule and for the reason [`bounded_length`] states: every
/// `NextToken` and every numeric id this service hands out is rendered by
/// `to_string()` over the number itself, so the only spellings it can ever have
/// handed out are the ones that render back to the same string. A value whose
/// digits do parse but whose spelling differs — a leading `+`, a padding zero —
/// is a value this service never minted, so it continues no walk and names no
/// resource, and the reader that is handed it refuses it instead of honouring it
/// as the number it resembles. `str::parse` admits both of those spellings, so
/// no reader of a token or of a minted id decodes one with `parse` alone.
pub fn minted_number(value: &str) -> Option<u64> {
    let number: u64 = value.parse().ok()?;
    // The rendering the minting sites write is the only spelling of this number
    // the service hands out, so a value that is not that rendering is not one of
    // them, however it parses.
    (number.to_string() == value).then_some(number)
}

/// Where a paginated request resumes in a sequence its `NextToken` walks by
/// POSITION, and the page that cursor reports out of it.
///
/// ONE copy of this positional-cursor rule and of its wording, for the reason
/// [`bounded_length`] states and beside the page-size rule [`list_page_size`]
/// states: an operation of this service that pages over a sequence it DERIVES —
/// a channel's alert log, a cluster's, a channel's schedule — resumes at a place
/// in that sequence, because an entry of it carries nothing that identifies the
/// entry on its own, so none of them can drift into its own reading of what a
/// token means, of which token is refused or of when a token is handed out at
/// all.
///
/// The caller maps the refusal message onto the modelled client error its own
/// operation declares, exactly as the operations wrapping [`list_page_size`] do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListPosition {
    /// How many entries of the sequence the previous pages already reported.
    already_reported: usize,
    /// The page size [`list_page_size`] cut this request at.
    page_size: usize,
}

impl ListPosition {
    /// The cursor one request walks from, out of the `NextToken` it carried and
    /// the page size it is served at — or the message its refusal carries when
    /// that token is not a position at all.
    ///
    /// `position_of` is the noun phrase, carrying its article, naming what the
    /// positions count as the refusal reads it (`an alert`, `a schedule`).
    ///
    /// A request naming no token asks for the first page. A token that is not a
    /// whole number is NOT read as the first page: it continues nothing this
    /// service handed out, so it is refused — and so is a token that spells a
    /// whole number some other way than [`minted_number`] renders it, because
    /// that spelling is not one this service ever handed out either. A position
    /// PAST the end is not refused, because these sequences shrink on their own:
    /// a token handed out for a page that has since been dropped resumes at the
    /// end of the sequence instead of failing a walk that was valid when it
    /// began.
    pub fn read(
        next_token: Option<&str>,
        page_size: usize,
        position_of: &str,
    ) -> Result<Self, String> {
        let already_reported = match next_token {
            Some(token) => minted_number(token)
                .and_then(|position| usize::try_from(position).ok())
                .ok_or_else(|| format!("nextToken {token} is not {position_of} position"))?,
            None => 0,
        };
        Ok(Self {
            already_reported,
            page_size,
        })
    }

    /// The page of `sequence` this cursor reports — each entry carried into the
    /// shape the response reports it as by `report` — and the token that resumes
    /// after it.
    ///
    /// The token is the position the next page resumes at, handed out ONLY while
    /// an entry is left over after this page, so a response carrying none is the
    /// last page. A page is a snapshot of the sequence it was read from and never
    /// a cursor the service holds open: an entry dropped between two pages shifts
    /// the ones after it.
    pub fn page<Entry, Reported>(
        &self,
        sequence: &[Entry],
        report: impl FnMut(&Entry) -> Reported,
    ) -> (Vec<Reported>, Option<String>) {
        let page: Vec<Reported> = sequence
            .iter()
            .skip(self.already_reported)
            .take(self.page_size)
            .map(report)
            .collect();
        let reported = self.already_reported.saturating_add(page.len());
        let next_token = (reported < sequence.len()).then(|| reported.to_string());
        (page, next_token)
    }
}

/// The BATCH slice: the per-resource outcome machinery and the id-count bound
/// every Batch operation of this service answers by, in one module of its own so
/// the three of them cannot drift into three readings of one rule. What stays in
/// this file is the part the rest of the state needs: the per-operation caps
/// below, which [`batch_results::within_id_cap`] is handed.
pub mod batch_results;

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

/// The most resource ids ONE `BatchStop` call may name, counting its
/// `ChannelIds` and `MultiplexIds` together.
///
/// The same ceiling as [`MAX_BATCH_DELETE_IDS`] and for the same reason: the
/// model targets both members at plain `__listOf__string` with no length
/// constraint, and each named id costs a lookup and a projection under the state
/// guard plus one entry in the response. `BatchStop` models
/// `BadRequestException`, which is the refusal for a batch above this bound.
pub const MAX_BATCH_STOP_IDS: usize = MAX_BATCH_DELETE_IDS;

/// The most resource ids ONE `BatchStart` call may name, counting its
/// `ChannelIds` and `MultiplexIds` together.
///
/// The same ceiling as [`MAX_BATCH_STOP_IDS`] and for the same reason: the
/// model targets both members at plain `__listOf__string` with no length
/// constraint, and each named id costs a lookup and a projection under the state
/// guard plus one entry in the response. `BatchStart` models
/// `BadRequestException`, which is the refusal for a batch above this bound.
pub const MAX_BATCH_START_IDS: usize = MAX_BATCH_STOP_IDS;

/// The state a channel's in-flight transition settles into once it comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// `CREATING`, `STOPPING` and `UPDATING` settle into `IDLE`.
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
/// computes from infrastructure it owns (a VPC network interface id, an
/// engine-version expiry) are NOT invented here: they stay absent, because this
/// emulator has no such infrastructure to report. The two members the service
/// SELECTS for every channel — its maintenance window and the public address
/// each pipeline egresses from — are stored, because a channel always has them.
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
    /// The public address each of this channel's pipelines egresses from, in
    /// pipeline order — `ChannelEgressEndpoint$SourceIp`, "Public IP of where a
    /// channel's output comes from".
    ///
    /// PRIVATE: the only way in is
    /// [`ComAmazonawsMedialiveMedialiveState::allocate_channel_egress_addresses`],
    /// which owns the pool and never reassigns a pipeline's address, and the
    /// only way out is [`ChannelRecord::egress_endpoints`].
    egress_ips: Vec<String>,
    /// The MediaConnect Router handoff settings a request supplied but the
    /// `Channel` response shape reports no member for — stored rather than
    /// dropped, exactly as [`InputRecord::special_router_settings`] is for an
    /// input, so the member a caller wrote is the member the store holds.
    pub special_router_settings: Option<SpecialRouterSettings>,
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
    /// This channel's alert log, which `ListAlerts` reports: it lives on the
    /// channel for the same reason the schedule does. Empty until something
    /// raises an alert on this channel ([`Self::raise_alert`]); see the
    /// [`channel_alerts`] module for why nothing in this emulator does so on its
    /// own. Read for reporting through [`Self::reportable_alerts`], which is
    /// where an alert's pipeline is resolved against this channel's class.
    pub alerts: ChannelAlerts,
    /// The thumbnail frames this channel's pipelines have captured, oldest
    /// first — the [`thumbnails`] slice's store. Nothing in this emulator
    /// captures one, so this is empty unless
    /// [`ComAmazonawsMedialiveMedialiveState::capture_channel_thumbnail`] put
    /// something in it. Private, and written ONLY through
    /// [`ChannelRecord::push_thumbnail_bounded`] just below, which is where the
    /// bound ([`MAX_CHANNEL_THUMBNAILS`]) is enforced; the capture writer owns
    /// the conditions under which a frame can have been captured at all.
    thumbnails: Vec<ThumbnailRecord>,
    /// The transition the clock owes this channel, if any.
    transition: Option<Transition>,
    /// The instant this channel entered the state it is in NOW: the instant of
    /// the call for a state a mutation put it in, and the instant the
    /// transition came DUE for one the clock settled it into — never the
    /// instant of the call that happened to materialise that transition.
    ///
    /// PRIVATE, read through [`Self::state_since`]: whether this channel's
    /// pipelines are up is what the multiplex slice observes an alert condition
    /// about a multiplex program from, and such a condition became true (or
    /// stopped being true) when this channel last changed state, so this is the
    /// instant the alert it raises carries.
    state_since: SystemTime,
}

impl ChannelRecord {
    /// Append `thumbnail` as the newest frame of this channel's capture log,
    /// dropping the OLDEST frames first so the log never holds more than
    /// [`MAX_CHANNEL_THUMBNAILS`] of them.
    ///
    /// The clamp lives here, beside the private store it bounds and BEFORE the
    /// push, because the log is caller-driven — one frame per capture — so an
    /// unbounded log would grow the emulator's heap without end, exactly as
    /// [`MAX_CLUSTER_ALERTS`] bounds the cluster slice's alert log. The field is
    /// private and this is its only writer, so no caller can push past the
    /// bound.
    pub(crate) fn push_thumbnail_bounded(&mut self, thumbnail: ThumbnailRecord) {
        while self.thumbnails.len() >= MAX_CHANNEL_THUMBNAILS {
            self.thumbnails.remove(0);
        }
        self.thumbnails.push(thumbnail);
    }

    /// The number of encoding pipelines this channel's class gives it: two for
    /// `STANDARD`, one for `SINGLE_PIPELINE` (the `ChannelClass` doc: "A standard
    /// channel has two encoding pipelines and a single pipeline channel only has
    /// one").
    pub fn pipeline_count(&self) -> i32 {
        self.pipelines().end
    }

    /// The pipelines this channel's class gives it, numbered as
    /// `PipelineDetail$PipelineId` numbers them (the `ChannelClass` doc: a
    /// `STANDARD` channel has two, a `SINGLE_PIPELINE` channel one).
    ///
    /// Every other reading of the class-to-pipeline rule — whether a request may
    /// name a pipeline, which of them are up, how a schedule action's pause
    /// target is resolved — derives from here, which derives from the one place
    /// that rule lives ([`class_pipelines`]).
    pub fn pipelines(&self) -> ::std::ops::Range<i32> {
        class_pipelines(&self.channel_class)
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
        self.pipelines()
            .filter(|pipeline| !self.restarting_pipelines.contains(pipeline))
            .collect()
    }

    /// Record an alert the service has observed on this channel, and answer the
    /// `Id` it was given — or `None` when the observation names a pipeline this
    /// channel has not got.
    ///
    /// The one way an alert enters a channel's log, so the `PipelineId` rule
    /// holds for every alert in it: `ChannelAlert$PipelineId` is "The ID of the
    /// pipeline this alert is associated with", and a pipeline this channel's
    /// class does not give it ([`Self::has_pipeline`]) is not one an alert can be
    /// associated with — so there is no such alert to record, and the `None` says
    /// so rather than quietly recording one with the pipeline dropped.
    pub fn raise_alert(
        &mut self,
        alert_type: &str,
        message: Option<&str>,
        pipeline: Option<i32>,
        now: SystemTime,
    ) -> Option<String> {
        if pipeline.is_some_and(|pipeline| !self.has_pipeline(pipeline)) {
            return None;
        }
        Some(
            self.alerts
                .raise(&self.id, alert_type, message, pipeline, now),
        )
    }

    /// Whether `pipeline` is one of the pipelines this channel's class gives it
    /// ([`Self::pipelines`]), so a request may name it.
    pub fn has_pipeline(&self, pipeline: i32) -> bool {
        self.pipelines().contains(&pipeline)
    }

    /// The alerts of this channel's log that a `StateFilter` admits and this
    /// channel has the pipeline for, in the order the service set them — what
    /// `ListAlerts` reports.
    ///
    /// `ChannelAlert$PipelineId` is "The ID of the pipeline this alert is
    /// associated with", and the pipelines a channel has are the ones its class
    /// gives it ([`Self::pipelines`]), so the rule [`Self::raise_alert`] applies
    /// when an alert is recorded is applied again at every READING of the log:
    /// the class a later write stores is what decides which pipelines the
    /// channel has, and an alert associated with one it has not got describes
    /// nothing the channel is running. Whatever state the alert is in, since the
    /// pipeline it names is what the association is.
    ///
    /// The record itself is KEPT, exactly as the address a dropped pipeline
    /// egressed from is ([`Self::egress_endpoints`]) and as the frames it
    /// captured are ([`Self::latest_thumbnail`]), so a channel put back into the
    /// class that gives it that pipeline reports its alerts again.
    pub fn reportable_alerts(&self, filter: Option<&str>) -> Vec<&ChannelAlertRecord> {
        self.alerts
            .in_set_order(filter)
            .into_iter()
            .filter(|alert| {
                alert
                    .pipeline_id
                    .is_none_or(|pipeline| self.has_pipeline(pipeline))
            })
            .collect()
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

    /// The endpoints this channel's outgoing connections initiate from —
    /// `EgressEndpoints`, "The endpoints where outgoing connections initiate
    /// from", one `ChannelEgressEndpoint` per pipeline carrying the public
    /// address allocated to that pipeline.
    ///
    /// One entry per pipeline the channel's class gives it
    /// ([`Self::pipeline_count`]), in pipeline order, so a `STANDARD` channel
    /// reports two and a `SINGLE_PIPELINE` channel one — in every state,
    /// because the address a channel egresses from is provisioned with the
    /// channel and not with a run. A class change that drops a pipeline stops
    /// reporting its address but keeps it recorded, so changing back puts the
    /// same address back on the same pipeline.
    pub fn egress_endpoints(&self) -> Vec<ChannelEgressEndpoint> {
        let pipelines = usize::try_from(self.pipeline_count()).unwrap_or(0);
        self.egress_ips
            .iter()
            .take(pipelines)
            .map(|source_ip| ChannelEgressEndpoint {
                source_ip: Some(source_ip.clone()),
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

    /// Whether this channel's pipelines are UP on the hardware it is placed on:
    /// a start has been accepted for it and a stop has not finished, so it is
    /// coming up (`STARTING`), up (`RUNNING`), bringing a pipeline back up
    /// (`RECOVERING`) or coming down (`STOPPING`).
    ///
    /// The notion "this channel is running work" — what makes the MediaLive
    /// Anywhere resource it runs on busy rather than idle
    /// ([`ComAmazonawsMedialiveMedialiveState::channels_running_on_node`]). A
    /// channel that is being created, having its configuration applied, waiting
    /// `IDLE` or on its way out of the account runs nothing: `PipelineDetail`
    /// is "Runtime details of a pipeline when a channel is running", and those
    /// are exactly the states [`Self::healthy_pipelines`] reports none for.
    pub fn is_on_air(&self) -> bool {
        matches!(
            self.state.as_str(),
            channel_state::STARTING
                | channel_state::RUNNING
                | channel_state::RECOVERING
                | channel_state::STOPPING
        )
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

    /// The instant this channel entered the state it is in now — the one thing
    /// anything derived from a channel's state can be dated by.
    pub fn state_since(&self) -> SystemTime {
        self.state_since
    }

    /// Put the channel into `state` as of `now` and owe `to` once
    /// [`TRANSIENT_STATE_SPAN`] has passed.
    fn begin_transition(&mut self, state: &str, to: Settled, now: SystemTime) {
        self.state = state.to_string();
        self.state_since = now;
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
        // The state was owed at `transition.due` whether or not anybody called,
        // so that — and not this call's clock — is when the channel entered it.
        self.state_since = transition.due;
        self.transition = None;
        Some(transition.to)
    }
}

/// Why a channel MUTATION could not resolve the `ChannelId` it was given, as
/// [`ComAmazonawsMedialiveMedialiveState::mutable_channel`] reports it.
///
/// Carries what a refusal needs and nothing else, so each operation answers in
/// the error shape its own model declares (a 404 for most of them, the 400
/// `UpdateChannel` models instead) without re-reading the store.
#[derive(Debug, Clone)]
pub struct DeadChannel {
    /// The message the refusal carries: `already deleted` for a tombstone the
    /// account still holds, `not found` for an id it never held.
    pub message: String,
    /// The ARN of the record the account still holds under this id, when it
    /// holds one — a tombstone keeps its ARN, and the batch operations report it
    /// beside the per-entry refusal. `None` for an id no channel ever had.
    pub arn: Option<String>,
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
///
/// A SOFT delete: the record stays addressable through
/// `DescribeInputSecurityGroup` and stays MUTABLE, and the only thing the delete
/// takes away is the group's place in `ListInputSecurityGroups`.
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

/// The message a request carrying more whitelist rules than this emulator keeps
/// on one group is refused with: the ceiling and the member as the wire spells
/// it, worded as the network lists' own ceiling is
/// ([`too_many_network_entries_message`]).
pub fn too_many_whitelist_rules_message() -> String {
    format!("An input security group takes at most {MAX_WHITELIST_RULES} whitelistRules entries.")
}

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
///
/// The list's LENGTH is bounded here too ([`MAX_WHITELIST_RULES`]), for the same
/// reason the network lists are: every accepted entry is kept on a record that
/// outlives the request, so a longer list is refused before any of it is stored
/// rather than growing the store by whatever one request chose.
pub fn whitelist_rules_from_request(
    requested: Vec<InputWhitelistRuleCidr>,
) -> Result<Vec<String>, String> {
    // Not sized from the request: the capacity a caller asks for is not an
    // allocation this emulator makes before it has accepted the entries.
    let mut cidrs: Vec<String> = Vec::new();
    for rule in requested {
        if cidrs.len() >= MAX_WHITELIST_RULES {
            return Err(too_many_whitelist_rules_message());
        }
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
    /// so the only way into it is
    /// [`ComAmazonawsMedialiveMedialiveState::delete_sdi_source`] and the only way
    /// to read it is [`SdiSourceRecord::is_deleted`].
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
    /// The instant the creation — or a later accepted settings change — becomes
    /// due, so `CREATING` and `UPDATING` settle into `ACTIVE`. Stored, never
    /// slept on.
    active_due: SystemTime,
    /// The instant an accepted deletion becomes due, so `DELETING` settles into
    /// the terminal `DELETED`. `None` until a delete is accepted, and `None`
    /// again once that deletion has come due — [`Self::deleted`] is what records
    /// it from then on. Stored, never slept on.
    deletion_due: Option<SystemTime>,
    /// Whether this network's deletion has come due, which is what puts it in
    /// the terminal `DELETED` state it then keeps.
    deleted: bool,
}

impl NetworkRecord {
    /// Whether a delete has been accepted for this network and the clock has not
    /// made that deletion due yet.
    pub fn is_deleting(&self) -> bool {
        self.deletion_due.is_some()
    }

    /// Whether this network's deletion has come due, which is what puts it in
    /// the terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Whether a delete has been accepted for this network, in either half of
    /// the deletion lifecycle — so a repeated `DeleteNetwork` is idempotent
    /// rather than restarting the clock, and a network on its way out no longer
    /// holds the name it was created under
    /// ([`ComAmazonawsMedialiveMedialiveState::network_named`]).
    pub fn is_delete_accepted(&self) -> bool {
        self.is_deleting() || self.is_deleted()
    }

    /// Accept a settings change for this network: it moves into `UPDATING` and
    /// owes `ACTIVE` once [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// `State` is documented "The current state of the Network. Only MediaLive
    /// Anywhere can change the state" and the model's `NetworkState` declares
    /// `UPDATING`, so applying a network's new settings is asynchronous just as
    /// its creation is: the caller polls `DescribeNetwork` for `ACTIVE` again.
    /// The due instant is stored, never slept on.
    fn begin_update(&mut self, now: SystemTime) {
        self.state = network_state::UPDATING.to_string();
        self.active_due = now + TRANSIENT_STATE_SPAN;
    }

    /// Materialise the creation, the settings change or the deletion the clock
    /// has already made due,
    /// answering whether THIS call is the one that settled the deletion. Each is
    /// owed at its stored instant whether or not anybody called, so applying it
    /// in a reporting operation changes nothing a later caller would have seen.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.deletion_due.is_some_and(|due| now >= due) {
            self.deletion_due = None;
            self.deleted = true;
            self.state = network_state::DELETED.to_string();
            return true;
        }
        if (self.state == network_state::CREATING || self.state == network_state::UPDATING)
            && now >= self.active_due
        {
            self.state = network_state::ACTIVE.to_string();
        }
        false
    }
}

/// Why [`ComAmazonawsMedialiveMedialiveState::apply_network_changes`] changed
/// nothing. Each variant is one refusal `UpdateNetwork` answers with, and each
/// leaves the stored network exactly as it was.
#[derive(Debug)]
pub enum NetworkUpdateRefusal {
    /// The `NetworkId` names no network this store has ever held.
    NoSuchNetwork,
    /// The store still holds the network — a delete keeps the record readable —
    /// but that delete has been accepted, so the id names nothing a caller may
    /// still change.
    AlreadyDeleted,
    /// Another live network of the account already carries the new `Name`.
    NameTaken,
}

/// What an `UpdateNetwork` request asks to change.
///
/// Each member is `Some` only when the request carried it — "Include this
/// parameter only if you want to change …" on all three — so an omitted member
/// leaves that setting exactly as it was, and a member carrying an empty list
/// clears it. The two CIDR lists arrive already validated into the shapes the
/// store keeps ([`network_ip_pools`], [`network_routes`]), so a refused list
/// never reaches the store at all. One struct rather than three positional
/// `Option`s on
/// [`ComAmazonawsMedialiveMedialiveState::apply_network_changes`], so a handler
/// cannot pass the pools where the routes belong.
#[derive(Debug, Clone, Default)]
pub struct NetworkChanges {
    pub name: Option<String>,
    pub ip_pools: Option<Vec<IpPool>>,
    pub routes: Option<Vec<Route>>,
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
/// Takes the CIDRs rather than the request shape, so the one rule serves both
/// operations that write a network's pools (`CreateNetwork` and `UpdateNetwork`)
/// even though the model gives each of
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
        deletion_due: None,
        deleted: false,
    }
}

/// Accept a delete for `network`: it moves into `DELETING` and owes the terminal
/// `DELETED` once [`TRANSIENT_STATE_SPAN`] has passed.
///
/// Deleting a network is asynchronous — the model answers `DeleteNetwork` with
/// `202 Accepted` and `State` is documented "The current state of the Network.
/// Only MediaLive Anywhere can change the state" — so the caller polls
/// `DescribeNetwork` for that terminal state. The record then stays in the
/// account in it, so the readers go on reporting the network there rather than
/// losing it.
///
/// Idempotent: a delete already accepted keeps the instant it was accepted at,
/// so a repeated call neither restarts the clock nor revives a deleted network.
pub fn begin_delete_network(network: &mut NetworkRecord, now: SystemTime) {
    if network.is_delete_accepted() {
        return;
    }
    network.deletion_due = Some(now + TRANSIENT_STATE_SPAN);
    network.state = network_state::DELETING.to_string();
}

/// Project a [`DescribeNetworkSummary`] onto one of the structurally identical
/// per-operation network output shapes (`CreateNetworkOutput`,
/// `DescribeNetworkOutput`, `DeleteNetworkOutput`, `UpdateNetworkOutput`, which
/// all carry the same seven members).
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
    /// The identifier names a group the SERVICE publishes
    /// ([`managed_monitor_templates`]) rather than one the account created, so
    /// the account cannot delete it however its own state stands.
    Published,
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

/// Why a SIGNAL MAP's delete removed nothing. Each variant is one modelled
/// refusal, and each of them leaves the store exactly as it was.
///
/// The two conflicts are the one class the delete of a resource with
/// asynchronous lifecycle actions refuses on: work on that resource is still in
/// flight, so there is nothing to delete it out from under yet. Each carries the
/// id of the signal map and the status it is in, so the handler composes its
/// message out of STATE rather than out of the request.
#[derive(Debug)]
pub enum SignalMapDeleteRefusal {
    /// No signal map answers to the identifier, by either form `Identifier`
    /// documents (its id or its current name).
    NotFound,
    /// A lifecycle action of the signal map itself is still running, so the
    /// signal map is not in a state a delete may take it out of.
    LifecycleRunning { id: String, status: String },
    /// A monitor deployment of the signal map is still running — the same
    /// in-flight deployment `StartMonitorDeployment` and
    /// `StartDeleteMonitorDeployment` refuse a second one over.
    DeploymentRunning { id: String, status: String },
}

/// Why [`ComAmazonawsMedialiveMedialiveState::start_signal_map_update`] changed
/// nothing. Each variant is one modelled refusal of `StartUpdateSignalMap`, and
/// each of them leaves the store exactly as it was.
#[derive(Debug)]
pub enum SignalMapUpdateRefusal {
    /// No signal map answers to the identifier, by either form `Identifier`
    /// documents (its id or its current name).
    NotFound,
    /// A lifecycle action of the signal map itself is still running, so the
    /// signal map is not in a state a further update may take it out of.
    /// Carries the id and the status it is in.
    LifecycleRunning { id: String, status: String },
    /// A monitor deployment of the signal map is still running — the same
    /// in-flight deployment `DeleteSignalMap`, `StartMonitorDeployment` and
    /// `StartDeleteMonitorDeployment` refuse over. Carries the id and the
    /// deployment status it is in.
    DeploymentRunning { id: String, status: String },
    /// Another signal map already carries the name asked for, which `Name` is
    /// documented to make unique within the resource type in a region.
    NameTaken,
    /// The `DiscoveryEntryPointArn` the request supplied is not an AWS resource
    /// ARN at all, so it names no resource to discover a graph from. Carries the
    /// value the request supplied.
    EntryPointMalformed { arn: String },
    /// The entry point a rediscovery would walk from names no resource this
    /// store holds, so there is no workflow to map. Carries the ARN that
    /// resolved to nothing.
    EntryPointNotFound { arn: String },
    /// A `CloudWatchAlarmTemplateGroupIdentifiers` entry names no stored alarm
    /// template group, so there is no group to associate the signal map with.
    /// Carries the identifier that resolved to nothing.
    AlarmGroupNotFound { identifier: String },
    /// An `EventBridgeRuleTemplateGroupIdentifiers` entry names no stored rule
    /// template group, on the same terms as [`Self::AlarmGroupNotFound`].
    RuleGroupNotFound,
}

/// What one `StartUpdateSignalMap` request asks the store to change: a member
/// the request carried is `Some` and replaces the stored one, a member it
/// omitted is `None` and leaves that setting as it was.
#[derive(Debug, Clone, Default)]
pub struct SignalMapChanges {
    pub name: Option<String>,
    pub description: Option<String>,
    /// The entry point to discover the signal map's graph from. A CHANGED one
    /// makes the update rediscover the graph, which is what
    /// `StartUpdateSignalMap` documents ("Will discover a new signal map if a
    /// changed discoveryEntryPointArn is provided").
    pub discovery_entry_point_arn: Option<String>,
    /// `ForceRediscovery`: "If true, will force a rediscovery of a signal map if
    /// an unchanged discoveryEntryPointArn is provided."
    pub force_rediscovery: bool,
    /// The identifiers of the alarm template groups to associate the signal map
    /// with, each resolved to a group id before anything is written.
    pub cloud_watch_alarm_template_group_identifiers: Option<Vec<String>>,
    /// The identifiers of the eventbridge rule template groups to associate the
    /// signal map with, resolved on the same terms.
    pub event_bridge_rule_template_group_identifiers: Option<Vec<String>>,
}

/// One REGION's resources for com-amazonaws-medialive-medialive. A handler
/// reaches the partition of the region its own request was signed for through
/// [`service_state`], and no other
/// ([`ComAmazonawsMedialiveMedialiveStore`] is what holds them all).
#[derive(Debug, Default)]
pub struct ComAmazonawsMedialiveMedialiveState {
    /// Every channel that exists, keyed by its minted `Id`. A `BTreeMap` so
    /// `ListChannels` walks it in id order on every call and its `NextToken`
    /// means the same thing on the next one.
    pub channels: BTreeMap<String, ChannelRecord>,
    /// Every input security group that exists, keyed by its minted `Id`. A
    /// `BTreeMap` for the same reason as [`Self::channels`]: a reader walks it in
    /// id order on every call.
    pub input_security_groups: BTreeMap<String, InputSecurityGroupRecord>,
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
    /// `CreateInput`'s `RequestId` (`@idempotencyToken`) -> the input it created.
    input_tokens: IdempotencyTokens,
    /// The id mints every region partition of one store SHARES, so no two
    /// regions can hand out the same id and no address this service reports in
    /// one region is reported in another. Set by
    /// [`ComAmazonawsMedialiveMedialiveStore::region`] when it creates the
    /// partition.
    ///
    /// PRIVATE: every id comes out of a `mint_*` method of this store, which is
    /// where the kind of id and its first value live.
    mints: ::std::sync::Arc<IdMints>,
    /// Every cloudwatch alarm template group that exists — the ones this account
    /// created, keyed by their minted `Id`, and the ones the SERVICE publishes,
    /// keyed by the `aws-` prefixed id the [`managed_monitor_templates`] catalog
    /// publishes them under and seeded when the partition came up. A `BTreeMap`
    /// so a later `ListCloudWatchAlarmTemplateGroups` walks it in id order on
    /// every call.
    ///
    /// PRIVATE, so the name-uniqueness rule, the cap and the mint stay invariants
    /// of [`Self::insert_alarm_template_group`] rather than of each handler.
    alarm_template_groups: BTreeMap<String, CloudWatchAlarmTemplateGroupRecord>,
    /// `CreateCloudWatchAlarmTemplateGroup`'s `RequestId` (`@idempotencyToken`),
    /// keyed per region by [`template_group_token_key`] -> the group it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    alarm_template_group_tokens: IdempotencyTokens,
    /// Every cloudwatch alarm template that exists — this account's own, keyed by
    /// their minted `Id`, and the ones the SERVICE publishes inside its own
    /// groups, seeded when the partition came up, exactly as
    /// [`Self::alarm_template_groups`] holds both provenances. A `BTreeMap` so a
    /// later `ListCloudWatchAlarmTemplates` walks it in id order on every call.
    ///
    /// PRIVATE, so the group resolution, the name-uniqueness rule, the cap and
    /// the mint stay invariants of [`Self::insert_alarm_template`] rather than of
    /// each handler.
    alarm_templates: BTreeMap<String, CloudWatchAlarmTemplateRecord>,
    /// `CreateCloudWatchAlarmTemplate`'s `RequestId` (`@idempotencyToken`), keyed
    /// per region by [`template_group_token_key`] -> the template it created.
    /// PRIVATE for the same reason as [`Self::channel_tokens`].
    alarm_template_tokens: IdempotencyTokens,
    /// Every SdiSource that exists, keyed by its minted `Id`, in a `BTreeMap`
    /// for the same reason as [`Self::channels`].
    pub sdi_sources: BTreeMap<String, SdiSourceRecord>,
    /// `CreateSdiSource`'s `RequestId` (`@idempotencyToken`) -> the SdiSource it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    sdi_source_tokens: IdempotencyTokens,
    /// Every Network that exists, keyed by its minted `Id`, in a `BTreeMap` for
    /// the same reason as [`Self::channels`]: `ListNetworks` walks it in id
    /// order on every call and its `NextToken` means the same thing on the next
    /// one.
    pub networks: BTreeMap<String, NetworkRecord>,
    /// `CreateNetwork`'s `RequestId` (`@idempotencyToken`) -> the network it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    network_tokens: IdempotencyTokens,
    /// Every eventbridge rule template group that exists, keyed by its minted
    /// `Id`. A `BTreeMap` so a walk over the groups is in id order on every
    /// call.
    pub event_bridge_rule_template_groups: BTreeMap<String, EventBridgeRuleTemplateGroupRecord>,
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
    /// `CreateEventBridgeRuleTemplate`'s `RequestId` (`@idempotencyToken`), keyed
    /// per region by [`template_group_token_key`] -> the template it created.
    /// PRIVATE for the same reason as [`Self::channel_tokens`].
    event_bridge_rule_template_tokens: IdempotencyTokens,
    /// Every signal map that exists, keyed by its minted `Id`, in mint order for
    /// the same reason as [`Self::channels`].
    pub signal_maps: BTreeMap<String, SignalMapRecord>,
    /// `CreateSignalMap`'s `RequestId` (`@idempotencyToken`) -> the signal map it
    /// created.
    signal_map_tokens: IdempotencyTokens,
    /// Every multiplex that exists, keyed by its minted `Id`, in id order for
    /// the same reason as [`Self::channels`].
    pub multiplexes: BTreeMap<String, MultiplexRecord>,
    /// `CreateMultiplex`'s `RequestId` (`@idempotencyToken`) -> the multiplex it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    multiplex_tokens: IdempotencyTokens,
    /// `CreateMultiplexProgram`'s `RequestId` (`@idempotencyToken`) -> the NAME
    /// of the program it created, keyed per multiplex by the multiplex slice's
    /// `multiplex_program_token_key` because a program is created against one
    /// multiplex's endpoint and is named by that multiplex together with its own
    /// name rather than by an id. PRIVATE for the same reason as
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
    /// `CreateCluster`'s `RequestId` (`@idempotencyToken`) -> the cluster it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    cluster_tokens: IdempotencyTokens,
    /// Every channel placement group that exists, keyed by its minted `Id`. A
    /// `BTreeMap` so a later `ListChannelPlacementGroups` walks it in id order
    /// on every call.
    pub channel_placement_groups: BTreeMap<String, ChannelPlacementGroupRecord>,
    /// `CreateChannelPlacementGroup`'s `RequestId` (`@idempotencyToken`), keyed
    /// per cluster by the slice's `channel_placement_group_token_key` -> the
    /// group it created. PRIVATE for the same reason as [`Self::channel_tokens`].
    channel_placement_group_tokens: IdempotencyTokens,
    /// Every node that exists, in every cluster, keyed by its minted `Id`, in id
    /// order for the same reason as [`Self::channels`]. The cluster a node
    /// belongs to is on the record, and every node lookup matches it
    /// ([`Self::node`]), because a node's id is unique in its cluster.
    pub nodes: BTreeMap<String, NodeRecord>,
    /// `CreateNode`'s `RequestId` (`@idempotencyToken`) -> the node it created,
    /// keyed by the node slice's `node_token_key` because a node is created
    /// against one cluster's endpoint. PRIVATE for the same reason as
    /// [`Self::channel_tokens`].
    node_tokens: IdempotencyTokens,
    /// Every node registration script this account has been issued, keyed by the
    /// id the registration script slice minted for it, in id order — which is
    /// also the order they were issued in, so the ring that bounds the map drops
    /// the oldest.
    ///
    /// PRIVATE, so the ring's bound, the per-cluster lookup and the rendering
    /// stay invariants of the [`node_registration_scripts`] slice's own methods
    /// rather than of each handler. The operation reports a script's TEXT and no
    /// id, so nothing outside that slice resolves a record.
    node_registration_scripts: BTreeMap<String, NodeRegistrationScriptRecord>,
    /// `CreateNodeRegistrationScript`'s `RequestId` (`@idempotencyToken`) -> the
    /// script it issued, keyed per cluster by that slice's
    /// `node_registration_script_token_key` because a script is issued against
    /// one cluster's endpoint. PRIVATE for the same reason as
    /// [`Self::channel_tokens`].
    node_registration_script_tokens: IdempotencyTokens,
    /// Every reservation the account has purchased, keyed by its minted `Id`, in
    /// id order for the same reason as [`Self::channels`]: a reader walks it in
    /// id order on every call.
    pub reservations: BTreeMap<String, ReservationRecord>,
    /// `PurchaseOffering`'s `RequestId` (`@idempotencyToken`) -> the reservation
    /// it purchased. PRIVATE for the same reason as [`Self::channel_tokens`].
    reservation_tokens: IdempotencyTokens,
    /// The account's encoder engine version catalogue, as `ListVersions` reports
    /// the available ones out of it.
    ///
    /// There is no mint counter and no token map beside it: an engine version is
    /// not a resource a caller creates, it is one the service publishes, so
    /// nothing on a request path writes this catalogue — and an account the
    /// service has published no version to carries the EMPTY one the
    /// [`engine_versions`] slice's `Default` gives, never a set derived from the
    /// local clock.
    pub channel_engine_versions: ChannelEngineVersionCatalog,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// An empty partition of one region's resources, minting them out of
    /// `mints` — the store's own set, which every partition shares, so no two
    /// regions hand out the same id. The [`regions`] slice is what calls this;
    /// `Default` (which gives a set of its own) is for a standalone store.
    ///
    /// Empty of the ACCOUNT's resources, that is. The monitor template resources
    /// the SERVICE publishes exist in every account and every region without
    /// anybody creating them, so the partition comes up carrying them (the
    /// [`managed_monitor_templates`] catalog), which is what makes a listing
    /// report them on an account that has created nothing. They are seeded here,
    /// once per partition, rather than merged in by each reader, so every reader
    /// — the listings, the single gets, the `Scope` filter and a group's derived
    /// `TemplateCount` — answers from the one store.
    fn with_mints(region: &str, mints: ::std::sync::Arc<IdMints>) -> Self {
        let mut state = Self {
            mints,
            ..Self::default()
        };
        for group in published_groups(region) {
            state.alarm_template_groups.insert(group.id.clone(), group);
        }
        for template in published_templates(region) {
            state.alarm_templates.insert(template.id.clone(), template);
        }
        state
    }

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
        // Collected and forgotten on the same terms as the channel tokens
        // above: only an input that settles into `DELETED` on THIS call is
        // collected, and a token never outlives the input it minted, so a
        // retried create mints a new input rather than answering with a
        // deleted one.
        let mut deleted_inputs: Vec<String> = Vec::new();
        for (id, input) in &mut self.inputs {
            if input.settle(now) {
                deleted_inputs.push(id.clone());
            }
        }
        for id in deleted_inputs {
            self.input_tokens.forget(&id);
        }
        for signal_map in self.signal_maps.values_mut() {
            signal_map.settle(now);
        }
        // Collected and forgotten on the same terms as the channel tokens
        // above: only a network that settles into `DELETED` on THIS call is
        // collected, and a token never outlives the network it minted, so a
        // retried create mints a new network rather than answering with a
        // deleted one.
        let mut deleted_networks: Vec<String> = Vec::new();
        for (id, network) in &mut self.networks {
            if network.settle(now) {
                deleted_networks.push(id.clone());
            }
        }
        for id in deleted_networks {
            self.network_tokens.forget(&id);
        }
        self.settle_multiplexes(now);
        self.settle_input_devices(now);
        self.settle_clusters(now);
        self.settle_channel_placement_groups(now);
        self.settle_nodes(now);
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
        (FIRST_CHANNEL_ID + self.mints.next(|mints| &mut mints.channel)).to_string()
    }

    /// The next input security group id, as AWS shapes them: a numeric string.
    pub fn mint_input_security_group_id(&mut self) -> String {
        (FIRST_INPUT_SECURITY_GROUP_ID
            + self.mints.next(|mints| &mut mints.input_security_group))
            .to_string()
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
            // The SAME cut point the channel side applies, which is the one this
            // module documents for every derived "currently using" list: an
            // input stops using the group once it has reached its TERMINAL
            // `DELETED` state. One still in the transient `DELETING` step the
            // clock owes is a resource the account holds, so it goes on being a
            // current user of the group — exactly as a `DELETING` channel does
            // in [`Self::channels_using_security_group`] above.
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
        (FIRST_SDI_SOURCE_ID + self.mints.next(|mints| &mut mints.sdi_source)).to_string()
    }

    /// Record `token` as having created `sdi_source_id`.
    pub fn record_sdi_source_token(&mut self, token: String, sdi_source_id: &str) {
        self.sdi_source_tokens
            .record(token, sdi_source_id, MAX_SDI_SOURCES);
    }

    /// The SdiSource a stored `RequestId` created, if that source is still one a
    /// caller may use.
    ///
    /// LIVENESS is part of the lookup, through the same
    /// [`SdiSourceRecord::is_deleted`] every other resolution of a source
    /// applies: the delete keeps the record readable, but a token's idempotency
    /// window is over once the source it minted has reached the terminal
    /// `DELETED` state — the name it carried is already free for a new source —
    /// so the retried create mints one of its own rather than answering with a
    /// record the caller could neither use nor change.
    pub fn sdi_source_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.sdi_source_tokens.created(request_id)?;
        self.sdi_sources
            .get(id)
            .filter(|source| !source.is_deleted())
            .map(|source| source.id.as_str())
    }

    /// The id of the SdiSource whose `Name` is `name`, if the store holds one.
    ///
    /// `CreateSdiSource`'s and `UpdateSdiSource`'s `Name` carry the same
    /// documented rule as the cluster's and the network's own ("Specify a name
    /// that is unique in the AWS account… Names are case-sensitive"), so the
    /// match is EXACT (never case-folded) and spans every source this store
    /// holds, and it is what makes a create — or a rename — under a name the
    /// account already holds the modelled `ConflictException` rather than a
    /// second source nothing can tell apart by the name the model says
    /// identifies it. This is the ONE lookup both of them resolve against, so
    /// the two cannot disagree about what a stored source may be called
    /// ([`Self::network_named`] plays the same part on the network side).
    ///
    /// A deleted source is skipped: `DELETED` is terminal, so the record stays
    /// readable but the source is out of the account and the name it was created
    /// under is free for the next create.
    pub fn sdi_source_named(&self, name: &str) -> Option<&str> {
        self.sdi_sources
            .values()
            .find(|source| source.name.as_deref() == Some(name) && !source.is_deleted())
            .map(|source| source.id.as_str())
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
            // The one cut point every derived "currently using" list of this
            // store applies ([`Self::inputs_using_security_group`]): an input
            // stops using the source when it reaches its TERMINAL `DELETED`
            // state, not when its delete is merely accepted, so one still in the
            // transient `DELETING` step is reported here just as a `DELETING`
            // channel is by [`Self::channels_attaching`].
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

    /// Delete the SdiSource `id` names and answer it as the delete left it, or
    /// `None` when the store holds no source with that id.
    ///
    /// The source moves to the terminal `DELETED` state and STAYS in the
    /// account, so `DescribeSdiSource` and `ListSdiSources` keep reporting it
    /// there rather than losing it. Idempotent — deleting an already deleted
    /// source changes nothing.
    ///
    /// A token never outlives the resource it minted: the source's idempotency
    /// window is over the moment it reaches that terminal state, so the tokens
    /// that created it are dropped here and a create retried with one of them
    /// mints a new source instead of answering with the deleted one — the rule
    /// the channel, input, network and cluster tokens follow when their own
    /// deletes come due.
    ///
    /// The state write and the projection live together here, so the response
    /// reports the source that was stored rather than one a handler assembled.
    pub fn delete_sdi_source(&mut self, id: &str) -> Option<SdiSource> {
        self.sdi_sources.get_mut(id)?.deleted = true;
        self.sdi_source_tokens.forget(id);
        self.sdi_source_description(id)
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
        (FIRST_NETWORK_ID + self.mints.next(|mints| &mut mints.network)).to_string()
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
    ///
    /// A network whose delete this service has accepted is skipped: it is on its
    /// way out of the account, so the name it was created under is free for the
    /// next create even though its record stays readable.
    pub fn network_named(&self, name: &str) -> Option<&str> {
        self.networks
            .values()
            .find(|network| {
                network.name.as_deref() == Some(name) && !network.is_delete_accepted()
            })
            .map(|network| network.id.as_str())
    }

    /// The network `id` names while it is still LIVE: one no accepted delete has
    /// taken out of the account.
    ///
    /// The lookup a request REFERENCING a network resolves through — an
    /// `InterfaceMapping`'s `NetworkId`, "the ID of the network that you want to
    /// connect to the specified logicalInterfaceName" — because a network whose
    /// delete this service has accepted is on its way out of the account and is
    /// not one a new resource may take a dependency on, even though its record
    /// stays readable in that state.
    ///
    /// NOT the lookup a mutation OF the network goes through: those resolve the
    /// id across the whole [`Self::networks`] store and answer a tombstone with
    /// the already-deleted refusal of their own.
    pub fn live_network(&self, id: &str) -> Option<&NetworkRecord> {
        self.networks
            .get(id)
            .filter(|network| !network.is_delete_accepted())
    }

    /// Apply an `UpdateNetwork` request's changes to the network `id` names, or
    /// refuse.
    ///
    /// Every refusal is checked BEFORE the first write, so a refused update
    /// leaves the stored network exactly as it was. A member the request omitted
    /// is not changed ("Include this parameter only if you want to change …" on
    /// `Name`, `IpPools` and `Routes`), and neither is a member no update request
    /// carries at all: the id, the ARN, the tags and the derived
    /// `AssociatedClusterIds` are not something this operation can move.
    ///
    /// The id is resolved through the LIVE record, like every other mutation of
    /// this service: a delete keeps the record readable, but an id whose delete
    /// has been accepted names nothing a caller may still change. The documented
    /// account-wide name uniqueness is the create's own rule, resolved through
    /// the same lookup ([`Self::network_named`]) so a rename cannot store a name
    /// a create could not, while re-sending the network's own name is not a
    /// clash with itself.
    ///
    /// An accepted change puts the network in `UPDATING` and leaves the clock
    /// owing it `ACTIVE` ([`NetworkRecord::begin_update`]), because `State` is
    /// "The current state of the Network. Only MediaLive Anywhere can change the
    /// state": the settings are applied to the stored record at once and the
    /// lifecycle member a caller polls follows the clock.
    pub fn apply_network_changes(
        &mut self,
        id: &str,
        changes: NetworkChanges,
        now: SystemTime,
    ) -> Result<(), NetworkUpdateRefusal> {
        let Some(network) = self.networks.get(id) else {
            return Err(NetworkUpdateRefusal::NoSuchNetwork);
        };
        if network.is_delete_accepted() {
            return Err(NetworkUpdateRefusal::AlreadyDeleted);
        }
        // Decided while the borrow is still shared, so the write below needs one
        // lookup rather than a second fallible one.
        if changes
            .name
            .as_deref()
            .and_then(|name| self.network_named(name))
            .is_some_and(|holder| holder != id)
        {
            return Err(NetworkUpdateRefusal::NameTaken);
        }

        let Some(network) = self.networks.get_mut(id) else {
            return Err(NetworkUpdateRefusal::NoSuchNetwork);
        };
        if let Some(name) = changes.name {
            network.name = Some(name);
        }
        if let Some(ip_pools) = changes.ip_pools {
            network.ip_pools = ip_pools;
        }
        if let Some(routes) = changes.routes {
            network.routes = routes;
        }
        network.begin_update(now);
        Ok(())
    }

    /// The ids of the clusters currently attached to the network `network_id` —
    /// what `AssociatedClusterIds` reports, and what `DeleteNetwork` refuses a
    /// delete on when it is not empty. Derived from the cluster store on
    /// every read, so it tracks every later cluster create rather than being
    /// recorded at create time.
    ///
    /// The association the store knows is the one a cluster's own request
    /// carries: an `InterfaceMapping`'s `NetworkId`, "The ID of the network that
    /// you want to connect to the specified logicalInterfaceName"
    /// (`smithy_query(query="shape", shape="InterfaceMapping")`). A cluster that
    /// maps several of its interfaces onto the same network is attached to it
    /// once, and `DefaultRoute` names an interface rather than a network, so it
    /// attaches nothing.
    ///
    /// BOUNDED work, on both factors: the store holds at most [`MAX_CLUSTERS`]
    /// clusters (enforced in [`Self::insert_cluster`]) and each carries at most
    /// [`MAX_CLUSTER_INTERFACE_MAPPINGS`] mappings (`CreateCluster` refuses a
    /// longer list), so one call scans at most the product of the two and a
    /// listing — itself capped at `MAX_LIST_PAGE_SIZE` networks per page — calls
    /// it once per reported network.
    ///
    /// Reported in `BTreeMap` key order, which is the order the cluster ids were
    /// minted in because they are minted from a counter at a fixed seven-digit
    /// width (see `FIRST_CLUSTER_ID`), so the keys sort numerically.
    pub fn clusters_using_network(&self, network_id: &str) -> Vec<String> {
        self.clusters
            .values()
            // A cluster whose deletion has come due is no longer *currently
            // using* anything, so it leaves this derived association exactly as
            // a deleted channel leaves a cluster's own `ChannelIds`; its record
            // stays readable in that terminal state.
            .filter(|cluster| !cluster.is_deleted())
            .filter(|cluster| {
                cluster
                    .network_settings
                    .as_ref()
                    .and_then(|settings| settings.interface_mappings.as_ref())
                    .is_some_and(|mappings| {
                        mappings
                            .iter()
                            .any(|mapping| mapping.network_id.as_deref() == Some(network_id))
                    })
            })
            .map(|cluster| cluster.id.clone())
            .collect()
    }

    /// The `DescribeNetworkSummary` projection of one stored network, or `None`
    /// when no network has that id.
    ///
    /// The ONE projection every operation that reports a network
    /// (`CreateNetwork`, `DescribeNetwork`, `UpdateNetwork`, `DeleteNetwork`,
    /// `ListNetworks`)
    /// answers from, so none
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
            + self
                .mints
                .next(|mints| &mut mints.event_bridge_rule_template_group);
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

    /// The eventbridge rule template groups stored in `region`, in the order
    /// their ids were minted, beginning at the group `start` names when a
    /// `NextToken` carried a resume point.
    ///
    /// The same reading of a group store a listing of the TEMPLATES inside them
    /// already uses ([`Self::event_bridge_rule_templates_in`]): a `BTreeMap`
    /// range scan from the resume point, which is why a token naming a group
    /// deleted since the previous page resumes at the next surviving id instead
    /// of failing. Groups of other regions are invisible, for the reason
    /// [`Self::event_bridge_rule_template_group`] states.
    pub fn event_bridge_rule_template_groups_in<'a>(
        &'a self,
        region: &'a str,
        start: Option<&'a str>,
    ) -> impl Iterator<Item = &'a EventBridgeRuleTemplateGroupRecord> + 'a {
        let first = match start {
            Some(token) => ::std::ops::Bound::Included(token),
            None => ::std::ops::Bound::Unbounded,
        };
        self.event_bridge_rule_template_groups
            .range::<str, _>((first, ::std::ops::Bound::Unbounded))
            .map(|(_, group)| group)
            .filter(move |group| group.region == region)
    }

    /// Project a stored group onto the `EventBridgeRuleTemplateGroupSummary` a
    /// listing reports.
    ///
    /// Every member the summary shares with the flat group shape
    /// ([`event_bridge_rule_template_group_output`]) carries the stored value, so
    /// a listing and the `GetEventBridgeRuleTemplateGroup` beside it cannot
    /// disagree about one group. Where the two shapes differ, the summary adds
    /// `TemplateCount` — "The number of templates in a group", which is derived
    /// from the templates that exist right now
    /// ([`Self::event_bridge_rule_template_ids_in_group`]) rather than from
    /// anything recorded on the group, so it tracks every later template create
    /// and delete.
    pub fn event_bridge_rule_template_group_summary(
        &self,
        group: &EventBridgeRuleTemplateGroupRecord,
    ) -> EventBridgeRuleTemplateGroupSummary {
        EventBridgeRuleTemplateGroupSummary {
            arn: group.arn.clone(),
            created_at: ::aws_smithy_types::DateTime::from(group.created_at),
            description: group.description.clone(),
            id: group.id.clone(),
            modified_at: Some(::aws_smithy_types::DateTime::from(group.modified_at)),
            name: group.name.clone(),
            tags: Some(group.tags.clone()),
            template_count: i32::try_from(
                self.event_bridge_rule_template_ids_in_group(&group.id).len(),
            )
            .unwrap_or(i32::MAX),
        }
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
            // One endpoint per pipeline, each carrying the public address that
            // pipeline egresses from — allocated with the channel, so it is
            // read out of the store like every other member here.
            egress_endpoints: Some(channel.egress_endpoints()),
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
                .map(|version| self.described_engine_version(version)),
            linked_channel_settings: channel
                .linked_channel_settings
                .as_ref()
                .map(|linked| self.described_linked(&channel.arn, linked)),
            channel_security_groups: Some(channel.channel_security_groups.clone()),
            inference_settings: channel.inference_settings.as_ref().map(described_inference),
        })
    }

    /// The `ChannelEngineVersionResponse` reporting the engine version a channel
    /// is pinned to: the build identifier the channel stores, and the expiry
    /// this account's engine version catalogue stores for that identifier.
    ///
    /// A channel stores only the identifier it was pinned with, so the expiry a
    /// channel-shaped read reports is read out of the same catalogue
    /// `ListVersions` reports from
    /// ([`ChannelEngineVersionCatalog::described`]) — one stored instant,
    /// however it is read.
    fn described_engine_version(
        &self,
        version: &str,
    ) -> crate::ops::types::ChannelEngineVersionResponse {
        self.channel_engine_versions.described(version)
    }

    /// The engine version this account's catalogue offers as its DEFAULT at
    /// `now` — the engine a channel that pinned none runs on.
    ///
    /// `ChannelEngineVersionRequest$Version` is "The build identifier of the
    /// engine version to use for this channel. Specify 'DEFAULT' to reset to the
    /// default version", so the model states that an account HAS a default engine
    /// version and never which build identifier it is. It is therefore DERIVED
    /// from the catalogue rather than stored beside it: the newest version still
    /// available to the account, which is the entry
    /// [`ChannelEngineVersionCatalog::available_at`] reports first (it orders the
    /// available versions newest release first), because the engine a service puts
    /// a channel on when the caller pins none is its current release. An account
    /// the service has published no engine version to has no default version, and
    /// then there is none to report.
    fn default_engine_version(
        &self,
        now: SystemTime,
    ) -> Option<crate::ops::types::ChannelEngineVersionResponse> {
        self.channel_engine_versions
            .available_at(now)
            .into_iter()
            .next()
    }

    /// The engine versions a channel's RUNNING pipelines are using, as
    /// `ChannelSummary$UsedChannelEngineVersions` reports them ("The engine
    /// version that the running pipelines are using").
    ///
    /// Derived from the two facts the store already holds about a running
    /// channel — the same two the sibling `PipelinesRunningCount` and
    /// `ChannelEngineVersion` members are derived from: which of its pipelines
    /// are up ([`ChannelRecord::healthy_pipelines`]) and which engine build it is
    /// pinned to. A channel with no pipeline up is using no engine version, so it
    /// reports none; while a pipeline is up, the engine those pipelines run is
    /// the channel's pinned build, or the account's default version
    /// ([`Self::default_engine_version`]) when the channel pinned none — and
    /// nothing at all when the channel pinned none and the account has no default
    /// version to run it on, because then there is no identifier to name.
    ///
    /// Every pipeline of one channel runs that one engine, and the member reports
    /// the versions in USE rather than one entry per pipeline, so a running
    /// channel reports exactly that one version — with the expiry the catalogue
    /// stores for it ([`Self::described_engine_version`]), the same instant
    /// `ChannelEngineVersion` and `ListVersions` report for that identifier.
    fn used_engine_versions(
        &self,
        channel: &ChannelRecord,
        now: SystemTime,
    ) -> Vec<crate::ops::types::ChannelEngineVersionResponse> {
        if channel.healthy_pipelines().is_empty() {
            return Vec::new();
        }
        channel
            .channel_engine_version
            .as_deref()
            .map(|version| self.described_engine_version(version))
            .or_else(|| self.default_engine_version(now))
            .into_iter()
            .collect()
    }

    /// The `ChannelSummary` projection of one stored channel — the same stored
    /// record as [`Self::channel_description`], minus the members the summary
    /// shape does not carry (`EncoderSettings`, `PipelineDetails`) and plus the
    /// one it carries beyond that description, `UsedChannelEngineVersions`, which
    /// [`Self::used_engine_versions`] derives from the channel's healthy
    /// pipelines and its engine version pin.
    ///
    /// `now` is the instant of the call, for the one member whose value the clock
    /// bears on: an account's default engine version is the newest of the
    /// versions still available to it, and which of them are available is a
    /// question for the clock.
    pub fn channel_summary(&self, channel: &ChannelRecord, now: SystemTime) -> ChannelSummary {
        ChannelSummary {
            arn: Some(channel.arn.clone()),
            cdi_input_specification: channel.cdi_input_specification.clone(),
            channel_class: Some(channel.channel_class.clone()),
            destinations: Some(channel.destinations.clone()),
            // The same endpoints as in `channel_description`.
            egress_endpoints: Some(channel.egress_endpoints()),
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
                .map(|version| self.described_engine_version(version)),
            used_channel_engine_versions: Some(self.used_engine_versions(channel, now)),
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
        (FIRST_INPUT_ID + self.mints.next(|mints| &mut mints.input)).to_string()
    }

    /// The message refusing the configuration `input` carries, or `None` when it
    /// is one this service can ingest through.
    ///
    /// ONE rule for every operation that WRITES an input's configuration
    /// (`CreateInput`, `UpdateInput`), applied to the record that WOULD be
    /// stored, so the two cannot accept configurations the other refuses:
    ///
    /// * every id the write ADDS to the configuration must resolve to a resource
    ///   this account holds — an input security group, an SdiSource, a claimed
    ///   input device. Those resources are what the input ingests through, and
    ///   each derives the inputs using it from this store
    ///   ([`Self::inputs_using_security_group`], [`Self::inputs_using_sdi_source`]),
    ///   so an id nothing minted would leave the input claiming a resource no
    ///   read of the account can show.
    /// * an input must carry the ingest list its `Type` ingests through: the
    ///   whitelist that admits the system pushing to a pushed-to type
    ///   ([`PUSH_INGEST_PROTOCOLS`]), and the source URLs a pulled-from type
    ///   pulls ([`PULL_SOURCE_INPUT_TYPES`]).
    /// * the two cross-member rules the `Vpc` member's own doc states — it
    ///   "requires setting the roleArn property on Input creation" and is "not
    ///   compatible with the inputSecurityGroups property".
    ///
    /// A VPC input and an on-premises input are the two cases the whitelist does
    /// NOT admit anything for: the former's ingest addresses "will be created in
    /// a VPC rather than with public Internet addresses", and the latter lives
    /// "in a customer network" (`InputNetworkLocation`), so neither is reached
    /// through an input security group of this account.
    ///
    /// `held` is the input as the store HOLDS it — `None` for a create, which
    /// adds every id it names. An id the held record already names is NOT
    /// resolved again: it is part of the configuration this input already
    /// claims, so a write that adds nothing to a reference list is never refused
    /// for it. Only the ids a write ADDS are resolved, which is what keeps a
    /// write that changes other members (a rename) possible after a referenced
    /// resource whose delete this service accepts while an input still uses it
    /// (an input security group, [`delete_input_security_group`]) has gone.
    pub fn input_configuration_error(
        &self,
        input: &InputRecord,
        held: Option<&InputRecord>,
    ) -> Option<String> {
        if input.vpc.is_some() {
            if !input.security_groups.is_empty() {
                return Some(VPC_WITH_SECURITY_GROUPS_MESSAGE.to_string());
            }
            if input.role_arn.as_deref().unwrap_or_default().is_empty() {
                return Some(VPC_WITHOUT_ROLE_ARN_MESSAGE.to_string());
            }
        }
        let held_groups: &[String] = held.map_or(&[], |held| &held.security_groups);
        for group in &input.security_groups {
            if held_groups.contains(group) {
                continue;
            }
            if self.live_input_security_group(group).is_none() {
                return Some(unknown_reference_message(
                    "inputSecurityGroups",
                    "input security group",
                    group,
                ));
            }
        }
        let held_sdi_sources: &[String] = held.map_or(&[], |held| &held.sdi_sources);
        for source in &input.sdi_sources {
            if held_sdi_sources.contains(source) {
                continue;
            }
            if self
                .sdi_sources
                .get(source)
                .is_none_or(SdiSourceRecord::is_deleted)
            {
                return Some(unknown_reference_message("sdiSources", "SdiSource", source));
            }
        }
        for device in &input.input_devices {
            let Some(id) = device.id.as_deref().filter(|id| !id.is_empty()) else {
                return Some(missing_reference_id_message("inputDevices"));
            };
            let held_device = held.is_some_and(|held| {
                held.input_devices
                    .iter()
                    .any(|device| device.id.as_deref() == Some(id))
            });
            if held_device {
                continue;
            }
            if self.input_device(id).is_none() {
                return Some(unknown_reference_message("inputDevices", "input device", id));
            }
        }
        let input_type = input.input_type.as_deref().unwrap_or_default();
        if input.needs_input_whitelist() && input.security_groups.is_empty() {
            return Some(required_for_input_type_message(
                "inputSecurityGroups",
                input_type,
            ));
        }
        if pulls_from_sources(input.input_type.as_deref()) && input.sources.is_empty() {
            return Some(required_for_input_type_message("sources", input_type));
        }
        None
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
            .map(|_| self.allocate_pool_address())
            .collect();
        if let Some(input) = self.inputs.get_mut(id) {
            input.ingest_ips.extend(fresh);
        }
    }

    /// Allocate the public egress address of every pipeline of the channel `id`
    /// that does not hold one yet, so the channel reports the address its
    /// outputs originate from — `ChannelEgressEndpoint$SourceIp` ("Public IP of
    /// where a channel's output comes from"), one endpoint per pipeline.
    ///
    /// Call this from the operations that WRITE a channel's pipelines
    /// (`CreateChannel`, `UpdateChannelClass`): allocating an address is a
    /// write, so no reporting operation does it.
    ///
    /// Top-up only, out of the same pool the ingest addresses come from, so an
    /// address already allocated for a pipeline stays with that pipeline: a
    /// class change that adds a pipeline spends one more address and leaves the
    /// first pipeline egressing from the address it already reported.
    pub fn allocate_channel_egress_addresses(&mut self, id: &str) {
        let Some(channel) = self.channels.get(id) else {
            return;
        };
        let allocated = channel.egress_ips.len();
        let wanted = usize::try_from(channel.pipeline_count()).unwrap_or(0);
        let fresh: Vec<String> = (allocated..wanted)
            .map(|_| self.allocate_pool_address())
            .collect();
        if let Some(channel) = self.channels.get_mut(id) {
            channel.egress_ips.extend(fresh);
        }
    }

    /// The next address out of the emulated public address pool.
    ///
    /// The counter wraps at [`IP_POOL_LEN`], so an address could in
    /// principle be reissued - but only after 2^17 allocations, and the store
    /// holds at most [`MAX_INPUTS`] inputs, each with a caller-bounded
    /// destination list, and at most [`MAX_CHANNELS`] channels, each with at
    /// most two pipelines, so the wrap is the pool's last-resort bound rather
    /// than a path a client can reach.
    fn allocate_pool_address(&mut self) -> String {
        let offset = (self.mints.next(|mints| &mut mints.pool_ip) % u64::from(IP_POOL_LEN)) as u32;
        ::std::net::Ipv4Addr::from(IP_POOL_BASE + offset).to_string()
    }

    /// Record `token` as having created `input_id` for the create that spent it
    /// — `partner_of` is the input the new input is a partner OF, or `None` for
    /// a standalone create — under the same bounded, evicting discipline as the
    /// channel tokens.
    pub fn record_input_idempotency_token(
        &mut self,
        partner_of: Option<&str>,
        token: &str,
        input_id: &str,
    ) {
        self.input_tokens.record(
            input_token_key(partner_of, token),
            input_id,
            MAX_INPUT_IDEMPOTENCY_TOKENS,
        );
    }

    /// The input a stored `RequestId` created for the create described by
    /// `partner_of`, if that input is still one a caller may use and is still
    /// the input that create asks for.
    ///
    /// The scope is part of the key AND re-checked on the record, so a token
    /// spent on a different create resolves to nothing here: the create it
    /// named is not the one this request describes.
    ///
    /// LIVENESS is part of the lookup, through the same [`Self::live_input`] a
    /// mutation of an input resolves its id by: a delete keeps the record
    /// readable, but a token's idempotency window is over once this service has
    /// accepted the delete of the input the token minted, so the retried create
    /// mints a new input rather than answering with one the caller could
    /// neither use nor change.
    pub fn input_for_token(&self, partner_of: Option<&str>, request_id: &str) -> Option<&str> {
        let id = self
            .input_tokens
            .created(&input_token_key(partner_of, request_id))?;
        self.live_input(id)
            .filter(|input| input.partner_of.as_deref() == partner_of)
            .map(|input| input.id.as_str())
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

    /// The channel a MUTATION may change, or the refusal to answer with — the
    /// one guard every channel mutation resolves its id through, BEFORE it looks
    /// at any channel's state.
    ///
    /// An id the account holds no record for and an id whose delete this service
    /// has accepted (the record is `DELETING`, or in the terminal `DELETED`
    /// state it keeps) are ONE refusal class: neither names a channel a caller
    /// may still change, so each is answered with the operation's own modelled
    /// not-found error and nothing is written. Only the message tells them
    /// apart — `already deleted` for a tombstone the account still holds and
    /// `not found` for an id it never held.
    ///
    /// Taking this refusal first is what keeps a state conflict
    /// (`ConflictException`) for a channel that still EXISTS: a tombstone has no
    /// state a caller can conflict with, so reporting one would tell a client to
    /// retry an id that will never become startable again.
    pub fn mutable_channel(&mut self, id: &str) -> Result<&mut ChannelRecord, DeadChannel> {
        // The refusal is built from the SAME lookup that resolves the id, and
        // before the mutable borrow, so a caller reporting it (the batch
        // operations, which name the tombstone's ARN) needs no second read of
        // the store.
        let refusal = match self.channels.get(id) {
            Some(channel) if channel.is_delete_accepted() => Some(DeadChannel {
                message: already_deleted_message(&format!("Channel {id}")),
                arn: Some(channel.arn.clone()),
            }),
            Some(_) => None,
            None => Some(DeadChannel {
                message: not_found_message("Channel", id),
                arn: None,
            }),
        };
        match refusal {
            Some(refusal) => Err(refusal),
            // The lookup above already resolved the id to a live channel, so
            // this is that channel; the fallback keeps the function total
            // rather than asserting it.
            None => self.live_channel_mut(id).ok_or_else(|| DeadChannel {
                message: not_found_message("Channel", id),
                arn: None,
            }),
        }
    }

    /// The input security group `id` names while it is still LIVE: one
    /// `DeleteInputSecurityGroup` has not deleted.
    ///
    /// The lookup a request REFERENCING a group resolves through — an input
    /// configuration's `InputSecurityGroups` entry — because a deleted group is
    /// gone from `ListInputSecurityGroups` and is not a group a new resource may
    /// take a dependency on.
    ///
    /// NOT the lookup a mutation OF the group goes through: a group's delete is
    /// a SOFT delete, so the deleted group stays addressable and stays mutable
    /// (see the module docs), and `UpdateInputSecurityGroup` therefore resolves
    /// its id across the whole [`Self::input_security_groups`] store.
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
        (FIRST_SIGNAL_MAP_ID + self.mints.next(|mints| &mut mints.signal_map)).to_string()
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

    /// Remove the signal map `identifier` names, answering the record that was
    /// removed — or refuse.
    ///
    /// `DeleteSignalMap` "deletes the specified signal map" and documents no
    /// precondition, so the only refusals are an identifier that resolves to no
    /// signal map and a signal map with work of its own still in flight: its own
    /// lifecycle action, or a monitor deployment of it. Once removed the signal
    /// map is GONE from the store — `SignalMapStatus` declares no deleted state
    /// for a reader to go on reporting it in — so every later read of either of
    /// its identifiers misses, its name is free again, its tags go with it and
    /// the template groups it named are no longer held by it.
    ///
    /// The resolution, the preconditions and the removal live together here, so a
    /// handler cannot check one of them and write past another; the handler's job
    /// is only to turn a [`SignalMapDeleteRefusal`] into the operation's own
    /// modelled error.
    pub fn delete_signal_map(
        &mut self,
        identifier: &str,
    ) -> Result<SignalMapRecord, SignalMapDeleteRefusal> {
        let id = self
            .signal_map_id_for_identifier(identifier)
            .ok_or(SignalMapDeleteRefusal::NotFound)?
            .to_string();
        let map = self
            .signal_maps
            .get(&id)
            .ok_or(SignalMapDeleteRefusal::NotFound)?;

        if map.lifecycle_in_progress() {
            return Err(SignalMapDeleteRefusal::LifecycleRunning {
                id,
                status: map.status().to_string(),
            });
        }
        if map.monitor_deployment_in_progress() {
            return Err(SignalMapDeleteRefusal::DeploymentRunning {
                id,
                status: map.monitor_deployment_status().to_string(),
            });
        }

        let removed = self
            .signal_maps
            .remove(&id)
            .ok_or(SignalMapDeleteRefusal::NotFound)?;
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted signal map.
        self.signal_map_tokens.forget(&id);
        Ok(removed)
    }

    /// Apply `changes` to the signal map `identifier` names and put it into the
    /// `UPDATE_IN_PROGRESS` status the clock settles, answering its id — or
    /// refuse.
    ///
    /// `StartUpdateSignalMap` "initiates an update for the specified signal map.
    /// Will discover a new signal map if a changed discoveryEntryPointArn is
    /// provided", and `ForceRediscovery` forces that rediscovery for an
    /// unchanged one — so a rediscovery walks the store exactly as the create's
    /// discovery does, and the graph it finds replaces the stored one when the
    /// update completes. Every other member is a partial update: one the request
    /// carried replaces what is stored, one it omitted leaves it alone.
    ///
    /// Like every other mutation of a signal map, an update needs the signal map
    /// to have no asynchronous work of its own in flight: neither its own
    /// lifecycle action nor a monitor deployment of it, which deploys the very
    /// monitor contents an update rewrites.
    ///
    /// The resolution, the preconditions, the rediscovery and the write live
    /// together here, so a handler cannot check one of them and write past
    /// another; the handler's job is only to turn a [`SignalMapUpdateRefusal`]
    /// into the operation's own modelled error.
    pub fn start_signal_map_update(
        &mut self,
        region: &str,
        identifier: &str,
        changes: SignalMapChanges,
        now: SystemTime,
    ) -> Result<String, SignalMapUpdateRefusal> {
        let id = self
            .signal_map_id_for_identifier(identifier)
            .ok_or(SignalMapUpdateRefusal::NotFound)?
            .to_string();
        let map = self
            .signal_maps
            .get(&id)
            .ok_or(SignalMapUpdateRefusal::NotFound)?;

        // A lifecycle action of the signal map is asynchronous, so one still in
        // flight is already changing the resource this call would change.
        if map.lifecycle_in_progress() {
            return Err(SignalMapUpdateRefusal::LifecycleRunning {
                id,
                status: map.status().to_string(),
            });
        }
        // A monitor deployment is asynchronous too, and it deploys the monitor
        // this update would replace — the graph the signal map maps and the
        // template groups it is associated with — so one still in flight is
        // already working on what this call would change.
        if map.monitor_deployment_in_progress() {
            return Err(SignalMapUpdateRefusal::DeploymentRunning {
                id,
                status: map.monitor_deployment_status().to_string(),
            });
        }
        let stored_entry_point = map.discovery_entry_point_arn.clone();

        // `Name` is unique "within the scope of a resource type in a specific
        // region", so a rename cannot take a name another signal map holds —
        // while renaming a signal map to the name it already has changes
        // nothing and is not a collision with itself.
        if let Some(name) = changes.name.as_deref() {
            if self.signal_map_named(name).is_some_and(|held| held != id) {
                return Err(SignalMapUpdateRefusal::NameTaken);
            }
        }

        // Request SYNTAX first: a `DiscoveryEntryPointArn` that is not an ARN at
        // all cannot name a resource, whichever operation supplied it.
        let requested_entry_point = changes.discovery_entry_point_arn.as_deref();
        if let Some(arn) = requested_entry_point {
            if !is_resource_arn(arn) {
                return Err(SignalMapUpdateRefusal::EntryPointMalformed {
                    arn: arn.to_string(),
                });
            }
        }

        // Then rediscovery: a changed entry point discovers a new graph, and an
        // unchanged one only when the request forces it. The graph is resolved
        // against the resources that exist now, so an entry point naming none is
        // refused and nothing is written.
        let entry_point = requested_entry_point
            .unwrap_or(stored_entry_point.as_str())
            .to_string();
        let rediscover = requested_entry_point.is_some_and(|arn| arn != stored_entry_point)
            || changes.force_rediscovery;
        let discovered = if rediscover {
            Some(self.discover_media_resource_map(&entry_point).ok_or(
                SignalMapUpdateRefusal::EntryPointNotFound { arn: entry_point },
            )?)
        } else {
            None
        };

        // A group identifier names a group that must already exist, and the
        // signal map records the ID of the group it resolved — never the
        // identifier it was given — exactly as the create resolves them.
        let mut alarm_group_ids = None;
        if let Some(identifiers) = changes.cloud_watch_alarm_template_group_identifiers.as_ref() {
            let mut resolved = Vec::with_capacity(identifiers.len());
            for identifier in identifiers {
                let group = self.alarm_template_group(region, identifier).ok_or_else(|| {
                    SignalMapUpdateRefusal::AlarmGroupNotFound {
                        identifier: identifier.clone(),
                    }
                })?;
                resolved.push(group.id.clone());
            }
            alarm_group_ids = Some(resolved);
        }
        let mut rule_group_ids = None;
        if let Some(identifiers) = changes
            .event_bridge_rule_template_group_identifiers
            .as_ref()
        {
            let mut resolved = Vec::with_capacity(identifiers.len());
            for identifier in identifiers {
                let group = self
                    .event_bridge_rule_template_group(region, identifier)
                    .ok_or(SignalMapUpdateRefusal::RuleGroupNotFound)?;
                resolved.push(group.id.clone());
            }
            rule_group_ids = Some(resolved);
        }

        // Every precondition is met, so the change is written in one step.
        let map = self
            .signal_maps
            .get_mut(&id)
            .ok_or(SignalMapUpdateRefusal::NotFound)?;
        if let Some(name) = changes.name {
            map.name = name;
        }
        if let Some(description) = changes.description {
            map.description = Some(description);
        }
        if let Some(arn) = changes.discovery_entry_point_arn {
            map.discovery_entry_point_arn = arn;
        }
        if let Some(ids) = alarm_group_ids {
            map.cloud_watch_alarm_template_group_ids = ids;
        }
        if let Some(ids) = rule_group_ids {
            map.event_bridge_rule_template_group_ids = ids;
        }
        map.begin_update(discovered, now);
        Ok(id)
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
    /// discovery runs: the real
    /// service rediscovers a signal map's graph only when a discovery is asked
    /// for (`StartUpdateSignalMap`, with a changed entry point or
    /// `ForceRediscovery`), so a resource created or deleted afterwards does not
    /// change a stored signal map's `MediaResourceMap`.
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
        // A discovered graph is reported once a discovery of this signal map has
        // COMPLETED — which is also what stamps `LastDiscoveredAt` — so a later
        // lifecycle action (an update) does not blank out the graph the signal
        // map still maps while that action runs.
        let discovered = map.last_discovered_at.is_some();
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
        if let Some(record) = self.nodes.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Node(record.id.clone()));
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
        // The monitor TEMPLATES, each a taggable resource of its own with its
        // own ARN and its own stored tag map, exactly like the GROUPS above:
        // the tags a template create supplied are readable, writable and
        // removable through the generic tag operations, not only through the
        // template's own reader.
        if let Some(record) = self.alarm_templates.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::AlarmTemplate(record.id.clone()));
        }
        if let Some(record) = self
            .event_bridge_rule_templates
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::EventBridgeRuleTemplate(record.id.clone()));
        }
        if let Some(record) = self.clusters.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Cluster(record.id.clone()));
        }
        if let Some(record) = self
            .channel_placement_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::ChannelPlacementGroup(record.id.clone()));
        }
        if let Some(record) = self.reservations.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Reservation(record.id.clone()));
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
            TaggedResource::Node(id) => self.nodes.get(&id).map(|record| &record.tags),
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
            TaggedResource::AlarmTemplate(id) => {
                self.alarm_templates.get(&id).map(|record| &record.tags)
            }
            TaggedResource::EventBridgeRuleTemplate(id) => self
                .event_bridge_rule_templates
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::Cluster(id) => self.clusters.get(&id).map(|record| &record.tags),
            TaggedResource::ChannelPlacementGroup(id) => self
                .channel_placement_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::Reservation(id) => {
                self.reservations.get(&id).map(|record| &record.tags)
            }
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
    /// The INPUT SECURITY GROUP is handed out even once it is deleted, because
    /// its delete is a SOFT delete: the group stays addressable and stays
    /// mutable (see the module docs), so its tags are writable exactly as its
    /// `WhitelistRules` are.
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
                .map(|record| &mut record.tags),
            TaggedResource::SdiSource(id) => self
                .sdi_sources
                .get_mut(&id)
                .filter(|record| !record.is_deleted())
                .map(|record| &mut record.tags),
            TaggedResource::Network(id) => self
                .networks
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::SignalMap(id) => self
                .signal_maps
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Multiplex(id) => self
                .multiplexes
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Node(id) => self
                .nodes
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            // A claimed device is never deleted — nothing in this service
            // un-claims one — so there is no dead id to filter out here.
            TaggedResource::InputDevice(id) => self
                .input_devices
                .get_mut(&id)
                .map(|record| &mut record.tags),
            // The monitor template resources the SERVICE publishes are read-only
            // for the account, tags included: they are not its resources, so a
            // tag write is not handed their stored map and the tag writers answer
            // the ARN with their modelled not-found error.
            TaggedResource::AlarmTemplateGroup(id) => self
                .alarm_template_groups
                .get_mut(&id)
                .filter(|record| !is_published_id(&record.id))
                .map(|record| &mut record.tags),
            TaggedResource::EventBridgeRuleTemplateGroup(id) => self
                .event_bridge_rule_template_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
            // No delete filter on either template: a deleted template is
            // REMOVED from its store, so its ARN resolves to nothing at all and
            // never reaches this match.
            TaggedResource::AlarmTemplate(id) => self
                .alarm_templates
                .get_mut(&id)
                .filter(|record| !is_published_id(&record.id))
                .map(|record| &mut record.tags),
            TaggedResource::EventBridgeRuleTemplate(id) => self
                .event_bridge_rule_templates
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Cluster(id) => self
                .clusters
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::ChannelPlacementGroup(id) => self
                .channel_placement_groups
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            // A reservation this account has DELETED is a tombstone: its record
            // stays readable but names no resource a caller may still change, so
            // its tags are no longer writable either.
            TaggedResource::Reservation(id) => self
                .reservations
                .get_mut(&id)
                .filter(|record| !record.is_deleted())
                .map(|record| &mut record.tags),
        }
    }

    /// Add `tags` to the resource `arn` names, bounded.
    ///
    /// The bounds and the reserved key namespace live here rather than in the
    /// handler, so no caller-driven write can grow a resource's tag map past
    /// them or plant a system tag in it: every refusal is checked BEFORE the
    /// first insert, and a refused request leaves the stored tags exactly as
    /// they were.
    pub fn insert_resource_tags(
        &mut self,
        arn: &str,
        tags: HashMap<String, String>,
    ) -> Result<(), TagRefusal> {
        // Request shape first — a key a caller may not name at all, or a key or
        // value past its length, is refused whatever resource it was aimed at.
        if let Some(key) = first_reserved_tag_key(tags.keys()) {
            return Err(TagRefusal::ReservedKeyPrefix {
                key: key.to_string(),
            });
        }
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
    /// naming one is not a refusal. What it does refuse is a key in the
    /// reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, which is not the
    /// caller's to remove, and an ARN this store holds no resource for.
    pub fn remove_resource_tags(
        &mut self,
        arn: &str,
        keys: &[String],
    ) -> Result<(), TagRefusal> {
        // A system tag is not the caller's to remove either, so the reserved
        // namespace is refused here exactly as it is on the write path —
        // before resolution, and before anything is removed.
        if let Some(key) = first_reserved_tag_key(keys.iter()) {
            return Err(TagRefusal::ReservedKeyPrefix {
                key: key.to_string(),
            });
        }
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
    /// A tag key lies in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, so
    /// it is not a key a caller may create, change or remove. Carries the key,
    /// so the refusal names it.
    ReservedKeyPrefix { key: String },
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
    Node(String),
    InputDevice(String),
    AlarmTemplateGroup(String),
    EventBridgeRuleTemplateGroup(String),
    AlarmTemplate(String),
    EventBridgeRuleTemplate(String),
    Cluster(String),
    ChannelPlacementGroup(String),
    Reservation(String),
}

/// What a monitor deployment of a signal map DOES, which is what decides the
/// pair of statuses it moves through and what completing it leaves behind.
///
/// `StartMonitorDeployment` "initiates a deployment to deploy the latest monitor
/// of the specified signal map" — a real one, or the dry run its `DryRun` member
/// asks for — and `StartDeleteMonitorDeployment` "initiates a deployment to
/// delete the monitor of the specified signal map". All three are the same
/// asynchronous deployment of one signal map's monitor, differing only in what
/// they leave behind, so one record models them and this is the kind of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorDeploymentKind {
    /// Deploy the signal map's latest monitor.
    Deploy,
    /// Run the deployment without deploying anything.
    DryRun,
    /// Delete the signal map's monitor.
    Delete,
}

impl MonitorDeploymentKind {
    /// The `SignalMapMonitorDeploymentStatus` a deployment of this kind runs in.
    fn in_progress_status(self) -> &'static str {
        match self {
            Self::Deploy => signal_map_monitor_deployment_status::DEPLOYMENT_IN_PROGRESS,
            Self::DryRun => signal_map_monitor_deployment_status::DRY_RUN_DEPLOYMENT_IN_PROGRESS,
            Self::Delete => signal_map_monitor_deployment_status::DELETE_IN_PROGRESS,
        }
    }

    /// The status a deployment of this kind completes in.
    fn complete_status(self) -> &'static str {
        match self {
            Self::Deploy => signal_map_monitor_deployment_status::DEPLOYMENT_COMPLETE,
            Self::DryRun => signal_map_monitor_deployment_status::DRY_RUN_DEPLOYMENT_COMPLETE,
            Self::Delete => signal_map_monitor_deployment_status::DELETE_COMPLETE,
        }
    }
}

/// What completing a monitor deployment does to the signal map's
/// `LastSuccessfulMonitorDeployment`, "the latest successful monitor deployment
/// of a signal map".
enum LatestSuccessfulDeployment {
    /// A deployment that deployed the monitor IS that latest successful
    /// deployment.
    Deployed(SuccessfulMonitorDeployment),
    /// A dry run deploys nothing, so it neither becomes the latest successful
    /// deployment nor undoes the one before it.
    Unchanged,
    /// A completed deletion leaves the signal map with no deployed monitor, so
    /// there is no successful deployment of one left to report.
    Gone,
}

/// One monitor deployment of one signal map, as the operation that started it
/// started it.
///
/// A deployment is ASYNCHRONOUS — the operation answers 202 and the caller polls
/// the signal map's `MonitorDeployment$Status` — so the record carries the status
/// it is in now and the instant the clock makes its outcome due. The instant is
/// STORED and never slept on.
#[derive(Debug, Clone)]
pub struct MonitorDeploymentRecord {
    /// What this deployment does, which is what decides both the pair of
    /// statuses it moves through and what completing it leaves behind.
    kind: MonitorDeploymentKind,
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
    /// Whether this deployment is still running, in the IN_PROGRESS status of
    /// its own kind.
    fn in_progress(&self) -> bool {
        self.status == self.kind.in_progress_status()
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
    /// The instant an update started by `StartUpdateSignalMap` becomes due, so
    /// `UPDATE_IN_PROGRESS` settles into `UPDATE_COMPLETE`, or `None` while no
    /// update is running. Stored, never slept on, on the same terms as
    /// [`Self::discovery_due`].
    update_due: Option<SystemTime>,
    /// The graph a running update's rediscovery found, held until that update
    /// comes due: until then the signal map still reports the graph its last
    /// completed discovery produced, because that is the one it currently maps.
    pending_media_resource_map: Option<BTreeMap<String, MediaResource>>,
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
        // The update an earlier call started, owed at its own due instant: the
        // signal map reaches `UPDATE_COMPLETE` and, when that update asked for a
        // rediscovery, the graph it found becomes the graph the signal map maps,
        // discovered at the instant the update completed.
        if let Some(due) = self
            .update_due
            .filter(|due| self.status == signal_map_status::UPDATE_IN_PROGRESS && now >= *due)
        {
            self.update_due = None;
            self.status = signal_map_status::UPDATE_COMPLETE.to_string();
            if let Some(discovered) = self.pending_media_resource_map.take() {
                self.media_resource_map = discovered;
                self.last_discovered_at = Some(due);
            }
            self.modified_at = due;
        }
        // Completed on THIS call: the due instant to stamp as the signal map's
        // `ModifiedAt`, and what that completion does to the latest successful
        // deployment. Read out of the deployment before the signal map's own
        // fields are written, so the record is borrowed once.
        let settled = self.monitor_deployment.as_mut().and_then(|deployment| {
            if !deployment.in_progress() || now < deployment.due {
                return None;
            }
            deployment.status = deployment.kind.complete_status().to_string();
            let latest = match deployment.kind {
                MonitorDeploymentKind::Deploy => {
                    LatestSuccessfulDeployment::Deployed(SuccessfulMonitorDeployment {
                        details_uri: deployment.details_uri.clone(),
                        status: deployment.status.clone(),
                    })
                }
                MonitorDeploymentKind::DryRun => LatestSuccessfulDeployment::Unchanged,
                MonitorDeploymentKind::Delete => LatestSuccessfulDeployment::Gone,
            };
            Some((deployment.due, latest))
        });
        if let Some((due, latest)) = settled {
            self.modified_at = due;
            match latest {
                LatestSuccessfulDeployment::Deployed(successful) => {
                    self.last_successful_monitor_deployment = Some(successful);
                }
                LatestSuccessfulDeployment::Unchanged => {}
                LatestSuccessfulDeployment::Gone => {
                    self.last_successful_monitor_deployment = None;
                }
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
    /// has nothing to deploy yet — and neither has one whose own lifecycle
    /// action is still in flight, which is the graph being rebuilt underneath.
    pub fn discovery_complete(&self) -> bool {
        self.last_discovered_at.is_some() && !self.lifecycle_in_progress()
    }

    /// Whether a lifecycle action of the signal map ITSELF is still running, so
    /// nothing may take the signal map out from under it. Those are the
    /// `SignalMapStatus` values whose name ends in `_IN_PROGRESS` — the
    /// `CREATE_IN_PROGRESS` a discovery runs in and the `UPDATE_IN_PROGRESS` an
    /// update runs in — read off the stored status, so a status this slice does
    /// not mint yet counts as soon as something stores it.
    pub fn lifecycle_in_progress(&self) -> bool {
        self.status.ends_with("_IN_PROGRESS")
    }

    /// Whether a monitor deployment of this signal map is already running, so a
    /// second one — of any kind — cannot be started on top of it.
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
        let kind = if dry_run {
            MonitorDeploymentKind::DryRun
        } else {
            MonitorDeploymentKind::Deploy
        };
        self.begin_deployment(kind, now);
    }

    /// Start a deployment to DELETE this signal map's monitor, replacing the
    /// latest deployment on the same terms as [`Self::begin_monitor_deployment`]
    /// replaces it: `StartDeleteMonitorDeployment` "initiates a deployment to
    /// delete the monitor of the specified signal map", which is the same
    /// asynchronous deployment of that monitor, so it runs in the deletion's own
    /// IN_PROGRESS status and comes due [`TRANSIENT_STATE_SPAN`] later.
    pub fn begin_delete_monitor_deployment(&mut self, now: SystemTime) {
        self.begin_deployment(MonitorDeploymentKind::Delete, now);
    }

    /// Start an update of this signal map: `StartUpdateSignalMap` "initiates an
    /// update for the specified signal map" and answers 202, so the signal map
    /// enters `UPDATE_IN_PROGRESS` and comes due [`TRANSIENT_STATE_SPAN`] later,
    /// where [`Self::settle`] finishes it — a caller that reads straight after
    /// the call still observes the update running.
    ///
    /// `discovered` is the graph a rediscovery found, held pending until the
    /// update completes, and `None` for an update that asked for none.
    pub fn begin_update(
        &mut self,
        discovered: Option<BTreeMap<String, MediaResource>>,
        now: SystemTime,
    ) {
        self.status = signal_map_status::UPDATE_IN_PROGRESS.to_string();
        self.update_due = Some(now + TRANSIENT_STATE_SPAN);
        self.pending_media_resource_map = discovered;
        self.modified_at = now;
    }

    /// Record `kind` as this signal map's latest monitor deployment, running and
    /// due [`TRANSIENT_STATE_SPAN`] from `now`. One writer for every kind, so no
    /// two of them can disagree about how a deployment starts.
    fn begin_deployment(&mut self, kind: MonitorDeploymentKind, now: SystemTime) {
        self.monitor_deployment = Some(MonitorDeploymentRecord {
            kind,
            status: kind.in_progress_status().to_string(),
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
        // Nothing has updated a signal map nobody has updated yet, and nothing
        // is pending rediscovery for it: `StartUpdateSignalMap` is what starts
        // the first update.
        update_due: None,
        pending_media_resource_map: None,
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

/// The message a request carrying more entries in one of a channel's
/// caller-chosen lists than this emulator stores is refused with: the member as
/// the wire spells it, how many entries the request carried, and the ceiling.
///
/// One helper, so `CreateChannel` and `UpdateChannel` — which store the same
/// two lists under the same two ceilings ([`MAX_INPUT_ATTACHMENTS`],
/// [`MAX_CHANNEL_DESTINATIONS`]) — cannot word that bound two ways.
pub fn too_many_channel_entries_message(member: &str, count: usize, limit: usize) -> String {
    format!("{member} carries {count} entries; this emulator accepts at most {limit}")
}

/// The message a not-found refusal carries: the resource type, the identifier
/// the request named, then `not found` — the phrasing the real service composes
/// from the request. One helper, so no two operations word it two ways.
pub fn not_found_message(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} {identifier} not found")
}

/// The message a write refused because the configuration it carries references
/// a resource this account does not hold: the member as the wire spells it, the
/// kind of resource the entry names, and the id it named.
///
/// Composed from the REQUEST, so it names what the caller must change. One
/// helper for every reference list an input configuration carries
/// (`InputSecurityGroups`, `SdiSources`, `InputDevices`), so the refusals cannot
/// be worded three ways.
pub fn unknown_reference_message(member: &str, resource_type: &str, identifier: &str) -> String {
    format!("{member} references {resource_type} {identifier}, which this account does not hold")
}

/// The message a write refused because an entry of a reference list names no id
/// at all carries: the member as the wire spells it.
pub fn missing_reference_id_message(member: &str) -> String {
    format!("every entry of {member} must name the id of a resource this account holds")
}

/// The message a write refused because a SINGLE-VALUED member that names a
/// resource carries no id the write could resolve carries: the member as the
/// wire spells it.
///
/// The scalar sibling of [`missing_reference_id_message`], which words the same
/// problem for one ENTRY of a reference list — so the two refusals say the same
/// thing about the same kind of omission.
pub fn blank_reference_id_message(member: &str) -> String {
    format!("{member} must name the id of a resource this account holds")
}

/// The message a channel-configuration write refused for a member the
/// configuration does not declare at all carries: the member as the wire spells
/// it. One helper, so `CreateChannel` and `UpdateChannel` cannot word the same
/// omission two ways.
pub fn required_channel_configuration_message(member: &str) -> String {
    format!("a channel configuration must declare {member}")
}

/// The `message` a channel-configuration refusal carries beside its
/// `ValidationErrors` entries: the entries say WHERE each problem is and what it
/// is, and this says what the refusal as a whole is about.
pub const CHANNEL_CONTAINS_ERRORS_MESSAGE: &str = "Channel contains errors.";

/// One entry of `UnprocessableEntityException$ValidationErrors`: where the
/// problem is (`ElementPath` is "Path to the source of the error") and what it
/// is.
///
/// The one constructor for every per-element problem this service reports, so
/// the schedule slice and the channel-configuration rule report a problem in the
/// same shape.
pub fn validation_problem(element_path: &str, error_message: &str) -> ValidationError {
    ValidationError {
        element_path: Some(element_path.to_string()),
        error_message: Some(error_message.to_string()),
    }
}

/// The message a write refused because the configuration leaves out the ingest
/// list the requested `InputType` ingests through carries: the member as the
/// wire spells it and the type that requires it, both out of the request.
pub fn required_for_input_type_message(member: &str, input_type: &str) -> String {
    format!("{member} is required for an input of type {input_type}")
}

/// The message a write refused for a private-VPC input that also attaches input
/// security groups carries: `Vpc` is "not compatible with the
/// inputSecurityGroups property". Nothing is interpolated — the incompatibility
/// is between two members the caller named, not about either one's value.
pub const VPC_WITH_SECURITY_GROUPS_MESSAGE: &str =
    "vpc is not compatible with the inputSecurityGroups property";

/// The message a write refused for a private-VPC input with no role carries:
/// `Vpc` "requires setting the roleArn property on Input creation".
pub const VPC_WITHOUT_ROLE_ARN_MESSAGE: &str =
    "vpc requires setting the roleArn property on Input creation";

/// Whether an `Accept` header allows a response of the media type `media_type`.
///
/// One definition for every operation that negotiates a response media type, so
/// the refusal and the response cannot disagree about what was asked for. It
/// lives beside [`not_found_message`] rather than on a resource slice, because it
/// is a property of the REQUEST and not of any resource this state holds.
///
/// The header is a comma-separated list of media ranges, each optionally carrying
/// parameters after a `;`. A range allows a type when it names that type, that
/// type's wildcard (`type/*`) or the wildcard that allows anything (`*/*`);
/// matching is case-insensitive, as media types are. A range whose quality
/// parameter is zero allows NOTHING — a zero quality is how a caller says a type
/// is not acceptable — so it is skipped, and every other parameter is ignored
/// because none of them narrows which type may be sent. A request that carries no
/// `Accept` at all asks for no particular type, so every type is allowed.
pub fn accept_allows(accept: &str, media_type: &str) -> bool {
    if accept.trim().is_empty() {
        return true;
    }
    let media_type = media_type.to_ascii_lowercase();
    let (top_level, _) = media_type.split_once('/').unwrap_or((&media_type, ""));
    let wildcard = format!("{top_level}/*");
    accept.split(',').any(|range| {
        let mut parts = range.split(';');
        let name = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        if name != "*/*" && name != wildcard && name != media_type {
            return false;
        }
        // A quality of zero is a refusal of the range it is on, so the range
        // allows nothing; any other quality still allows the type, since this
        // service has one representation to offer rather than a choice between
        // several to rank.
        !parts.any(|parameter| {
            let (key, value) = parameter.split_once('=').unwrap_or_default();
            key.trim().eq_ignore_ascii_case("q")
                && value.trim().parse::<f64>().is_ok_and(|quality| quality <= 0.0)
        })
    })
}

/// The message a read refused for an `Accept` it cannot satisfy carries: what the
/// request asked for and the media types the operation can answer with. Built
/// from the request, so it names what the caller must change.
pub fn unacceptable_media_type_message(accept: &str, served: &[&str]) -> String {
    format!(
        "{accept} is not a media type this operation can answer with: it serves {}",
        served.join(", ")
    )
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

/// The message a channel stop refused on the channel's state carries: the
/// channel the request named, the state it is in, and the state a stop acts from
/// — a channel runs before it can be stopped, so every other state is refused.
///
/// ONE helper, for the same reason as [`busy_input_message`]: `StopChannel` and
/// `BatchStop` refuse the same channel, so they cannot word the refusal two
/// ways.
pub fn channel_not_running_message(channel_id: &str, state: &str) -> String {
    format!(
        "Channel {channel_id} is in state {state} and can only be stopped from {}",
        channel_state::RUNNING
    )
}

/// The message a channel start refused on the channel's state carries: the
/// channel the request named, the state it is in, and the state a start acts
/// from — a channel runs from a stopped one, so every other state is refused.
///
/// ONE helper, for the same reason as [`channel_not_running_message`]:
/// `StartChannel` and `BatchStart` refuse the same channel, so they cannot word
/// the refusal two ways.
pub fn channel_not_startable_message(channel_id: &str, state: &str) -> String {
    format!(
        "Channel {channel_id} is in state {state} and can only be started from {}",
        channel_state::IDLE
    )
}

/// The message a multiplex start refused on the multiplex's state carries, with
/// the same shape and for the same reason as
/// [`channel_not_startable_message`]: `StartMultiplex` and `BatchStart` refuse
/// the same multiplex, so they cannot word the refusal two ways.
pub fn multiplex_not_startable_message(multiplex_id: &str, state: &str) -> String {
    format!(
        "Multiplex {multiplex_id} is in state {state} and can only be started from {}",
        multiplex_state::IDLE
    )
}

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

/// The message a create or a rename refused because the account already holds a
/// resource of that type under the requested `Name` carries.
///
/// Every monitor resource — the cloudwatch alarm template and its group, the
/// eventbridge rule template and its group, and the signal map — documents the
/// same uniqueness rule for `Name` ("Names must be unique within the scope of a
/// resource type in a specific region"), and the anywhere resources — the
/// cluster, the node, the channel placement group and the SdiSource — document
/// theirs ("Specify a name that is unique in the AWS account"), so the name
/// collision is ONE rejection class across all of them and this is the fixed
/// sentence it carries. Nothing is interpolated: the colliding resource's
/// internal id and the region are state the caller did not supply, and a caller
/// matching on the message would be matching on this emulator's environment.
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
    /// than restarting the clock, and the id names no input a caller may still
    /// CHANGE ([`ComAmazonawsMedialiveMedialiveState::live_input`]). NOT what
    /// decides whether this input still USES the resources it references: that
    /// is cut at the terminal `DELETED` state alone
    /// ([`ComAmazonawsMedialiveMedialiveState::inputs_using_security_group`]).
    pub fn is_delete_accepted(&self) -> bool {
        self.is_deleting() || self.is_deleted()
    }

    /// Materialise the deletion the clock has already made due, answering
    /// whether THIS call is the one that settled it. Owed at the stored instant
    /// whether or not anybody called, so applying it in a reporting operation
    /// changes nothing a later caller would have seen.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.deletion_due.is_some_and(|due| now >= due) {
            self.deletion_due = None;
            self.deleted = true;
            return true;
        }
        false
    }

    /// Whether this input is ingested by an upstream system PUSHING to an
    /// endpoint of this service, which the input security groups attached to it
    /// are what admit: its `Type` has a push ingest protocol
    /// ([`PUSH_INGEST_PROTOCOLS`]) and its ingest addresses are this service's
    /// public ones — neither created inside a customer VPC (`Vpc`, whose
    /// addresses replace "public Internet addresses" and which is "not
    /// compatible with the inputSecurityGroups property") nor on premises
    /// (`InputNetworkLocation`, "On-Prem for an input in a customer network").
    pub fn needs_input_whitelist(&self) -> bool {
        push_ingest_protocol(self.input_type.as_deref()).is_some()
            && self.vpc.is_none()
            && self.input_network_location == AWS_INPUT_NETWORK_LOCATION
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

/// The key the input idempotency-token map records a token against.
///
/// Both input creates spend a `RequestId` against this ONE map, so the create
/// the token was spent on is part of the key: a standalone create and the
/// create of a partner for a given input each keep their own entry, and a
/// token spent on one of them is not a replay of the other. `partner_of` is the
/// input a partner is created for, or `None` for a standalone create. The scope
/// is re-checked on the record the token resolves to
/// ([`ComAmazonawsMedialiveMedialiveState::input_for_token`]), so the key's
/// shape carries no invariant of its own.
fn input_token_key(partner_of: Option<&str>, token: &str) -> String {
    match partner_of {
        Some(primary) => format!("partner/{primary}/{token}"),
        None => format!("input/{token}"),
    }
}

/// Build the record of a new input, with NO identity yet: every modelled member
/// starts at the empty value the request then fills in, and `id` and `arn` stay
/// empty until the create stores it.
///
/// The identity is assigned where the record is inserted, so a create refused
/// for a configuration this service cannot ingest through
/// ([`ComAmazonawsMedialiveMedialiveState::input_configuration_error`]) spends
/// no input id: the ids a client is handed are the ids of inputs that exist.
///
/// There is no transient state to owe: the real service answers `CreateInput`
/// with an input that is already `DETACHED`, so this one is usable from the
/// instant it is stored.
pub fn new_input() -> InputRecord {
    InputRecord {
        id: String::new(),
        arn: String::new(),
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

/// Build the record of the input `CreatePartnerInput` creates as a partner of
/// `primary`: a second input, under its own minted `id` and `arn`, that ingests
/// the configuration the input it partners was created with, carrying the tags
/// the partner request supplied and naming the input it partners — which is
/// what makes the pair report each other in `InputPartnerIds`
/// ([`ComAmazonawsMedialiveMedialiveState::input_partner_ids`]).
///
/// EVERY configuration member of the partnered input is inherited, since
/// `CreatePartnerInput` carries no member to configure the partner with: its
/// `name`, `input_type`, `input_network_location`, `role_arn`,
/// `security_groups`, the per-pipeline `sources`, `destinations`,
/// `media_connect_flows`, `input_devices`, `sdi_sources`, `srt_settings`,
/// `multicast_settings`, `smpte2110_receiver_group_settings`,
/// `router_settings`, and the `vpc` and `special_router_settings` a request
/// supplied but the `Input` shape reports no member for.
///
/// What it does NOT inherit is the rest — a per-input value rather than part of
/// that configuration:
///
/// * `ingest_ips` starts empty, so
///   [`ComAmazonawsMedialiveMedialiveState::allocate_input_ingest_addresses`]
///   gives the partner an ingest endpoint of its OWN for each destination slot
///   instead of sharing the primary's allocated one — a partner exists so a
///   second source can be pushed somewhere else. A destination slot whose
///   request named its own ON-PREM `StaticIpAddress` keeps reporting that
///   address on both inputs, because that address is the CALLER's value for the
///   slot rather than one this service allocates;
/// * `tags` are the partner request's own, never a copy of the primary's;
/// * the deletion lifecycle starts fresh: the partner is a newly created input,
///   not a copy of the primary's state. Like `CreateInput`, there is no
///   transient state to owe — the input is usable from the instant it is
///   stored.
pub fn new_partner_input(
    primary: &InputRecord,
    id: String,
    arn: String,
    tags: HashMap<String, String>,
) -> InputRecord {
    InputRecord {
        id,
        arn,
        ingest_ips: Vec::new(),
        tags,
        partner_of: Some(primary.id.clone()),
        deletion_due: None,
        deleted: false,
        ..primary.clone()
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

/// The days `MaintenanceDay` gives the service to choose a channel's
/// maintenance window from, in the model's own order
/// (`smithy_query(query="shape", shape="MaintenanceDay")`: MONDAY | TUESDAY |
/// WEDNESDAY | THURSDAY | FRIDAY | SATURDAY | SUNDAY).
const MAINTENANCE_DAYS: [&str; 7] = [
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
];

/// The hours a maintenance start time can name, which is what makes the start
/// time a whole hour of the day ("Choose the hour that maintenance will
/// start").
const MAINTENANCE_HOURS: usize = 24;

/// The maintenance window the service selects for a channel.
///
/// EVERY channel has one selected: the response member is a *status* whose day
/// is "The currently selected maintenance day" and whose time is "The currently
/// selected maintenance start time", while the create setting that would choose
/// them is optional — so the selection is made here, when the channel is
/// created, rather than left absent until some caller writes one.
///
/// Derived from the channel's own minted id, so the window is stable for the
/// channel's whole lifetime (nothing about the request or the clock enters it)
/// and the windows of an account's channels are spread over the week and over
/// the hours of the day instead of all falling together. The start time is a
/// whole hour, the only form the member accepts.
///
/// `MaintenanceDeadline` ("Maintenance is required by the displayed date and
/// time") and `MaintenanceScheduledDate` ("The currently scheduled maintenance
/// date and time") are assigned by the real service's maintenance scheduler,
/// which this emulator does not run, so they stay absent: no maintenance has
/// been scheduled for or required of this channel.
fn assigned_maintenance_window(id: &str) -> MaintenanceStatus {
    let seed = usize::try_from(id.parse::<u64>().unwrap_or_default()).unwrap_or_default();
    let hour = seed % MAINTENANCE_HOURS;
    MaintenanceStatus {
        maintenance_day: Some(MAINTENANCE_DAYS[seed % MAINTENANCE_DAYS.len()].to_string()),
        maintenance_deadline: None,
        maintenance_scheduled_date: None,
        maintenance_start_time: Some(format!("{hour:02}:00")),
    }
}

/// Apply the day and start time a `CreateChannel` request chose to the window
/// the service assigned the channel.
///
/// Member by member, because each of the two is optional and documented on its
/// own ("Choose one day of the week for maintenance", "Choose the hour that
/// maintenance will start"): a request that names one leaves the other at the
/// service's selection, so the channel carries a complete window either way.
pub fn choose_maintenance_window(
    stored: &mut Option<MaintenanceStatus>,
    chosen: MaintenanceCreateSettings,
) {
    let stored = stored.get_or_insert_with(MaintenanceStatus::default);
    if chosen.maintenance_day.is_some() {
        stored.maintenance_day = chosen.maintenance_day;
    }
    if chosen.maintenance_start_time.is_some() {
        stored.maintenance_start_time = chosen.maintenance_start_time;
    }
}

/// Build a new channel record from a `CreateChannel` request's values.
///
/// Every member is the value the request supplied; the lifecycle members and the
/// service-selected maintenance window are the only computed ones. The channel
/// starts in `CREATING` and owes `IDLE`, which is what the real service does.
pub fn new_channel(
    id: String,
    arn: String,
    channel_class: String,
    now: SystemTime,
) -> ChannelRecord {
    let maintenance = assigned_maintenance_window(&id);
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
        maintenance: Some(maintenance),
        role_arn: None,
        tags: HashMap::new(),
        channel_security_groups: Vec::new(),
        anywhere_settings: None,
        linked_channel_settings: None,
        inference_settings: None,
        vpc: None,
        channel_engine_version: None,
        egress_ips: Vec::new(),
        special_router_settings: None,
        restarting_pipelines: Vec::new(),
        schedule: ChannelSchedule::default(),
        alerts: ChannelAlerts::default(),
        thumbnails: Vec::new(),
        transition: None,
        state_since: now,
    };
    channel.begin_transition(channel_state::CREATING, Settled::Idle, now);
    channel
}

/// Move a channel from `IDLE` into `STARTING`, owing `RUNNING`.
pub fn begin_start(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::STARTING, Settled::Running, now);
}

/// Move a stopped channel into `UPDATING`, owing `IDLE` again.
///
/// `UPDATING` is the `ChannelState` value for a channel whose configuration is
/// being applied, and applying it leaves the channel stopped, so the state the
/// clock owes is the `IDLE` the update started from.
pub fn begin_update(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::UPDATING, Settled::Idle, now);
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

/// The message for the failures that are not about the request: this service was
/// mounted on an engine that holds no store for it, or the store keeps no
/// partition for the region the request was signed for and has no room for
/// another ([`MAX_RESOURCE_REGIONS`]). One constant, so the service's operations
/// cannot word it several ways.
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

/// The REGION slice: the store the engine holds, the per-region partition of
/// resources a handler is handed, the ceiling on how many of those partitions
/// the store keeps, and the decode that says which region a request is for — in
/// one module of its own for the same reason as [`account_configuration`]. What
/// stays in this file is the part the rest of the state needs: the records
/// themselves, and the [`IdMints`] every partition shares.
pub mod regions;

pub use regions::{
    request_region, service_state, service_state_in, service_store,
    ComAmazonawsMedialiveMedialiveStore, RegionState, MAX_RESOURCE_REGIONS,
};

/// The counters this service mints every resource id out of, one per kind of id.
///
/// Held behind one lock rather than one per counter, so a mint is a single
/// critical section and the whole set can be handed to every region partition of
/// a store as one shared value. `Mints` itself is the counters; [`Self::next`] is
/// the only way to move one.
#[derive(Debug, Default)]
pub struct IdMints(::std::sync::Mutex<Mints>);

impl IdMints {
    /// The next value of the counter `pick` names — the offset the caller adds
    /// its own kind's first id to — which no later call hands out again.
    ///
    /// Recovers from a poisoned lock for the same reason the state guard does:
    /// one panicking handler must not stop every later request from being able
    /// to mint an id.
    fn next(&self, pick: fn(&mut Mints) -> &mut u64) -> u64 {
        let mut mints = self
            .0
            .lock()
            .unwrap_or_else(::std::sync::PoisonError::into_inner);
        let counter = pick(&mut mints);
        let minted = *counter;
        *counter += 1;
        minted
    }
}

/// One counter per kind of id this service hands out, each the number of ids of
/// that kind minted so far: the id itself is that offset past the kind's first
/// id (`FIRST_CHANNEL_ID` and the constants beside it), which is what keeps one
/// kind's ids out of another's range.
#[derive(Debug, Default)]
struct Mints {
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_channel_id`].
    channel: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_input_id`].
    input: u64,
    /// Behind
    /// [`ComAmazonawsMedialiveMedialiveState::mint_input_security_group_id`].
    input_security_group: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_sdi_source_id`].
    sdi_source: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_network_id`].
    network: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_signal_map_id`].
    signal_map: u64,
    /// Behind the multiplex slice's `mint_multiplex_id`.
    multiplex: u64,
    /// Behind the alert ids the multiplex slice's `apply_alerts` hands out:
    /// `MultiplexAlert$Id` is "The unique ID for this alert instance", so two
    /// multiplexes cannot raise an alert under the same id.
    multiplex_alert: u64,
    /// Behind the cluster slice's `mint_cluster_id`.
    cluster: u64,
    /// Behind the channel placement group slice's
    /// `mint_channel_placement_group_id`.
    channel_placement_group: u64,
    /// Behind the node slice's `mint_node_id`.
    node: u64,
    /// Behind the registration script slice's
    /// `mint_node_registration_script_id`.
    node_registration_script: u64,
    /// Behind the reservation slice's `mint_reservation_id`.
    reservation: u64,
    /// Behind the alarm template group slice's `mint_alarm_template_group_id`.
    alarm_template_group: u64,
    /// Behind the alarm template slice's `mint_alarm_template_id`.
    alarm_template: u64,
    /// Behind
    /// [`ComAmazonawsMedialiveMedialiveState::mint_event_bridge_rule_template_group_id`].
    event_bridge_rule_template_group: u64,
    /// Behind the eventbridge rule template slice's
    /// `mint_event_bridge_rule_template_id`.
    event_bridge_rule_template: u64,
    /// The offset into the emulated public address pool
    /// ([`ComAmazonawsMedialiveMedialiveState::allocate_pool_address`]), so no
    /// two endpoints this service reports — a push input's ingest address, a
    /// channel pipeline's egress address — are the same, in one region or
    /// across two.
    pool_ip: u64,
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
