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
    Bridge, BridgeFlowSource, BridgeNetworkOutput, BridgeNetworkSource, BridgeOutput, BridgeSource,
    DestinationConfiguration, EgressGatewayBridge, EncodingConfig, EncodingParameters, Encryption,
    Entitlement, FailoverConfig, Flow, FlowTransitEncryption, Gateway, GatewayNetwork,
    GrantEntitlementRequest, IngressGatewayBridge, Interface, ListedBridge, ListedFlow,
    ListedGateway,
    Maintenance, MediaStream, MediaStreamAttributes, MediaStreamAttributesRequest,
    MediaStreamOutputConfiguration, MediaStreamOutputConfigurationRequest, MonitoringConfig,
    NdiConfig, Output, SetSourceRequest, Source, Transport, UpdateFailoverConfig, VpcInterface,
    VpcInterfaceAttachment,
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

/// The `BridgeState` enum the model declares for a bridge
/// (`smithy_query(query="shape",shape="BridgeState")`: CREATING | STANDBY | STARTING |
/// DEPLOYING | ACTIVE | STOPPING | DELETING | DELETED | START_FAILED | START_PENDING |
/// STOP_FAILED | UPDATING). The two a bridge's creation reaches are named first; the
/// states a STARTED bridge moves through follow, because DeleteBridge's documented
/// precondition is stated against them (see [`is_stopped_bridge_state`]). Reaching any of
/// them is UpdateBridgeState's business, which is still scaffolded.
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

/// The ceiling on how many sources ONE FLOW holds, however many calls fill it.
///
/// AWS publishes no count for a flow's sources, so the number is
/// [`MAX_FLOW_COLLECTION_ENTRIES`] — the bound CreateFlow applies to one request's
/// `Sources` — named once so the create side and the add side cannot drift apart. Like
/// [`MAX_FLOW_MEDIA_STREAMS`] it is a different RULE from that per-request bound: this
/// one bounds the STORED flow, so a sequence of adds cannot grow one past what a single
/// create could store. AddFlowSources models no 420, so it refuses past this bound with
/// `BadRequestException`, the error the model describes for "a semantic error" in the
/// request.
pub const MAX_FLOW_SOURCES: usize = MAX_FLOW_COLLECTION_ENTRIES;

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
    /// `SetSourceRequest$SourceTags` is not stored: `Source` models no tag member, so no
    /// implemented operation could report it — exactly as the output and media-stream tags
    /// of a create request are not stored either.
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
            // `Transport.Protocol` is required, so a source that names no protocol (an
            // entitled one, say) carries no transport rather than an empty one.
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
            ..Source::default()
        }
    }
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
}

impl StoredOutput {
    /// Materialise one `AddOutputRequest` into the output record a flow keeps.
    ///
    /// Every operation that adds an output to a flow (CreateFlow's `Outputs`,
    /// AddFlowOutputs) materialises it HERE, so one request shape always becomes the
    /// same stored record: the service mints the `OutputArn`, resolves `OutputStatus`
    /// to the documented `ENABLED` default, and folds the request's transport members
    /// into `Output.Transport`.
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

/// A media stream of a stored flow: the modelled `MediaStream` the flow reports, plus
/// the `MediaStreamTags` the request that added it carried.
///
/// `MediaStream` models no tag member and the tagging operations are still scaffolded,
/// so — like [`StoredFlow::tags`] — the tags are written here and not yet read, rather
/// than dropped on the floor when a request carries them.
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
    pub entitlements: Vec<Entitlement>,
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
            .map(|entitlement| entitlement.name.clone())
            .collect()
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
            entitlements: self.entitlements.clone(),
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
    /// Every live bridge, keyed by its ARN. Ordered, so ListBridges pages stably.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]:
    /// the model puts no uniqueness rule on `Bridge$Name`, and the bridge id in the ARN
    /// is what keeps two same-named bridges apart.
    pub bridges: BTreeMap<String, StoredBridge>,
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
    /// A flow is the only resource whose TAGS this emulator persists: a gateway record
    /// carries no tag map (no gateway operation in the model takes or reports tags) and
    /// every other MediaConnect create (bridge, router input/output, reservation) is still
    /// scaffolded — so a tag call resolves its `ResourceArn` against the flow store, and
    /// any other ARN addresses nothing whose tags exist in the system.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        self.flows.get(resource_arn).map(|flow| &flow.tags)
    }

    /// The tags of the resource `resource_arn` addresses, for a tag write. See
    /// [`ComAmazonawsMediaconnectMediaconnectState::tags_of`] for which ARNs resolve.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        self.flows.get_mut(resource_arn).map(|flow| &mut flow.tags)
    }

    /// Settle every flow, gateway and bridge whose status change the clock already made
    /// due.
    pub fn settle_due_transitions(&mut self, now: SystemTime) {
        for flow in self.flows.values_mut() {
            flow.settle(now);
        }
        for gateway in self.gateways.values_mut() {
            gateway.settle(now);
        }
        for bridge in self.bridges.values_mut() {
            bridge.settle(now);
        }
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
/// `GrantEntitlementRequest$EntitlementTags` is not stored: `Entitlement` models no tag
/// member, so no implemented operation can report it, exactly as the output and
/// media-stream tags of a create request are not stored either.
pub fn mint_entitlement(
    request: &GrantEntitlementRequest,
    name: String,
    region: &str,
    suffix: u64,
) -> Entitlement {
    Entitlement {
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

/// The egress address of a flow — `Flow.EgressIp`, "the IP address from which video
/// will be sent to output destinations".
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
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
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
        && !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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
