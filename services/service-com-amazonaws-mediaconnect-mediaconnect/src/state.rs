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
    AddMediaStreamRequest, AddOutputRequest, EncodingConfig, Encryption, Entitlement,
    FailoverConfig, Flow, Fmtp, Gateway, GatewayNetwork, GrantEntitlementRequest, ListedFlow,
    ListedGateway, Maintenance, MediaStream, MediaStreamAttributes, MonitoringConfig, NdiConfig,
    Output, Source, Transport, VpcInterface, VpcInterfaceAttachment,
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
/// ERROR | DELETING | DELETED). Only the two a gateway's lifecycle reaches here are
/// named; the rest belong to operations that are still scaffolded.
pub const GATEWAY_STATE_CREATING: &str = "CREATING";
/// See [`GATEWAY_STATE_CREATING`].
pub const GATEWAY_STATE_ACTIVE: &str = "ACTIVE";

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
}

impl StoredSource {
    /// The modelled `Source` for this record.
    pub fn to_source(&self) -> Source {
        Source {
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
            output_arn: self.output_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            destination: self.destination.clone(),
            port: self.port,
            encryption: self.encryption.clone(),
            output_status: self.output_status.clone(),
            vpc_interface_attachment: self.vpc_interface_attachment.clone(),
            transport: self.transport.clone(),
            ..Output::default()
        }
    }
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
            clock_rate: request.clock_rate.or_else(|| {
                matches!(
                    request.media_stream_type.as_str(),
                    MEDIA_STREAM_TYPE_VIDEO | MEDIA_STREAM_TYPE_ANCILLARY_DATA
                )
                .then_some(VIDEO_CLOCK_RATE_HZ)
            }),
            description: request.description.clone(),
            video_format: request.video_format.clone(),
            attributes: request
                .attributes
                .as_ref()
                .map(|attributes| MediaStreamAttributes {
                    lang: attributes.lang.clone(),
                    fmtp: attributes
                        .fmtp
                        .as_ref()
                        .map(|fmtp| Fmtp {
                            channel_order: fmtp.channel_order.clone(),
                            colorimetry: fmtp.colorimetry.clone(),
                            exact_framerate: fmtp.exact_framerate.clone(),
                            par: fmtp.par.clone(),
                            range: fmtp.range.clone(),
                            scan_mode: fmtp.scan_mode.clone(),
                            tcs: fmtp.tcs.clone(),
                        })
                        .unwrap_or_default(),
                }),
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
            media_streams: (!self.media_streams.is_empty()).then(|| {
                self.media_streams
                    .iter()
                    .map(StoredMediaStream::to_media_stream)
                    .collect()
            }),
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

/// Per-service state for com-amazonaws-mediaconnect-mediaconnect. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediaconnectMediaconnectState {
    /// Every live flow, keyed by its ARN. Ordered, so ListFlows pages stably.
    ///
    /// There is no index by name beside it: the model puts no uniqueness rule on a flow
    /// name, and the flow id in the ARN is what keeps two same-named flows apart — the
    /// same reasoning as for [`Self::gateways`].
    pub flows: BTreeMap<String, StoredFlow>,
    /// Every live gateway, keyed by its ARN. Ordered, so ListGateways pages stably.
    ///
    /// There is no index by name beside it: like a flow name, a gateway name is under no
    /// uniqueness rule in the model, and the gateway id in the ARN is what keeps two
    /// same-named gateways apart.
    pub gateways: BTreeMap<String, StoredGateway>,
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

    /// Settle every flow and gateway whose status change the clock already made due.
    pub fn settle_due_transitions(&mut self, now: SystemTime) {
        for flow in self.flows.values_mut() {
            flow.settle(now);
        }
        for gateway in self.gateways.values_mut() {
            gateway.settle(now);
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

/// Whether `name` can be a gateway name.
///
/// The model puts no pattern or length on a gateway's `Name`
/// (`smithy_query(query="shape",shape="Gateway")`: `Name` targets `String`), so this is
/// only what minting an ADDRESSABLE ARN requires: the name is the last segment of the
/// `GatewayArn` the service mints, and that ARN travels as an `@httpLabel` on
/// DescribeGateway and DeleteGateway — so it must not be empty and must not carry the
/// ARN segment separator `:` or the URI path separator `/`.
pub fn is_valid_gateway_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(':') && !name.contains('/')
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
