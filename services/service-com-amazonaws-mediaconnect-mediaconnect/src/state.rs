// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediaconnect-mediaconnect service.
//!
//! The store is keyed by the flow's own ARN, which is what every flow operation
//! addresses a flow by (`@httpLabel FlowArn`) and which is the flow's identity: the ARN
//! the service mints carries a per-flow generated id as well as the name
//! ([`flow_resource_fragment`]), so two flows that share a name are still distinct
//! resources with distinct ARNs. There is no index by name beside it, because no
//! operation addresses a flow by name and the model puts no uniqueness rule on
//! `Flow.Name`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AddBridgeOutputRequest, AddBridgeSourceRequest, AddMediaStreamRequest, AddOutputRequest,
    AutomaticEncryptionKeyConfiguration,
    Bridge, BridgeFlowSource, BridgeNetworkOutput, BridgeNetworkSource, BridgeOutput, BridgeSource,
    ContentQualityAnalysisFeatureConfiguration,
    DestinationConfiguration, EgressGatewayBridge, EncodingConfig, EncodingParameters, Encryption,
    Entitlement, FabricConfiguration, FailoverConfig, FailoverRouterInputIndexedStreamDetails,
    FailoverRouterInputStreamDetails, Flow, FlowTransitEncryption, Gateway,
    GatewayInstance,
    GatewayNetwork,
    GatewayBridgeSource, GrantEntitlementRequest, IngressGatewayBridge, InputConfiguration,
    Interface, ListedBridge, ListedEntitlement, ListedFlow,
    ListedGateway,
    Maintenance, MaintenanceConfiguration, MaintenanceSchedule,
    MediaConnectFlowRouterInputStreamDetails,
    MediaConnectFlowRouterOutputStreamDetails, MediaStream,
    MediaLiveChannelRouterInputStreamDetails,
    MediaStreamAttributes, MediaStreamAttributesRequest,
    MediaLiveInputRouterOutputStreamDetails,
    MediaStreamOutputConfiguration, MediaStreamOutputConfigurationRequest,
    MediaStreamSourceConfiguration, MediaStreamSourceConfigurationRequest,
    MergeRouterInputIndexedStreamDetails,
    MergeRouterInputStreamDetails, MonitoringConfig,
    NdiConfig, Output, Reservation, ResourceSpecification,
    RouterContentQualityAnalysisConfiguration, RouterInput,
    RouterInputConfiguration, RouterInputStreamDetails, RouterInputTransitEncryption,
    RouterInputTransitEncryptionKeyConfiguration,
    RouterNetworkInterface, RouterNetworkInterfaceConfiguration,
    RouterOutput, RouterOutputConfiguration, RouterOutputMessage,
    RouterOutputProtocolConfiguration, RouterOutputStreamDetails, SetSourceRequest, Source,
    StandardRouterInputStreamDetails, StandardRouterOutputConfiguration,
    StandardRouterOutputStreamDetails, Transport, UpdateEncryption,
    UpdateFailoverConfig, VpcInterface,
    VpcInterfaceAttachment, WindowMaintenanceSchedule,
};

/// The `Status` enum the model declares for a flow
/// (`smithy_query(query="shape",shape="Status")`: STANDBY | ACTIVE | UPDATING | DELETING
/// | STARTING | STOPPING | ERROR). Only the five a flow's lifecycle reaches are named
/// here; the rest belong to operations that are still scaffolded.
pub const STATUS_STANDBY: &str = "STANDBY";
/// See [`STATUS_STANDBY`].
pub const STATUS_STARTING: &str = "STARTING";
/// See [`STATUS_STANDBY`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_STANDBY`].
pub const STATUS_STOPPING: &str = "STOPPING";
/// See [`STATUS_STANDBY`].
pub const STATUS_DELETING: &str = "DELETING";

/// The `ReservationState` enum the model declares for a reservation
/// (`smithy_query(query="shape",shape="ReservationState")`: ACTIVE | EXPIRED | PROCESSING
/// | CANCELED). All four are named here: a purchase is reported in the non-terminal
/// `PROCESSING` state until the clock settles it `ACTIVE` (see
/// [`StoredReservation::settle`]), the clock retires an active reservation into the
/// terminal `EXPIRED` at its end instant, and `CANCELED` is the other terminal value.
pub const RESERVATION_STATE_PROCESSING: &str = "PROCESSING";
/// See [`RESERVATION_STATE_PROCESSING`].
pub const RESERVATION_STATE_ACTIVE: &str = "ACTIVE";
/// See [`RESERVATION_STATE_PROCESSING`].
pub const RESERVATION_STATE_EXPIRED: &str = "EXPIRED";
/// See [`RESERVATION_STATE_PROCESSING`].
pub const RESERVATION_STATE_CANCELED: &str = "CANCELED";

/// The `GatewayState` enum the model declares for a gateway
/// (`smithy_query(query="shape",shape="GatewayState")`: CREATING | ACTIVE | UPDATING |
/// ERROR | DELETING | DELETED). Only the four a gateway's lifecycle reaches here are
/// named; the rest belong to operations that are still scaffolded.
pub const GATEWAY_STATE_CREATING: &str = "CREATING";
/// See [`GATEWAY_STATE_CREATING`].
pub const GATEWAY_STATE_ACTIVE: &str = "ACTIVE";
/// See [`GATEWAY_STATE_CREATING`]. The transient state a gateway enters when
/// DeleteGateway accepts the delete; the clock settles it into
/// [`GATEWAY_STATE_DELETED`] one [`TRANSITION_SPAN`] later.
pub const GATEWAY_STATE_DELETING: &str = "DELETING";
/// See [`GATEWAY_STATE_CREATING`]. The TERMINAL state of a deleted gateway: its presence
/// in the `GatewayState` enum is the model's own statement that a deleted gateway stays a
/// readable resource, so a delete retires the record rather than evicting it (see
/// [`StoredGateway::deleted_at`]).
pub const GATEWAY_STATE_DELETED: &str = "DELETED";

/// The `InstanceState` enum the model declares for a gateway instance
/// (`smithy_query(query="shape",shape="InstanceState")`: REGISTERING | ACTIVE |
/// DEREGISTERING | DEREGISTERED | REGISTRATION_ERROR | DEREGISTRATION_ERROR).
///
/// The non-terminal state an instance is reported in while its registration is in flight;
/// the clock settles it into [`INSTANCE_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later (see
/// [`ComAmazonawsMediaconnectMediaconnectState::register_gateway_instance`]).
pub const INSTANCE_STATE_REGISTERING: &str = "REGISTERING";
/// See [`INSTANCE_STATE_REGISTERING`]. A registered instance that is hosting bridges.
pub const INSTANCE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`INSTANCE_STATE_REGISTERING`]. The transient state an instance enters when
/// DeregisterGatewayInstance accepts the deregistration; the clock settles it into
/// [`INSTANCE_STATE_DEREGISTERED`] one [`TRANSITION_SPAN`] later.
pub const INSTANCE_STATE_DEREGISTERING: &str = "DEREGISTERING";
/// See [`INSTANCE_STATE_REGISTERING`]. The TERMINAL state of a deregistered instance: its
/// presence in the `InstanceState` enum is the model's own statement that a deregistered
/// instance stays a readable resource, so a deregistration retires the record rather than
/// evicting it — the same reasoning as [`GATEWAY_STATE_DELETED`].
pub const INSTANCE_STATE_DEREGISTERED: &str = "DEREGISTERED";
/// See [`INSTANCE_STATE_REGISTERING`]. A terminal failure of the appliance-side
/// registration, which reaches the service out of band (see
/// [`ComAmazonawsMediaconnectMediaconnectState::register_gateway_instance`]) and which no
/// modelled operation can cause.
pub const INSTANCE_STATE_REGISTRATION_ERROR: &str = "REGISTRATION_ERROR";
/// See [`INSTANCE_STATE_REGISTRATION_ERROR`], for a deregistration that failed.
pub const INSTANCE_STATE_DEREGISTRATION_ERROR: &str = "DEREGISTRATION_ERROR";

/// The `BridgePlacement` enum the model declares for a gateway instance
/// (`smithy_query(query="shape",shape="BridgePlacement")`: AVAILABLE | LOCKED).
/// `GatewayInstance$BridgePlacement`: "the availability of the instance to host new
/// bridges … If it is AVAILABLE, new bridges can be added to this instance" — the value a
/// newly registered instance carries, and the one UpdateGatewayInstance moves.
pub const BRIDGE_PLACEMENT_AVAILABLE: &str = "AVAILABLE";
/// See [`BRIDGE_PLACEMENT_AVAILABLE`]: "if it is LOCKED, no new bridges can be deployed to
/// this instance".
pub const BRIDGE_PLACEMENT_LOCKED: &str = "LOCKED";

/// The `ConnectionStatus` enum the model declares for a gateway instance
/// (`smithy_query(query="shape",shape="ConnectionStatus")`: CONNECTED | DISCONNECTED) —
/// "the connection state of the instance", i.e. whether the on-premises appliance is
/// talking to the service.
pub const CONNECTION_STATUS_CONNECTED: &str = "CONNECTED";
/// See [`CONNECTION_STATUS_CONNECTED`].
pub const CONNECTION_STATUS_DISCONNECTED: &str = "DISCONNECTED";

/// The `BridgeState` enum the model declares for a bridge
/// (`smithy_query(query="shape",shape="BridgeState")`: CREATING | STANDBY | STARTING |
/// DEPLOYING | ACTIVE | STOPPING | DELETING | DELETED | START_FAILED | START_PENDING |
/// STOP_FAILED | UPDATING). The two a bridge's creation reaches are named first; the
/// states a STARTED bridge moves through follow, because DeleteBridge's documented
/// precondition is stated against them (see [`is_stopped_bridge_state`]). UpdateBridgeState
/// is what moves a bridge between the settled ones (see [`bridge_transition_to`]).
pub const BRIDGE_STATE_CREATING: &str = "CREATING";
/// See [`BRIDGE_STATE_CREATING`]. A bridge that finished being created is not running
/// yet: it waits in `STANDBY` until UpdateBridgeState asks for a desired state
/// (`DesiredState`: ACTIVE | DELETED | STANDBY).
pub const BRIDGE_STATE_STANDBY: &str = "STANDBY";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_STARTING: &str = "STARTING";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_START_PENDING: &str = "START_PENDING";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_DEPLOYING: &str = "DEPLOYING";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_STOPPING: &str = "STOPPING";
/// See [`BRIDGE_STATE_CREATING`]. A bridge whose stop did not take effect is not a
/// stopped bridge.
pub const BRIDGE_STATE_STOP_FAILED: &str = "STOP_FAILED";
/// See [`BRIDGE_STATE_CREATING`]. A bridge whose start did not take effect never ran, so
/// it needs no stop.
pub const BRIDGE_STATE_START_FAILED: &str = "START_FAILED";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_UPDATING: &str = "UPDATING";
/// See [`BRIDGE_STATE_CREATING`]. The transient state a bridge enters when
/// UpdateBridgeState asks it to reach [`BRIDGE_STATE_DELETED`]; the clock settles it into
/// that state one [`TRANSITION_SPAN`] later.
pub const BRIDGE_STATE_DELETING: &str = "DELETING";
/// See [`BRIDGE_STATE_CREATING`]. The TERMINAL state of a bridge asked to reach the
/// `DELETED` member of `DesiredState`: its presence in the `BridgeState` enum is the
/// model's own statement that such a bridge stays a readable resource, exactly as
/// [`GATEWAY_STATE_DELETED`] is for a gateway, so UpdateBridgeState retires the record in
/// this state rather than evicting it. DeleteBridge is the other thing: it deletes the
/// RESOURCE and its answer carries only the ARN of the record it removed, so it drops the
/// record — and a bridge sitting in `DELETED` is stopped
/// ([`is_stopped_bridge_state`]), so that delete is admitted whenever a caller wants the
/// record gone.
pub const BRIDGE_STATE_DELETED: &str = "DELETED";

/// The pair of `BridgeState` values a bridge passes through to reach the `DesiredState`
/// `desired_state` names — the transient state it enters at once, and the settled state
/// the clock owes it one [`TRANSITION_SPAN`] later — or `None` when the value is not one
/// the `DesiredState` enum declares.
///
/// `DesiredState` (`smithy_query(query="shape",shape="DesiredState")`: ACTIVE | DELETED |
/// STANDBY) names the state a bridge is to END UP in, and each of its three values is
/// also a value of `BridgeState`, so the settled state is the desired state itself; the
/// transient one is the `BridgeState` value that reports that move being in progress
/// (STARTING toward ACTIVE, STOPPING toward STANDBY, DELETING toward DELETED). This is the
/// one mapping, so UpdateBridgeState cannot word a transition two ways.
pub fn bridge_transition_to(desired_state: &str) -> Option<(&'static str, &'static str)> {
    match desired_state {
        BRIDGE_STATE_ACTIVE => Some((BRIDGE_STATE_STARTING, BRIDGE_STATE_ACTIVE)),
        BRIDGE_STATE_STANDBY => Some((BRIDGE_STATE_STOPPING, BRIDGE_STATE_STANDBY)),
        BRIDGE_STATE_DELETED => Some((BRIDGE_STATE_DELETING, BRIDGE_STATE_DELETED)),
        _ => None,
    }
}

/// Whether a bridge in `bridge_state` is STOPPED — the precondition DeleteBridge
/// documents ("before you can delete a bridge, you must stop the bridge").
///
/// A bridge needs stopping exactly while it is started or moving to or from being
/// started: [`BRIDGE_STATE_STARTING`], [`BRIDGE_STATE_START_PENDING`],
/// [`BRIDGE_STATE_DEPLOYING`], [`BRIDGE_STATE_ACTIVE`], [`BRIDGE_STATE_STOPPING`],
/// [`BRIDGE_STATE_STOP_FAILED`] and [`BRIDGE_STATE_UPDATING`]. Every other state of the
/// modelled enum is one in which nothing is running — a bridge still being created, one
/// waiting in `STANDBY`, one whose start failed, or one already on its way out — so the
/// documented precondition is met.
pub fn is_stopped_bridge_state(bridge_state: &str) -> bool {
    !matches!(
        bridge_state,
        BRIDGE_STATE_STARTING
            | BRIDGE_STATE_START_PENDING
            | BRIDGE_STATE_DEPLOYING
            | BRIDGE_STATE_ACTIVE
            | BRIDGE_STATE_STOPPING
            | BRIDGE_STATE_STOP_FAILED
            | BRIDGE_STATE_UPDATING
    )
}

/// The `RouterInputState` enum the model declares for a router input
/// (`smithy_query(query="shape",shape="RouterInputState")`: CREATING | STANDBY | STARTING
/// | ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING). Only the
/// states a router input's create, start, restart and delete reach are named here; the rest
/// belong to the router-input operations that are still scaffolded (stop, update, migrate).
///
/// The non-terminal state a router input is answered in while it is being created; the
/// clock settles it into [`ROUTER_INPUT_STATE_STANDBY`] one [`TRANSITION_SPAN`] later.
pub const ROUTER_INPUT_STATE_CREATING: &str = "CREATING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. A router input that finished being created is not
/// running yet: it waits in `STANDBY` until StartRouterInput starts it, exactly as a
/// created bridge waits (see [`BRIDGE_STATE_STANDBY`]).
pub const ROUTER_INPUT_STATE_STANDBY: &str = "STANDBY";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The non-terminal state a router input is answered
/// in while StartRouterInput's start is in flight; the clock settles it into
/// [`ROUTER_INPUT_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later.
pub const ROUTER_INPUT_STATE_STARTING: &str = "STARTING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The state of a router input that is running.
pub const ROUTER_INPUT_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The non-terminal state a router input is answered
/// in while a restart is in flight: RestartRouterInput "can be used to recover from errors
/// or refresh the input state"
/// (`smithy_query(query="doc",shape="RestartRouterInput")`), which is what this state
/// reports is happening.
pub const ROUTER_INPUT_STATE_RECOVERING: &str = "RECOVERING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The state of a router input that is not carrying
/// content because something is wrong with it — the state RestartRouterInput recovers
/// from. Nothing in this emulator PUTS an input here (there is no data plane to fail), so
/// it is reached only by a caller seeding the store; a restart of such an input returns it
/// to [`ROUTER_INPUT_STATE_ACTIVE`].
pub const ROUTER_INPUT_STATE_ERROR: &str = "ERROR";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The non-terminal state a router input is answered
/// in while its deletion is in progress.
///
/// There is no terminal `DELETED` value beside it — unlike `GatewayState`, which carries
/// one (see [`GATEWAY_STATE_DELETED`]) — so a deleted router input is a resource that
/// stops existing rather than one that stays readable: it is reported `DELETING` while the
/// deletion is in progress and is then gone (see [`StoredRouterInput::removal_due_at`]).
pub const ROUTER_INPUT_STATE_DELETING: &str = "DELETING";

/// `ListedBridge$BridgeType` ("the type of the bridge") for a cloud-to-ground bridge —
/// the kind `Bridge$EgressGatewayBridge` makes it ("an egress bridge is a cloud-to-ground
/// bridge"). The two bridge-type members are exclusive, so a bridge reports exactly one of
/// these two.
pub const BRIDGE_TYPE_EGRESS: &str = "egress";
/// See [`BRIDGE_TYPE_EGRESS`]: a ground-to-cloud bridge, the kind
/// `Bridge$IngressGatewayBridge` makes it ("an ingress bridge is a ground-to-cloud
/// bridge").
pub const BRIDGE_TYPE_INGRESS: &str = "ingress";

/// `SourceType` (`smithy_query(query="shape",shape="SourceType")`): a source that
/// originated in this account is `OWNED`; one reached through another account's
/// entitlement is `ENTITLED`.
pub const SOURCE_TYPE_OWNED: &str = "OWNED";
/// See [`SOURCE_TYPE_OWNED`].
pub const SOURCE_TYPE_ENTITLED: &str = "ENTITLED";

/// `OutputStatus` / `EntitlementStatus` enum value the model documents as the default
/// MediaConnect applies when the request omits the member: "If you don't specify the
/// outputStatus field in your request, MediaConnect sets it to ENABLED"
/// (`AddOutputRequest$OutputStatus`), and the same sentence on
/// `GrantEntitlementRequest$EntitlementStatus`.
pub const STATUS_ENABLED: &str = "ENABLED";

/// The `State` enum (`smithy_query(query="shape",shape="State")`: ENABLED | DISABLED) —
/// the two values every on/off setting of this service targets, from a flow's failover
/// state to a source's or an output's router integration state. Named here once so the
/// operations that read a state and the projections that report one cannot drift apart.
pub const STATE_ENABLED: &str = "ENABLED";
/// See [`STATE_ENABLED`].
pub const STATE_DISABLED: &str = "DISABLED";

/// The `RouterIntegrationState` a source or an output reports, given the state the
/// request that configured it carried (`None` when it carried none).
///
/// `Source$RouterIntegrationState` ("indicates if router integration is enabled or
/// disabled on the flow source") and `Output$RouterIntegrationState` both target the
/// `State` enum and are reported on every source and every output, so a record whose
/// request never mentioned router integration still reports a state: the disabled one,
/// because router integration is off until a request asks for it. Every projection of a
/// source or an output resolves it HERE, so one stored record always reports one state.
pub fn reported_router_integration_state(configured: Option<&str>) -> Option<String> {
    Some(configured.unwrap_or(STATE_DISABLED).to_string())
}

/// How long a flow stays in a transient status (`STARTING`, `STOPPING`) before the
/// clock settles it into the steady one.
///
/// AWS documents no duration for this transition, so the span is this emulator's own
/// choice inside the project's 250-750 ms window: non-zero, so a caller that polls
/// immediately after StartFlow still observes `STARTING`, and short enough that a test
/// can wait it out.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace and sigv4 signing name (`OPERATIONS.md`): the third
/// segment of every ARN the service mints.
pub const SERVICE_NAMESPACE: &str = "mediaconnect";

/// The prefix MediaConnect puts in front of the identifier a request named when it
/// resolves no resource: the whole `NotFoundException$Message` is this prefix followed by
/// that identifier, and nothing else.
///
/// `NotFoundException` models `Message` as **required** and the model states the precise
/// meaning of the failure "is documented in the error message", so the message is part of
/// what a client reads — a caller that parses the ARN back out of a not-found message
/// reads it from here. Every `NotFoundException` this crate returns is built through
/// [`not_found_message`] so no handler can word one its own way.
pub const NOT_FOUND_PREFIX: &str = "Not found: ";

/// The prefix MediaConnect puts in front of the detail of a refused request, followed by a
/// newline: `BadRequestException$Message` is `Validation failed:` then a line stating what
/// about the request was unacceptable.
///
/// Operators and clients tell a validation refusal from a semantic one by this prefix, so
/// every `BadRequestException` this crate returns ABOUT THE REQUEST'S CONTENT is built
/// through [`validation_failed_message`] — including the ones the framework's own boundary
/// walk raises, which `crate::service::EmulatedService::override_refusal` words through the
/// same function. A request refused by the addressed resource's STATE instead carries the
/// other envelope, [`BAD_REQUEST_PREFIX`].
pub const VALIDATION_FAILED_PREFIX: &str = "Validation failed:";

/// The prefix MediaConnect puts in front of the detail of a request refused for a
/// SEMANTIC reason — the request is well formed, but the resource it addresses is not in a
/// state that admits it: `BadRequestException$Message` is `Bad request: ` then the
/// sentence naming the precondition.
///
/// `BadRequestException` is "thrown if the request contains a semantic error. The precise
/// meaning depends on the API, and is documented in the error message", so the two kinds
/// of refusal this service answers 400 for read differently: a request whose CONTENT is
/// unacceptable carries [`VALIDATION_FAILED_PREFIX`], and one refused by the addressed
/// resource's STATE carries this prefix. The status and the error code are the contract;
/// the sentence after the envelope is this emulator's wording of the documented
/// precondition.
pub const BAD_REQUEST_PREFIX: &str = "Bad request: ";

/// MediaConnect's `NotFoundException` message for a request that named `identifier` — an
/// ARN, or whatever else the request addressed the resource by — and resolved no resource.
///
/// The message is derived from the REQUEST, never from the store, so it is the same
/// whether the resource never existed, was deleted, or was never an ARN at all.
///
/// TOTAL for every `identifier` a request can carry: one `format!` of a borrowed `&str`,
/// with no parsing, no indexing or slicing, no `unwrap`/`expect` and no arithmetic — so a
/// label that satisfies no pattern (the flow labels deliberately hand their value to the
/// handler unjudged; see the note on `DescribeFlowInput` in `crate::ops::types`) cannot
/// panic here, whatever bytes it holds. The identifier is a URI path label, so what it
/// adds to the response is bounded by the request line that carried it.
pub fn not_found_message(identifier: &str) -> String {
    format!("{NOT_FOUND_PREFIX}{identifier}")
}

/// MediaConnect's `BadRequestException` message for a request refused as unacceptable:
/// [`VALIDATION_FAILED_PREFIX`], a newline, then `detail`.
///
/// `detail` states what was wrong — the member and the rule it broke. The envelope is the
/// contract; the wording of the detail is this emulator's.
///
/// TOTAL for every `detail`, for the same reason as [`not_found_message`]: one `format!`
/// of a borrowed `&str`.
pub fn validation_failed_message(detail: &str) -> String {
    format!("{VALIDATION_FAILED_PREFIX}\n{detail}")
}

/// MediaConnect's `BadRequestException` message for a well-formed request the addressed
/// resource's STATE refuses: [`BAD_REQUEST_PREFIX`] then `detail`.
///
/// TOTAL for every `detail`, for the same reason as [`validation_failed_message`].
pub fn bad_request_message(detail: &str) -> String {
    format!("{BAD_REQUEST_PREFIX}{detail}")
}

/// The refusal message for a lifecycle precondition an operation documents: `resource`
/// (`Flow`, `Gateway`, `Bridge`, …) may be addressed by `operation` (the verb as the API
/// names it) only while it is in `required_state`.
///
/// Every operation whose documentation states such a precondition words its refusal HERE,
/// so one rule cannot be worded two ways: the operation names the state its documentation
/// requires and this builds the one envelope a client reads
/// ([`bad_request_message`]).
pub fn state_precondition_message(resource: &str, required_state: &str, operation: &str) -> String {
    bad_request_message(&format!(
        "{resource} must be in {required_state} in order for {operation} to be invoked."
    ))
}

/// The ceiling on how many flows this store holds: the cap that bounds caller-driven
/// growth of the flow map.
///
/// This emulator keeps its resources in memory for the life of the process and
/// CreateFlow is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// DescribeFlow must still answer what CreateFlow created. CreateFlow refuses past this
/// cap with the service's own modelled `CreateFlow420Exception`, "raised by Elemental
/// MediaConnect when creating the flow" and declared on CreateFlow alone (never on a
/// read). The number is the emulator's own bound, not a published AWS default — AWS
/// publishes the flows-per-Region quota as adjustable, so no single value is the
/// contract — and it is high enough that no ordinary workflow meets it.
pub const MAX_FLOWS: usize = 5_000;

/// The ceiling on how many entries one CreateFlow request may put in any one of a
/// flow's sub-collections (sources, outputs, entitlements, media streams, VPC
/// interfaces, tags), so a single request cannot store an arbitrarily large record.
///
/// For outputs and entitlements this is the limit the operation itself documents: "The
/// request optionally can include outputs (up to 50) and entitlements (up to 50)"
/// (`smithy_query(query="operation",operation="CreateFlow")`), and it is also the
/// tags-per-resource limit AWS applies service-wide. AWS documents no count for the
/// remaining collections, so the same number is applied to them as the emulator's own
/// bound. CreateFlow refuses past it with `BadRequestException`, which the model
/// describes as the error for "a semantic error" in the request.
pub const MAX_FLOW_COLLECTION_ENTRIES: usize = 50;

/// The ceiling on how many gateways this store holds, for the same reason as
/// [`MAX_FLOWS`]: nothing evicts a gateway and DescribeGateway must still answer what
/// CreateGateway created, so the store needs a maximum entry count. CreateGateway refuses
/// past it with the service's own modelled `CreateGateway420Exception`, "raised by
/// Elemental MediaConnect when creating the gateway" and declared on CreateGateway alone.
/// The number is the emulator's own bound, not a published AWS default, and it is high
/// enough that no ordinary workflow meets it.
pub const MAX_GATEWAYS: usize = 1_000;

/// How many RETIRED (deleted) gateway records this store keeps beside its live ones.
///
/// A gateway delete is a soft delete, so a deleted gateway stays readable in
/// [`GATEWAY_STATE_DELETED`] (see [`StoredGateway::deleted_at`]) — which, left unbounded,
/// would let a create-then-delete loop grow the gateway map without limit. So the store
/// retains the most recently deleted gateways up to this count and drops the oldest
/// retired record past it, exactly as [`MAX_GATEWAYS`] bounds the live ones. The number is
/// this emulator's own bound (AWS publishes no retention for a deleted gateway) and it is
/// high enough that a client polling a delete it just made always finds its record.
pub const MAX_RETIRED_GATEWAYS: usize = MAX_GATEWAYS;

/// How many networks one gateway may carry: "Creates a new gateway. The request must
/// include at least one network (up to four)"
/// (`smithy_query(query="operation",operation="CreateGateway")`). CreateGateway refuses a
/// request outside this range with `BadRequestException`, the error the model describes
/// as the one for "a semantic error" in the request.
pub const MAX_GATEWAY_NETWORKS: usize = 4;

/// The most egress CIDR blocks one gateway holds, so one request cannot store a record of
/// caller-chosen size in a store nothing evicts (and that DescribeGateway echoes back
/// whole).
///
/// AWS publishes no count for `EgressCidrBlocks` and the model puts no `@length` on it, so
/// the bound is the one this crate already applies to a request-sized collection
/// ([`MAX_FLOW_COLLECTION_ENTRIES`], CreateFlow's per-request bound), named through that
/// constant so the two cannot drift. CreateGateway refuses past it with
/// `BadRequestException`, the error the model describes as the one for "a semantic error"
/// in the request.
pub const MAX_GATEWAY_EGRESS_CIDR_BLOCKS: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The ceiling on how many bridges this store holds, for the same reason as
/// [`MAX_GATEWAYS`]: nothing evicts a bridge and DescribeBridge must still answer what
/// CreateBridge created, so the store needs a maximum entry count. CreateBridge refuses
/// past it with the service's own modelled `CreateBridge420Exception`, "raised by
/// Elemental MediaConnect when creating the bridge" and declared on CreateBridge alone
/// (never on a read). The number is the emulator's own bound, not a published AWS
/// default, and it is high enough that no ordinary workflow meets it.
pub const MAX_BRIDGES: usize = 1_000;

/// The ceiling on how many router inputs this store holds, for the same reason as
/// [`MAX_GATEWAYS`]: nothing evicts a router input and GetRouterInput must still answer
/// what CreateRouterInput created, so the store needs a maximum entry count.
/// CreateRouterInput refuses past it with the service's own modelled
/// `RouterInputServiceQuotaExceededException`, the 420 error declared on that create alone
/// (never on a read). The number is the emulator's own bound, not a published AWS default,
/// and it is high enough that no ordinary workflow meets it.
pub const MAX_ROUTER_INPUTS: usize = 1_000;

/// The ceiling on how many sources one bridge carries.
///
/// AWS publishes no count for a bridge's sources and the model puts no `@length` on
/// `__listOfAddBridgeSourceRequest`, so the bound is the one this crate already applies to
/// a request-sized collection ([`MAX_FLOW_COLLECTION_ENTRIES`]), named through that
/// constant so the two cannot drift. CreateBridge refuses past it with
/// `BadRequestException`, the error the model describes as the one for "a semantic error"
/// in the request.
pub const MAX_BRIDGE_SOURCES: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The ceiling on how many outputs one bridge carries, for the same reason as
/// [`MAX_BRIDGE_SOURCES`].
pub const MAX_BRIDGE_OUTPUTS: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// How many outputs a bridge may hold, read off the bridge's own type.
///
/// `ingress_max_outputs` is `Some` for a ground-to-cloud bridge, carrying the value of its
/// REQUIRED `IngressGatewayBridge$MaxOutputs` — "the maximum number of outputs on the
/// ingress bridge" — and `None` for a cloud-to-ground bridge, whose `EgressGatewayBridge`
/// declares no output count at all, so the only bound on it is the one this crate applies
/// to every bridge ([`MAX_BRIDGE_OUTPUTS`]).
///
/// A declared maximum is never read as room for more than the store itself holds, and a
/// non-positive declaration is room for nothing: the member is an `Integer`, so the model
/// alone does not keep a negative one out.
///
/// What a ground-to-cloud bridge declares bounds the outputs it can ever hold — the flow
/// outputs the service materialises for it — and not the ones a request configures, of
/// which it takes none at all (see [`request_bridge_output_rejection`], which refuses
/// those by the bridge's direction before it counts anything).
pub fn bridge_output_capacity(ingress_max_outputs: Option<i32>) -> usize {
    match ingress_max_outputs {
        Some(declared) => usize::try_from(declared)
            .unwrap_or(0)
            .min(MAX_BRIDGE_OUTPUTS),
        None => MAX_BRIDGE_OUTPUTS,
    }
}

/// Why a bridge cannot take `adding` more outputs a REQUEST configures on top of the
/// `held` it already holds, or `None` when it can. `ingress_max_outputs` is the bridge's
/// own type, as [`bridge_output_capacity`] reads it.
///
/// Two rules, both about the direction a bridge carries content in, decided in that
/// order — which bridge takes a request-configured output at all, and then how many it
/// may hold:
///
/// * an output a request configures is a `NetworkOutput` — the only member
///   `AddBridgeOutputRequest` has, "delivered to your premises" — which is what a
///   cloud-to-ground bridge delivers ("the content comes from an existing MediaConnect
///   flow and is delivered to your premises"). A ground-to-cloud bridge delivers the
///   other way ("the content originates at your premises and is delivered to the cloud"),
///   so its outputs are the `BridgeFlowOutput`s the service itself materialises when a
///   cloud flow takes the bridge as its source (`SetGatewayBridgeSourceRequest`), and
///   none a request carries; and
/// * a bridge that does take them holds no more outputs than its own type declares
///   ([`bridge_output_capacity`]), which for a cloud-to-ground bridge is the bound this
///   crate applies to every bridge, its `EgressGatewayBridge` declaring no output count.
///
/// A request that configures no output at all admits nothing, so it is nothing this
/// decides: the operations refuse an empty required list in their own words.
///
/// Every operation that puts request-configured outputs on a bridge (CreateBridge's
/// `Outputs`, AddBridgeOutputs) refuses through this one function, with
/// `BadRequestException` — the error the model describes as the one for "a semantic
/// error" in the request.
pub fn request_bridge_output_rejection(
    ingress_max_outputs: Option<i32>,
    held: usize,
    adding: usize,
) -> Option<String> {
    if adding == 0 {
        return None;
    }
    if ingress_max_outputs.is_some() {
        return Some(
            "An output a request configures is a network output, which is delivered to the \
             premises network — what a cloud-to-ground bridge delivers. This bridge is a \
             ground-to-cloud bridge: its outputs are the flow outputs created for it when a \
             flow takes the bridge as its source, so it accepts no output in a request."
                .to_string(),
        );
    }
    let capacity = bridge_output_capacity(ingress_max_outputs);
    if held + adding > capacity {
        return Some(format!(
            "This bridge holds at most {capacity} outputs; it already holds {held} and this \
             request adds {adding}."
        ));
    }
    None
}

/// The most tags one resource carries, checked against the MERGED set a tag write would
/// leave on the resource, so repeated tag writes cannot grow a resource's tag map past
/// what a single create could set.
///
/// The number is [`MAX_FLOW_COLLECTION_ENTRIES`] — the bound CreateFlow applies to one
/// request's `FlowTags`, and the tags-per-resource limit AWS applies service-wide — named
/// once so the create side and the tagging side cannot drift apart. TagResource refuses
/// past it with `BadRequestException`, which the model describes as the error for "a
/// semantic error" in the request.
pub const MAX_TAGS_PER_RESOURCE: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The longest tag key this service stores: "Tag keys can have a maximum character length
/// of 128 characters" (`smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`).
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value this service stores: "tag values can have a maximum length of 256
/// characters" (same member doc as [`MAX_TAG_KEY_LENGTH`]).
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Why `key` / `value` is not a tag pair this service stores, or `None` when it is.
///
/// The model declares `__mapOfString` without length bounds, so the boundary validator
/// passes a pair the service itself would refuse: the lengths come from the member's own
/// documentation ([`MAX_TAG_KEY_LENGTH`], [`MAX_TAG_VALUE_LENGTH`]) and a tag is addressed
/// by its key, so an empty key names no tag. Both bounds count CHARACTERS, as the
/// documentation does. Every operation that writes tags (CreateFlow's `FlowTags`,
/// TagResource) refuses through this one function.
pub fn tag_pair_rejection(key: &str, value: &str) -> Option<String> {
    if key.is_empty() {
        return Some("A tag key must not be empty.".to_string());
    }
    let key_length = key.chars().count();
    let value_length = value.chars().count();
    if key_length > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "Tag key {key:?} is {key_length} characters long; a tag key may have at most \
             {MAX_TAG_KEY_LENGTH} characters."
        ));
    }
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Some(format!(
            "The value of tag {key:?} is {value_length} characters long; a tag value may have \
             at most {MAX_TAG_VALUE_LENGTH} characters."
        ));
    }
    None
}

/// The tag map a resource is created with, from the tag member of the request that created
/// it (`FlowTags`, `OutputTags`, `SourceTags`, `EntitlementTags`, `MediaStreamTags`): the
/// pairs the member carries, ordered, and EMPTY when the member is absent — a resource
/// created without tags carries no tags, which is not the same as carrying none yet known.
pub fn tag_map_of(
    tags: Option<&std::collections::HashMap<String, String>>,
) -> BTreeMap<String, String> {
    tags.into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Why the tag member `tags` of a create request is not a tag map this service stores on
/// `resource`, or `None` when it is.
///
/// Every tag member of a create request writes the same kind of tag map TagResource writes
/// on the same resource, so it is bounded by the same two rules, read from the same place:
/// each pair through [`tag_pair_rejection`], and the count through
/// [`MAX_TAGS_PER_RESOURCE`] — a create cannot leave a resource carrying more tags than a
/// tag write could put on it. The caller refuses past either with its own
/// `BadRequestException`, the error the model describes for "a semantic error" in the
/// request.
pub fn tag_set_rejection(
    resource: &str,
    tags: Option<&std::collections::HashMap<String, String>>,
) -> Option<String> {
    let pairs = tags?;
    if pairs.len() > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "A resource keeps at most {MAX_TAGS_PER_RESOURCE} tags; this request would give \
             {resource} {}.",
            pairs.len()
        ));
    }
    pairs
        .iter()
        .find_map(|(key, value)| tag_pair_rejection(key, value))
}

/// The ceiling on how many outputs ONE FLOW holds, however many calls fill it: "You can
/// create up to 50 outputs per flow"
/// (`smithy_query(query="operation",operation="AddFlowOutputs")`), the one count AWS
/// publishes for a flow's outputs. It happens to equal
/// [`MAX_FLOW_COLLECTION_ENTRIES`] — CreateFlow's per-request bound — but it is a
/// different rule: this one bounds the STORED flow, so a sequence of adds cannot grow
/// one past it. AddFlowOutputs refuses past it with its own modelled
/// `AddFlowOutputs420Exception`, "raised by Elemental MediaConnect when adding the flow
/// output".
pub const MAX_FLOW_OUTPUTS: usize = 50;

/// The ceiling on how many entitlements ONE FLOW holds, however many calls fill it: "the
/// request optionally can include outputs (up to 50) and entitlements (up to 50)"
/// (`smithy_query(query="operation",operation="CreateFlow")`), the one count AWS
/// publishes for a flow's entitlements. Like [`MAX_FLOW_OUTPUTS`] it bounds the STORED
/// flow, so a sequence of grants cannot grow one past it. GrantFlowEntitlements refuses
/// past it with its own modelled `GrantFlowEntitlements420Exception`, "raised by Elemental
/// MediaConnect when granting the entitlement".
pub const MAX_FLOW_ENTITLEMENTS: usize = 50;

/// The ceiling on how many media streams ONE FLOW holds, however many calls fill it.
///
/// AWS publishes no count for a flow's media streams, so the number is
/// [`MAX_FLOW_COLLECTION_ENTRIES`] — the bound CreateFlow applies to one request's
/// `MediaStreams` — named once so the create side and the add side cannot drift apart.
/// Like [`MAX_FLOW_OUTPUTS`] it is a different RULE from that per-request bound: this one
/// bounds the STORED flow, so a sequence of adds cannot grow one past what a single
/// create could store. AddFlowMediaStreams models no 420, so it refuses past this bound
/// with `BadRequestException`, the error the model describes for "a semantic error" in
/// the request.
pub const MAX_FLOW_MEDIA_STREAMS: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The ceiling on how many sources ONE FLOW holds while its source failover is ACTIVE,
/// however many calls fill it.
///
/// `FailoverConfig$State` (`smithy_query(query="doc",shape="FailoverConfig",member="State")`)
/// publishes the count: "The state of source failover on the flow. If the state is
/// inactive, the flow can have only one source. If the state is active, the flow can have
/// one or two sources." So a flow's sources are bounded by its own failover state, not by
/// a generic collection bound — see [`max_flow_sources`], the one place that resolves the
/// bound, and [`MAX_FLOW_SOURCES_FAILOVER_INACTIVE`] for the other half.
///
/// Like [`MAX_FLOW_MEDIA_STREAMS`] this bounds the STORED flow, so a sequence of adds
/// cannot grow one past what a single create could store. Neither CreateFlow nor
/// AddFlowSources models a count exception for it, so both refuse past the bound with
/// `BadRequestException`, the error the model describes for "a semantic error" in the
/// request.
pub const MAX_FLOW_SOURCES: usize = 2;

/// The ceiling on how many sources ONE FLOW holds while its source failover is INACTIVE:
/// "if the state is inactive, the flow can have only one source" (`FailoverConfig$State`).
/// See [`MAX_FLOW_SOURCES`] and [`max_flow_sources`].
pub const MAX_FLOW_SOURCES_FAILOVER_INACTIVE: usize = 1;

/// Whether source failover is ACTIVE on a resource holding `config`.
///
/// `FailoverConfig$State` targets the `State` enum (ENABLED | DISABLED), so failover is
/// active exactly while that state is [`STATE_ENABLED`]: a resource that holds no failover
/// config at all, and one whose config leaves `State` unset, has failover inactive —
/// a setting nothing turned on is off, the same reading
/// [`reported_router_integration_state`] applies to the other members of this enum.
pub fn source_failover_is_active(config: Option<&FailoverConfig>) -> bool {
    config.and_then(|config| config.state.as_deref()) == Some(STATE_ENABLED)
}

/// How many sources a flow whose source failover config is `config` may hold:
/// [`MAX_FLOW_SOURCES`] while failover is active, and
/// [`MAX_FLOW_SOURCES_FAILOVER_INACTIVE`] while it is not.
///
/// Every operation that decides a flow's source count reads the bound HERE — CreateFlow's
/// `Sources`/`Source`, AddFlowSources' append and UpdateFlow's `SourceFailoverConfig`
/// (which moves the bound under the sources the flow already holds) — so the add side, the
/// create side and the failover side cannot drift apart.
pub fn max_flow_sources(config: Option<&FailoverConfig>) -> usize {
    if source_failover_is_active(config) {
        MAX_FLOW_SOURCES
    } else {
        MAX_FLOW_SOURCES_FAILOVER_INACTIVE
    }
}

/// The detail a request is refused with when it would leave a flow holding more sources
/// than its failover state allows: `held` sources already stored, `adding` more requested
/// (zero on a request that only moves the failover state), under `config`.
///
/// Both halves of the rule are worded here once, so the create path, the add path and
/// UpdateFlow state the same bound for the same reason.
pub fn flow_source_bound_rejection(
    config: Option<&FailoverConfig>,
    held: usize,
    adding: usize,
) -> Option<String> {
    let bound = max_flow_sources(config);
    let total = held + adding;
    if total <= bound {
        return None;
    }
    let state = if source_failover_is_active(config) {
        format!("{STATE_ENABLED} (active)")
    } else {
        match config.and_then(|config| config.state.as_deref()) {
            Some(state) => format!("{state} (inactive)"),
            None => "unset (inactive)".to_string(),
        }
    };
    Some(format!(
        "A flow whose source failover state is {state} can hold at most {bound} \
         source(s); this request would leave it with {total} ({held} already stored, \
         {adding} requested). If the state is inactive, the flow can have only one \
         source; if the state is active, the flow can have one or two sources."
    ))
}

/// The `FlowSize` values (`smithy_query(query="shape",shape="FlowSize")`: MEDIUM | LARGE |
/// LARGE_4X) whose "processing capacity and feature set" carries NDI®.
///
/// `CreateFlowRequest$FlowSize` is "set this optional parameter to `LARGE` if you want to
/// enable NDI sources or outputs on the flow", and UpdateFlow's own documentation says "if
/// you have a MEDIUM flow and you want to change the flow source to NDI: first, use the
/// UpdateFlow operation to upgrade the flow size to LARGE". The capable sizes are therefore
/// an ALLOW-list — `LARGE` and the larger `LARGE_4X` — so a flow whose size was never set
/// is not NDI-capable either. Named ONCE here because every operation that can put NDI on a
/// flow reads it (CreateFlow, AddFlowSources, AddFlowOutputs, UpdateFlowSource,
/// UpdateFlowOutput, UpdateFlow).
pub const NDI_CAPABLE_FLOW_SIZES: [&str; 2] = ["LARGE", "LARGE_4X"];

/// The `Protocol` value that names the NDI® transport
/// (`smithy_query(query="shape",shape="Protocol")`: … | ndi-speed-hq). A source or an
/// output whose transport uses it is an NDI source or output.
pub const PROTOCOL_NDI_SPEED_HQ: &str = "ndi-speed-hq";

/// Whether `flow_size` is one of [`NDI_CAPABLE_FLOW_SIZES`]. A flow whose size was never
/// set is NOT: the capable sizes are the ones the documentation names.
pub fn flow_size_supports_ndi(flow_size: Option<&str>) -> bool {
    flow_size.is_some_and(|size| NDI_CAPABLE_FLOW_SIZES.contains(&size))
}

/// Whether `config` turns NDI® on: `NdiConfig$NdiState` is "a setting that controls whether
/// NDI sources or outputs can be used in the flow… The default value is DISABLED. This
/// value must be set as ENABLED for your flow to support NDI sources or outputs", so a flow
/// with no `NdiConfig`, or one whose state is unset, has NDI off.
pub fn ndi_is_enabled(config: Option<&NdiConfig>) -> bool {
    config.and_then(|config| config.ndi_state.as_deref()) == Some(STATE_ENABLED)
}

/// Whether a transport described by `protocol` and whether it carries `NdiSourceSettings`
/// is an NDI® transport.
///
/// The TWO markers of NDI on a source or an output, read here once: the protocol is
/// [`PROTOCOL_NDI_SPEED_HQ`], or the record carries `NdiSourceSettings` ("the settings for
/// the NDI® source… the exact name of the upstream NDI sender"), which only an NDI source
/// has. Every caller — the stored record, the create/add request shapes and the update
/// request shapes, which are all different types carrying the same two members — resolves
/// NDI through this function.
pub fn uses_ndi_transport(protocol: Option<&str>, ndi_source_settings: bool) -> bool {
    protocol == Some(PROTOCOL_NDI_SPEED_HQ) || ndi_source_settings
}

/// Whether `transport` is an NDI® transport — see [`uses_ndi_transport`].
pub fn transport_uses_ndi(transport: Option<&Transport>) -> bool {
    transport.is_some_and(|transport| {
        uses_ndi_transport(
            Some(transport.protocol.as_str()),
            transport.ndi_source_settings.is_some(),
        )
    })
}

/// Whether a stored source is an NDI® source — see [`transport_uses_ndi`].
pub fn stored_source_uses_ndi(source: &StoredSource) -> bool {
    transport_uses_ndi(source.transport.as_ref())
}

/// Whether a `SetSourceRequest` describes an NDI® source: the same two markers
/// [`uses_ndi_transport`] reads, on the request the source is materialised from.
pub fn source_request_uses_ndi(request: &SetSourceRequest) -> bool {
    uses_ndi_transport(
        request.protocol.as_deref(),
        request.ndi_source_settings.is_some(),
    )
}

/// Whether an `AddOutputRequest` describes an NDI® output: its protocol is the NDI
/// transport. `NdiSourceSettings` is a SOURCE's member, and the request's other `Ndi…`
/// members tune an NDI sender the output only has when that protocol is what carries it
/// (`AddOutputRequest$NdiProgramName` is "a suffix for the name of the NDI® sender that the
/// flow creates"), so the PROTOCOL is what makes an output an NDI output.
pub fn output_request_uses_ndi(request: &AddOutputRequest) -> bool {
    uses_ndi_transport(request.protocol.as_deref(), false)
}

/// The detail a request is refused with when it would leave a flow including NDI® at a size
/// that does not carry it: `ndi_feature` names what uses NDI, `flow_size` is the size the
/// flow ends up with. `None` when that size is NDI-capable ([`flow_size_supports_ndi`]), so
/// a caller states the rule by naming the feature and the size and nothing else.
///
/// Every operation that can put NDI on a flow words the refusal HERE, so the create side,
/// the add sides and the update sides state one rule for one reason.
pub fn ndi_flow_size_rejection(ndi_feature: &str, flow_size: Option<&str>) -> Option<String> {
    if flow_size_supports_ndi(flow_size) {
        return None;
    }
    Some(format!(
        "{ndi_feature} needs a flow of size {}; this flow's size is {}. Set the flow size to \
         one that supports NDI before enabling NDI sources or outputs on it.",
        NDI_CAPABLE_FLOW_SIZES.join(" or "),
        flow_size.unwrap_or("unset")
    ))
}

/// Whether `hour` is a maintenance start hour: "UTC time when the maintenance will happen.
/// Use 24-hour HH:MM format. Minutes must be 00. Example: 13:00."
/// (`smithy_query(query="doc",shape="AddMaintenance",member="MaintenanceStartHour")`, the
/// same sentence `UpdateMaintenance$MaintenanceStartHour` and
/// `Maintenance$MaintenanceStartHour` carry).
pub fn is_maintenance_start_hour(hour: &str) -> bool {
    let Some((hours, minutes)) = hour.split_once(':') else {
        return false;
    };
    hours.len() == 2
        && minutes == "00"
        && hours.bytes().all(|byte| byte.is_ascii_digit())
        && hours
            .parse::<u8>()
            .is_ok_and(|hours| (0..=23).contains(&hours))
}

/// Whether `date` is a scheduled maintenance date: "a scheduled date in ISO UTC format
/// when the maintenance will happen. Use YYYY-MM-DD format. Example: 2021-01-30."
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceScheduledDate")`).
/// The day is bounded by the longest month rather than by the month named, which is as far
/// as the documented format goes.
pub fn is_maintenance_scheduled_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return false;
    };
    year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && [year, month, day]
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
        && year.parse::<u16>().is_ok()
        && month
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
        && day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
}

/// The detail a request asking for a maintenance window is refused with when one of the
/// two free-form members it carries is not in its documented format, or `None` when the
/// window is expressible as one.
///
/// Every operation that WRITES a flow's maintenance window — the create shape
/// (`AddMaintenance`, which requires the start hour) and the update shape
/// (`UpdateMaintenance`, whose members are each optional) — checks it here, so one rule
/// refuses one value for one reason whichever path stores it.
pub fn maintenance_window_rejection(
    maintenance_start_hour: Option<&str>,
    maintenance_scheduled_date: Option<&str>,
) -> Option<String> {
    if let Some(hour) = maintenance_start_hour.filter(|hour| !is_maintenance_start_hour(hour)) {
        return Some(format!(
            "Invalid MaintenanceStartHour {hour:?}. Use 24-hour HH:MM format with minutes 00, \
             for example 13:00."
        ));
    }
    if let Some(date) =
        maintenance_scheduled_date.filter(|date| !is_maintenance_scheduled_date(date))
    {
        return Some(format!(
            "Invalid MaintenanceScheduledDate {date:?}. Use YYYY-MM-DD format, for example \
             2021-01-30."
        ));
    }
    None
}

/// The detail a request naming an Availability Zone is refused with when that zone is not
/// one of `region`, or `None` when it is.
///
/// Every member a request can name a zone in is "limited to the Availability Zones within
/// the current Region" (`CreateFlowRequest$AvailabilityZone`) or "must be a valid
/// Availability Zone for the region specified by regionName, or the current region if no
/// regionName is provided" (`CreateRouterInputRequest$AvailabilityZone`), so each create
/// path words the refusal HERE, over the one encoding of the zone naming
/// ([`is_zone_of_region`]).
fn availability_zone_rejection(zone: &str, region: &str) -> Option<String> {
    if is_zone_of_region(zone, region) {
        return None;
    }
    Some(format!(
        "AvailabilityZone {zone:?} is not an Availability Zone of Region {region:?}."
    ))
}

/// The Availability Zone a create STORES for a request that named `requested` in `region`,
/// or `Err` with the detail the request is refused with when that zone is not one of
/// `region` ([`availability_zone_rejection`]). A request that names none is given a zone of
/// the Region ([`default_availability_zone`]), because every resource that carries a zone
/// reports one.
///
/// This is the crate's ONE resolution of the member, so a zone the flow create stores is a
/// zone the router creates would also accept, and no path stores one its own Region has
/// not got.
pub fn resolved_availability_zone(requested: Option<&str>, region: &str) -> Result<String, String> {
    match requested {
        Some(zone) => match availability_zone_rejection(zone, region) {
            Some(detail) => Err(detail),
            None => Ok(zone.to_string()),
        },
        None => Ok(default_availability_zone(region)),
    }
}

/// The first port handed to a source that did not ask for one. MediaConnect assigns an
/// ingest port from its own pool when the request omits `IngestPort`; the emulator
/// assigns from a WRAPPING pool that starts at this base, so a source's port is drawn
/// from the pool rather than guaranteed unique — two sources far enough apart in the
/// ARN counter can share one, which is harmless because nothing in the emulator binds
/// these ports.
const INGEST_PORT_BASE: i32 = 5000;

/// The highest port the ingest-port pool hands out, so the assignment stays inside the
/// 16-bit port space however many sources are created.
const INGEST_PORT_CEILING: i32 = 65_535;

/// `MediaStreamType` (`smithy_query(query="shape",shape="MediaStreamType")`: video |
/// audio | ancillary-data).
pub const MEDIA_STREAM_TYPE_VIDEO: &str = "video";
/// See [`MEDIA_STREAM_TYPE_VIDEO`].
pub const MEDIA_STREAM_TYPE_AUDIO: &str = "audio";
/// See [`MEDIA_STREAM_TYPE_VIDEO`].
pub const MEDIA_STREAM_TYPE_ANCILLARY_DATA: &str = "ancillary-data";

/// The sample rate the service applies to a video or ancillary-data media stream whose
/// request names none: `AddMediaStreamRequest$ClockRate` documents "If the media stream
/// type is video or ancillary data, set this value to 90000." Audio has two documented
/// values (48000 or 96000) and so no single default — an audio stream that names no
/// clock rate keeps none.
pub const VIDEO_CLOCK_RATE_HZ: i32 = 90_000;

/// The first dynamic RTP payload type (RFC 3551 §6: 96-127 are dynamic), and so the
/// first `Fmt` this service hands a media stream. `MediaStream$Fmt` is "the format type
/// number (sometimes referred to as RTP payload type) of the media stream. MediaConnect
/// assigns this value to the media stream."
pub const DYNAMIC_RTP_PAYLOAD_TYPE_BASE: i32 = 96;

/// The `Protocol` enum values the smoothing max bitrate applies to:
/// `Transport$MaxBitrate` is "the smoothing max bitrate (in bps) for RIST, RTP, and
/// RTP-FEC streams", which are the `rist`, `rtp` and `rtp-fec` values of `Protocol`
/// (`smithy_query(query="shape",shape="Protocol")`). A source on any other protocol
/// carries no smoothing bitrate unless its request named one.
pub const SMOOTHING_BITRATE_PROTOCOLS: [&str; 3] = ["rist", "rtp", "rtp-fec"];

/// The smoothing max bitrate (in bits per second) MediaConnect assigns a source on one
/// of [`SMOOTHING_BITRATE_PROTOCOLS`] whose request named none.
///
/// `SetSourceRequest$MaxBitrate` is optional and the service reports a populated
/// `Transport$MaxBitrate` on such a source either way, so the value is service-assigned
/// rather than echoed: the recorded CreateFlow and DescribeFlow bodies this emulator is
/// replayed against report 160 Mbps for an `rtp` source, and they report it for a source
/// whose request named NO bitrate — a request that names one already round-trips
/// unchanged (see `tests/all/flow_projection.rs`), so an echo would have matched the
/// recording before this assignment existed, and it did not. The model publishes no
/// default on the member, so the number is named here once and nowhere else.
pub const ASSIGNED_SOURCE_MAX_BITRATE_BPS: i32 = 160_000_000;

/// The `Transport$MaxBitrate` a source on `protocol` reports, given the `requested`
/// bitrate its request carried (`None` when it carried none).
///
/// A request that names a bitrate keeps it, whatever the protocol; one that names none
/// gets the service's assignment where the member applies, and nothing where it does
/// not. Every operation that materialises a source resolves it HERE, so one request
/// shape always becomes the same stored transport.
pub fn assigned_source_max_bitrate(protocol: &str, requested: Option<i32>) -> Option<i32> {
    requested.or_else(|| {
        SMOOTHING_BITRATE_PROTOCOLS
            .contains(&protocol)
            .then_some(ASSIGNED_SOURCE_MAX_BITRATE_BPS)
    })
}

/// The recovery window (in milliseconds) a flow's source failover config reports when
/// the request that set it named none.
///
/// `FailoverConfig$RecoveryWindow` / `UpdateFailoverConfig$RecoveryWindow` are the
/// "search window time to look for dash-7 packets" and are optional on the way in, yet a
/// flow that carries a failover config reports a window either way: 200 ms, the
/// documented default search window for dash-7 merge. The model publishes no default on
/// the member, so the number is named here once.
pub const ASSIGNED_RECOVERY_WINDOW_MS: i32 = 200;

/// `config` with the members MediaConnect assigns itself resolved.
///
/// Every operation that writes a `SourceFailoverConfig` (CreateFlow's
/// `SourceFailoverConfig`, UpdateFlow's merge of `UpdateFailoverConfig`, CreateBridge's
/// `SourceFailoverConfig` — all three target the same `FailoverConfig` shape, whose
/// `RecoveryWindow` member carries the same documentation wherever it appears) stores the
/// config through this function, so failover settings never depend on which call wrote
/// them.
pub fn resolved_failover_config(config: FailoverConfig) -> FailoverConfig {
    FailoverConfig {
        recovery_window: Some(
            config
                .recovery_window
                .unwrap_or(ASSIGNED_RECOVERY_WINDOW_MS),
        ),
        ..config
    }
}

/// The failover setting a resource holds after `update` is applied to `stored`.
///
/// `UpdateFailoverConfig` mirrors `FailoverConfig` member for member and every member of
/// it is optional, so applying one is a member-wise merge: a member the request carries
/// replaces its counterpart, one it omits keeps the stored value. The merged result is
/// resolved through [`resolved_failover_config`], so the window MediaConnect assigns a
/// config that names none is the same here as on a create path.
///
/// Shared by every operation that applies an `UpdateFailoverConfig` — UpdateFlow's
/// `SourceFailoverConfig` and UpdateBridge's, which target the same request shape and the
/// same stored `FailoverConfig` — so one request shape always becomes the same stored
/// config.
pub fn merged_failover_config(
    stored: Option<&FailoverConfig>,
    update: &UpdateFailoverConfig,
) -> FailoverConfig {
    let mut merged = stored.cloned().unwrap_or_default();
    if let Some(mode) = update.failover_mode.clone() {
        merged.failover_mode = Some(mode);
    }
    if let Some(window) = update.recovery_window {
        merged.recovery_window = Some(window);
    }
    if let Some(priority) = update.source_priority.clone() {
        merged.source_priority = Some(priority);
    }
    if let Some(state) = update.state.clone() {
        merged.state = Some(state);
    }
    resolved_failover_config(merged)
}

/// The encryption a resource holds after `update` is applied to `stored`, or `None` when
/// the merge would leave an encryption with no role ARN.
///
/// `UpdateEncryption` mirrors `Encryption` member for member and every member of it is
/// optional, so applying one is a member-wise merge — the same merge
/// [`merged_failover_config`] performs for an `UpdateFailoverConfig`: a member the request
/// carries replaces its counterpart, one it omits keeps the stored value.
///
/// `Encryption$RoleArn` is REQUIRED (`smithy_query(query="shape",shape="Encryption")`)
/// while `UpdateEncryption$RoleArn` is not, so a resource that holds no encryption yet can
/// only be given one by a request that names the role: `None` says the merge is not
/// expressible, and the caller refuses the request with its own modelled client error.
///
/// Every operation that applies an `UpdateEncryption` resolves it HERE — UpdateFlowOutput's
/// `Encryption`, UpdateFlowEntitlement's and UpdateFlowSource's `Decryption`, which target
/// the same request shape and the same stored `Encryption` — so one request shape always
/// becomes the same stored record.
pub fn merged_encryption(stored: Option<&Encryption>, update: &UpdateEncryption) -> Option<Encryption> {
    let role_arn = update
        .role_arn
        .clone()
        .or_else(|| stored.map(|encryption| encryption.role_arn.clone()))?;
    let mut merged = stored.cloned().unwrap_or_default();
    merged.role_arn = role_arn;
    if let Some(algorithm) = update.algorithm.clone() {
        merged.algorithm = Some(algorithm);
    }
    if let Some(vector) = update.constant_initialization_vector.clone() {
        merged.constant_initialization_vector = Some(vector);
    }
    if let Some(device_id) = update.device_id.clone() {
        merged.device_id = Some(device_id);
    }
    if let Some(key_type) = update.key_type.clone() {
        merged.key_type = Some(key_type);
    }
    if let Some(region) = update.region.clone() {
        merged.region = Some(region);
    }
    if let Some(resource_id) = update.resource_id.clone() {
        merged.resource_id = Some(resource_id);
    }
    if let Some(secret_arn) = update.secret_arn.clone() {
        merged.secret_arn = Some(secret_arn);
    }
    if let Some(url) = update.url.clone() {
        merged.url = Some(url);
    }
    Some(merged)
}

/// A status change the clock owes: at `due_at` the flow's status becomes `status`.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The status the flow settles into.
    pub status: String,
    /// The instant from which the change is owed.
    pub due_at: SystemTime,
}

/// A source of a stored flow. `Flow.Source` is the first entry, `Flow.Sources` the
/// whole list.
#[derive(Debug, Clone, Default)]
pub struct StoredSource {
    pub source_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub ingest_ip: Option<String>,
    pub ingest_port: Option<i32>,
    pub whitelist_cidr: Option<String>,
    pub entitlement_arn: Option<String>,
    pub vpc_interface_name: Option<String>,
    pub sender_ip_address: Option<String>,
    pub sender_control_port: Option<i32>,
    pub decryption: Option<Encryption>,
    pub transport: Option<Transport>,
    /// The router integration state the request that set this source asked for, or
    /// `None` when it asked for none — see [`reported_router_integration_state`], which
    /// is what turns this into the state the source REPORTS.
    pub router_integration_state: Option<String>,
    /// `Source$RouterIntegrationTransitDecryption`: "the decryption configuration for the
    /// flow source when router integration is enabled", as the request that set the
    /// source configured it.
    pub router_integration_transit_decryption: Option<FlowTransitEncryption>,
    /// `Source$MediaStreamSourceConfigurations` — "the media streams that are associated
    /// with the source, and the parameters for those associations", as
    /// [`materialise_media_stream_source_configurations`] materialised the request's.
    /// `None` for a source that is associated with no media stream, so the member is
    /// absent rather than an empty list.
    pub media_stream_source_configurations: Option<Vec<MediaStreamSourceConfiguration>>,
    /// `Source$GatewayBridgeSource` — "the source configuration for cloud flows receiving
    /// a stream from a bridge", as the request that set the source configured it.
    pub gateway_bridge_source: Option<GatewayBridgeSource>,
    /// The source's own tags: what `SetSourceRequest$SourceTags` put on it, as TagResource
    /// and UntagResource have since changed it. A source has its own ARN
    /// (`Source$SourceArn`), so it is a resource the tag operations address in its own
    /// right — see [`ComAmazonawsMediaconnectMediaconnectState::tags_of`]. `Source` itself
    /// carries no tag member, so no flow response reports this map.
    pub tags: BTreeMap<String, String>,
}

impl StoredSource {
    /// Materialise one `SetSourceRequest` into the source record a flow keeps, under the
    /// name [`resolve_source_name`] gave it.
    ///
    /// Every operation that adds a source to a flow (CreateFlow's `Source` / `Sources`,
    /// AddFlowSources) materialises it HERE, so one request shape always becomes the same
    /// stored record: the service mints the `SourceArn`, assigns the ingest endpoint an
    /// owned source listens on, and folds the request's transport members into
    /// `Source.Transport`.
    ///
    /// `SetSourceRequest$SourceTags` — "the key-value pairs that can be used to tag and
    /// organize the source" — is stored on the record: the source's ARN is a `ResourceArn`
    /// the tag operations resolve, so those pairs are what a later ListTagsForResource on
    /// that ARN reports.
    pub fn from_set_request(
        request: &SetSourceRequest,
        name: String,
        region: &str,
        suffix: u64,
    ) -> Self {
        // A source reached through another account's entitlement is fed by that account's
        // flow, so this service assigns it no ingest endpoint of its own; an owned source
        // listens on the address and port the service assigns it.
        let entitled = request.entitlement_arn.is_some();
        StoredSource {
            source_arn: ::engine::emulator_arn(
                SERVICE_NAMESPACE,
                region,
                &source_resource_fragment(suffix, &name),
            ),
            name,
            description: request.description.clone(),
            ingest_ip: (!entitled).then(|| assigned_ingest_ip(suffix)),
            ingest_port: (!entitled).then(|| {
                request
                    .ingest_port
                    .unwrap_or_else(|| assigned_ingest_port(suffix))
            }),
            whitelist_cidr: request.whitelist_cidr.clone(),
            entitlement_arn: request.entitlement_arn.clone(),
            vpc_interface_name: request.vpc_interface_name.clone(),
            sender_ip_address: request.sender_ip_address.clone(),
            sender_control_port: request.sender_control_port,
            decryption: request.decryption.clone(),
            // What the request asked for; the state the source REPORTS is resolved by
            // `reported_router_integration_state`, which gives a source whose request
            // never mentioned router integration the disabled state.
            router_integration_state: request.router_integration_state.clone(),
            router_integration_transit_decryption: request
                .router_integration_transit_decryption
                .clone(),
            // "The media streams that are associated with the source": the association
            // list the request carries, materialised the one way this crate materialises
            // that request shape. A request that carries none, or an empty list,
            // associates no media stream, so the member is absent rather than empty.
            media_stream_source_configurations: request
                .media_stream_source_configurations
                .as_deref()
                .filter(|configurations| !configurations.is_empty())
                .map(|configurations| {
                    materialise_media_stream_source_configurations(configurations, suffix)
                }),
            // `SetGatewayBridgeSourceRequest` mirrors `GatewayBridgeSource` member for
            // member, with `BridgeArn` required on both.
            gateway_bridge_source: request.gateway_bridge_source.as_ref().map(|bridge| {
                GatewayBridgeSource {
                    bridge_arn: bridge.bridge_arn.clone(),
                    vpc_interface_attachment: bridge.vpc_interface_attachment.clone(),
                }
            }),
            // `Transport.Protocol` is required, so a source that names no protocol (an
            // entitled one, say) carries no transport rather than an empty one.
            // "The key-value pairs that can be used to tag and organize the source":
            // stored on the source, which is the resource its own ARN addresses.
            tags: tag_map_of(request.source_tags.as_ref()),
            transport: request.protocol.clone().map(|protocol| Transport {
                max_bitrate: assigned_source_max_bitrate(&protocol, request.max_bitrate),
                protocol,
                max_latency: request.max_latency,
                max_sync_buffer: request.max_sync_buffer,
                min_latency: request.min_latency,
                sender_control_port: request.sender_control_port,
                sender_ip_address: request.sender_ip_address.clone(),
                source_listener_address: request.source_listener_address.clone(),
                source_listener_port: request.source_listener_port,
                stream_id: request.stream_id.clone(),
                ndi_source_settings: request.ndi_source_settings.clone(),
                ..Transport::default()
            }),
        }
    }

    /// The modelled `Source` for this record.
    pub fn to_source(&self) -> Source {
        Source {
            router_integration_state: reported_router_integration_state(
                self.router_integration_state.as_deref(),
            ),
            router_integration_transit_decryption: self
                .router_integration_transit_decryption
                .clone(),
            source_arn: self.source_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            ingest_ip: self.ingest_ip.clone(),
            ingest_port: self.ingest_port,
            whitelist_cidr: self.whitelist_cidr.clone(),
            entitlement_arn: self.entitlement_arn.clone(),
            vpc_interface_name: self.vpc_interface_name.clone(),
            sender_ip_address: self.sender_ip_address.clone(),
            sender_control_port: self.sender_control_port,
            decryption: self.decryption.clone(),
            transport: self.transport.clone(),
            media_stream_source_configurations: self.media_stream_source_configurations.clone(),
            gateway_bridge_source: self.gateway_bridge_source.clone(),
            ..Source::default()
        }
    }
}

/// Materialise the `MediaStreamSourceConfigurationRequest`s a request carries into the
/// `MediaStreamSourceConfiguration`s the source record keeps.
///
/// The crate's ONE materialisation of that request shape: CreateFlow's `Source` /
/// `Sources` and AddFlowSources reach it through [`StoredSource::from_set_request`] and
/// UpdateFlowSource applies it to the source it updates, so one request shape always
/// becomes the same stored record.
///
/// The request and the stored shape mirror each other member for member except
/// `InputConfiguration$InputIp`, which has no counterpart in the request shape and is
/// REQUIRED on the stored one: it is "the IP address that the flow listens on for incoming
/// content for a media stream", so the SERVICE assigns it, from the same documentation
/// range a source's own ingest address is drawn from ([`assigned_ingest_ip`]) and keyed by
/// the ARN counter value `suffix` of the call that materialised the association. It is
/// stored, so every later read reports the same address.
pub fn materialise_media_stream_source_configurations(
    requests: &[MediaStreamSourceConfigurationRequest],
    suffix: u64,
) -> Vec<MediaStreamSourceConfiguration> {
    let input_ip = assigned_ingest_ip(suffix);
    requests
        .iter()
        .map(|request| MediaStreamSourceConfiguration {
            media_stream_name: request.media_stream_name.clone(),
            encoding_name: request.encoding_name.clone(),
            input_configurations: request.input_configurations.as_ref().map(|configurations| {
                configurations
                    .iter()
                    .map(|configuration| InputConfiguration {
                        input_ip: input_ip.clone(),
                        input_port: configuration.input_port,
                        interface: Interface {
                            name: configuration.interface.name.clone(),
                        },
                    })
                    .collect()
            }),
        })
        .collect()
}

/// An output of a stored flow.
#[derive(Debug, Clone, Default)]
pub struct StoredOutput {
    pub output_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub destination: Option<String>,
    pub port: Option<i32>,
    pub encryption: Option<Encryption>,
    pub output_status: Option<String>,
    pub vpc_interface_attachment: Option<VpcInterfaceAttachment>,
    pub transport: Option<Transport>,
    /// `Output.MediaStreamOutputConfigurations` — "the configuration for each media stream
    /// that is associated with the output", as
    /// [`materialise_media_stream_output_configurations`] materialised the request's.
    /// `None` for an output that is associated with no media stream, so the member is
    /// absent rather than an empty list.
    pub media_stream_output_configurations: Option<Vec<MediaStreamOutputConfiguration>>,
    /// The router integration state the request that added this output asked for, or
    /// `None` when it asked for none — see [`reported_router_integration_state`], which
    /// is what turns this into the state the output REPORTS.
    pub router_integration_state: Option<String>,
    /// `Output$RouterIntegrationTransitEncryption`: "the encryption configuration for the
    /// output when router integration is enabled", as the request that added the output
    /// configured it.
    pub router_integration_transit_encryption: Option<FlowTransitEncryption>,
    /// The output's own tags: what `AddOutputRequest$OutputTags` put on it, as TagResource
    /// and UntagResource have since changed it. An output has its own ARN
    /// (`Output$OutputArn`), so it is a resource the tag operations address in its own
    /// right — see [`ComAmazonawsMediaconnectMediaconnectState::tags_of`]. `Output` itself
    /// carries no tag member, so no flow response reports this map.
    pub tags: BTreeMap<String, String>,
}

impl StoredOutput {
    /// Materialise one `AddOutputRequest` into the output record a flow keeps.
    ///
    /// Every operation that adds an output to a flow (CreateFlow's `Outputs`,
    /// AddFlowOutputs) materialises it HERE, so one request shape always becomes the
    /// same stored record: the service mints the `OutputArn`, resolves `OutputStatus`
    /// to the documented `ENABLED` default, folds the request's transport members
    /// into `Output.Transport`, and keeps `AddOutputRequest$OutputTags` — "the key-value
    /// pairs that can be used to tag and organize the output" — on the record, because the
    /// output's ARN is a `ResourceArn` the tag operations resolve.
    pub fn from_add_request(
        request: &AddOutputRequest,
        name: String,
        region: &str,
        suffix: u64,
    ) -> Self {
        StoredOutput {
            output_arn: ::engine::emulator_arn(
                SERVICE_NAMESPACE,
                region,
                &output_resource_fragment(suffix, &name),
            ),
            name,
            description: request.description.clone(),
            destination: request.destination.clone(),
            port: request.port,
            encryption: request.encryption.clone(),
            output_status: Some(
                request
                    .output_status
                    .clone()
                    .unwrap_or_else(|| STATUS_ENABLED.to_string()),
            ),
            vpc_interface_attachment: request.vpc_interface_attachment.clone(),
            media_stream_output_configurations: request
                .media_stream_output_configurations
                .as_deref()
                .filter(|configurations| !configurations.is_empty())
                .map(|configurations| {
                    materialise_media_stream_output_configurations(configurations, suffix)
                }),
            router_integration_state: request.router_integration_state.clone(),
            router_integration_transit_encryption: request
                .router_integration_transit_encryption
                .clone(),
            // "The key-value pairs that can be used to tag and organize the output":
            // stored on the output, which is the resource its own ARN addresses.
            tags: tag_map_of(request.output_tags.as_ref()),
            // `Transport.Protocol` is required, so an output that names no protocol
            // carries no transport rather than an empty one.
            transport: request.protocol.clone().map(|protocol| Transport {
                protocol,
                cidr_allow_list: request.cidr_allow_list.clone(),
                max_latency: request.max_latency,
                min_latency: request.min_latency,
                smoothing_latency: request.smoothing_latency,
                remote_id: request.remote_id.clone(),
                sender_control_port: request.sender_control_port,
                stream_id: request.stream_id.clone(),
                ndi_speed_hq_quality: request.ndi_speed_hq_quality,
                ndi_program_name: request.ndi_program_name.clone(),
                ndi_output_timecode_source: request.ndi_output_timecode_source.clone(),
                ndi_audio_reference_level: request.ndi_audio_reference_level,
                ..Transport::default()
            }),
        }
    }

    /// The modelled `Output` for this record.
    pub fn to_output(&self) -> Output {
        Output {
            router_integration_state: reported_router_integration_state(
                self.router_integration_state.as_deref(),
            ),
            router_integration_transit_encryption: self
                .router_integration_transit_encryption
                .clone(),
            output_arn: self.output_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            destination: self.destination.clone(),
            port: self.port,
            encryption: self.encryption.clone(),
            output_status: self.output_status.clone(),
            vpc_interface_attachment: self.vpc_interface_attachment.clone(),
            transport: self.transport.clone(),
            media_stream_output_configurations: self.media_stream_output_configurations.clone(),
            ..Output::default()
        }
    }
}

/// Materialise the `MediaStreamOutputConfigurationRequest`s a request carries into the
/// `MediaStreamOutputConfiguration`s the output record keeps.
///
/// The crate's ONE materialisation of that request shape: CreateFlow's and
/// AddFlowOutputs' `Outputs` reach it through [`StoredOutput::from_add_request`] and
/// UpdateFlowOutput applies it to the output it updates, so one request shape always
/// becomes the same stored record.
///
/// The request and the stored shape mirror each other member for member except
/// `DestinationConfiguration$OutboundIp`, which has no counterpart in the request shape
/// and is REQUIRED on the stored one: it is "the IP address that the receiver requires in
/// order to establish a connection with the flow", so the SERVICE assigns it, from the
/// same documentation-range egress pool a flow's `EgressIp` is drawn from
/// ([`assigned_egress_ip`]) and keyed by the ARN counter value `suffix` of the call that
/// materialised the association. It is stored, so every later read reports the same
/// address.
pub fn materialise_media_stream_output_configurations(
    requests: &[MediaStreamOutputConfigurationRequest],
    suffix: u64,
) -> Vec<MediaStreamOutputConfiguration> {
    let outbound_ip = assigned_egress_ip(suffix);
    requests
        .iter()
        .map(|request| MediaStreamOutputConfiguration {
            media_stream_name: request.media_stream_name.clone(),
            encoding_name: request.encoding_name.clone(),
            encoding_parameters: request.encoding_parameters.as_ref().map(|parameters| {
                EncodingParameters {
                    compression_factor: parameters.compression_factor,
                    encoder_profile: parameters.encoder_profile.clone(),
                }
            }),
            destination_configurations: request.destination_configurations.as_ref().map(
                |configurations| {
                    configurations
                        .iter()
                        .map(|configuration| DestinationConfiguration {
                            destination_ip: configuration.destination_ip.clone(),
                            destination_port: configuration.destination_port,
                            interface: Interface {
                                name: configuration.interface.name.clone(),
                            },
                            outbound_ip: outbound_ip.clone(),
                        })
                        .collect()
                },
            ),
        })
        .collect()
}

/// An entitlement of a stored flow: the modelled `Entitlement` the flow reports, plus the
/// entitlement's own tags.
///
/// `Entitlement` models no tag member, so the tags travel beside the projection rather
/// than inside it — the same shape [`StoredMediaStream`] has. An entitlement DOES have its
/// own ARN (`Entitlement$EntitlementArn`), so unlike a media stream it is a resource the
/// tag operations address in its own right (see
/// [`ComAmazonawsMediaconnectMediaconnectState::tags_of`]).
#[derive(Debug, Clone, Default)]
pub struct StoredEntitlement {
    pub entitlement: Entitlement,
    pub tags: BTreeMap<String, String>,
}

impl StoredEntitlement {
    /// The modelled `Entitlement` for this record.
    pub fn to_entitlement(&self) -> Entitlement {
        self.entitlement.clone()
    }

    /// The entitlement's ARN — the identity every entitlement operation, and a tag
    /// operation's `ResourceArn`, addresses it by.
    pub fn arn(&self) -> &str {
        &self.entitlement.entitlement_arn
    }

    /// The `ListedEntitlement` summary a listing reports for this record.
    ///
    /// Every member the summary shape carries
    /// (`smithy_query(query="shape",shape="ListedEntitlement")`: `EntitlementArn`,
    /// `EntitlementName`, `DataTransferSubscriberFeePercent`), each projected from the
    /// stored entitlement — the summary holds no value of its own.
    pub fn to_listed_entitlement(&self) -> ListedEntitlement {
        ListedEntitlement {
            entitlement_arn: self.entitlement.entitlement_arn.clone(),
            entitlement_name: self.entitlement.name.clone(),
            data_transfer_subscriber_fee_percent: self
                .entitlement
                .data_transfer_subscriber_fee_percent,
        }
    }

    /// Whether this entitlement is granted TO the account `account_id`: whether that
    /// account is one of its subscribers.
    ///
    /// `Entitlement$Subscribers` is "the Amazon Web Services account IDs that you want to
    /// share your content with. The receiving accounts (subscribers) will be allowed to
    /// create their own flow using your content as the source"
    /// (`smithy_query(query="shape",shape="Entitlement",member="Subscribers")`), so a
    /// grant's subscriber list is exactly the set of accounts the entitlement is granted
    /// to.
    pub fn is_granted_to(&self, account_id: &str) -> bool {
        self.entitlement
            .subscribers
            .iter()
            .any(|subscriber| subscriber == account_id)
    }
}

/// A media stream of a stored flow: the modelled `MediaStream` the flow reports, plus
/// the `MediaStreamTags` the request that added it carried.
///
/// `MediaStream` models no tag member, and a media stream has no ARN of its own either
/// (it is addressed by name within its flow), so no `ResourceArn` resolves to this map:
/// the tags are written here, rather than dropped on the floor when a request carries
/// them, and are read by nothing yet.
#[derive(Debug, Clone, Default)]
pub struct StoredMediaStream {
    pub stream: MediaStream,
    pub tags: BTreeMap<String, String>,
}

impl StoredMediaStream {
    /// The modelled `MediaStream` for this record.
    pub fn to_media_stream(&self) -> MediaStream {
        self.stream.clone()
    }
}

/// Why `request` is not a media stream this flow can hold beside the media stream names
/// and ids in `names` / `ids`, as the message the refusal carries, or `None` when it is.
///
/// A media stream is addressed by name within its flow (RemoveFlowMediaStream and
/// UpdateFlowMediaStream take `MediaStreamName` as an `@httpLabel`), so an empty name
/// names no media stream, and its `MediaStreamId` is "a unique identifier for the media
/// stream" — so neither the name nor the id may repeat within one flow. CreateFlow's
/// `MediaStreams` and AddFlowMediaStreams both add media streams to a flow, so both
/// refuse through this one function and with the same wording: no request can leave a
/// flow in a state the other operation would never permit.
pub fn media_stream_rejection(
    request: &AddMediaStreamRequest,
    names: &BTreeSet<String>,
    ids: &BTreeSet<i32>,
) -> Option<String> {
    if request.media_stream_name.is_empty() {
        return Some(
            "A media stream name must not be empty: it is how the media stream is addressed \
             within its flow."
                .to_string(),
        );
    }
    if names.contains(&request.media_stream_name) {
        return Some(format!(
            "More than one media stream of this flow would be named {:?}. A media stream name \
             must be unique within its flow.",
            request.media_stream_name
        ));
    }
    if ids.contains(&request.media_stream_id) {
        return Some(format!(
            "More than one media stream of this flow would have ID {}. A media stream ID must \
             be unique within its flow.",
            request.media_stream_id
        ));
    }
    None
}

/// The `Fmt` the service assigns the next media stream of a flow: the lowest RTP
/// payload type, counting from [`DYNAMIC_RTP_PAYLOAD_TYPE_BASE`], that none of the
/// flow's media streams already holds — so a flow's payload types stay unique however
/// its streams were added or removed.
///
/// The dynamic range ends at 127, and a flow may hold more media streams than that
/// range has slots (the ceiling is [`MAX_FLOW_COLLECTION_ENTRIES`]); such a flow is
/// numbered past the range rather than reusing a payload type, because uniqueness is
/// what identifies the stream to a receiver.
pub fn next_media_stream_fmt(existing: &[StoredMediaStream]) -> i32 {
    let mut fmt = DYNAMIC_RTP_PAYLOAD_TYPE_BASE;
    while existing.iter().any(|stream| stream.stream.fmt == fmt) {
        fmt += 1;
    }
    fmt
}

/// The sample rate a media stream of `media_stream_type` reports when no request has
/// named one, or `None` when the type has no single documented rate.
///
/// `AddMediaStreamRequest$ClockRate` documents "If the media stream type is video or
/// ancillary data, set this value to 90000. If the media stream type is audio, set this
/// value to either 48000 or 96000" — so video and ancillary data have one rate to fall
/// back on ([`VIDEO_CLOCK_RATE_HZ`]) and audio has none. The rule is stated once here so
/// every operation that stores a media stream's type (CreateFlow and AddFlowMediaStreams,
/// which add one; UpdateFlowMediaStream, which can change its type) resolves the same
/// fallback.
pub fn default_clock_rate(media_stream_type: &str) -> Option<i32> {
    matches!(
        media_stream_type,
        MEDIA_STREAM_TYPE_VIDEO | MEDIA_STREAM_TYPE_ANCILLARY_DATA
    )
    .then_some(VIDEO_CLOCK_RATE_HZ)
}

/// The `MediaStreamAttributes` a media stream holds once the request shape `request` is
/// applied to the attributes `stored` it holds now (`None` when it holds none).
///
/// `MediaStreamAttributesRequest` and `FmtpRequest` mirror `MediaStreamAttributes` and
/// `Fmtp` member for member with every member optional, so this is the member-wise merge
/// this crate applies to every request shape of that form: a member the request carries
/// replaces its counterpart, one it omits keeps the stored value. `MediaStreamAttributes$Fmtp`
/// is REQUIRED (`smithy_query(query="shape",shape="MediaStreamAttributes")`), so a stream
/// given attributes by a request that names no `Fmtp` reports an empty one rather than none.
pub fn merged_media_stream_attributes(
    stored: Option<&MediaStreamAttributes>,
    request: &MediaStreamAttributesRequest,
) -> MediaStreamAttributes {
    let mut merged = stored.cloned().unwrap_or_default();
    if let Some(lang) = request.lang.clone() {
        merged.lang = Some(lang);
    }
    if let Some(fmtp) = request.fmtp.as_ref() {
        if let Some(channel_order) = fmtp.channel_order.clone() {
            merged.fmtp.channel_order = Some(channel_order);
        }
        if let Some(colorimetry) = fmtp.colorimetry.clone() {
            merged.fmtp.colorimetry = Some(colorimetry);
        }
        if let Some(exact_framerate) = fmtp.exact_framerate.clone() {
            merged.fmtp.exact_framerate = Some(exact_framerate);
        }
        if let Some(par) = fmtp.par.clone() {
            merged.fmtp.par = Some(par);
        }
        if let Some(range) = fmtp.range.clone() {
            merged.fmtp.range = Some(range);
        }
        if let Some(scan_mode) = fmtp.scan_mode.clone() {
            merged.fmtp.scan_mode = Some(scan_mode);
        }
        if let Some(tcs) = fmtp.tcs.clone() {
            merged.fmtp.tcs = Some(tcs);
        }
    }
    merged
}

/// Materialise one `AddMediaStreamRequest` into the media stream a flow keeps, with the
/// payload type `fmt` the service assigned it (see [`next_media_stream_fmt`]).
///
/// Both CreateFlow (`MediaStreams`) and AddFlowMediaStreams add media streams to a flow,
/// and they store the same record: this function is the crate's ONE materialisation of
/// an `AddMediaStreamRequest`.
pub fn materialise_media_stream(request: &AddMediaStreamRequest, fmt: i32) -> StoredMediaStream {
    StoredMediaStream {
        stream: MediaStream {
            fmt,
            media_stream_id: request.media_stream_id,
            media_stream_name: request.media_stream_name.clone(),
            media_stream_type: request.media_stream_type.clone(),
            clock_rate: request
                .clock_rate
                .or_else(|| default_clock_rate(&request.media_stream_type)),
            description: request.description.clone(),
            video_format: request.video_format.clone(),
            attributes: request
                .attributes
                .as_ref()
                .map(|attributes| merged_media_stream_attributes(None, attributes)),
        },
        tags: request
            .media_stream_tags
            .iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    }
}

/// A flow, as the service holds it between calls. Every flow operation's response is
/// computed from this record.
#[derive(Debug, Clone)]
pub struct StoredFlow {
    pub flow_arn: String,
    pub name: String,
    /// The flow's service-derived description — see [`derived_flow_description`], which
    /// is where it is computed. Always non-empty, because `ListedFlow.Description` is a
    /// required member and `Flow.Description` is reported for every owned flow.
    pub description: String,
    pub availability_zone: String,
    pub status: String,
    /// The status change the clock owes, if any. See [`TRANSITION_SPAN`].
    pub pending: Option<PendingStatus>,
    pub egress_ip: Option<String>,
    pub flow_size: Option<String>,
    pub maintenance: Option<Maintenance>,
    pub source_failover_config: Option<FailoverConfig>,
    pub source_monitoring_config: Option<MonitoringConfig>,
    pub ndi_config: Option<NdiConfig>,
    pub encoding_config: Option<EncodingConfig>,
    pub sources: Vec<StoredSource>,
    pub outputs: Vec<StoredOutput>,
    pub entitlements: Vec<StoredEntitlement>,
    pub media_streams: Vec<StoredMediaStream>,
    pub vpc_interfaces: Vec<VpcInterface>,
    /// The flow's tags: what CreateFlow's `FlowTags` put on it, as TagResource and
    /// UntagResource have since changed it. ListTagsForResource reports this map, and
    /// DeleteFlow drops it with the flow — "when a resource is deleted, the tags associated
    /// with that resource are deleted as well". `Flow` itself carries no tag member, so no
    /// flow response reports it.
    pub tags: BTreeMap<String, String>,
    /// When the flow was created. `Flow` models no creation timestamp either, so like
    /// [`StoredFlow::tags`] this is recorded now for the operations that will report it
    /// (the maintenance / reservation slices) rather than read in this one.
    pub created_at: SystemTime,
}

impl StoredFlow {
    /// Materialise the status change the clock already made due, if it is due at `now`.
    ///
    /// This is the only write a reporting operation (DescribeFlow, ListFlows) performs:
    /// the change was owed at `due_at` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.status = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.status = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// The names of the outputs the flow already holds — what
    /// [`resolve_output_name`] checks a new output's name against.
    pub fn output_names(&self) -> BTreeSet<String> {
        self.outputs
            .iter()
            .map(|output| output.name.clone())
            .collect()
    }

    /// The names of the sources the flow already holds — what [`resolve_source_name`]
    /// checks a generated source name against.
    pub fn source_names(&self) -> BTreeSet<String> {
        self.sources
            .iter()
            .map(|source| source.name.clone())
            .collect()
    }

    /// The names of the entitlements the flow already carries — what
    /// [`resolve_entitlement_name`] checks a new entitlement's name against.
    pub fn entitlement_names(&self) -> BTreeSet<String> {
        self.entitlements
            .iter()
            .map(|entitlement| entitlement.entitlement.name.clone())
            .collect()
    }

    /// What the media stream named `media_stream_name` is associated with, as the phrase a
    /// refusal names it by, or `None` when nothing of this flow carries it.
    ///
    /// A media stream is associated with a source through
    /// `Source.MediaStreamSourceConfigurations` ("the media streams that are associated
    /// with the source") and with an output through
    /// `Output.MediaStreamOutputConfigurations` ("the configuration for each media stream
    /// that is associated with the output"), and both name the media stream by name. The
    /// lookup is stated once here because every operation whose documented precondition is
    /// an association — RemoveFlowMediaStream ("only available if the media stream is not
    /// associated with a source or output") — must read the SAME associations.
    pub fn media_stream_association(&self, media_stream_name: &str) -> Option<String> {
        let source = self.sources.iter().find(|source| {
            source
                .media_stream_source_configurations
                .iter()
                .flatten()
                .any(|configuration| configuration.media_stream_name == media_stream_name)
        });
        if let Some(source) = source {
            return Some(format!("source {}", source.source_arn));
        }
        let output = self.outputs.iter().find(|output| {
            output
                .media_stream_output_configurations
                .iter()
                .flatten()
                .any(|configuration| configuration.media_stream_name == media_stream_name)
        });
        output.map(|output| format!("output {}", output.output_arn))
    }

    /// What the VPC interface named `vpc_interface_name` is referenced by, as the phrase a
    /// refusal names it by, or `None` when nothing of this flow references it.
    ///
    /// A source references a VPC interface by name through `Source.VpcInterfaceName` ("the
    /// name of the VPC interface that is used for this source") and, for a source fed by a
    /// gateway bridge, through `GatewayBridgeSource.VpcInterfaceAttachment` ("the name of
    /// the VPC interface attachment to use for this bridge source"); an output references
    /// one through `Output.VpcInterfaceAttachment` ("the name of the VPC interface
    /// attachment to use for this output"). The lookup is stated once here because every
    /// operation whose documented precondition is such a reference —
    /// RemoveFlowVpcInterface ("can be made only on a VPC interface that does not have a
    /// Source or Output associated with it") — must read the SAME references.
    pub fn vpc_interface_reference(&self, vpc_interface_name: &str) -> Option<String> {
        let source = self.sources.iter().find(|source| {
            source.vpc_interface_name.as_deref() == Some(vpc_interface_name)
                || source
                    .gateway_bridge_source
                    .as_ref()
                    .and_then(|bridge_source| bridge_source.vpc_interface_attachment.as_ref())
                    .and_then(|attachment| attachment.vpc_interface_name.as_deref())
                    == Some(vpc_interface_name)
        });
        if let Some(source) = source {
            return Some(format!("source {}", source.source_arn));
        }
        let output = self.outputs.iter().find(|output| {
            output
                .vpc_interface_attachment
                .as_ref()
                .and_then(|attachment| attachment.vpc_interface_name.as_deref())
                == Some(vpc_interface_name)
        });
        output.map(|output| format!("output {}", output.output_arn))
    }

    /// `ListedFlow.SourceType`: `ENTITLED` when the flow ingests through an entitlement
    /// granted by another account, `OWNED` otherwise.
    pub fn source_type(&self) -> &'static str {
        if self
            .sources
            .iter()
            .any(|source| source.entitlement_arn.is_some())
        {
            SOURCE_TYPE_ENTITLED
        } else {
            SOURCE_TYPE_OWNED
        }
    }

    /// The tags of the ARN-bearing sub-resource of this flow that `resource_arn` addresses
    /// — an output, a source or an entitlement — or `None` when this flow holds no such
    /// sub-resource.
    ///
    /// Each of those three has its own ARN and its own tag map, so each is a resource a
    /// tag operation's `ResourceArn` addresses in its own right; the ARNs are minted from
    /// disjoint resource-type fragments, so at most one sub-resource of one flow matches.
    pub fn sub_resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(output) = self
            .outputs
            .iter()
            .find(|output| output.output_arn == resource_arn)
        {
            return Some(&output.tags);
        }
        if let Some(source) = self
            .sources
            .iter()
            .find(|source| source.source_arn == resource_arn)
        {
            return Some(&source.tags);
        }
        self.entitlements
            .iter()
            .find(|entitlement| entitlement.arn() == resource_arn)
            .map(|entitlement| &entitlement.tags)
    }

    /// The tags of the sub-resource `resource_arn` addresses, for a tag write. See
    /// [`StoredFlow::sub_resource_tags`].
    pub fn sub_resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        if let Some(output) = self
            .outputs
            .iter_mut()
            .find(|output| output.output_arn == resource_arn)
        {
            return Some(&mut output.tags);
        }
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.source_arn == resource_arn)
        {
            return Some(&mut source.tags);
        }
        self.entitlements
            .iter_mut()
            .find(|entitlement| entitlement.entitlement.entitlement_arn == resource_arn)
            .map(|entitlement| &mut entitlement.tags)
    }

    /// The modelled `Flow` for this record.
    pub fn to_flow(&self) -> Flow {
        Flow {
            flow_arn: self.flow_arn.clone(),
            name: self.name.clone(),
            // `Flow.Description` is optional on the shape but the service always has one
            // for a flow it owns, so it is reported rather than omitted.
            description: Some(self.description.clone()),
            availability_zone: self.availability_zone.clone(),
            status: self.status.clone(),
            egress_ip: self.egress_ip.clone(),
            flow_size: self.flow_size.clone(),
            maintenance: self.maintenance.clone(),
            source_failover_config: self.source_failover_config.clone(),
            source_monitoring_config: self.source_monitoring_config.clone(),
            ndi_config: self.ndi_config.clone(),
            encoding_config: self.encoding_config.clone(),
            entitlements: self
                .entitlements
                .iter()
                .map(StoredEntitlement::to_entitlement)
                .collect(),
            outputs: self.outputs.iter().map(StoredOutput::to_output).collect(),
            // A flow always carries at least one source (CreateFlow refuses a request
            // without one), so the first source is `Flow.Source` and the whole list is
            // `Flow.Sources`.
            source: self
                .sources
                .first()
                .map(StoredSource::to_source)
                .unwrap_or_default(),
            sources: (!self.sources.is_empty())
                .then(|| self.sources.iter().map(StoredSource::to_source).collect()),
            // `Flow.MediaStreams` is reported whether or not the flow has any: a flow
            // with none carries the member as an EMPTY list, so a client that iterates it
            // can tell "no media streams" from "not reported" without a default.
            media_streams: Some(
                self.media_streams
                    .iter()
                    .map(StoredMediaStream::to_media_stream)
                    .collect(),
            ),
            vpc_interfaces: (!self.vpc_interfaces.is_empty()).then(|| self.vpc_interfaces.clone()),
        }
    }

    /// The modelled `ListedFlow` summary for this record.
    pub fn to_listed_flow(&self) -> ListedFlow {
        ListedFlow {
            flow_arn: self.flow_arn.clone(),
            name: self.name.clone(),
            // `ListedFlow.Description` is REQUIRED, and it is the same value
            // `Flow.Description` reports for the flow.
            description: self.description.clone(),
            availability_zone: self.availability_zone.clone(),
            source_type: self.source_type().to_string(),
            status: self.status.clone(),
            maintenance: self.maintenance.clone(),
        }
    }
}

/// A gateway, as the service holds it between calls. Every gateway operation's response
/// is computed from this record.
#[derive(Debug, Clone)]
pub struct StoredGateway {
    pub gateway_arn: String,
    pub name: String,
    pub egress_cidr_blocks: Vec<String>,
    pub networks: Vec<GatewayNetwork>,
    pub gateway_state: String,
    /// The state change the clock owes, if any. See [`TRANSITION_SPAN`].
    pub pending: Option<PendingStatus>,
    /// When DeleteGateway accepted the delete of this gateway, or `None` while it is live.
    ///
    /// A gateway delete is a SOFT delete — `GatewayState` carries the terminal `DELETED`
    /// value, so a deleted gateway stays a readable resource (see
    /// [`GATEWAY_STATE_DELETED`]) — and this is the record of it: it makes a repeated
    /// delete idempotent rather than a not-found, keeps a retired gateway out of the live
    /// count the create ceiling bounds, and orders the retired records the store retains
    /// (see [`MAX_RETIRED_GATEWAYS`]).
    pub deleted_at: Option<SystemTime>,
}

impl StoredGateway {
    /// Materialise the state change the clock already made due, if it is due at `now`.
    ///
    /// As for a flow (see [`StoredFlow::settle`]) this is the only write a reporting
    /// operation performs: the change was owed at `due_at` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.gateway_state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// gateway counterpart of [`StoredFlow::begin_transition`], so every state a gateway
    /// is answered in while the clock still owes a change is started one way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.gateway_state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// Whether this gateway has been deleted: the delete was accepted, so the record is
    /// retired (`DELETING`, settling into `DELETED`) rather than live.
    pub fn is_retired(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// The modelled `Gateway` for this record.
    pub fn to_gateway(&self) -> Gateway {
        Gateway {
            egress_cidr_blocks: self.egress_cidr_blocks.clone(),
            gateway_arn: self.gateway_arn.clone(),
            // `Gateway.GatewayMessages` carries "messages with information about the
            // gateway"; a gateway this emulator serves reports none, so the member stays
            // absent rather than being an empty list.
            gateway_messages: None,
            gateway_state: Some(self.gateway_state.clone()),
            name: self.name.clone(),
            networks: self.networks.clone(),
        }
    }

    /// The modelled `ListedGateway` summary for this record.
    pub fn to_listed_gateway(&self) -> ListedGateway {
        ListedGateway {
            gateway_arn: self.gateway_arn.clone(),
            gateway_state: self.gateway_state.clone(),
            name: self.name.clone(),
        }
    }
}

/// An instance of a gateway, as the service holds it between calls. Every gateway-instance
/// operation's response is computed from this record.
///
/// An instance is NOT caller-created: the model declares no operation that registers one
/// (`smithy_query(query="list_operations")` — the only instance operations are
/// DescribeGatewayInstance, ListGatewayInstances, UpdateGatewayInstance and
/// DeregisterGatewayInstance). On AWS an instance appears when the MediaConnect Gateway
/// software is installed on the on-premises appliance and registers itself with its
/// gateway — `GatewayInstance$InstanceId` is "the instance ID generated by the SSM install"
/// — so the registration reaches the service out of band, through
/// [`ComAmazonawsMediaconnectMediaconnectState::register_gateway_instance`], and nothing a
/// client sends over the wire creates one.
#[derive(Debug, Clone)]
pub struct StoredGatewayInstance {
    /// The identity every instance operation addresses this instance by (`@httpLabel
    /// GatewayInstanceArn`). See [`gateway_instance_arn`] for its form.
    pub gateway_instance_arn: String,
    /// The gateway this instance is registered with — the ARN [`gateway_instance_arn`]
    /// nests, kept on the record so an instance resolves its gateway without re-parsing
    /// its own ARN.
    pub gateway_arn: String,
    /// `GatewayInstance$InstanceId`: "the instance ID generated by the SSM install. This
    /// will begin with `mi-`." See [`gateway_instance_id`].
    pub instance_id: String,
    /// `GatewayInstance$InstanceState`, one of the [`INSTANCE_STATE_REGISTERING`] values.
    pub instance_state: String,
    /// `GatewayInstance$BridgePlacement`, one of [`BRIDGE_PLACEMENT_AVAILABLE`] /
    /// [`BRIDGE_PLACEMENT_LOCKED`] — what UpdateGatewayInstance moves.
    pub bridge_placement: String,
    /// `GatewayInstance$ConnectionStatus`: whether the appliance is talking to the service.
    /// Reported out of band, like the registration itself.
    pub connection_status: String,
    /// The state change the clock owes, if any. See [`TRANSITION_SPAN`].
    pub pending: Option<PendingStatus>,
}

impl StoredGatewayInstance {
    /// Materialise the state change the clock already made due, if it is due at `now`.
    ///
    /// As for a gateway (see [`StoredGateway::settle`]) this is the only write a reporting
    /// operation performs: the change was owed at `due_at` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.instance_state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// instance counterpart of [`StoredGateway::begin_transition`], so every state an
    /// instance is answered in while the clock still owes a change is started one way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.instance_state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// Whether this instance is still registered with its gateway, i.e. has not reached the
    /// terminal [`INSTANCE_STATE_DEREGISTERED`].
    ///
    /// This is what "the gateway's instances" means to the operations that count them:
    /// DeleteGateway's documented precondition ("before you can delete a gateway, you must
    /// deregister its instances") and the bridges an instance hosts
    /// ([`ComAmazonawsMediaconnectMediaconnectState::running_bridge_count`]).
    pub fn is_registered(&self) -> bool {
        self.instance_state != INSTANCE_STATE_DEREGISTERED
    }

    /// Whether a deregistration of this instance is already under way or finished — the
    /// states in which the `@idempotent` DeregisterGatewayInstance has nothing left to do.
    pub fn is_deregistering_or_deregistered(&self) -> bool {
        matches!(
            self.instance_state.as_str(),
            INSTANCE_STATE_DEREGISTERING | INSTANCE_STATE_DEREGISTERED
        )
    }

    /// `GatewayInstance$ConnectionStatus` for this record.
    ///
    /// An instance that reached the terminal [`INSTANCE_STATE_DEREGISTERED`] is no longer
    /// registered with its gateway, so it is no longer connected to the service whatever
    /// the appliance last reported; while it is registered the reported status is the
    /// stored one.
    pub fn connection_status(&self) -> &str {
        if self.is_registered() {
            &self.connection_status
        } else {
            CONNECTION_STATUS_DISCONNECTED
        }
    }

    /// The modelled `GatewayInstance` for this record, with the running-bridge count
    /// resolved by the caller (it lives on the bridge store, not here — see
    /// [`ComAmazonawsMediaconnectMediaconnectState::gateway_instance_projection`]).
    pub fn to_gateway_instance(&self, running_bridge_count: i32) -> GatewayInstance {
        GatewayInstance {
            bridge_placement: self.bridge_placement.clone(),
            connection_status: self.connection_status().to_string(),
            gateway_arn: self.gateway_arn.clone(),
            gateway_instance_arn: self.gateway_instance_arn.clone(),
            instance_id: self.instance_id.clone(),
            // `GatewayInstance.InstanceMessages` carries "messages with information about
            // the gateway"; an instance this emulator serves reports none, so the optional
            // member stays absent rather than being an empty list — the same as
            // `Gateway.GatewayMessages` (see [`StoredGateway::to_gateway`]).
            instance_messages: None,
            instance_state: self.instance_state.clone(),
            running_bridge_count,
        }
    }
}

/// The modelled `BridgeSource` a request entry configures, or `None` when the entry
/// names neither a flow source nor a network source.
///
/// `AddBridgeSourceRequest` carries two optional members — `FlowSource` ("the source of
/// the flow") and `NetworkSource` ("the source of the network") — which are the two kinds
/// a `BridgeSource` reports, so one entry materialises exactly one of them. Every
/// operation that stores a bridge source (CreateBridge's `Sources`, and AddBridgeSources
/// once it is implemented) materialises it HERE, so one request entry always becomes the
/// same stored source.
pub fn materialise_bridge_source(request: &AddBridgeSourceRequest) -> Option<BridgeSource> {
    match (&request.flow_source, &request.network_source) {
        (Some(flow_source), None) => Some(BridgeSource {
            flow_source: Some(BridgeFlowSource {
                flow_arn: flow_source.flow_arn.clone(),
                flow_vpc_interface_attachment: flow_source.flow_vpc_interface_attachment.clone(),
                name: flow_source.name.clone(),
                // `BridgeFlowSource$OutputArn` is "the Amazon Resource Number (ARN) of the
                // output" MediaConnect creates on the cloud flow for this bridge. This
                // emulator creates no output on the flow a bridge names — the flow's
                // outputs are what AddFlowOutputs / RemoveFlowOutput own — so the optional
                // member stays absent rather than carrying an ARN that addresses nothing.
                output_arn: None,
            }),
            network_source: None,
        }),
        (None, Some(network_source)) => Some(BridgeSource {
            flow_source: None,
            network_source: Some(BridgeNetworkSource {
                multicast_ip: network_source.multicast_ip.clone(),
                multicast_source_settings: network_source.multicast_source_settings.clone(),
                name: network_source.name.clone(),
                network_name: network_source.network_name.clone(),
                port: network_source.port,
                protocol: network_source.protocol.clone(),
            }),
        }),
        _ => None,
    }
}

/// The modelled `BridgeOutput` a request entry configures, or `None` when the entry names
/// no network output.
///
/// `AddBridgeOutputRequest` carries one optional member, `NetworkOutput` ("the network
/// output of the bridge. A network output is delivered to your premises"), so an entry
/// that names none configures no output at all. A `BridgeOutput` can also be a
/// `BridgeFlowOutput`, which no request shape in this model configures: a bridge's flow
/// output is the one a cloud flow creates when it takes the bridge as its source
/// (`SetGatewayBridgeSourceRequest`), never one a create or add request carries.
pub fn materialise_bridge_output(request: &AddBridgeOutputRequest) -> Option<BridgeOutput> {
    let network_output = request.network_output.as_ref()?;
    Some(BridgeOutput {
        flow_output: None,
        network_output: Some(BridgeNetworkOutput {
            ip_address: network_output.ip_address.clone(),
            name: network_output.name.clone(),
            network_name: network_output.network_name.clone(),
            port: network_output.port,
            protocol: network_output.protocol.clone(),
            ttl: network_output.ttl,
        }),
    })
}

/// The name of a stored bridge source: the flow source's or the network source's `Name`,
/// the one a bridge addresses that source by.
pub fn bridge_source_name(source: &BridgeSource) -> &str {
    match (&source.flow_source, &source.network_source) {
        (Some(flow_source), _) => &flow_source.name,
        (None, Some(network_source)) => &network_source.name,
        (None, None) => "",
    }
}

/// The name of a stored bridge output: the flow output's or the network output's `Name`.
pub fn bridge_output_name(output: &BridgeOutput) -> &str {
    match (&output.flow_output, &output.network_output) {
        (Some(flow_output), _) => &flow_output.name,
        (None, Some(network_output)) => &network_output.name,
        (None, None) => "",
    }
}

/// Whether `name` can name a source or an output of a bridge.
///
/// RemoveBridgeSource / UpdateBridgeSource take `SourceName` and RemoveBridgeOutput /
/// UpdateBridgeOutput take `OutputName` as an `@httpLabel` (`OPERATIONS.md`), so the name
/// must not be empty and must not carry the URI path separator — otherwise the stored
/// entry could never be addressed again.
pub fn is_valid_bridge_member_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/')
}

/// A bridge, as the service holds it between calls. Every bridge operation's response is
/// computed from this record.
#[derive(Debug, Clone)]
pub struct StoredBridge {
    pub bridge_arn: String,
    pub name: String,
    /// `Bridge$PlacementArn`: "the placement Amazon Resource Number (ARN) of the bridge" —
    /// the gateway the bridge is placed on (`ListedBridge$PlacementArn`: "the ARN of the
    /// gateway associated with the bridge").
    pub placement_arn: String,
    pub bridge_state: String,
    /// The state change the clock owes, if any. See [`TRANSITION_SPAN`].
    pub pending: Option<PendingStatus>,
    /// Set on a cloud-to-ground bridge, exclusive with [`Self::ingress_gateway_bridge`].
    pub egress_gateway_bridge: Option<EgressGatewayBridge>,
    /// Set on a ground-to-cloud bridge, exclusive with [`Self::egress_gateway_bridge`].
    pub ingress_gateway_bridge: Option<IngressGatewayBridge>,
    pub source_failover_config: Option<FailoverConfig>,
    pub sources: Vec<BridgeSource>,
    pub outputs: Vec<BridgeOutput>,
}

impl StoredBridge {
    /// Materialise the state change the clock already made due, if it is due at `now`.
    ///
    /// As for a flow (see [`StoredFlow::settle`]) this is the only write a reporting
    /// operation performs: the change was owed at `due_at` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.bridge_state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// bridge counterpart of [`StoredFlow::begin_transition`], so every state a bridge is
    /// answered in while the clock still owes a change is started one way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.bridge_state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// The state this bridge ends up in once the clock has settled everything it owes it:
    /// the pending state while a transition is in flight, and the current state otherwise.
    ///
    /// This is what `UpdateBridgeStateResponse$DesiredState` ("the new state of the
    /// bridge") reports — the state the bridge is HEADED for, read off the stored record
    /// rather than echoed from the request — and what makes a repeated UpdateBridgeState
    /// idempotent: a bridge already settling into the state a request asks for needs no
    /// new transition.
    pub fn settling_state(&self) -> &str {
        match self.pending.as_ref() {
            Some(pending) => &pending.status,
            None => &self.bridge_state,
        }
    }

    /// The modelled `Bridge` for this record.
    pub fn to_bridge(&self) -> Bridge {
        Bridge {
            bridge_arn: self.bridge_arn.clone(),
            // `Bridge.BridgeMessages` carries "messages with details about the bridge"; a
            // bridge this emulator serves reports none, so the member stays absent rather
            // than being an empty list.
            bridge_messages: None,
            bridge_state: self.bridge_state.clone(),
            egress_gateway_bridge: self.egress_gateway_bridge.clone(),
            ingress_gateway_bridge: self.ingress_gateway_bridge.clone(),
            name: self.name.clone(),
            // `Bridge.Outputs` / `Bridge.Sources` are optional lists, so a bridge that
            // holds none reports no member at all rather than an empty one.
            outputs: (!self.outputs.is_empty()).then(|| self.outputs.clone()),
            placement_arn: self.placement_arn.clone(),
            source_failover_config: self.source_failover_config.clone(),
            sources: (!self.sources.is_empty()).then(|| self.sources.clone()),
        }
    }

    /// `ListedBridge$BridgeType` for this record: the kind of bridge the create named,
    /// read off the bridge-type member it set (see [`BRIDGE_TYPE_EGRESS`]).
    ///
    /// The two members are exclusive, so the egress member decides and the ingress member
    /// is the only other possibility a stored bridge has.
    pub fn bridge_type(&self) -> &'static str {
        if self.egress_gateway_bridge.is_some() {
            BRIDGE_TYPE_EGRESS
        } else {
            BRIDGE_TYPE_INGRESS
        }
    }

    /// The modelled `ListedBridge` summary for this record — what ListBridges reports.
    ///
    /// Every member of the summary is required, and each one is the stored bridge's own:
    /// no summary member is computed any differently from the full [`Self::to_bridge`]
    /// projection, so a listing and a describe cannot disagree about one bridge.
    pub fn to_listed_bridge(&self) -> ListedBridge {
        ListedBridge {
            bridge_arn: self.bridge_arn.clone(),
            bridge_state: self.bridge_state.clone(),
            bridge_type: self.bridge_type().to_string(),
            name: self.name.clone(),
            placement_arn: self.placement_arn.clone(),
        }
    }

    /// The position of the source this bridge addresses by `name`
    /// (`AddBridgeNetworkSourceRequest$Name`: "used to reference the source"), or `None`
    /// when the bridge holds no such source.
    pub fn source_position(&self, name: &str) -> Option<usize> {
        self.sources
            .iter()
            .position(|source| bridge_source_name(source) == name)
    }

    /// The position of the output this bridge addresses by `name`
    /// (`AddBridgeNetworkOutputRequest$Name`: "used to reference the output"), or `None`
    /// when the bridge holds no such output.
    pub fn output_position(&self, name: &str) -> Option<usize> {
        self.outputs
            .iter()
            .position(|output| bridge_output_name(output) == name)
    }

    /// The names of this bridge's sources — what a source name added to the bridge must
    /// be unique among ("must be unique among sources in this bridge").
    pub fn source_names(&self) -> BTreeSet<String> {
        self.sources
            .iter()
            .map(|source| bridge_source_name(source).to_string())
            .collect()
    }

    /// The names of this bridge's outputs — what an output name added to the bridge must
    /// be unique among ("must be unique among outputs in this bridge").
    pub fn output_names(&self) -> BTreeSet<String> {
        self.outputs
            .iter()
            .map(|output| bridge_output_name(output).to_string())
            .collect()
    }
}

/// `RouterInputType` (`smithy_query(query="shape",shape="RouterInputType")`: STANDARD |
/// FAILOVER | MERGE | MEDIACONNECT_FLOW | MEDIALIVE_CHANNEL) — the kind of a router input,
/// which is DERIVED from the `RouterInputConfiguration` the create carried rather than
/// asked for: the configuration is a union with one member per kind, and
/// `RouterInput$InputType` is reported for every input. [`router_input_type`] is the one
/// mapping.
pub const ROUTER_INPUT_TYPE_STANDARD: &str = "STANDARD";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_FAILOVER: &str = "FAILOVER";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_MERGE: &str = "MERGE";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_MEDIACONNECT_FLOW: &str = "MEDIACONNECT_FLOW";
/// See [`ROUTER_INPUT_TYPE_STANDARD`].
pub const ROUTER_INPUT_TYPE_MEDIALIVE_CHANNEL: &str = "MEDIALIVE_CHANNEL";

/// The `RouterOutputState` enum the model declares for a router output
/// (`smithy_query(query="shape",shape="RouterOutputState")`: CREATING | STANDBY | STARTING
/// | ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING). Only the
/// states a router output's creation, stop, reconfiguration and deletion reach are named
/// here; the rest belong to operations that are still scaffolded.
pub const ROUTER_OUTPUT_STATE_CREATING: &str = "CREATING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. A router output that finished being created is
/// not carrying content yet: it waits in `STANDBY` until StartRouterOutput — and it is
/// where a STOPPED output settles, because stopping an output is what returns it to not
/// carrying content (see [`ROUTER_OUTPUT_STATE_STOPPING`]).
pub const ROUTER_OUTPUT_STATE_STANDBY: &str = "STANDBY";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The state of a router output that is carrying
/// content — what StartRouterOutput (still scaffolded) settles an output into, and the
/// state StopRouterOutput takes it out of.
pub const ROUTER_OUTPUT_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The non-terminal state a router output is
/// answered in while a stop is in flight: StopRouterOutput accepts the stop and the clock
/// settles the output into [`ROUTER_OUTPUT_STATE_STANDBY`] one [`TRANSITION_SPAN`] later,
/// so a caller reading the output straight after the stop still sees it stopping.
pub const ROUTER_OUTPUT_STATE_STOPPING: &str = "STOPPING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The settled state of a router output that is not
/// carrying content because something is wrong with it. Nothing in this emulator PUTS an
/// output here (there is no data plane to fail), so it is reached only by a caller seeding
/// the store — the same way [`ROUTER_INPUT_STATE_ERROR`] is; a stop takes an output out of
/// it like any other settled state.
pub const ROUTER_OUTPUT_STATE_ERROR: &str = "ERROR";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The transient state an output is RECONFIGURED in:
/// UpdateRouterOutput answers `202 Accepted`, so the settings it accepted are not in effect
/// the instant it answers, and the clock settles the output back into the state it was
/// heading for one [`TRANSITION_SPAN`] later
/// ([`StoredRouterOutput::begin_transition`]).
pub const ROUTER_OUTPUT_STATE_UPDATING: &str = "UPDATING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The state a router output is in while its delete
/// is in progress — `DeleteRouterOutputResponse$State` is "the current state of the deleted
/// router output, indicating where it is in the deletion process".
///
/// `RouterOutputState` carries NO terminal deleted value, unlike `GatewayState`'s
/// `DELETED`, so a router output delete is a HARD delete: the record LEAVES the store once
/// the clock completes the deletion rather than staying readable in a deleted state. See
/// [`StoredRouterOutput::removal_due_at`].
pub const ROUTER_OUTPUT_STATE_DELETING: &str = "DELETING";
/// See [`ROUTER_OUTPUT_STATE_CREATING`]. The transient state of a router output that is
/// being RESTARTED: RestartRouterOutput "restarts a router output … to recover from errors
/// or refresh the output state", and `RECOVERING` is the one value of the enum that names
/// that work in progress.
pub const ROUTER_OUTPUT_STATE_RECOVERING: &str = "RECOVERING";

/// The `RouterOutputRoutedState` enum the model declares
/// (`smithy_query(query="shape",shape="RouterOutputRoutedState")`: ROUTED | ROUTING |
/// UNROUTED). A router output is ROUTED exactly while a router input is routed to it
/// (`RouterOutput$RoutedInputArn`), and UNROUTED otherwise — see
/// [`StoredRouterOutput::routed_state`]. `ROUTING` is the transient state of the call that
/// takes an input (TakeRouterInput), which is still scaffolded.
pub const ROUTED_STATE_ROUTED: &str = "ROUTED";
/// See [`ROUTED_STATE_ROUTED`].
pub const ROUTED_STATE_UNROUTED: &str = "UNROUTED";

/// The `RouterOutputType` enum the model declares
/// (`smithy_query(query="shape",shape="RouterOutputType")`: STANDARD | MEDIACONNECT_FLOW |
/// MEDIALIVE_INPUT). The type is not a request member: it is the variant of the output's
/// `RouterOutputConfiguration` union, reported through
/// [`StoredRouterOutput::output_type`].
pub const ROUTER_OUTPUT_TYPE_STANDARD: &str = "STANDARD";
/// See [`ROUTER_OUTPUT_TYPE_STANDARD`].
pub const ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW: &str = "MEDIACONNECT_FLOW";
/// See [`ROUTER_OUTPUT_TYPE_STANDARD`].
pub const ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT: &str = "MEDIALIVE_INPUT";

/// The `RouterNetworkInterfaceState` enum the model declares for a router network
/// interface (`smithy_query(query="shape",shape="RouterNetworkInterfaceState")`: CREATING |
/// ACTIVE | UPDATING | DELETING | ERROR | RECOVERING). Only the four a network interface's
/// creation, update and deletion reach are named here; the rest belong to operations that
/// are still scaffolded.
pub const ROUTER_NETWORK_INTERFACE_STATE_CREATING: &str = "CREATING";
/// See [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`]. A network interface that finished being
/// created is usable: unlike a router input or output there is no start call for one, so
/// `ACTIVE` is where its creation settles.
pub const ROUTER_NETWORK_INTERFACE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`]. The non-terminal state a network
/// interface is answered in while an accepted UpdateRouterNetworkInterface is being applied
/// (that operation answers HTTP 202, `smithy_query(query="operation",
/// operation="UpdateRouterNetworkInterface")`); the clock settles it back into
/// [`ROUTER_NETWORK_INTERFACE_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later.
pub const ROUTER_NETWORK_INTERFACE_STATE_UPDATING: &str = "UPDATING";
/// See [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`]. The non-terminal state an interface is
/// answered in while an accepted DeleteRouterNetworkInterface is being carried out; the
/// enum carries no terminal deleted value, so the clock completes the deletion by taking
/// the record out of the store (see
/// [`StoredRouterNetworkInterface::removal_due_at`]).
pub const ROUTER_NETWORK_INTERFACE_STATE_DELETING: &str = "DELETING";

/// The `RouterNetworkInterfaceType` enum the model declares
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceType")`: PUBLIC | VPC). Like a
/// router input's or output's type it is not a request member: it is the variant of the
/// interface's `RouterNetworkInterfaceConfiguration` union, reported through the one
/// mapping [`router_network_interface_type`].
pub const ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC: &str = "PUBLIC";
/// See [`ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC`].
pub const ROUTER_NETWORK_INTERFACE_TYPE_VPC: &str = "VPC";

/// `RouterNetworkInterface$NetworkInterfaceType` for an interface configured by
/// `configuration` — the variant of the configuration union, which is what the type is.
pub fn router_network_interface_type(
    configuration: &RouterNetworkInterfaceConfiguration,
) -> &'static str {
    match configuration {
        RouterNetworkInterfaceConfiguration::Public(_) => ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
        RouterNetworkInterfaceConfiguration::Vpc(_) => ROUTER_NETWORK_INTERFACE_TYPE_VPC,
    }
}

/// The `MaintenanceType` enum the model declares
/// (`smithy_query(query="shape",shape="MaintenanceType")`: PREFERRED_DAY_TIME | DEFAULT).
/// Like a router resource's type, it is not a request member: it is the variant of the
/// resource's `MaintenanceConfiguration` union, reported through the one mapping
/// [`maintenance_type`] (which [`StoredRouterOutput::maintenance_type`] also resolves
/// through).
pub const MAINTENANCE_TYPE_PREFERRED_DAY_TIME: &str = "PREFERRED_DAY_TIME";
/// See [`MAINTENANCE_TYPE_PREFERRED_DAY_TIME`].
pub const MAINTENANCE_TYPE_DEFAULT: &str = "DEFAULT";

/// `RouterContentQualityAnalysisType`
/// (`smithy_query(query="shape",shape="RouterContentQualityAnalysisType")`): the enum has
/// exactly ONE value, and `RouterInput$ContentQualityAnalysisType` is reported for every
/// router input, so this is the value every input reports.
pub const ROUTER_CQA_TYPE_CONTENT_LEVEL: &str = "CONTENT_LEVEL";

/// The `RouterInputType` of an input configured by `configuration`.
///
/// `RouterInputConfiguration` is a union with one member per input kind
/// (`smithy_query(query="shape",shape="RouterInputConfiguration")`) and `RouterInputType`
/// names exactly those kinds, so the type an input reports is a function of the
/// configuration it was created with — never a separate request member. Every projection
/// of a stored router input resolves it HERE.
pub fn router_input_type(configuration: &RouterInputConfiguration) -> &'static str {
    match configuration {
        RouterInputConfiguration::Standard(_) => ROUTER_INPUT_TYPE_STANDARD,
        RouterInputConfiguration::Failover(_) => ROUTER_INPUT_TYPE_FAILOVER,
        RouterInputConfiguration::Merge(_) => ROUTER_INPUT_TYPE_MERGE,
        RouterInputConfiguration::MediaConnectFlow(_) => ROUTER_INPUT_TYPE_MEDIACONNECT_FLOW,
        RouterInputConfiguration::MediaLiveChannel(_) => ROUTER_INPUT_TYPE_MEDIALIVE_CHANNEL,
    }
}

/// The `MaintenanceType` of a resource whose maintenance is configured by
/// `configuration` — the union member it carries (`PreferredDayTime` or `Default`), which
/// is what the `MaintenanceType` enum names.
pub fn maintenance_type(configuration: &MaintenanceConfiguration) -> &'static str {
    match configuration {
        MaintenanceConfiguration::PreferredDayTime(_) => MAINTENANCE_TYPE_PREFERRED_DAY_TIME,
        MaintenanceConfiguration::Default(_) => MAINTENANCE_TYPE_DEFAULT,
    }
}

/// The `MaintenanceScheduleType` enum the model declares
/// (`smithy_query(query="shape",shape="MaintenanceScheduleType")`: WINDOW) — its only
/// value, and `MaintenanceSchedule` is a union with exactly one member (`Window`), so a
/// resource that has a maintenance schedule at all reports this type for it.
pub const MAINTENANCE_SCHEDULE_TYPE_WINDOW: &str = "WINDOW";

/// Seconds in a minute, in an hour and in a day — the units a maintenance preference
/// states its time of day in, and the step the next occurrence of one is found over.
const SECONDS_PER_MINUTE: u64 = 60;
/// See [`SECONDS_PER_MINUTE`].
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
/// See [`SECONDS_PER_MINUTE`].
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;
/// Days in a week — how far apart two occurrences of one preferred weekday are.
const DAYS_PER_WEEK: u64 = 7;

/// The time of day a maintenance window opens for a resource whose maintenance
/// configuration names no preferred time.
///
/// "UTC time when the maintenance will happen. Use 24-hour HH:MM format. Minutes must be
/// 00. Example: 13:00. The default value is 02:00."
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceStartHour")`) —
/// the default maintenance hour this service documents for a FLOW. AWS documents no default
/// hour for a router resource, so that one is carried over rather than a second hour being
/// invented: it is the hour this service states it maintains a resource at when nothing was
/// asked for.
pub const DEFAULT_MAINTENANCE_TIME_OF_DAY: Duration = Duration::from_secs(2 * SECONDS_PER_HOUR);

/// How long a maintenance window stays open: the distance between
/// `WindowMaintenanceSchedule$Start` and `WindowMaintenanceSchedule$End`.
///
/// AWS publishes no length for the window, so the span is this emulator's own choice. It is
/// non-zero, so the window a schedule reports always contains its `ScheduledTime`.
pub const MAINTENANCE_WINDOW_SPAN: Duration = Duration::from_secs(SECONDS_PER_HOUR);

/// The day of the week `day` names, for a value of the `Day` enum the model declares
/// (`smithy_query(query="shape",shape="Day")`: MONDAY | TUESDAY | WEDNESDAY | THURSDAY |
/// FRIDAY | SATURDAY | SUNDAY), or `None` for a string that is not one of them.
///
/// The enum's values are matched exactly, so no other spelling of a weekday is read as a
/// preference. A string this does not know leaves the schedule with no preferred day (see
/// [`maintenance_window_schedule`]), which keeps the derivation total; it is not reachable
/// through the API, because the member targets the enum and the boundary refuses every other
/// value.
pub fn maintenance_day_of_week(day: &str) -> Option<u64> {
    let index = match day {
        "MONDAY" => 0,
        "TUESDAY" => 1,
        "WEDNESDAY" => 2,
        "THURSDAY" => 3,
        "FRIDAY" => 4,
        "SATURDAY" => 5,
        "SUNDAY" => 6,
        _ => return None,
    };
    Some(index)
}

/// The distance from midnight UTC that the maintenance time `time` names, or `None` for a
/// string that is not a time of day.
///
/// `PreferredDayTimeMaintenanceConfiguration$Time` is "the preferred time for maintenance
/// operations", stated in the 24-hour `HH:MM` form this service documents for the same
/// preference on a flow
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceStartHour")`:
/// "UTC time when the maintenance will happen. Use 24-hour HH:MM format"). A seconds field
/// is accepted after the minutes because that is the same time of day written out in full.
pub fn maintenance_time_of_day(time: &str) -> Option<Duration> {
    /// One field of the time, at most two digits and at most `ceiling`.
    fn field(value: &str, ceiling: u64) -> Option<u64> {
        if value.is_empty() || value.len() > 2 || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        value.parse::<u64>().ok().filter(|parsed| *parsed <= ceiling)
    }

    let mut fields = time.split(':');
    let hours = field(fields.next()?, 23)?;
    let minutes = field(fields.next()?, 59)?;
    let seconds = match fields.next() {
        Some(seconds) => field(seconds, 59)?,
        None => 0,
    };
    if fields.next().is_some() {
        return None;
    }
    Some(Duration::from_secs(
        hours * SECONDS_PER_HOUR + minutes * SECONDS_PER_MINUTE + seconds,
    ))
}

/// The maintenance window a resource configured by `configuration` is next maintained in,
/// as of `from`.
///
/// The schedule is DERIVED from the resource's maintenance configuration and the instant it
/// was scheduled at, never carried by a request: a configuration that states a preferred day
/// and time is scheduled at the next occurrence of that weekday at that UTC time, and the
/// `Default` configuration, which states no day, at the next occurrence of
/// [`DEFAULT_MAINTENANCE_TIME_OF_DAY`]. The window opens there, lasts
/// [`MAINTENANCE_WINDOW_SPAN`], and the maintenance is scheduled for the instant it opens,
/// which is inside it.
///
/// The `Time` of a preference this cannot read is the documented default hour, which keeps
/// this a total function: such a preference does not reach the store, because the operation
/// that takes one refuses it as a bad request (see CreateRouterInput).
pub fn maintenance_window_schedule(
    configuration: &MaintenanceConfiguration,
    from: SystemTime,
) -> MaintenanceSchedule {
    let (preferred_day, time_of_day) = match configuration {
        MaintenanceConfiguration::PreferredDayTime(preference) => (
            maintenance_day_of_week(&preference.day),
            maintenance_time_of_day(&preference.time).unwrap_or(DEFAULT_MAINTENANCE_TIME_OF_DAY),
        ),
        MaintenanceConfiguration::Default(_) => (None, DEFAULT_MAINTENANCE_TIME_OF_DAY),
    };
    let start = next_time_of_day_occurrence(preferred_day, time_of_day, from);
    MaintenanceSchedule::Window(WindowMaintenanceSchedule {
        start: ::aws_smithy_types::DateTime::from(start),
        end: ::aws_smithy_types::DateTime::from(start + MAINTENANCE_WINDOW_SPAN),
        scheduled_time: ::aws_smithy_types::DateTime::from(start),
    })
}

/// Whether the maintenance window `schedule` reports has closed by `now`.
///
/// `MaintenanceSchedule` is a union with one member, so the window is always the one to
/// read; a closed window is spent, which is what makes the next occurrence of the
/// preference due (see [`StoredRouterInput::settle`]).
fn maintenance_window_has_closed(schedule: &MaintenanceSchedule, now: SystemTime) -> bool {
    let MaintenanceSchedule::Window(window) = schedule;
    let now_seconds = i64::try_from(
        now.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX);
    now_seconds >= window.end.secs()
}

/// The first instant strictly after `from` that falls at `time_of_day` past midnight UTC on
/// the weekday `day_of_week` (a Monday-first index as [`maintenance_day_of_week`] gives it,
/// or `None` for any day).
///
/// A UTC instant's weekday and time of day are a function of its epoch second: UTC keeps no
/// daylight saving, and neither the day boundary nor the week depends on the calendar's
/// months, so no month or leap rule takes part in this derivation — only the weekday the
/// epoch day itself fell on, which [`EPOCH_DAY_OF_WEEK`] anchors. A preference that has
/// already passed comes round again a week later when it names a day, and the next day when
/// it does not.
fn next_time_of_day_occurrence(
    day_of_week: Option<u64>,
    time_of_day: Duration,
    from: SystemTime,
) -> SystemTime {
    /// The Monday-first index of the epoch day, 1970-01-01, which was a Thursday.
    const EPOCH_DAY_OF_WEEK: u64 = 3;

    let since_epoch = from
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let midnight = since_epoch - since_epoch % SECONDS_PER_DAY;
    let (days_ahead, step_days) = match day_of_week {
        Some(day_of_week) => {
            let today = (since_epoch / SECONDS_PER_DAY + EPOCH_DAY_OF_WEEK) % DAYS_PER_WEEK;
            (
                (day_of_week % DAYS_PER_WEEK + DAYS_PER_WEEK - today) % DAYS_PER_WEEK,
                DAYS_PER_WEEK,
            )
        }
        // No preferred day: the window comes round every day, so the next one is today's if
        // it has not passed and tomorrow's otherwise.
        None => (0, 1),
    };
    let candidate = midnight + days_ahead * SECONDS_PER_DAY + time_of_day.as_secs();
    let start = if candidate > since_epoch {
        candidate
    } else {
        // The preference has already passed: the next occurrence is one step on.
        candidate + step_days * SECONDS_PER_DAY
    };
    SystemTime::UNIX_EPOCH + Duration::from_secs(start)
}

/// The maximum bitrate, in BITS PER SECOND, a router input of `tier` may carry, or `None`
/// for a tier this function does not know.
///
/// "The tier sets the maximum bitrate, in Mbps, for the router input. Valid values include
/// the following: INPUT_20 – 20 Mbps, INPUT_50 – 50 Mbps, INPUT_100 – 100 Mbps, INPUT_250
/// – 250 Mbps, INPUT_500 – 500 Mbps"
/// (`smithy_query(query="doc",shape="CreateRouterInputRequest",member="Tier")`), so the
/// tier a create names bounds the `MaximumBitrate` it may ask for. The published numbers
/// are in Mbps and MediaConnect states a bitrate in bits per second, so each is converted
/// through [`BITS_PER_MEGABIT`] — the router output side of the same rule is
/// [`router_output_tier_bitrate_ceiling`].
pub fn router_input_tier_bitrate_ceiling(tier: &str) -> Option<i64> {
    let megabits_per_second = match tier {
        "INPUT_20" => 20,
        "INPUT_50" => 50,
        "INPUT_100" => 100,
        "INPUT_250" => 250,
        "INPUT_500" => 500,
        _ => return None,
    };
    Some(megabits_per_second * BITS_PER_MEGABIT)
}

/// The `RouterInputTransitEncryption` an input reports when the create named none.
///
/// `RouterInput$TransitEncryption` is a REQUIRED member while
/// `CreateRouterInputRequest$TransitEncryption` is optional, so the service supplies a
/// value for an input whose create omitted it: the key configuration union's `Automatic`
/// member — "automatic" key management, which needs no secret and no role from the caller
/// — under the matching `AUTOMATIC` key type.
pub fn default_router_input_transit_encryption() -> RouterInputTransitEncryption {
    RouterInputTransitEncryption {
        encryption_key_type: Some(ROUTER_INPUT_TRANSIT_ENCRYPTION_KEY_TYPE_AUTOMATIC.to_string()),
        encryption_key_configuration: RouterInputTransitEncryptionKeyConfiguration::Automatic(
            AutomaticEncryptionKeyConfiguration {},
        ),
    }
}

/// `RouterInputTransitEncryptionKeyType`
/// (`smithy_query(query="shape",shape="RouterInputTransitEncryptionKeyType")`:
/// SECRETS_MANAGER | AUTOMATIC) — the key type of the transit encryption
/// [`default_router_input_transit_encryption`] supplies.
pub const ROUTER_INPUT_TRANSIT_ENCRYPTION_KEY_TYPE_AUTOMATIC: &str = "AUTOMATIC";

/// The `RouterContentQualityAnalysisConfiguration` an input reports when the create named
/// none.
///
/// `RouterInput$ContentQualityAnalysisConfiguration` is reported for every input while
/// `CreateRouterInputRequest$ContentQualityAnalysisConfiguration` is optional, and the
/// union has a single member (`ContentLevel`), so an input whose create configured no
/// content quality analysis reports that member with none of its three features
/// (`BlackFrames`, `FrozenFrames`, `SilentAudio`) configured — each is optional, and an
/// absent one is a feature nothing analyses.
pub fn default_router_content_quality_analysis_configuration()
-> RouterContentQualityAnalysisConfiguration {
    RouterContentQualityAnalysisConfiguration::ContentLevel(
        ContentQualityAnalysisFeatureConfiguration {
            black_frames: None,
            frozen_frames: None,
            silent_audio: None,
        },
    )
}

/// The `RouterInputStreamDetails` of an input configured by `configuration` that is not
/// carrying a stream.
///
/// `RouterInputStreamDetails` is a union whose members are the same five input kinds as
/// [`router_input_type`]'s, so which member an input reports follows from its
/// configuration. Every per-source address inside it stays ABSENT: the emulator serves no
/// media data plane, so no source address has been observed on an input — a failover and a
/// merge input still report both of their indexed entries, because the model makes those
/// two members required, and the index is the position the configuration lists the source
/// at.
pub fn initial_router_input_stream_details(
    configuration: &RouterInputConfiguration,
) -> RouterInputStreamDetails {
    match configuration {
        RouterInputConfiguration::Standard(_) => {
            RouterInputStreamDetails::Standard(StandardRouterInputStreamDetails {
                source_ip_address: None,
            })
        }
        RouterInputConfiguration::Failover(_) => {
            RouterInputStreamDetails::Failover(FailoverRouterInputStreamDetails {
                source_index_zero_stream_details: FailoverRouterInputIndexedStreamDetails {
                    source_index: 0,
                    source_ip_address: None,
                },
                source_index_one_stream_details: FailoverRouterInputIndexedStreamDetails {
                    source_index: 1,
                    source_ip_address: None,
                },
            })
        }
        RouterInputConfiguration::Merge(_) => {
            RouterInputStreamDetails::Merge(MergeRouterInputStreamDetails {
                source_index_zero_stream_details: MergeRouterInputIndexedStreamDetails {
                    source_index: 0,
                    source_ip_address: None,
                },
                source_index_one_stream_details: MergeRouterInputIndexedStreamDetails {
                    source_index: 1,
                    source_ip_address: None,
                },
            })
        }
        RouterInputConfiguration::MediaConnectFlow(_) => {
            RouterInputStreamDetails::MediaConnectFlow(MediaConnectFlowRouterInputStreamDetails {})
        }
        RouterInputConfiguration::MediaLiveChannel(_) => {
            RouterInputStreamDetails::MediaLiveChannel(MediaLiveChannelRouterInputStreamDetails {})
        }
    }
}

/// The ROUTER NETWORK INTERFACE ARN an input configured by `configuration` ingests on
/// (`StandardRouterInputConfiguration$NetworkInterfaceArn` and the same member on the
/// FAILOVER and MERGE configurations), or `None` for a configuration that takes its content
/// from another AWS resource instead and so names no interface.
///
/// The crate's ONE reading of that relation on the input side: it is what
/// `RouterNetworkInterface$AssociatedInputCount` counts
/// ([`ComAmazonawsMediaconnectMediaconnectState::associated_router_input_count`]) and what
/// [`router_input_has_own_address`] answers from.
pub fn router_input_network_interface_arn(
    configuration: &RouterInputConfiguration,
) -> Option<&str> {
    match configuration {
        RouterInputConfiguration::Standard(standard) => {
            Some(standard.network_interface_arn.as_str())
        }
        RouterInputConfiguration::Failover(failover) => {
            Some(failover.network_interface_arn.as_str())
        }
        RouterInputConfiguration::Merge(merge) => Some(merge.network_interface_arn.as_str()),
        RouterInputConfiguration::MediaConnectFlow(_)
        | RouterInputConfiguration::MediaLiveChannel(_) => None,
    }
}

/// The ROUTER NETWORK INTERFACE ARN an output configured by `configuration` egresses from
/// (`StandardRouterOutputConfiguration$NetworkInterfaceArn`), or `None` for a configuration
/// that delivers into another AWS resource and so names no interface.
///
/// The output-side counterpart of [`router_input_network_interface_arn`], and what
/// `RouterNetworkInterface$AssociatedOutputCount` counts
/// ([`ComAmazonawsMediaconnectMediaconnectState::associated_router_output_count`]).
pub fn router_output_network_interface_arn(
    configuration: &RouterOutputConfiguration,
) -> Option<&str> {
    match configuration {
        RouterOutputConfiguration::Standard(standard) => {
            Some(standard.network_interface_arn.as_str())
        }
        RouterOutputConfiguration::MediaConnectFlow(_)
        | RouterOutputConfiguration::MediaLiveInput(_) => None,
    }
}

/// Whether an input configured by `configuration` ingests on a ROUTER NETWORK INTERFACE of
/// this account — the configurations that carry a `NetworkInterfaceArn`.
///
/// Such an input is reachable at an address of its own, which is what
/// `RouterInput$IpAddress` ("the IP address of the router input") reports; an input that
/// takes its content from another AWS resource instead (a MediaConnect flow output, a
/// MediaLive channel output) has no address of its own, so the optional member stays
/// absent for it rather than carrying an address that addresses nothing.
pub fn router_input_has_own_address(configuration: &RouterInputConfiguration) -> bool {
    router_input_network_interface_arn(configuration).is_some()
}

/// A router input, as the service holds it between calls. Every router input operation's
/// response is computed from this record.
///
/// Keyed in the store by its own ARN, which is what every router input operation addresses
/// one by (`@httpLabel Arn`) and which carries the input's generated id, so two router
/// inputs that share a name are still distinct resources.
#[derive(Debug, Clone)]
pub struct StoredRouterInput {
    pub arn: String,
    /// `RouterInput$Id`: the per-input id the ARN carries, minted by
    /// [`router_resource_id`].
    pub id: String,
    pub name: String,
    pub state: String,
    /// The state change the clock owes, if any. See [`TRANSITION_SPAN`].
    pub pending: Option<PendingStatus>,
    pub configuration: RouterInputConfiguration,
    pub maximum_bitrate: i64,
    pub tier: String,
    pub routing_scope: String,
    pub region_name: String,
    pub availability_zone: String,
    pub transit_encryption: RouterInputTransitEncryption,
    pub maintenance_configuration: MaintenanceConfiguration,
    pub content_quality_analysis_configuration: RouterContentQualityAnalysisConfiguration,
    pub tags: BTreeMap<String, String>,
    /// `RouterInput$IpAddress`, for an input that ingests on a network interface of its
    /// own (see [`router_input_has_own_address`]).
    pub ip_address: Option<String>,
    /// The `ClientToken` of the create that minted this input, when it carried one — what
    /// makes a repeated create with the same token return THIS input rather than mint a
    /// second one (`CreateRouterInputRequest$ClientToken` is an `@idempotencyToken`).
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    /// When this record was last changed. A restart is such a change, so the member moves;
    /// a read never moves it.
    pub updated_at: SystemTime,
    /// The maintenance window this input is next maintained in
    /// (`RouterInput$MaintenanceSchedule`), or `None` while nothing has scheduled one.
    ///
    /// StartRouterInput is the call that schedules the window, from the input's maintenance
    /// CONFIGURATION ([`maintenance_window_schedule`]); it is held on the record rather than
    /// recomputed per read so every later read reports the same window — until the clock
    /// passes its end, when the next occurrence of the same preference is owed (see
    /// [`Self::settle`]).
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    /// When the record stops existing, or `None` while the input is not being deleted.
    ///
    /// A router input delete is not instantaneous and it is not a soft delete: the input
    /// reports `DELETING` for one [`TRANSITION_SPAN`], and then it is gone, because
    /// `RouterInputState` declares no terminal state a deleted input could be reported in.
    /// Holding the instant rather than sleeping for it is what lets the delete answer
    /// `DELETING` while a later call sees the input removed; the removal itself is
    /// materialised by [`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`],
    /// because it was owed at this instant whether or not anybody called.
    pub removal_due_at: Option<SystemTime>,
}

impl StoredRouterInput {
    /// Materialise the state change the clock already made due, if it is due at `now`, and
    /// re-schedule a maintenance window the clock has already carried this input past.
    ///
    /// As for a flow (see [`StoredFlow::settle`]) these are the only writes a reporting
    /// operation performs: each was owed at its own instant whether or not anybody called. A
    /// window that has closed is spent, and the resource is maintained again at the next
    /// occurrence of the preference it carries, so a read never reports a maintenance window
    /// that lies entirely in the past.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
        if self
            .maintenance_schedule
            .as_ref()
            .is_some_and(|schedule| maintenance_window_has_closed(schedule, now))
        {
            self.maintenance_schedule = Some(maintenance_window_schedule(
                &self.maintenance_configuration,
                now,
            ));
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// router input counterpart of [`StoredFlow::begin_transition`], so every state a
    /// router input is answered in while the clock still owes a change is started one way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// Whether a state change is in flight for this input: the clock still owes it one, so
    /// the input is between two states rather than settled in one — including the deletion
    /// it owes once DeleteRouterInput has accepted one, because an input on its way out is
    /// not an input a later call can still act on.
    pub fn is_transitioning(&self) -> bool {
        self.pending.is_some() || self.is_deleting()
    }

    /// Whether this input is already on its way out: DeleteRouterInput accepted a delete
    /// for it, so a repeat of that `@idempotent` call answers the same response rather than
    /// restarting the deletion.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has made this input's removal due at `now`.
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due_at| now >= due_at)
    }

    /// Enter `DELETING` now and owe the record's removal once [`TRANSITION_SPAN`] has
    /// passed — the deletion counterpart of [`Self::begin_transition`], which owes a STATE
    /// instead.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.state = ROUTER_INPUT_STATE_DELETING.to_string();
        self.removal_due_at = Some(now + TRANSITION_SPAN);
    }

    /// The modelled `RouterInput` for this record.
    ///
    /// `routed_outputs` is the number of router outputs routed to this input, which lives
    /// on the OUTPUT records rather than here
    /// ([`ComAmazonawsMediaconnectMediaconnectState::routed_output_count`] counts them), so
    /// it is passed in by the caller that holds the store.
    pub fn to_router_input(&self, routed_outputs: i32) -> RouterInput {
        RouterInput {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            state: self.state.clone(),
            input_type: router_input_type(&self.configuration).to_string(),
            configuration: self.configuration.clone(),
            // `RouterInput$RoutedOutputs` is "the number of outputs currently routed to
            // this input": the router outputs this store holds whose `RoutedInputArn` is
            // this input's ARN, counted by the caller (see above).
            routed_outputs,
            // `MaximumRoutedOutputs` is "the maximum number of outputs that can be
            // simultaneously routed to this input". AWS publishes no number for it and
            // nothing in this emulator enforces one, so the optional member stays absent
            // rather than reporting a ceiling nothing applies.
            maximum_routed_outputs: None,
            region_name: self.region_name.clone(),
            availability_zone: self.availability_zone.clone(),
            maximum_bitrate: self.maximum_bitrate,
            tier: self.tier.clone(),
            routing_scope: self.routing_scope.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            // `RouterInput$Messages` carries "messages with information about the router
            // input"; an input this emulator serves has nothing to report about itself, so
            // the required list is empty.
            messages: Vec::new(),
            transit_encryption: self.transit_encryption.clone(),
            tags: self
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            stream_details: initial_router_input_stream_details(&self.configuration),
            ip_address: self.ip_address.clone(),
            maintenance_type: maintenance_type(&self.maintenance_configuration).to_string(),
            maintenance_configuration: self.maintenance_configuration.clone(),
            // A maintenance SCHEDULE is the window MediaConnect picked inside the
            // configured preference, which StartRouterInput schedules as it starts an input
            // (its response makes `MaintenanceScheduleType` and `MaintenanceSchedule`
            // required). Both members are optional HERE because an input nothing has
            // started yet has no window: the type follows from the schedule the record
            // holds, the same way `MaintenanceType` follows from the configuration.
            maintenance_schedule_type: self
                .maintenance_schedule
                .as_ref()
                .map(|_| MAINTENANCE_SCHEDULE_TYPE_WINDOW.to_string()),
            maintenance_schedule: self.maintenance_schedule.clone(),
            content_quality_analysis_type: ROUTER_CQA_TYPE_CONTENT_LEVEL.to_string(),
            content_quality_analysis_configuration: self
                .content_quality_analysis_configuration
                .clone(),
        }
    }
}

/// The `FabricLatencyMode` the model documents as the DEFAULT recovery latency mode
/// (`FabricConfiguration$RecoveryLatencyMode`: "`BALANCED` (default) – Optimizes for
/// stream quality"), used for a request that sets no `FabricConfiguration`.
pub const FABRIC_LATENCY_MODE_BALANCED: &str = "BALANCED";

/// Bits in one megabit — the unit conversion between the tier's documented ceiling (Mbps)
/// and a router output's `MaximumBitrate` (bits per second, as every other MediaConnect
/// bitrate in this model is; see [`ASSIGNED_SOURCE_MAX_BITRATE_BPS`]).
pub const BITS_PER_MEGABIT: i64 = 1_000_000;

/// The maximum bitrate, in BITS PER SECOND, that the tier `tier` allows a router output,
/// or `None` when `tier` is not one the model declares.
///
/// The catalog is `CreateRouterOutput$Tier`'s own documentation
/// (`smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`: "The tier
/// sets the maximum bitrate, in Mbps, for the router output. … `OUTPUT_20` – 20 Mbps,
/// `OUTPUT_50` – 50 Mbps, `OUTPUT_100` – 100 Mbps, `OUTPUT_250` – 250 Mbps, `OUTPUT_500` –
/// 500 Mbps"), converted to bits per second through [`BITS_PER_MEGABIT`].
pub fn router_output_tier_bitrate_ceiling(tier: &str) -> Option<i64> {
    let megabits_per_second = match tier {
        "OUTPUT_20" => 20,
        "OUTPUT_50" => 50,
        "OUTPUT_100" => 100,
        "OUTPUT_250" => 250,
        "OUTPUT_500" => 500,
        _ => return None,
    };
    Some(megabits_per_second * BITS_PER_MEGABIT)
}

/// Why a router output of tier `tier` may not carry a `MaximumBitrate` of
/// `maximum_bitrate`, or `None` when that pair is acceptable.
///
/// "The tier sets the maximum bitrate, in Mbps, for the router output"
/// (`smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`), so the
/// tier an output ENDS UP with bounds the bitrate it may be asked for, through the one
/// catalog [`router_output_tier_bitrate_ceiling`]; and a bitrate is a count of bits per
/// second, so it is positive. Every operation that sets either member
/// (CreateRouterOutput, UpdateRouterOutput) judges the pair HERE, so the bound cannot
/// drift between them. The detail is the sentence the refusal's envelope carries
/// ([`validation_failed_message`]).
pub fn router_output_bitrate_rejection(tier: &str, maximum_bitrate: i64) -> Option<String> {
    let Some(ceiling) = router_output_tier_bitrate_ceiling(tier) else {
        return Some(format!(
            "Tier {tier:?} is not a router output tier this service offers."
        ));
    };
    if maximum_bitrate <= 0 {
        return Some(format!(
            "MaximumBitrate is a number of bits per second and must be positive; this \
             request asks for {maximum_bitrate}."
        ));
    }
    if maximum_bitrate > ceiling {
        return Some(format!(
            "MaximumBitrate {maximum_bitrate} bps exceeds the {ceiling} bps the {tier} tier \
             allows."
        ));
    }
    None
}

/// The ceiling on how many router outputs this store holds.
///
/// The same bound, for the same reason, as [`MAX_GATEWAYS`]: nothing evicts a router
/// output and a read must still answer what the create created, so the create refuses once
/// the store is full — with the modelled `RouterOutputServiceQuotaExceededException`, "the
/// request to create a new router output would exceed the service quotas (limits) set for
/// the account", which the model declares on CreateRouterOutput alone. The number is this
/// emulator's own bound, not a published AWS default.
pub const MAX_ROUTER_OUTPUTS: usize = 1_000;

/// The `RouterOutputProtocol` the protocol configuration `configuration` configures —
/// the value `StandardRouterOutputConfiguration$Protocol` names
/// (`smithy_query(query="shape",shape="RouterOutputProtocol")`: RTP | RIST | SRT_CALLER |
/// SRT_LISTENER | RTMP_PUSH).
///
/// The protocol is implied by the union variant, so this is the ONE mapping between the
/// two: a create derives the reported `Protocol` from it rather than trusting the request,
/// and refuses a request whose `Protocol` names a different one.
pub fn router_output_protocol(configuration: &RouterOutputProtocolConfiguration) -> &'static str {
    match configuration {
        RouterOutputProtocolConfiguration::Rtp(_) => "RTP",
        RouterOutputProtocolConfiguration::Rist(_) => "RIST",
        RouterOutputProtocolConfiguration::SrtCaller(_) => "SRT_CALLER",
        RouterOutputProtocolConfiguration::SrtListener(_) => "SRT_LISTENER",
        RouterOutputProtocolConfiguration::RtmpPush(_) => "RTMP_PUSH",
    }
}

/// The `RouterOutputConfiguration` a request's configuration is STORED as, or the detail of
/// why it cannot be stored as one.
///
/// A STANDARD output's `Protocol` is implied by the variant of its `ProtocolConfiguration`
/// ([`router_output_protocol`]), so the stored configuration reports the protocol that
/// configuration configures, and a request whose `Protocol` names a different one is
/// refused rather than stored self-contradicting. The two configurations that deliver into
/// another AWS resource carry no protocol member to reconcile, so each is stored as it
/// arrived. Every operation that takes a configuration (CreateRouterOutput,
/// UpdateRouterOutput) normalises it HERE, so no second spelling can drift from this one.
pub fn normalized_router_output_configuration(
    configuration: RouterOutputConfiguration,
) -> Result<RouterOutputConfiguration, String> {
    match configuration {
        RouterOutputConfiguration::Standard(standard) => {
            let configured = router_output_protocol(&standard.protocol_configuration);
            if let Some(named) = standard
                .protocol
                .as_deref()
                .filter(|protocol| !protocol.is_empty())
            {
                if named != configured {
                    return Err(format!(
                        "Configuration sets a {configured} protocol configuration, but \
                         Protocol names {named}."
                    ));
                }
            }
            Ok(RouterOutputConfiguration::Standard(
                StandardRouterOutputConfiguration {
                    protocol: Some(configured.to_string()),
                    ..standard
                },
            ))
        }
        other => Ok(other),
    }
}

/// The `RouterOutputType` of an output configured by `configuration`.
///
/// `RouterOutputConfiguration` is a union with one member per output kind and
/// `RouterOutputType` names exactly those kinds, so the type an output reports is a
/// function of the configuration it holds — never a separate request member. The router
/// output counterpart of [`router_input_type`], and the one mapping every projection
/// ([`StoredRouterOutput::output_type`]) and every check on the configuration's KIND
/// resolves through.
pub fn router_output_type(configuration: &RouterOutputConfiguration) -> &'static str {
    match configuration {
        RouterOutputConfiguration::Standard(_) => ROUTER_OUTPUT_TYPE_STANDARD,
        RouterOutputConfiguration::MediaConnectFlow(_) => ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW,
        RouterOutputConfiguration::MediaLiveInput(_) => ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT,
    }
}

/// The destination address a router output's protocol configuration names, or `None` for a
/// protocol that names none.
///
/// Every push protocol carries a `DestinationAddress`; an SRT LISTENER carries only a
/// `Port`, because a listener waits for a caller instead of sending to an address — so a
/// listener output reports no destination address.
pub fn protocol_destination_address(
    configuration: &RouterOutputProtocolConfiguration,
) -> Option<String> {
    match configuration {
        RouterOutputProtocolConfiguration::Rtp(rtp) => Some(rtp.destination_address.clone()),
        RouterOutputProtocolConfiguration::Rist(rist) => Some(rist.destination_address.clone()),
        RouterOutputProtocolConfiguration::SrtCaller(caller) => {
            Some(caller.destination_address.clone())
        }
        RouterOutputProtocolConfiguration::RtmpPush(push) => Some(push.destination_address.clone()),
        RouterOutputProtocolConfiguration::SrtListener(_) => None,
    }
}

/// The 12-character id the service mints for a ROUTER resource (output, input, network
/// interface) from the ARN counter.
///
/// The modelled `RouterOutputArn` pattern
/// (`smithy_query(query="shape",shape="RouterOutputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerOutput:[a-z0-9]{12}$`)
/// fixes the id at exactly twelve `[a-z0-9]` characters, so the counter is rendered as
/// zero-padded lowercase hex and only its low 48 bits are used — the store is bounded far
/// below that ([`MAX_ROUTER_OUTPUTS`]), so the id never repeats within a process.
pub fn router_resource_id(suffix: u64) -> String {
    format!("{:012x}", suffix & 0xffff_ffff_ffff)
}

/// The resource fragment of a ROUTER resource's ARN: `<resource type>:<id>`, with the id
/// [`router_resource_id`] mints.
///
/// Every router resource's modelled ARN pattern has this one shape — the resource type then
/// the twelve-character id, and no name — so the two fragment helpers
/// ([`router_input_resource_fragment`], [`router_output_resource_fragment`]) differ only in
/// that type and are built HERE.
fn router_resource_fragment(resource_type: &str, id: &str) -> String {
    format!("{resource_type}:{id}")
}

/// The resource fragment of a router output ARN: `routerOutput:<id>`, with the id
/// [`router_resource_id`] mints.
pub fn router_output_resource_fragment(id: &str) -> String {
    router_resource_fragment("routerOutput", id)
}

/// The resource fragment of a router network interface ARN: `routerNetworkInterface:<id>`,
/// with the id [`router_resource_id`] mints — the form the modelled
/// `RouterNetworkInterfaceArn` pattern fixes
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerNetworkInterface:[a-z0-9]{12}$`).
pub fn router_network_interface_resource_fragment(id: &str) -> String {
    router_resource_fragment("routerNetworkInterface", id)
}

/// Whether `arn` is an ARN this service could have minted for the ROUTER resource
/// `resource_type`: the prefix [`has_mintable_arn_prefix`] describes for that type, then an
/// id of the form [`router_resource_id`] mints.
///
/// The modelled ARN pattern of every router resource
/// (`smithy_query(query="shape",shape="RouterInputArn")` /
/// `…shape="RouterOutputArn"` /  `…shape="RouterNetworkInterfaceArn"`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:<type>:[a-z0-9]{12}$`) differs
/// only in that resource type, so this is the crate's ONE encoding of all of them, checked
/// segment by segment. [`is_mintable_router_input_arn`] and
/// [`is_mintable_router_output_arn`] are the two named entry points, so no second spelling
/// can drift from this one.
fn is_mintable_router_resource_arn(arn: &str, resource_type: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    let [.., id] = segments.as_slice() else {
        return false;
    };
    segments.len() == 7
        && has_mintable_arn_prefix(&segments, resource_type)
        && id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Whether `arn` is a router output ARN this service could have minted — the modelled
/// `RouterOutputArn` constraint, through the one encoding
/// [`is_mintable_router_resource_arn`]. The integration test asserts minted ARNs through
/// this function.
pub fn is_mintable_router_output_arn(arn: &str) -> bool {
    is_mintable_router_resource_arn(arn, "routerOutput")
}

/// Whether `arn` is a router network interface ARN this service could have minted — the
/// modelled `RouterNetworkInterfaceArn` constraint, through the one encoding
/// [`is_mintable_router_resource_arn`]. The integration test asserts minted ARNs through
/// this function.
pub fn is_mintable_router_network_interface_arn(arn: &str) -> bool {
    is_mintable_router_resource_arn(arn, "routerNetworkInterface")
}

/// Whether `zone` is an Availability Zone of the Region `region`.
///
/// `CreateRouterOutput$AvailabilityZone` must be "a valid Availability Zone for the region
/// specified by `regionName`, or the current region if no `regionName` is provided", and
/// AWS names an Availability Zone by its Region followed by one lowercase letter.
pub fn is_zone_of_region(zone: &str, region: &str) -> bool {
    zone.strip_prefix(region).is_some_and(|letter| {
        letter.len() == 1 && letter.bytes().all(|b| b.is_ascii_lowercase())
    })
}

/// The Availability Zone the service picks in `region` for a resource whose request named
/// none — the first zone of that Region.
pub fn default_availability_zone(region: &str) -> String {
    format!("{region}a")
}

/// A router output, as the service holds it between calls. Every router output
/// operation's response is computed from this record.
#[derive(Debug, Clone)]
pub struct StoredRouterOutput {
    /// `RouterOutput$Arn` — the output's identity, and the key of
    /// [`ComAmazonawsMediaconnectMediaconnectState::router_outputs`].
    pub arn: String,
    /// `RouterOutput$Id`: "the unique identifier of the router output" — the id segment of
    /// [`Self::arn`] ([`router_resource_id`]).
    pub id: String,
    pub name: String,
    /// The configuration the create stored, with `Protocol` normalised to the protocol its
    /// `ProtocolConfiguration` configures (see [`router_output_protocol`]). Its union
    /// variant is also what [`Self::output_type`] and [`Self::stream_details`] report.
    pub configuration: RouterOutputConfiguration,
    pub maximum_bitrate: i64,
    pub routing_scope: String,
    pub tier: String,
    pub region_name: String,
    pub availability_zone: String,
    /// `RouterOutput$IpAddress`: "the IP address of the router output" — the address the
    /// service assigns the output as it is created, from the same documentation-range pool
    /// a flow's egress address comes from ([`assigned_egress_ip`]).
    pub ip_address: Option<String>,
    pub maintenance_configuration: MaintenanceConfiguration,
    pub fabric_configuration: FabricConfiguration,
    /// `RouterOutput$Tags`, as the create set them. Ordered, so the record has one stable
    /// reading.
    pub tags: BTreeMap<String, String>,
    /// `RouterOutput$State` — see [`ROUTER_OUTPUT_STATE_CREATING`].
    pub state: String,
    /// The state change the clock owes this output, if any. See [`Self::settle`].
    pub pending: Option<PendingStatus>,
    /// `RouterOutput$RoutedInputArn`: the router input currently routed to this output, or
    /// `None` while nothing is. Only TakeRouterInput sets it, which is still scaffolded, so
    /// a created output is UNROUTED.
    pub routed_input_arn: Option<String>,
    /// `RouterOutput$Messages`: the messages the service has recorded about this output. A
    /// freshly created output has none, and the operations that record one are still
    /// scaffolded — the member is reported from here either way, never as a literal.
    pub messages: Vec<RouterOutputMessage>,
    /// The `@idempotencyToken` of the create that minted this output
    /// (`CreateRouterOutputRequest$ClientToken`, "a unique identifier for the request to
    /// ensure idempotency"), or `None` when the request carried none: a create replaying a
    /// token this store already holds returns THIS record instead of minting a second one.
    pub client_token: Option<String>,
    /// When the clock completes the delete of this router output, set by DeleteRouterOutput
    /// and `None` while the output is live.
    ///
    /// A router output delete is a HARD delete — `RouterOutputState` has no terminal deleted
    /// value (see [`ROUTER_OUTPUT_STATE_DELETING`]) — so this is the instant the deletion
    /// the call accepted becomes complete: until then the output is readable in `DELETING`
    /// and a repeated delete answers the same thing, and from then on the store no longer
    /// holds it. The removal is owed at this instant whether or not anybody calls, which is
    /// why a reporting operation may materialise it
    /// ([`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`]).
    pub removal_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl StoredRouterOutput {
    /// Materialise the state change the clock already made due, if it is due at `now`.
    ///
    /// The same contract as [`StoredFlow::settle`]: this is the only write a reporting
    /// operation (BatchGetRouterOutput, GetRouterOutput) performs, because the change was
    /// owed at `due_at` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// router output counterpart of [`StoredFlow::begin_transition`] and
    /// [`StoredRouterInput::begin_transition`], so every state a router output is answered
    /// in while the clock still owes a change is started one way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// Whether a state change is in flight for this output: the clock still owes it one, so
    /// the output is between two states rather than settled in one — including the deletion
    /// it owes once DeleteRouterOutput has accepted one, because an output on its way out is
    /// not an output a later call can still act on. The router output counterpart of
    /// [`StoredRouterInput::is_transitioning`].
    pub fn is_transitioning(&self) -> bool {
        self.pending.is_some() || self.is_deleting()
    }

    /// The state this output is heading for: the one the clock already owes it, or the one
    /// it is settled in when the clock owes none.
    ///
    /// What a call that puts the output through a TRANSIENT state of its own hands
    /// [`Self::begin_transition`] as the settled state, so an output reconfigured while it
    /// was still being created still reaches the state its creation owed it.
    pub fn settling_state(&self) -> String {
        self.pending
            .as_ref()
            .map_or_else(|| self.state.clone(), |pending| pending.status.clone())
    }

    /// Whether this router output's delete has been accepted, so it is on its way out of
    /// the store rather than live. See [`Self::removal_due_at`].
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has made this output's removal due at `now` — the same spelling
    /// [`StoredRouterInput::is_removal_due`] uses, so the two record types are settled by
    /// one rule in
    /// [`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`].
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due_at| now >= due_at)
    }

    /// `RouterOutput$OutputType`, the variant of the output's configuration union —
    /// resolved through the crate's one mapping, [`router_output_type`].
    pub fn output_type(&self) -> &'static str {
        router_output_type(&self.configuration)
    }

    /// Whether a router input is currently routed to this output — the ACTIVE ROUTE that
    /// `UpdateRouterOutput$FabricConfiguration` names as its precondition ("you cannot
    /// update the fabric configuration while the output has an active route"), which is the
    /// same condition [`Self::routed_state`] reports as `ROUTED`.
    pub fn is_routed(&self) -> bool {
        self.routed_input_arn.is_some()
    }

    /// `RouterOutput$RoutedState`: ROUTED exactly while a router input is routed to this
    /// output, UNROUTED otherwise.
    pub fn routed_state(&self) -> &'static str {
        if self.is_routed() {
            ROUTED_STATE_ROUTED
        } else {
            ROUTED_STATE_UNROUTED
        }
    }

    /// `RouterOutput$MaintenanceType`, the variant of the output's maintenance
    /// configuration union — resolved through the crate's one mapping,
    /// [`maintenance_type`].
    pub fn maintenance_type(&self) -> &'static str {
        maintenance_type(&self.maintenance_configuration)
    }

    /// `RouterOutput$StreamDetails` — the union member that matches the output's
    /// configuration, carrying what the service knows about the stream it carries: for a
    /// STANDARD output, the destination address its protocol configuration names
    /// ([`protocol_destination_address`]); for the two that deliver into another AWS
    /// resource, the empty details the model declares for them.
    pub fn stream_details(&self) -> RouterOutputStreamDetails {
        match &self.configuration {
            RouterOutputConfiguration::Standard(standard) => RouterOutputStreamDetails::Standard(
                StandardRouterOutputStreamDetails {
                    destination_ip_address: protocol_destination_address(
                        &standard.protocol_configuration,
                    ),
                },
            ),
            RouterOutputConfiguration::MediaConnectFlow(_) => {
                RouterOutputStreamDetails::MediaConnectFlow(
                    MediaConnectFlowRouterOutputStreamDetails {},
                )
            }
            RouterOutputConfiguration::MediaLiveInput(_) => {
                RouterOutputStreamDetails::MediaLiveInput(MediaLiveInputRouterOutputStreamDetails {})
            }
        }
    }

    /// The modelled `RouterOutput` for this record.
    pub fn to_router_output(&self) -> RouterOutput {
        RouterOutput {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            state: self.state.clone(),
            output_type: self.output_type().to_string(),
            configuration: self.configuration.clone(),
            routed_state: self.routed_state().to_string(),
            region_name: self.region_name.clone(),
            availability_zone: self.availability_zone.clone(),
            maximum_bitrate: self.maximum_bitrate,
            routing_scope: self.routing_scope.clone(),
            tier: self.tier.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            messages: self.messages.clone(),
            tags: self
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            stream_details: self.stream_details(),
            ip_address: self.ip_address.clone(),
            routed_input_arn: self.routed_input_arn.clone(),
            maintenance_type: self.maintenance_type().to_string(),
            maintenance_configuration: self.maintenance_configuration.clone(),
            // Nothing has scheduled maintenance for this output: the schedule members are
            // optional, and the operations that would fill them (the maintenance slice) are
            // still scaffolded, so they are reported as absent rather than invented.
            maintenance_schedule_type: None,
            maintenance_schedule: None,
            fabric_configuration: self.fabric_configuration.clone(),
        }
    }
}

/// The ceiling on how many router network interfaces this store holds.
///
/// The same bound, for the same reason, as [`MAX_ROUTER_OUTPUTS`]: nothing evicts a network
/// interface and a read must still answer what the create created, so the create refuses
/// once the store is full — with the modelled
/// `RouterNetworkInterfaceServiceQuotaExceededException` (HTTP 420), which the model
/// declares on CreateRouterNetworkInterface alone. The number is this emulator's own bound,
/// not a published AWS default.
pub const MAX_ROUTER_NETWORK_INTERFACES: usize = 1_000;

/// A router network interface, as the service holds it between calls — "a network interface
/// that can be associated with one or more router inputs and outputs"
/// (`smithy_query(query="shape",shape="RouterNetworkInterface")`). Every router network
/// interface operation's response is computed from this record, and the two association
/// counts from the input and output records that NAME this interface
/// ([`ComAmazonawsMediaconnectMediaconnectState::router_network_interface_projection`]).
#[derive(Debug, Clone)]
pub struct StoredRouterNetworkInterface {
    /// `RouterNetworkInterface$Arn` — the interface's identity, and the key of
    /// [`ComAmazonawsMediaconnectMediaconnectState::router_network_interfaces`].
    pub arn: String,
    /// `RouterNetworkInterface$Id`: "the unique identifier of the router network interface"
    /// — the id segment of [`Self::arn`] ([`router_resource_id`]).
    pub id: String,
    pub name: String,
    /// The configuration the create stored. Its union variant is also what
    /// [`Self::network_interface_type`] reports.
    pub configuration: RouterNetworkInterfaceConfiguration,
    pub region_name: String,
    /// `RouterNetworkInterface$Tags`, as the create set them. Ordered, so the record has
    /// one stable reading.
    pub tags: BTreeMap<String, String>,
    /// `RouterNetworkInterface$State` — see [`ROUTER_NETWORK_INTERFACE_STATE_CREATING`].
    pub state: String,
    /// The state change the clock owes this interface, if any. See [`Self::settle`].
    pub pending: Option<PendingStatus>,
    /// The `@idempotencyToken` of the create that minted this interface
    /// (`CreateRouterNetworkInterfaceRequest$ClientToken`), or `None` when the request
    /// carried none: a create replaying a token this store already holds returns THIS
    /// record instead of minting a second one.
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    /// When this record was last changed. An accepted UpdateRouterNetworkInterface that
    /// changes a member moves it; a read never moves it.
    pub updated_at: SystemTime,
    /// When the record stops existing, or `None` while the interface is not being deleted.
    ///
    /// A router network interface delete is a HARD delete, for the same reason a router
    /// input's and a router output's are: `RouterNetworkInterfaceState` declares no terminal
    /// deleted value (see [`ROUTER_NETWORK_INTERFACE_STATE_DELETING`]). So this is the
    /// instant the deletion the call accepted becomes complete: until then the interface is
    /// readable in `DELETING` and a repeated delete answers the same thing, and from then on
    /// the store no longer holds it. The removal is owed at this instant whether or not
    /// anybody calls, which is why a reporting operation may materialise it
    /// ([`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`]).
    pub removal_due_at: Option<SystemTime>,
}

impl StoredRouterNetworkInterface {
    /// Materialise the state change the clock already made due, if it is due at `now`.
    ///
    /// The same contract as [`StoredRouterOutput::settle`]: this is the only write a
    /// reporting operation (BatchGetRouterNetworkInterface) performs, because the change was
    /// owed at `due_at` whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
    }

    /// Enter `transient` now and owe `settled` once [`TRANSITION_SPAN`] has passed — the
    /// router network interface counterpart of [`StoredFlow::begin_transition`], so every
    /// state an interface is answered in while the clock still owes a change is started one
    /// way.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.state = transient.to_string();
        self.pending = Some(PendingStatus {
            status: settled.to_string(),
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// Whether a state change is in flight for this interface: the clock still owes it one,
    /// so the interface is between two states rather than settled in one — the same reading
    /// as [`StoredRouterInput::is_transitioning`], which is what an operation that MOVES the
    /// state (UpdateRouterNetworkInterface) refuses on. It includes the deletion the clock
    /// owes once DeleteRouterNetworkInterface has accepted one, because an interface on its
    /// way out is not an interface a later call can still act on.
    pub fn is_transitioning(&self) -> bool {
        self.pending.is_some() || self.is_deleting()
    }

    /// Whether this interface is already on its way out: DeleteRouterNetworkInterface
    /// accepted a delete for it, so a repeat of that `@idempotent` call answers the same
    /// response rather than restarting the deletion. See [`Self::removal_due_at`].
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has made this interface's removal due at `now` — the same spelling
    /// [`StoredRouterInput::is_removal_due`] and [`StoredRouterOutput::is_removal_due`] use,
    /// so every record type is settled by one rule in
    /// [`ComAmazonawsMediaconnectMediaconnectState::settle_due_transitions`].
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due_at| now >= due_at)
    }

    /// Enter `DELETING` now and owe the record's removal once [`TRANSITION_SPAN`] has
    /// passed — the deletion counterpart of [`Self::begin_transition`], which owes a STATE
    /// instead. The delete supersedes whatever state change was owed before it: an interface
    /// on its way out does not settle into `ACTIVE` on the way.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.state = ROUTER_NETWORK_INTERFACE_STATE_DELETING.to_string();
        self.pending = None;
        self.removal_due_at = Some(now + TRANSITION_SPAN);
        self.updated_at = now;
    }

    /// `RouterNetworkInterface$NetworkInterfaceType`, the variant of the interface's
    /// configuration union — through the crate's one mapping,
    /// [`router_network_interface_type`].
    pub fn network_interface_type(&self) -> &'static str {
        router_network_interface_type(&self.configuration)
    }

    /// The modelled `RouterNetworkInterface` for this record, with the two association
    /// counts resolved by the caller (they live on the input and output records, not here —
    /// see [`ComAmazonawsMediaconnectMediaconnectState::router_network_interface_projection`]).
    pub fn to_router_network_interface(
        &self,
        associated_input_count: i32,
        associated_output_count: i32,
    ) -> RouterNetworkInterface {
        RouterNetworkInterface {
            name: self.name.clone(),
            arn: self.arn.clone(),
            id: self.id.clone(),
            state: self.state.clone(),
            network_interface_type: self.network_interface_type().to_string(),
            configuration: self.configuration.clone(),
            associated_output_count,
            associated_input_count,
            region_name: self.region_name.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            tags: self
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }
}

/// The documented wire form of `Reservation$Start` and `Reservation$End`: "Use the
/// following format: `YYYY-MM-DDTHH:mm:SSZ`, where `T` and `Z` are literal characters"
/// (`smithy_query(query="doc",shape="PurchaseOfferingRequest",member="Start")`).
///
/// Both members are modelled as plain strings rather than timestamps, so the service
/// renders them itself. A reservation record keeps the two as instants
/// ([`StoredReservation::start_at`], [`StoredReservation::end_at`]) because the clock
/// decides when the reservation has expired; this is the one place they become the string
/// a response carries, truncated to the second, which is all that form holds.
pub fn format_reservation_instant(instant: SystemTime) -> String {
    let epoch_seconds = match instant.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        // Before the epoch: the error carries the distance back to the instant.
        Err(before_epoch) => -i64::try_from(before_epoch.duration().as_secs()).unwrap_or(i64::MAX),
    };
    ::aws_smithy_types::DateTime::from_secs(epoch_seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // An instant no RFC-3339 date can hold is reported as the empty string rather
        // than failing the read; no instant this service stores is one.
        .unwrap_or_default()
}

/// A reservation the account holds: "a pricing agreement for a discounted rate for a
/// specific outbound bandwidth" (`smithy_query(query="shape",shape="Reservation")`).
///
/// Every term of the offering — its duration, price, description and resource
/// specification — is COPIED onto the record when the offering is purchased, because
/// "MediaConnect defines this value in the offering"
/// (`smithy_query(query="shape",shape="Reservation",member="Duration")`): the reservation
/// is the agreement that was struck, so a later change to the offering catalogue cannot
/// rewrite a reservation already bought.
#[derive(Debug, Clone)]
pub struct StoredReservation {
    /// `Reservation$ReservationArn`, the ARN this service minted for the reservation and
    /// the identity DescribeReservation addresses it by (`@httpLabel ReservationArn`).
    pub reservation_arn: String,
    /// `Reservation$ReservationName`, "the name that you assigned to the reservation when
    /// you purchased the offering".
    pub reservation_name: String,
    /// `Reservation$OfferingArn`, the offering this reservation was bought from.
    pub offering_arn: String,
    /// `Reservation$OfferingDescription`, that offering's description.
    pub offering_description: String,
    /// `Reservation$CurrencyCode`.
    pub currency_code: String,
    /// `Reservation$PricePerUnit`, which with [`Self::price_units`] "makes up the rate".
    pub price_per_unit: String,
    /// `Reservation$PriceUnits` — a `PriceUnits` value.
    pub price_units: String,
    /// `Reservation$Duration`, "the length of time that this reservation is active", in
    /// [`Self::duration_units`].
    pub duration: i32,
    /// `Reservation$DurationUnits` — a `DurationUnits` value.
    pub duration_units: String,
    /// `Reservation$ResourceSpecification`, "a definition of the amount of outbound
    /// bandwidth" the reservation covers.
    pub resource_specification: ResourceSpecification,
    /// When the reservation becomes active — `Reservation$Start`, "you set this value when
    /// you purchase the offering". Rendered by [`format_reservation_instant`].
    pub start_at: SystemTime,
    /// When the reservation expires — `Reservation$End`, "calculated based on the start
    /// date and time that you set and the offering's duration". Rendered by
    /// [`format_reservation_instant`], and the instant from which [`Self::settle`] reports
    /// the reservation `EXPIRED`.
    pub end_at: SystemTime,
    /// `Reservation$ReservationState` — one of the [`RESERVATION_STATE_PROCESSING`] values.
    pub reservation_state: String,
    /// The reservation-state change the clock owes, or `None` when it owes none — the
    /// same mechanism every other record in this store uses (see [`PendingStatus`]).
    pub pending: Option<PendingStatus>,
}

impl StoredReservation {
    /// Materialise the reservation-state change the clock already made due, if it is due
    /// at `now`.
    ///
    /// Two changes are clock-owed, and neither waits on anybody calling: the state a
    /// purchase settles into one [`TRANSITION_SPAN`] after it was accepted, and EXPIRY —
    /// `End` is "the day and time that this reservation expires"
    /// (`smithy_query(query="shape",shape="Reservation",member="End")`), so from that
    /// instant on the reservation is expired whether or not it is read. Both terminal
    /// states stay put: an expired reservation does not come back, and a canceled one is
    /// never expired on top.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.take() {
            if now >= pending.due_at {
                self.reservation_state = pending.status;
            } else {
                self.pending = Some(pending);
            }
        }
        let runs_out = self.reservation_state == RESERVATION_STATE_PROCESSING
            || self.reservation_state == RESERVATION_STATE_ACTIVE;
        if runs_out && now >= self.end_at {
            self.reservation_state = RESERVATION_STATE_EXPIRED.to_string();
            // Expiry is terminal, so a settle still owed would otherwise move the
            // reservation back out of it.
            self.pending = None;
        }
    }

    /// The modelled `Reservation` for this record — the one projection every reservation
    /// operation answers from, so no two of them can report one reservation differently.
    pub fn to_reservation(&self) -> Reservation {
        Reservation {
            currency_code: self.currency_code.clone(),
            duration: self.duration,
            duration_units: self.duration_units.clone(),
            end: format_reservation_instant(self.end_at),
            offering_arn: self.offering_arn.clone(),
            offering_description: self.offering_description.clone(),
            price_per_unit: self.price_per_unit.clone(),
            price_units: self.price_units.clone(),
            reservation_arn: self.reservation_arn.clone(),
            reservation_name: self.reservation_name.clone(),
            reservation_state: self.reservation_state.clone(),
            resource_specification: self.resource_specification.clone(),
            start: format_reservation_instant(self.start_at),
        }
    }
}

/// Per-service state for com-amazonaws-mediaconnect-mediaconnect. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediaconnectMediaconnectState {
    /// Every live flow, keyed by its ARN. Ordered, so ListFlows pages stably.
    ///
    /// There is no index by name beside it: the model puts no uniqueness rule on a flow
    /// name, and the flow id in the ARN is what keeps two same-named flows apart — the
    /// same reasoning as for [`Self::gateways`].
    pub flows: BTreeMap<String, StoredFlow>,
    /// Every gateway the store holds, keyed by its ARN. Ordered, so ListGateways pages
    /// stably.
    ///
    /// Live gateways and the retired (deleted) ones alike: a gateway delete is a soft
    /// delete, so a deleted gateway stays in this map — and readable — in its terminal
    /// `DELETED` state (see [`StoredGateway::deleted_at`]).
    ///
    /// There is no index by name beside it: like a flow name, a gateway name is under no
    /// uniqueness rule in the model, and the gateway id in the ARN is what keeps two
    /// same-named gateways apart.
    pub gateways: BTreeMap<String, StoredGateway>,
    /// Every gateway instance the store holds, keyed by its ARN — the identity every
    /// instance operation addresses one by (`@httpLabel GatewayInstanceArn`). Ordered, so
    /// ListGatewayInstances can page stably once it is implemented.
    ///
    /// Registered instances and deregistered ones alike: `InstanceState` carries the
    /// terminal `DEREGISTERED` value, so a deregistered instance stays in this map — and
    /// readable — exactly as a deleted gateway does (see [`StoredGatewayInstance`]).
    ///
    /// One map for every gateway's instances, not one per gateway: ListGatewayInstances'
    /// `FilterArn` is a FILTER on an otherwise whole-account listing (the member is
    /// optional), and an instance names its gateway on its own record.
    pub gateway_instances: BTreeMap<String, StoredGatewayInstance>,
    /// Every live bridge, keyed by its ARN. Ordered, so ListBridges pages stably.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]:
    /// the model puts no uniqueness rule on `Bridge$Name`, and the bridge id in the ARN
    /// is what keeps two same-named bridges apart.
    pub bridges: BTreeMap<String, StoredBridge>,
    /// Every router input the store holds, keyed by its ARN — the identity every router
    /// input operation addresses one by (`@httpLabel Arn`). Ordered, so ListRouterInputs
    /// can page stably once it is implemented.
    ///
    /// A deleted router input leaves this map one [`TRANSITION_SPAN`] after the delete
    /// (see [`StoredRouterInput::removal_due_at`]): the model declares no terminal
    /// `DELETED` state it could be reported in, so unlike a gateway it is not retained.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `RouterInput$Name`, and the router input id in the
    /// ARN ([`router_resource_id`]) is what keeps two same-named inputs apart — the name is
    /// not even part of the ARN, which the modelled `RouterInputArn` pattern fixes to the
    /// id alone.
    pub router_inputs: BTreeMap<String, StoredRouterInput>,
    /// Every router output, keyed by its ARN — the identity every router output
    /// operation addresses one by (`@httpLabel Arn`, and the `Arns` BatchGetRouterOutput
    /// reads them by). Ordered, so ListRouterOutputs can page stably.
    ///
    /// A router output whose delete has been accepted is still in this map, in `DELETING`,
    /// until the clock completes the delete; then it LEAVES, because `RouterOutputState`
    /// has no terminal deleted value (see [`StoredRouterOutput::removal_due_at`]).
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `RouterOutput$Name`, and the generated output id in
    /// the ARN is what keeps two same-named outputs apart.
    pub router_outputs: BTreeMap<String, StoredRouterOutput>,
    /// Every router network interface, keyed by its ARN — the identity every router network
    /// interface operation addresses one by (`@httpLabel Arn`, and the `Arns`
    /// BatchGetRouterNetworkInterface reads them by). Ordered, so
    /// ListRouterNetworkInterfaces can page stably.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `RouterNetworkInterface$Name`, and the generated
    /// interface id in the ARN is what keeps two same-named interfaces apart — the name is
    /// not part of the ARN at all, which the modelled `RouterNetworkInterfaceArn` pattern
    /// fixes to the id alone.
    pub router_network_interfaces: BTreeMap<String, StoredRouterNetworkInterface>,
    /// Every reservation this account holds, keyed by its ARN — the identity
    /// DescribeReservation addresses one by (`@httpLabel ReservationArn`). Ordered, so
    /// ListReservations pages stably.
    ///
    /// A reservation is never removed: ListReservations "displays a list of all
    /// reservations that have been purchased by this account in the current Amazon Web
    /// Services Region. This list includes all reservations in all states (such as active
    /// and expired)" (`smithy_query(query="operation",operation="ListReservations")`), so a
    /// reservation the clock has expired stays in this map and keeps being reported, in its
    /// terminal state.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `Reservation$ReservationName`, and the reservation
    /// id in the ARN is what keeps two same-named reservations apart.
    pub reservations: BTreeMap<String, StoredReservation>,
    /// Monotonic counter minting the unique fragment of every flow / source / output /
    /// entitlement ARN and of the addresses and ports the service assigns.
    pub next_arn_suffix: u64,
}

impl ComAmazonawsMediaconnectMediaconnectState {
    /// The next value of the ARN-fragment counter. Never repeats within a process.
    pub fn next_suffix(&mut self) -> u64 {
        self.next_arn_suffix += 1;
        self.next_arn_suffix
    }

    /// The tags of the resource `resource_arn` addresses, or `None` when this service
    /// holds no tags for such a resource.
    ///
    /// A `ResourceArn` addresses "a resource ... in the current region", and every
    /// MediaConnect resource whose ARN this service mints and still holds in the flow
    /// hierarchy carries its OWN tag map: the flow itself, and each of its ARN-bearing
    /// sub-resources — an output (`Output$OutputArn`), a source (`Source$SourceArn`) and an
    /// entitlement (`Entitlement$EntitlementArn`), each taggable through its create
    /// request's own tag member (`AddOutputRequest$OutputTags`,
    /// `SetSourceRequest$SourceTags`, `GrantEntitlementRequest$EntitlementTags`). So an ARN
    /// this emulator minted and returned resolves here, whichever of those it names.
    ///
    /// What does NOT resolve: a resource that carries no tag map because no MediaConnect
    /// operation takes or reports its tags (a gateway, a gateway instance, a bridge, a
    /// reservation), a flow sub-resource with no ARN of its own (a media stream and a VPC
    /// interface are addressed by name within their flow, so no `ResourceArn` can name
    /// one), and a router input / router output / router network interface, whose tags are
    /// the GLOBAL tag operations' subject (TagGlobalResource / UntagGlobalResource /
    /// ListTagsForGlobalResource, which cover exactly those three). Any other ARN addresses
    /// nothing whose tags these operations reach.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(flow) = self.flows.get(resource_arn) {
            return Some(&flow.tags);
        }
        self.flows
            .values()
            .find_map(|flow| flow.sub_resource_tags(resource_arn))
    }

    /// The tags of the resource `resource_arn` addresses, for a tag write. See
    /// [`ComAmazonawsMediaconnectMediaconnectState::tags_of`] for which ARNs resolve.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        // Two lookups rather than one `get_mut`: returning the borrow from the first
        // branch would hold `flows` mutably borrowed across the fallback scan below, which
        // the borrow checker refuses (a conditional-return borrow the current region
        // inference cannot narrow).
        if self.flows.contains_key(resource_arn) {
            return self.flows.get_mut(resource_arn).map(|flow| &mut flow.tags);
        }
        self.flows
            .values_mut()
            .find_map(|flow| flow.sub_resource_tags_mut(resource_arn))
    }

    /// Every entitlement the store holds that is granted TO the account `account_id`,
    /// keyed by the entitlement's own ARN.
    ///
    /// An entitlement is a sub-resource of the flow that GRANTS it, and the accounts it is
    /// granted to are its subscribers ([`StoredEntitlement::is_granted_to`]) — so the
    /// entitlements one account holds are gathered from every stored flow rather than read
    /// off one list. The keys are ARNs, so the answer is ordered and a listing can page
    /// from one stably, exactly as the ARN-keyed resource maps above let one page.
    pub fn entitlements_granted_to(
        &self,
        account_id: &str,
    ) -> BTreeMap<String, ListedEntitlement> {
        self.flows
            .values()
            .flat_map(|flow| flow.entitlements.iter())
            .filter(|entitlement| entitlement.is_granted_to(account_id))
            .map(|entitlement| {
                (
                    entitlement.arn().to_string(),
                    entitlement.to_listed_entitlement(),
                )
            })
            .collect()
    }

    /// Settle every flow, gateway, bridge, router input, router output and router network
    /// interface whose status change the clock already made due, and materialise the removal
    /// of every router input, router output and router network interface whose deletion has
    /// come due.
    pub fn settle_due_transitions(&mut self, now: SystemTime) {
        for flow in self.flows.values_mut() {
            flow.settle(now);
        }
        for gateway in self.gateways.values_mut() {
            gateway.settle(now);
        }
        for instance in self.gateway_instances.values_mut() {
            instance.settle(now);
        }
        for bridge in self.bridges.values_mut() {
            bridge.settle(now);
        }
        for router_input in self.router_inputs.values_mut() {
            router_input.settle(now);
        }
        for router_output in self.router_outputs.values_mut() {
            router_output.settle(now);
        }
        for interface in self.router_network_interfaces.values_mut() {
            interface.settle(now);
        }
        for reservation in self.reservations.values_mut() {
            reservation.settle(now);
        }
        // A router input, router output or router network interface whose removal the clock
        // has made due stops existing: the deletion was owed at that instant whether or not
        // anybody called, and none of `RouterInputState`, `RouterOutputState` and
        // `RouterNetworkInterfaceState` declares a terminal state the record could be
        // reported in afterwards (see [`StoredRouterInput::removal_due_at`],
        // [`StoredRouterOutput::removal_due_at`] and
        // [`StoredRouterNetworkInterface::removal_due_at`]).
        self.router_inputs
            .retain(|_, router_input| !router_input.is_removal_due(now));
        self.router_outputs
            .retain(|_, router_output| !router_output.is_removal_due(now));
        self.router_network_interfaces
            .retain(|_, interface| !interface.is_removal_due(now));
    }

    /// The router input `arn` addresses, with everything the clock already made due
    /// materialised first — the state changes it owes, and the removal of an input whose
    /// deletion has come due, which is why an input the clock has already taken away is
    /// `None` here.
    ///
    /// The one lookup every router input operation resolves an `@httpLabel` ARN through,
    /// so one request can never see a state another request would not.
    pub fn router_input_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut StoredRouterInput> {
        self.settle_due_transitions(now);
        self.router_inputs.get_mut(arn)
    }

    /// The router output `arn` addresses, with every state change the clock already made
    /// due materialised first.
    ///
    /// The router output counterpart of [`Self::router_input_mut`]: the one lookup a router
    /// output operation resolves an `@httpLabel` ARN through, so one request can never see
    /// a state another request would not.
    pub fn router_output_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut StoredRouterOutput> {
        self.settle_due_transitions(now);
        self.router_outputs.get_mut(arn)
    }

    /// How many router outputs are currently ROUTED to the router input `input_arn` —
    /// `RouterInput$RoutedOutputs`.
    ///
    /// The relation lives on the output records (`StoredRouterOutput::routed_input_arn`,
    /// which only TakeRouterInput sets), so it is counted here rather than held a second
    /// time on the input.
    pub fn routed_output_count(&self, input_arn: &str) -> i32 {
        let routed = self
            .router_outputs
            .values()
            .filter(|router_output| router_output.routed_input_arn.as_deref() == Some(input_arn))
            .count();
        i32::try_from(routed).unwrap_or(i32::MAX)
    }

    /// The modelled `RouterInput` the store holds for `arn`, with everything the clock
    /// already made due materialised first (as [`Self::router_input_mut`] does) and
    /// [`Self::routed_output_count`] resolved — the one projection every router input
    /// operation answers from.
    pub fn router_input_projection(&mut self, arn: &str, now: SystemTime) -> Option<RouterInput> {
        self.settle_due_transitions(now);
        let routed_outputs = self.routed_output_count(arn);
        self.router_inputs
            .get(arn)
            .map(|router_input| router_input.to_router_input(routed_outputs))
    }

    /// How many router inputs ingest on the router network interface `interface_arn` —
    /// `RouterNetworkInterface$AssociatedInputCount`, "the number of router inputs
    /// associated with the network interface".
    ///
    /// The relation lives on the input records (the `NetworkInterfaceArn` their
    /// configuration names, see [`router_input_network_interface_arn`]), so it is counted
    /// here rather than held a second time on the interface, where a create or delete of an
    /// input could leave it stale.
    pub fn associated_router_input_count(&self, interface_arn: &str) -> i32 {
        let associated = self
            .router_inputs
            .values()
            .filter(|router_input| {
                router_input_network_interface_arn(&router_input.configuration)
                    == Some(interface_arn)
            })
            .count();
        i32::try_from(associated).unwrap_or(i32::MAX)
    }

    /// How many router outputs egress from the router network interface `interface_arn` —
    /// `RouterNetworkInterface$AssociatedOutputCount`. Counted from the output records for
    /// the same reason as [`Self::associated_router_input_count`].
    pub fn associated_router_output_count(&self, interface_arn: &str) -> i32 {
        let associated = self
            .router_outputs
            .values()
            .filter(|router_output| {
                router_output_network_interface_arn(&router_output.configuration)
                    == Some(interface_arn)
            })
            .count();
        i32::try_from(associated).unwrap_or(i32::MAX)
    }

    /// The modelled `RouterNetworkInterface` the store holds for `arn`, with both
    /// association counts resolved — the one projection every router network interface
    /// operation answers from, so no two of them can report one interface differently.
    ///
    /// Does NOT settle the clock: a caller that reports an interface settles the store once
    /// ([`Self::settle_due_transitions`]) and then projects, so one request cannot see a
    /// state another request would not.
    pub fn router_network_interface_projection(&self, arn: &str) -> Option<RouterNetworkInterface> {
        let interface = self.router_network_interfaces.get(arn)?;
        Some(interface.to_router_network_interface(
            self.associated_router_input_count(arn),
            self.associated_router_output_count(arn),
        ))
    }

    /// How many LIVE gateways the store holds — the count [`MAX_GATEWAYS`] bounds.
    ///
    /// A retired (deleted) gateway stays in the map so it remains readable in its terminal
    /// `DELETED` state, but it no longer occupies one of the account's gateway slots: the
    /// ceiling counts the gateways that exist, so deleting one makes room for another.
    pub fn live_gateway_count(&self) -> usize {
        self.gateways
            .values()
            .filter(|gateway| !gateway.is_retired())
            .count()
    }

    /// Register an instance with the gateway `gateway_arn` names, and hand back the record
    /// as it is now STORED — read back out of the map after the insert, so the ARN a caller
    /// sees is the key the store holds.
    ///
    /// This is the OUT-OF-BAND registration channel described on [`StoredGatewayInstance`]:
    /// the model declares no operation that registers an instance, because on AWS the
    /// MediaConnect Gateway software's SSM install on the on-premises appliance is what
    /// registers one. Nothing a client sends over the wire reaches here; the gateway
    /// operations only report, update and deregister what this recorded.
    ///
    /// A newly registered instance is not ready yet: it is reported in the non-terminal
    /// [`INSTANCE_STATE_REGISTERING`], which the clock settles into
    /// [`INSTANCE_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later — the same shape as the
    /// gateway's own `CREATING` -> `ACTIVE`. It can host new bridges
    /// ([`BRIDGE_PLACEMENT_AVAILABLE`]) and it is connected to the service, which is what
    /// having just registered means.
    pub fn register_gateway_instance(
        &mut self,
        gateway_arn: &str,
        now: SystemTime,
    ) -> StoredGatewayInstance {
        let instance_id = gateway_instance_id(self.next_suffix());
        let instance_arn = gateway_instance_arn(gateway_arn, &instance_id);
        self.gateway_instances
            .entry(instance_arn.clone())
            .or_insert_with(|| StoredGatewayInstance {
                gateway_instance_arn: instance_arn,
                gateway_arn: gateway_arn.to_string(),
                instance_id,
                instance_state: INSTANCE_STATE_REGISTERING.to_string(),
                bridge_placement: BRIDGE_PLACEMENT_AVAILABLE.to_string(),
                connection_status: CONNECTION_STATUS_CONNECTED.to_string(),
                pending: Some(PendingStatus {
                    status: INSTANCE_STATE_ACTIVE.to_string(),
                    due_at: now + TRANSITION_SPAN,
                }),
            })
            .clone()
    }

    /// How many bridges are RUNNING on `instance` — `GatewayInstance$RunningBridgeCount`,
    /// and the count DeregisterGatewayInstance's documented precondition is stated against
    /// ("before you deregister an instance, all bridges running on the instance must be
    /// stopped").
    ///
    /// DERIVED from the bridge store rather than held on the instance record: a bridge
    /// names the gateway it is placed on (`ListedBridge$PlacementArn`: "the ARN of the
    /// gateway associated with the bridge") and carries its own `BridgeState`, so the
    /// bridges running on a gateway are the bridges placed on it that are not stopped —
    /// the same reading of "stopped" DeleteBridge's precondition uses
    /// ([`is_stopped_bridge_state`]). This emulator runs no gateway appliance and so models
    /// no per-instance deployment of a gateway's bridges: a bridge running on a gateway is
    /// counted on each of that gateway's registered instances. An instance that has reached
    /// the terminal `DEREGISTERED` state is no longer part of its gateway and hosts
    /// nothing, so its count is zero.
    pub fn running_bridge_count(&self, instance: &StoredGatewayInstance) -> i32 {
        if !instance.is_registered() {
            return 0;
        }
        let running = self
            .bridges
            .values()
            .filter(|bridge| {
                bridge.placement_arn == instance.gateway_arn
                    && !is_stopped_bridge_state(&bridge.bridge_state)
            })
            .count();
        i32::try_from(running).unwrap_or(i32::MAX)
    }

    /// The modelled `GatewayInstance` the store holds for `arn`, with its running-bridge
    /// count resolved — the one projection every gateway-instance operation answers from,
    /// so no two of them can report one instance differently.
    ///
    /// Does NOT settle the clock: a caller settles the store once
    /// ([`Self::settle_due_transitions`]) and then projects, so one request cannot see a
    /// state another request would not.
    pub fn gateway_instance_projection(&self, arn: &str) -> Option<GatewayInstance> {
        let instance = self.gateway_instances.get(arn)?;
        Some(instance.to_gateway_instance(self.running_bridge_count(instance)))
    }

    /// The instances still REGISTERED with the gateway `gateway_arn` names — the ones
    /// DeleteGateway's documented precondition requires to be deregistered first ("before
    /// you can delete a gateway, you must deregister its instances and delete its
    /// bridges").
    ///
    /// A deregistered instance stays readable in its terminal `DEREGISTERED` state but is
    /// no longer one of the gateway's instances, exactly as a retired gateway is no longer
    /// one of the account's ([`Self::live_gateway_count`]).
    pub fn registered_gateway_instance_count(&self, gateway_arn: &str) -> usize {
        self.gateway_instances
            .values()
            .filter(|instance| instance.gateway_arn == gateway_arn && instance.is_registered())
            .count()
    }

    /// Drop the oldest retired gateway records once the store holds more than
    /// [`MAX_RETIRED_GATEWAYS`] of them, so a create-then-delete loop cannot grow the
    /// gateway map without limit.
    ///
    /// Called from DeleteGateway — the call that retires a record — and never from a
    /// read: what a reporting operation reports is the record the store holds.
    pub fn evict_oldest_retired_gateways(&mut self) {
        let mut retired: Vec<(SystemTime, String)> = self
            .gateways
            .values()
            .filter_map(|gateway| {
                gateway
                    .deleted_at
                    .map(|deleted_at| (deleted_at, gateway.gateway_arn.clone()))
            })
            .collect();
        if retired.len() <= MAX_RETIRED_GATEWAYS {
            return;
        }
        // Oldest first, so the records dropped are the ones retired longest ago; the ARN
        // breaks a tie between two retired in the same instant.
        retired.sort();
        let excess = retired.len() - MAX_RETIRED_GATEWAYS;
        for (_, gateway_arn) in retired.into_iter().take(excess) {
            self.gateways.remove(&gateway_arn);
        }
    }
}

/// The resource fragment of a flow ARN: `flow:<flow id>:<name>`.
///
/// The id is the ARN counter in zero-padded hex, so the fragment satisfies the
/// `FlowArn` pattern the model declares
/// (`^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$`)
/// and is unique per flow.
pub fn flow_resource_fragment(suffix: u64, name: &str) -> String {
    format!("flow:1-{suffix:08x}:{name}")
}

/// The description the service DERIVES for a flow as it is created.
///
/// `CreateFlow` models no `Description` member, yet `Flow.Description` is reported for
/// every flow and `ListedFlow.Description` is a REQUIRED member, so the value is
/// service-derived rather than client-supplied: it is the description given for the
/// flow's own source when the request supplies a non-blank one — the source description
/// "is not used or seen outside of the current MediaConnect account"
/// (`smithy_query(query="doc",shape="SetSourceRequest",member="Description")`), exactly
/// like the flow description — and otherwise the sentence naming the flow and that
/// source.
///
/// Derived ONCE, when the flow is created, and then stored: it is part of the flow
/// record, so every later read reports the same value.
pub fn derived_flow_description(
    flow_name: &str,
    source_name: &str,
    source_description: Option<&str>,
) -> String {
    match source_description
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        Some(description) => description.to_string(),
        None => format!("{flow_name} using source {source_name}"),
    }
}

/// The resource fragment of a gateway ARN: `gateway:<gateway id>:<name>`.
///
/// Shaped like [`flow_resource_fragment`] — the id is the ARN counter in zero-padded hex,
/// so the fragment is unique per gateway even when two gateways share a name — and it
/// satisfies the `GatewayArn` pattern the model declares
/// (`smithy_query(query="shape",shape="GatewayArn")`: `^arn:.+:mediaconnect.+:gateway:.+$`).
pub fn gateway_resource_fragment(suffix: u64, name: &str) -> String {
    format!("gateway:1-{suffix:08x}:{name}")
}

/// The ARN of the instance `instance_id` of the gateway `gateway_arn`: the gateway's own
/// ARN with an `instance:<instance id>` suffix.
///
/// The modelled `GatewayInstanceArn` pattern
/// (`smithy_query(query="shape",shape="GatewayInstanceArn")`:
/// `^arn:.+:mediaconnect.+:gateway:.+:instance:.+$`) puts the instance UNDER its gateway,
/// so the gateway's ARN is recoverable from the instance's ([`gateway_arn_of_instance`])
/// and the instance identity carries the gateway identity rather than repeating it.
pub fn gateway_instance_arn(gateway_arn: &str, instance_id: &str) -> String {
    format!("{gateway_arn}:instance:{instance_id}")
}

/// The ARN segment separating a gateway ARN from the instance id under it. See
/// [`gateway_instance_arn`].
const GATEWAY_INSTANCE_ARN_SEPARATOR: &str = ":instance:";

/// The gateway ARN and instance id an instance ARN is built from, or `None` when `arn`
/// carries no instance segment at all. The inverse of [`gateway_instance_arn`].
pub fn split_gateway_instance_arn(arn: &str) -> Option<(&str, &str)> {
    let (gateway_arn, instance_id) = arn.rsplit_once(GATEWAY_INSTANCE_ARN_SEPARATOR)?;
    (!gateway_arn.is_empty() && !instance_id.is_empty()).then_some((gateway_arn, instance_id))
}

/// The ARN of the gateway the instance ARN `arn` belongs to, or `None` when `arn` carries
/// no instance segment. See [`gateway_instance_arn`].
pub fn gateway_arn_of_instance(arn: &str) -> Option<&str> {
    split_gateway_instance_arn(arn).map(|(gateway_arn, _)| gateway_arn)
}

/// The id of a gateway instance: `mi-` and the ARN counter in zero-padded hex.
///
/// `GatewayInstance$InstanceId` is "the instance ID generated by the SSM install. This will
/// begin with `mi-`" — an SSM managed-instance id — so the prefix is the model's and the
/// 17 hex characters after it are the width SSM uses, filled from the per-store counter so
/// no two instances of one process share an id.
pub fn gateway_instance_id(suffix: u64) -> String {
    format!("mi-{suffix:017x}")
}

/// Whether `instance_id` can be a gateway instance id: the `mi-` prefix the model states,
/// then a non-empty run of the characters an SSM managed-instance id is made of.
pub fn is_valid_gateway_instance_id(instance_id: &str) -> bool {
    match instance_id.strip_prefix("mi-") {
        Some(rest) => !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()),
        None => false,
    }
}

/// The resource fragment of a bridge ARN: `bridge:<bridge id>:<name>`.
///
/// Shaped like [`gateway_resource_fragment`] — the id is the ARN counter in zero-padded
/// hex, so the fragment is unique per bridge even when two bridges share a name. The
/// model puts no pattern on `BridgeArn` (`smithy_query(query="shape",shape="BridgeArn")`:
/// a plain string carrying an `@arnReference` to `AWS::MediaConnect::Bridge`), so the form
/// is this service's own, shaped like every other ARN it mints.
pub fn bridge_resource_fragment(suffix: u64, name: &str) -> String {
    format!("bridge:1-{suffix:08x}:{name}")
}

/// The resource fragment of a router input ARN: `routerInput:<id>`, with the id
/// [`router_resource_id`] mints for every router resource.
///
/// The modelled `RouterInputArn` pattern
/// (`smithy_query(query="shape",shape="RouterInputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerInput:[a-z0-9]{12}$`)
/// fixes the id at exactly twelve `[a-z0-9]` characters and carries NO name segment, so
/// unlike [`flow_resource_fragment`] the fragment is the id alone — the same shape as
/// [`router_output_resource_fragment`], which is why both are built through
/// [`router_resource_fragment`].
pub fn router_input_resource_fragment(id: &str) -> String {
    router_resource_fragment("routerInput", id)
}

/// Whether `arn` is a router input ARN this service could have minted — the modelled
/// `RouterInputArn` constraint, through the one encoding
/// [`is_mintable_router_resource_arn`] the router output side also uses. The integration
/// test asserts minted ARNs through this function.
pub fn is_mintable_router_input_arn(arn: &str) -> bool {
    is_mintable_router_resource_arn(arn, "routerInput")
}

/// The resource fragment of an output ARN: `output:<output id>:<name>`.
///
/// The id is the ARN counter in zero-padded hex, like [`flow_resource_fragment`]'s, so
/// the fragment satisfies the pattern the model puts on every `OutputArn` label
/// (`smithy_query(query="operation",operation="RemoveFlowOutput")`:
/// `^arn:.+:mediaconnect.+:output:.+$`) and is unique per output.
pub fn output_resource_fragment(suffix: u64, name: &str) -> String {
    format!("output:1-{suffix:08x}:{name}")
}

/// The name an `AddOutputRequest` takes on a flow whose output names are `taken`.
///
/// `Output.Name` "must be unique within the current flow"
/// (`smithy_query(query="shape",shape="Output",member="Name")`), and `Name` is optional
/// on the request, so:
///
/// * a request that names the output keeps that name, unless `taken` already holds it —
///   then the answer is `None` and the caller refuses the request with its own modelled
///   client error;
/// * a request that names none is given the first `output-<n>` (n counting from 1) that
///   `taken` does not hold, so a generated name cannot collide either.
///
/// `taken` is every output name the flow already holds PLUS the names resolved for
/// earlier outputs of the same request, so one request cannot add two outputs with one
/// name.
pub fn resolve_output_name(requested: Option<&str>, taken: &BTreeSet<String>) -> Option<String> {
    match requested.filter(|name| !name.is_empty()) {
        Some(name) if taken.contains(name) => None,
        Some(name) => Some(name.to_string()),
        None => (1..)
            .map(|position| format!("output-{position}"))
            .find(|candidate| !taken.contains(candidate)),
    }
}

/// The resource fragment of a source ARN: `source:<source id>:<name>`.
///
/// The id is the ARN counter in zero-padded hex, like [`output_resource_fragment`]'s, so
/// the fragment satisfies the pattern the model puts on every `SourceArn` label
/// (`smithy_query(query="operation",operation="RemoveFlowSource")`:
/// `^arn:.+:mediaconnect.+:source:.+$`) and is unique per source.
pub fn source_resource_fragment(suffix: u64, name: &str) -> String {
    format!("source:1-{suffix:08x}:{name}")
}

/// The name a `SetSourceRequest` takes on a flow named `flow_name` whose source names
/// are `taken`, for a source landing at `position` in the flow's source list.
///
/// `SetSourceRequest$Name` is "the name of the source" and is OPTIONAL, while
/// `Source$Name` is REQUIRED — so a source whose request named none is named by the
/// service. The model states no uniqueness rule on a source's name (a source is addressed
/// by its ARN, never by its name: RemoveFlowSource and UpdateFlowSource take `SourceArn`
/// as the `@httpLabel`), so a name the REQUEST chose is kept as given, exactly like a flow
/// name; but a name the SERVICE generates skips the ones the flow already holds, so no
/// call mints a duplicate of a name already on the flow.
///
/// The generated sequence is the flow's own name for the flow's first source and
/// `<flow name>-<n>` (n counting from 2) after it, the shape MediaConnect's own
/// console-created flows carry. `taken` is every source name the flow already holds PLUS
/// the names resolved for earlier sources of the same request, so one request cannot
/// generate one name twice.
///
/// Every operation that adds a source to a flow (CreateFlow, AddFlowSources) names it
/// HERE, so a flow's source names do not depend on which call wrote them.
pub fn resolve_source_name(
    requested: Option<&str>,
    flow_name: &str,
    position: usize,
    taken: &BTreeSet<String>,
) -> String {
    if let Some(name) = requested.filter(|name| !name.is_empty()) {
        return name.to_string();
    }
    let mut candidate_position = position;
    loop {
        let candidate = if candidate_position == 0 {
            flow_name.to_string()
        } else {
            format!("{flow_name}-{}", candidate_position + 1)
        };
        if !taken.contains(&candidate) {
            return candidate;
        }
        candidate_position += 1;
    }
}

/// The resource fragment of an entitlement ARN: `entitlement:<entitlement id>:<name>`.
///
/// The id is the ARN counter in zero-padded hex, like [`output_resource_fragment`]'s, so
/// the fragment is unique per entitlement. `Entitlement$EntitlementArn` targets `String`,
/// so the model constrains this fragment no further.
pub fn entitlement_resource_fragment(suffix: u64, name: &str) -> String {
    format!("entitlement:1-{suffix:08x}:{name}")
}

/// The name a `GrantEntitlementRequest` takes on a flow whose entitlement names are
/// `taken`, resolved exactly as [`resolve_output_name`] resolves an output's.
///
/// `GrantEntitlementRequest$Name` "must be unique within the current flow"
/// (`smithy_query(query="shape",shape="GrantEntitlementRequest",member="Name")`), and
/// `Name` is optional on the request, so:
///
/// * a request that names the entitlement keeps that name, unless `taken` already holds
///   it — then the answer is `None` and the caller refuses the request with its own
///   modelled client error;
/// * a request that names none is given the first `entitlement-<n>` (n counting from 1)
///   that `taken` does not hold, so a generated name cannot collide either.
///
/// `taken` is every entitlement name the flow already carries PLUS the names resolved for
/// earlier entitlements of the same request, so one request cannot grant two entitlements
/// with one name.
pub fn resolve_entitlement_name(
    requested: Option<&str>,
    taken: &BTreeSet<String>,
) -> Option<String> {
    match requested.filter(|name| !name.is_empty()) {
        Some(name) if taken.contains(name) => None,
        Some(name) => Some(name.to_string()),
        None => (1..)
            .map(|position| format!("entitlement-{position}"))
            .find(|candidate| !taken.contains(candidate)),
    }
}

/// Materialise one `GrantEntitlementRequest`, under the name `resolve_entitlement_name`
/// gave it, into the `Entitlement` a flow carries.
///
/// This is the crate's ONE encoding of how an entitlement is granted: CreateFlow grants
/// the entitlements named in the create request and GrantFlowEntitlements grants them on
/// an existing flow, and both mint through here, so the two cannot drift. The minted
/// record is what the flow stores and what every reader (DescribeFlow, and the grant's own
/// response) reports.
///
/// `GrantEntitlementRequest$EntitlementTags` — "the key-value pairs that can be used to
/// tag and organize the entitlement" — is kept beside the projection: the entitlement's
/// ARN is a `ResourceArn` the tag operations resolve, so those pairs are what a later
/// ListTagsForResource on that ARN reports.
pub fn mint_entitlement(
    request: &GrantEntitlementRequest,
    name: String,
    region: &str,
    suffix: u64,
) -> StoredEntitlement {
    StoredEntitlement {
        entitlement: Entitlement {
            entitlement_arn: ::engine::emulator_arn(
                SERVICE_NAMESPACE,
                region,
                &entitlement_resource_fragment(suffix, &name),
            ),
            name,
            description: request.description.clone(),
            encryption: request.encryption.clone(),
            entitlement_status: Some(
                request
                    .entitlement_status
                    .clone()
                    .unwrap_or_else(|| STATUS_ENABLED.to_string()),
            ),
            data_transfer_subscriber_fee_percent: request.data_transfer_subscriber_fee_percent,
            subscribers: request.subscribers.clone(),
        },
        tags: tag_map_of(request.entitlement_tags.as_ref()),
    }
}

/// An ingest address for a source the emulator assigns an endpoint to.
///
/// Addresses come from the RFC 5737 documentation range `198.51.100.0/24`, which is
/// reserved for documentation and never routes anywhere. The range is a WRAPPING pool
/// of 254 addresses: an address repeats once the ARN counter passes the pool size,
/// which is harmless because nothing in the emulator binds these addresses.
pub fn assigned_ingest_ip(suffix: u64) -> String {
    format!("198.51.100.{}", 1 + (suffix % 254))
}

/// The egress address this service assigns to a resource that SENDS content: a flow's
/// `Flow.EgressIp`, "the IP address from which video will be sent to output destinations",
/// and a standard router output's `RouterOutput$IpAddress`, "the IP address of the router
/// output".
///
/// From the RFC 5737 range `203.0.113.0/24`, reserved for documentation. Like
/// [`assigned_ingest_ip`], a wrapping pool of 254 addresses that never route anywhere.
pub fn assigned_egress_ip(suffix: u64) -> String {
    format!("203.0.113.{}", 1 + (suffix % 254))
}

/// The ingest port assigned to a source that did not request one, drawn from the
/// wrapping pool [`INGEST_PORT_BASE`]..[`INGEST_PORT_CEILING`].
pub fn assigned_ingest_port(suffix: u64) -> i32 {
    let span = u64::try_from(INGEST_PORT_CEILING - INGEST_PORT_BASE).unwrap_or(1);
    INGEST_PORT_BASE + i32::try_from(suffix % span).unwrap_or(0)
}

/// The region this request was signed for, from its SigV4 credential scope, or
/// [`DEFAULT_REGION`] when the request carries no scope.
///
/// A scope value that is not a region token is ignored rather than interpolated: the
/// region is a segment of every ARN this service mints, and the modelled `FlowArn`
/// pattern constrains that segment to `[a-z0-9-]+`.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` can be the Region segment of an ARN this service mints: the modelled
/// `FlowArn` / `RouterInputArn` / `RouterOutputArn` patterns constrain that segment to
/// `[a-z0-9-]+`.
///
/// The one encoding of that constraint, shared by [`request_region`] (which ignores a
/// credential scope that is not a region token), [`has_mintable_arn_prefix`] and the
/// creates that take a Region as a request member (`CreateRouterInput$RegionName`,
/// `CreateRouterOutput$RegionName`).
pub fn is_region_token(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `name` can be a flow name: it is the last segment of the ARN the service
/// mints for the flow, and the modelled `FlowArn` pattern constrains that segment to
/// `[a-zA-Z0-9_-]+`.
pub fn is_valid_flow_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `name` can be the last segment of an ARN this service mints.
///
/// The one encoding of what minting an ADDRESSABLE ARN requires, shared by
/// [`is_valid_gateway_name`] and [`is_valid_bridge_name`] so the two cannot drift apart:
/// the name is the last segment of the ARN the service mints for the resource, and that
/// ARN travels as an `@httpLabel` on the operations that address it — so it must not be
/// empty and must not carry the ARN segment separator `:` or the URI path separator `/`.
fn is_addressable_resource_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(':') && !name.contains('/')
}

/// Whether `name` can be a gateway name.
///
/// The model puts no pattern or length on a gateway's `Name`
/// (`smithy_query(query="shape",shape="Gateway")`: `Name` targets `String`), so this is
/// only what [`is_addressable_resource_name`] requires of the last segment of the
/// `GatewayArn` the service mints, which travels as an `@httpLabel` on DescribeGateway and
/// DeleteGateway.
pub fn is_valid_gateway_name(name: &str) -> bool {
    is_addressable_resource_name(name)
}

/// Whether `name` can be a bridge name.
///
/// Like [`is_valid_gateway_name`]: the model puts no pattern or length on `Bridge$Name`
/// (`smithy_query(query="shape",shape="Bridge")`: `Name` targets `String`), and the name
/// is the last segment of the `BridgeArn` the service mints, which travels as an
/// `@httpLabel` on DescribeBridge, UpdateBridge, DeleteBridge and every bridge
/// source/output operation.
pub fn is_valid_bridge_name(name: &str) -> bool {
    is_addressable_resource_name(name)
}

/// Whether the leading segments of a colon-split ARN are those this service mints for
/// `resource_type`: the `arn` prefix, an `aws*` partition, this service's namespace, a
/// region token, a 12-digit account id and the resource type itself.
///
/// The one encoding of that prefix, shared by [`is_mintable_flow_arn`] and
/// [`is_mintable_gateway_arn`] so the two cannot drift apart.
fn has_mintable_arn_prefix(segments: &[&str], resource_type: &str) -> bool {
    let [prefix, partition, service, region, account, kind, ..] = segments else {
        return false;
    };
    prefix == &"arn"
        && partition.starts_with("aws")
        && partition[3..]
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
        && service == &SERVICE_NAMESPACE
        && is_region_token(region)
        && account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && kind == &resource_type
}

/// Whether `arn` is a gateway ARN this service could have minted: the prefix
/// [`has_mintable_arn_prefix`] describes for the `gateway` resource type, then the gateway
/// id [`gateway_resource_fragment`] mints and a name [`is_valid_gateway_name`] accepts.
/// It satisfies the modelled `GatewayArn` pattern
/// (`smithy_query(query="shape",shape="GatewayArn")`:
/// `^arn:.+:mediaconnect.+:gateway:.+$`), which is looser than this.
///
/// ListGateways' `NextToken` is the ARN its next page resumes at, so a token the service
/// never minted is a semantic error in the request rather than a key to page from. The
/// gateway it names need NOT still exist: paging must survive a delete between pages.
pub fn is_mintable_gateway_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    let [.., gateway_id, gateway_name] = segments.as_slice() else {
        return false;
    };
    segments.len() == 8
        && has_mintable_arn_prefix(&segments, "gateway")
        && !gateway_id.is_empty()
        && gateway_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && is_valid_gateway_name(gateway_name)
}

/// Whether `arn` is a gateway instance ARN this service could have minted: the ARN of a
/// gateway it could have minted ([`is_mintable_gateway_arn`]), then the `instance:` segment
/// and an instance id [`is_valid_gateway_instance_id`] accepts.
///
/// It satisfies the modelled `GatewayInstanceArn` pattern
/// (`smithy_query(query="shape",shape="GatewayInstanceArn")`:
/// `^arn:.+:mediaconnect.+:gateway:.+:instance:.+$`), which is looser than this. The
/// instance it names need NOT exist: this is a judgement on the ARN's FORM, which is what
/// ListGatewayInstances' `FilterArn` and a pagination token are checked for.
pub fn is_mintable_gateway_instance_arn(arn: &str) -> bool {
    let Some((gateway_arn, instance_id)) = split_gateway_instance_arn(arn) else {
        return false;
    };
    is_mintable_gateway_arn(gateway_arn) && is_valid_gateway_instance_id(instance_id)
}

/// Whether `arn` is a bridge ARN this service could have minted: the prefix
/// [`has_mintable_arn_prefix`] describes for the `bridge` resource type, then the bridge
/// id [`bridge_resource_fragment`] mints and a name [`is_valid_bridge_name`] accepts.
///
/// The model puts no pattern on `BridgeArn`, so this is the crate's own encoding of the
/// form CreateBridge mints — the integration test asserts minted ARNs through it, and
/// ListBridges resumes a page from one (its `NextToken`).
pub fn is_mintable_bridge_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    let [.., bridge_id, bridge_name] = segments.as_slice() else {
        return false;
    };
    segments.len() == 8
        && has_mintable_arn_prefix(&segments, "bridge")
        && !bridge_id.is_empty()
        && bridge_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && is_valid_bridge_name(bridge_name)
}

/// The resource fragment of a reservation ARN: `reservation:<reservation id>:<name>`.
///
/// Shaped like [`gateway_resource_fragment`] — the id is the ARN counter in zero-padded
/// hex, so the fragment is unique per reservation even when two reservations share a name.
/// The model puts no pattern on `ReservationArn`
/// (`smithy_query(query="shape",shape="ReservationArn")`: a plain string carrying an
/// `@arnReference` to `AWS::MediaConnect::Reservation`), so the form is this service's own,
/// shaped like every other ARN it mints.
pub fn reservation_resource_fragment(suffix: u64, name: &str) -> String {
    format!("reservation:1-{suffix:08x}:{name}")
}

/// Whether `name` can be a reservation name.
///
/// Like [`is_valid_gateway_name`]: the model puts no pattern or length on
/// `Reservation$ReservationName` (`smithy_query(query="shape",shape="Reservation")`:
/// `ReservationName` targets `String`), and the name is the last segment of the
/// `ReservationArn` the service mints, which travels as an `@httpLabel` on
/// DescribeReservation.
pub fn is_valid_reservation_name(name: &str) -> bool {
    is_addressable_resource_name(name)
}

/// Whether `arn` is a reservation ARN this service could have minted: the prefix
/// [`has_mintable_arn_prefix`] describes for the `reservation` resource type, then the
/// reservation id [`reservation_resource_fragment`] mints and a name
/// [`is_valid_reservation_name`] accepts.
///
/// ListReservations' `NextToken` is the ARN its next page resumes at, so a token the
/// service never minted is a semantic error in the request rather than a key to page from.
/// The reservation it names need NOT still be active: paging must survive the clock
/// expiring a reservation between pages.
pub fn is_mintable_reservation_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    let [.., reservation_id, reservation_name] = segments.as_slice() else {
        return false;
    };
    segments.len() == 8
        && has_mintable_arn_prefix(&segments, "reservation")
        && !reservation_id.is_empty()
        && reservation_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && is_valid_reservation_name(reservation_name)
}

/// Whether `arn` is an entitlement ARN this service could have minted: the prefix
/// [`has_mintable_arn_prefix`] describes for the `entitlement` resource type, then the
/// entitlement id [`entitlement_resource_fragment`] mints and a non-empty name.
///
/// The model puts no pattern on `Entitlement$EntitlementArn`
/// (`smithy_query(query="shape",shape="Entitlement",member="EntitlementArn")`: it targets
/// `String`), and no operation constrains an entitlement's name either, so the name is
/// only required to carry something: a name that itself holds the ARN segment separator
/// still yields an ARN this function accepts, which is why the trailing segments are
/// walked rather than counted.
///
/// ListEntitlements' `NextToken` is the entitlement ARN its next page resumes at, so a
/// token the service never minted is a semantic error in the request rather than a key to
/// page from. The entitlement it names need NOT still exist: paging must survive a revoke
/// between pages.
pub fn is_mintable_entitlement_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    if segments.len() < 8 || !has_mintable_arn_prefix(&segments, "entitlement") {
        return false;
    }
    let entitlement_id = segments[6];
    !entitlement_id.is_empty()
        && entitlement_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && segments[7..].iter().any(|segment| !segment.is_empty())
}

/// Whether `arn` is a flow ARN this service could have minted.
///
/// This function is the crate's ONE encoding of the modelled `FlowArn` constraint
/// (`smithy_query(query="shape",shape="FlowArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$`),
/// checked segment by segment: the `arn` prefix, an `aws*` partition, this service's
/// namespace, a region token, a 12-digit account id, the `flow` resource type, the flow
/// id and the flow name. The integration test asserts minted ARNs through this same
/// function, so there is no second spelling that can drift from it.
///
/// ListFlows' `NextToken` is the ARN its next page resumes at, so a token the service
/// never minted is a semantic error in the request rather than a key to page from. The
/// flow it names need NOT still exist: paging must survive a delete between pages.
pub fn is_mintable_flow_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.split(':').collect();
    let [.., flow_id, flow_name] = segments.as_slice() else {
        return false;
    };
    segments.len() == 8
        && has_mintable_arn_prefix(&segments, "flow")
        && !flow_id.is_empty()
        && flow_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && is_valid_flow_name(flow_name)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediaconnectMediaconnectState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediaconnectMediaconnectState>)
}
