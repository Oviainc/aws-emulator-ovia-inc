// SPDX-License-Identifier: CC0-1.0

//! The flow records this service stores: a flow, its sources and its outputs, and the
//! projections the flow operations report them through.

use super::*;

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
        // The ingest endpoint the source listens on: the address and the port TOGETHER,
        // resolved through the one place this crate pairs a source's entitlement with its
        // endpoint, so an owned source gets the address and the port the service assigns it
        // (the port the request named where it named one) and a source reached through
        // another account's entitlement gets neither.
        let (ingest_ip, ingest_port) = assigned_source_ingest_endpoint(
            request.entitlement_arn.as_deref(),
            request.protocol.as_deref(),
            Some(assigned_ingest_ip(suffix)),
            Some(
                request
                    .ingest_port
                    .unwrap_or_else(|| assigned_ingest_port(suffix)),
            ),
        );
        StoredSource {
            source_arn: ::engine::emulator_arn(
                SERVICE_NAMESPACE,
                region,
                &source_resource_fragment(suffix, &name),
            ),
            name,
            description: request.description.clone(),
            ingest_ip,
            ingest_port,
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

/// The bridge source an output of a flow was materialised for: the bridge that takes the
/// flow's content, and the name that bridge addresses the source by.
///
/// An output carrying one of these is the service's OWN record of a bridge source, not an
/// output a request configured — see [`StoredOutput::for_bridge_source`], which mints it,
/// and [`ComAmazonawsMediaconnectMediaconnectState::sync_bridge_source_flow_outputs`],
/// which keeps the set of them equal to the bridge flow sources the store holds. The pair
/// is the identity of that source (a source name "must be unique among sources in this
/// bridge"), so the reconciliation can tell which output belongs to which source without
/// depending on the output's own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeSourceOrigin {
    pub bridge_arn: String,
    pub source_name: String,
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
    /// The bridge source this output exists FOR, when the SERVICE materialised it rather
    /// than a request configuring it ([`StoredOutput::for_bridge_source`]).
    /// `Output.BridgeArn` — "the ARN of the bridge added to this output" — is reported from
    /// it, and
    /// [`ComAmazonawsMediaconnectMediaconnectState::sync_bridge_source_flow_outputs`] keys
    /// the reconciliation on it. `None` for every output a request added.
    pub bridge_source: Option<BridgeSourceOrigin>,
}

impl StoredOutput {
    /// Materialise one `AddOutputRequest` into the output record a flow keeps.
    ///
    /// Every operation that adds an output to a flow (CreateFlow's `Outputs`,
    /// AddFlowOutputs) materialises it HERE, so one request shape always becomes the
    /// same stored record: the service mints the `OutputArn`, resolves `OutputStatus`
    /// to the documented `ENABLED` default, folds the request's transport members
    /// into `Output.Transport` — materialising the NDI sender settings the documentation
    /// names a service value for ([`apply_ndi_output_defaults`]) — and keeps
    /// `AddOutputRequest$OutputTags` — "the key-value
    /// pairs that can be used to tag and organize the output" — on the record, because the
    /// output's ARN is a `ResourceArn` the tag operations resolve.
    pub fn from_add_request(
        request: &AddOutputRequest,
        name: String,
        region: &str,
        suffix: u64,
    ) -> Self {
        // `Transport.Protocol` is required, so an output that names no protocol
        // carries no transport rather than an empty one.
        let transport = request.protocol.clone().map(|protocol| {
            let mut transport = Transport {
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
            };
            // The NDI sender settings the documentation names a service value for are
            // materialised on the record, so a read of an NDI output reports them.
            apply_ndi_output_defaults(&mut transport, &name);
            transport
        });
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
            transport,
            // A request configured this output, so it is not the one the service creates on a
            // flow a bridge takes as its source ([`Self::for_bridge_source`]).
            bridge_source: None,
        }
    }

    /// Materialise the output the SERVICE creates on a cloud flow because a bridge takes
    /// that flow as one of its sources.
    ///
    /// `BridgeFlowSource$OutputArn` is "the Amazon Resource Number (ARN) of the output" a
    /// bridge's flow source carries, and `Output$BridgeArn` is "the ARN of the bridge added
    /// to this output": the two halves of one record, the output through which the cloud
    /// flow feeds the bridge. No request shape configures it — `AddBridgeFlowSourceRequest`
    /// carries the flow, a name and a VPC interface attachment and nothing about an output —
    /// so it is materialised from the bridge sources the store holds instead
    /// ([`ComAmazonawsMediaconnectMediaconnectState::sync_bridge_source_flow_outputs`]), the
    /// mirror of the bridge-side [`materialise_bridge_flow_output`].
    ///
    /// The `OutputArn` is minted exactly as [`Self::from_add_request`] mints one, so an
    /// output of a flow has one ARN form whichever side asked for it; `OutputStatus` is "an
    /// indication of whether the output is transmitting data or not" and this output exists
    /// to carry the flow to the bridge, so it reports the same [`STATUS_ENABLED`] an added
    /// output's unspecified status resolves to. Every other member belongs to an output a
    /// request configures (a destination, a port, a transport, an encryption, media stream
    /// associations) and stays absent, including `Output$BridgePorts` — "the bridge output
    /// ports currently in use" — because this emulator moves no content and so holds no port
    /// in use.
    pub fn for_bridge_source(
        origin: BridgeSourceOrigin,
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
            output_status: Some(STATUS_ENABLED.to_string()),
            bridge_source: Some(origin),
            ..StoredOutput::default()
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
            // "The ARN of the bridge added to this output": carried by the output the
            // service created for a bridge source and by no other, so a reader can tell the
            // output that feeds a bridge from the ones a request configured.
            bridge_arn: self
                .bridge_source
                .as_ref()
                .map(|origin| origin.bridge_arn.clone()),
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
/// stream" — so neither the name nor the id may repeat within one flow. Its
/// `MediaStreamTags` is the tag member of a create request, so it is bounded exactly as
/// every other one is ([`tag_set_rejection`]). CreateFlow's
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
    // The media stream carries a tag member of its own
    // (`AddMediaStreamRequest$MediaStreamTags`), kept verbatim on the record this request
    // stores — so its pairs and their count are bounded by the two rules every tag member
    // of a create request is bounded by, read from the one place that states them.
    tag_set_rejection(
        &format!("media stream {:?}", request.media_stream_name),
        request.media_stream_tags.as_ref(),
    )
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

/// The `NetworkInterfaceType` a VPC interface takes when the request that adds it names
/// none (`smithy_query(query="shape",shape="NetworkInterfaceType")`: ena | efa); `ena` is
/// the standard elastic network adapter. The member is REQUIRED on the reported
/// `VpcInterface` and optional on the request, so something has to be stored for a request
/// that omits it.
pub const NETWORK_INTERFACE_TYPE_ENA: &str = "ena";

/// Why `request` is not a VPC interface a flow can hold beside the VPC interface names in
/// `names`, as the message the refusal carries, or `None` when it is.
///
/// A VPC interface is addressed by NAME within its flow (RemoveFlowVpcInterface takes
/// `VpcInterfaceName` as an `@httpLabel`), so an empty name names no VPC interface, and the
/// name is "immutable and has to be a unique against other VpcInterfaces in this Flow"
/// (`VpcInterface$Name`; `VpcInterfaceRequest$Name`: "this name must be unique within the
/// flow") — so a name the flow already holds, or one another entry of the same request
/// takes, names an interface that already exists. Its `VpcInterfaceTags` is the tag member
/// of a create request, so it is bounded exactly as every other one is
/// ([`tag_set_rejection`]). CreateFlow's `VpcInterfaces` and
/// AddFlowVpcInterfaces both add VPC interfaces to a flow, so both refuse through this one
/// function and with the same wording: no request can leave a flow in a state the other
/// operation would never permit.
pub fn vpc_interface_rejection(
    request: &VpcInterfaceRequest,
    names: &BTreeSet<String>,
) -> Option<String> {
    if request.name.is_empty() {
        return Some(
            "A VPC interface name must not be empty: it is how the VPC interface is \
             addressed within its flow."
                .to_string(),
        );
    }
    if names.contains(&request.name) {
        return Some(format!(
            "More than one VPC interface of this flow would be named {:?}. A VPC interface \
             name must be unique within its flow.",
            request.name
        ));
    }
    // The VPC interface carries a tag member of its own
    // (`VpcInterfaceRequest$VpcInterfaceTags`), bounded by the same two rules as every
    // other tag member of a create request.
    tag_set_rejection(
        &format!("VPC interface {:?}", request.name),
        request.vpc_interface_tags.as_ref(),
    )
}

/// Materialise one `VpcInterfaceRequest` into the VPC interface a flow keeps, with the
/// unique `suffix` the store minted for the elastic network interface the service creates
/// in the caller's subnet (`VpcInterface$NetworkInterfaceIds`: "IDs of the network
/// interfaces created in customer's account by MediaConnect").
///
/// Both CreateFlow (`VpcInterfaces`) and AddFlowVpcInterfaces add VPC interfaces to a flow,
/// and they store the same record: this function is the crate's ONE materialisation of a
/// `VpcInterfaceRequest`. `VpcInterfaceTags` is not kept, because it tags the network
/// interface in the caller's own account and no MediaConnect operation reports a VPC
/// interface's tags (see
/// [`ComAmazonawsMediaconnectMediaconnectState::tags_of`]).
pub fn materialise_vpc_interface(request: &VpcInterfaceRequest, suffix: u64) -> VpcInterface {
    VpcInterface {
        name: request.name.clone(),
        network_interface_ids: vec![format!("eni-{suffix:017x}")],
        network_interface_type: request
            .network_interface_type
            .clone()
            .unwrap_or_else(|| NETWORK_INTERFACE_TYPE_ENA.to_string()),
        role_arn: request.role_arn.clone(),
        security_group_ids: request.security_group_ids.clone(),
        subnet_id: request.subnet_id.clone(),
    }
}

/// The member path a nested request member is named by in a refusal detail: `member`
/// qualified by `prefix` when the request nests it (one `Outputs` entry of an add, say),
/// and `member` alone when `prefix` is empty because the member belongs to the request
/// itself (an update, which addresses its output or source by ARN).
pub(super) fn qualified_member(prefix: &str, member: &str) -> String {
    if prefix.is_empty() {
        member.to_string()
    } else {
        format!("{prefix}.{member}")
    }
}

/// Why `name` names no media stream of a flow whose media streams are named `held`, as the
/// detail the refusal carries, or `None` when one of them carries that name. `member` is
/// the request member that named it.
///
/// A media stream association references its media stream BY NAME within the one flow:
/// `MediaStreamOutputConfigurationRequest$MediaStreamName` is "the name of the media
/// stream that is associated with the output" and
/// `MediaStreamSourceConfigurationRequest$MediaStreamName` is its source counterpart. A
/// media stream exists on a flow only once CreateFlow's `MediaStreams` or
/// AddFlowMediaStreams ("adds media streams to an existing flow") created it, so a name
/// none of the flow's media streams carries references nothing — storing it would leave
/// the flow holding an association the flow's own media streams cannot satisfy, and which
/// [`StoredFlow::media_stream_association`] would then resolve for a media stream that
/// does not exist.
pub fn unknown_media_stream_rejection(
    member: &str,
    name: &str,
    held: &BTreeSet<String>,
) -> Option<String> {
    if held.contains(name) {
        return None;
    }
    Some(format!(
        "{member} {name:?} does not name a media stream this flow holds."
    ))
}

/// Why `name` names no VPC interface of a flow whose VPC interfaces are named `held`, as
/// the detail the refusal carries, or `None` when one of them carries that name. `member`
/// is the request member that named it.
///
/// A VPC interface reference is a name within the one flow too —
/// `VpcInterfaceAttachment$VpcInterfaceName` is "the name of the VPC interface to use for
/// this resource" and `SetSourceRequest$VpcInterfaceName` "the name of the VPC interface
/// to use for this source" — and a VPC interface exists on a flow only once CreateFlow's
/// `VpcInterfaces` or AddFlowVpcInterfaces created it, its name "unique against other
/// VpcInterfaces in this Flow" (`VpcInterface$Name`). So a name none of the flow's VPC
/// interfaces carries references nothing, for the same reason a media stream name outside
/// the flow does ([`unknown_media_stream_rejection`]).
pub fn unknown_vpc_interface_rejection(
    member: &str,
    name: &str,
    held: &BTreeSet<String>,
) -> Option<String> {
    if held.contains(name) {
        return None;
    }
    Some(format!(
        "{member} {name:?} does not name a VPC interface this flow holds."
    ))
}

/// Why the media stream associations `requests` would give an OUTPUT cannot be stored on a
/// flow whose media streams are named `media_streams` and whose VPC interfaces are named
/// `vpc_interfaces`, or `None` when every name they reference resolves. `prefix` is the
/// request member that nests the list, if any (see [`qualified_member`]).
///
/// An entry references the flow by name twice: the media stream it associates, and —
/// through each destination configuration's `Interface` — the VPC interface the media
/// stream leaves on (`InterfaceRequest$Name` is "the name of the VPC interface", on "the
/// VPC interface that you want to use for the media stream associated with the output").
/// The list travels on `AddOutputRequest$MediaStreamOutputConfigurations` (CreateFlow's
/// `Outputs` and AddFlowOutputs) and on `UpdateFlowOutputRequest`'s member of the same
/// name, so the one resolution is stated once here and every entry point reads it.
pub fn media_stream_output_configuration_rejection(
    prefix: &str,
    requests: &[MediaStreamOutputConfigurationRequest],
    media_streams: &BTreeSet<String>,
    vpc_interfaces: &BTreeSet<String>,
) -> Option<String> {
    requests.iter().enumerate().find_map(|(index, request)| {
        let entry = qualified_member(prefix, &format!("MediaStreamOutputConfigurations[{index}]"));
        unknown_media_stream_rejection(
            &format!("{entry}.MediaStreamName"),
            &request.media_stream_name,
            media_streams,
        )
        .or_else(|| {
            request
                .destination_configurations
                .iter()
                .flatten()
                .enumerate()
                .find_map(|(position, configuration)| {
                    unknown_vpc_interface_rejection(
                        &format!("{entry}.DestinationConfigurations[{position}].Interface.Name"),
                        &configuration.interface.name,
                        vpc_interfaces,
                    )
                })
        })
    })
}

/// Why the media stream associations `requests` would give a SOURCE cannot be stored on a
/// flow whose media streams are named `media_streams` and whose VPC interfaces are named
/// `vpc_interfaces`, or `None` when every name they reference resolves — the source-side
/// counterpart of [`media_stream_output_configuration_rejection`], whose input
/// configurations name the VPC interface an incoming media stream arrives on, reached from
/// `SetSourceRequest$MediaStreamSourceConfigurations` (CreateFlow's `Source` / `Sources`
/// and AddFlowSources) and from `UpdateFlowSourceRequest`'s member of the same name.
pub fn media_stream_source_configuration_rejection(
    prefix: &str,
    requests: &[MediaStreamSourceConfigurationRequest],
    media_streams: &BTreeSet<String>,
    vpc_interfaces: &BTreeSet<String>,
) -> Option<String> {
    requests.iter().enumerate().find_map(|(index, request)| {
        let entry = qualified_member(prefix, &format!("MediaStreamSourceConfigurations[{index}]"));
        unknown_media_stream_rejection(
            &format!("{entry}.MediaStreamName"),
            &request.media_stream_name,
            media_streams,
        )
        .or_else(|| {
            request
                .input_configurations
                .iter()
                .flatten()
                .enumerate()
                .find_map(|(position, configuration)| {
                    unknown_vpc_interface_rejection(
                        &format!("{entry}.InputConfigurations[{position}].Interface.Name"),
                        &configuration.interface.name,
                        vpc_interfaces,
                    )
                })
        })
    })
}

/// Why the VPC interface `attachment` names cannot be attached on a flow whose VPC
/// interfaces are named `held`, or `None` when it resolves — including when there is no
/// attachment, or an attachment that names no interface, because neither references
/// anything. `member` is the request member that carries the attachment.
pub fn vpc_interface_attachment_rejection(
    member: &str,
    attachment: Option<&VpcInterfaceAttachment>,
    held: &BTreeSet<String>,
) -> Option<String> {
    let name = attachment?.vpc_interface_name.as_deref()?;
    unknown_vpc_interface_rejection(&format!("{member}.VpcInterfaceName"), name, held)
}

/// Why the output `request` cannot be stored on a flow whose media streams are named
/// `media_streams` and whose VPC interfaces are named `vpc_interfaces`, or `None` when
/// every name the request references resolves. `prefix` is the request member that nests
/// the output (`Outputs[0]`, say).
///
/// An output references the flow's own sub-resources by name in its media stream
/// associations and in its VPC interface attachment, and CreateFlow's `Outputs` and
/// AddFlowOutputs both store exactly this shape through
/// [`StoredOutput::from_add_request`], so both resolve those references here and refuse
/// the same requests.
pub fn output_request_reference_rejection(
    prefix: &str,
    request: &AddOutputRequest,
    media_streams: &BTreeSet<String>,
    vpc_interfaces: &BTreeSet<String>,
) -> Option<String> {
    if let Some(rejection) = media_stream_output_configuration_rejection(
        prefix,
        request
            .media_stream_output_configurations
            .as_deref()
            .unwrap_or_default(),
        media_streams,
        vpc_interfaces,
    ) {
        return Some(rejection);
    }
    vpc_interface_attachment_rejection(
        &qualified_member(prefix, "VpcInterfaceAttachment"),
        request.vpc_interface_attachment.as_ref(),
        vpc_interfaces,
    )
}

/// How many of the flow sources `requests` would point at each bridge, keyed by the ARN of
/// the bridge, each with the request member that nests the FIRST entry naming it
/// (`Sources[0]`, say).
///
/// The batch is counted TOGETHER, because what the bridge's declared maximum bounds is the
/// outputs it ends up holding: a request naming one bridge twice asks that bridge for two
/// of its outputs, so one batch cannot leave a bridge where a sequence of single adds could
/// not (see
/// [`ComAmazonawsMediaconnectMediaconnectState::flow_source_bridge_output_rejection`],
/// which this is read by, from every operation that stores a LIST of this shape —
/// CreateFlow's `Source` / `Sources` and AddFlowSources).
pub fn requested_gateway_bridge_counts(
    requests: &[SetSourceRequest],
) -> BTreeMap<&str, (String, usize)> {
    let mut counts: BTreeMap<&str, (String, usize)> = BTreeMap::new();
    for (index, request) in requests.iter().enumerate() {
        let Some(bridge_source) = request.gateway_bridge_source.as_ref() else {
            continue;
        };
        let entry = counts
            .entry(bridge_source.bridge_arn.as_str())
            .or_insert_with(|| (format!("Sources[{index}]"), 0));
        entry.1 += 1;
    }
    counts
}

/// Why the source `request` cannot be stored on a flow whose media streams are named
/// `media_streams` and whose VPC interfaces are named `vpc_interfaces`, or `None` when
/// every name the request references resolves — the source-side counterpart of
/// [`output_request_reference_rejection`], reached from CreateFlow's `Source` / `Sources`
/// and AddFlowSources, which both store this shape through
/// [`StoredSource::from_set_request`].
///
/// A source references the flow's VPC interfaces through `VpcInterfaceName`, through each
/// input configuration's `Interface.Name` and, for a source fed by a gateway bridge,
/// through its `GatewayBridgeSource`'s attachment — the same references
/// [`StoredFlow::vpc_interface_reference`] resolves when RemoveFlowVpcInterface asks what
/// still holds an interface. The bridge that same
/// `GatewayBridgeSource` names is a resource of the ACCOUNT rather than of the flow, so it
/// is resolved against the store instead, through
/// [`ComAmazonawsMediaconnectMediaconnectState::source_bridge_reference_rejection`].
pub fn source_request_reference_rejection(
    prefix: &str,
    request: &SetSourceRequest,
    media_streams: &BTreeSet<String>,
    vpc_interfaces: &BTreeSet<String>,
) -> Option<String> {
    if let Some(rejection) = media_stream_source_configuration_rejection(
        prefix,
        request
            .media_stream_source_configurations
            .as_deref()
            .unwrap_or_default(),
        media_streams,
        vpc_interfaces,
    ) {
        return Some(rejection);
    }
    if let Some(rejection) = request.vpc_interface_name.as_deref().and_then(|name| {
        unknown_vpc_interface_rejection(
            &qualified_member(prefix, "VpcInterfaceName"),
            name,
            vpc_interfaces,
        )
    }) {
        return Some(rejection);
    }
    vpc_interface_attachment_rejection(
        &qualified_member(prefix, "GatewayBridgeSource.VpcInterfaceAttachment"),
        request
            .gateway_bridge_source
            .as_ref()
            .and_then(|bridge_source| bridge_source.vpc_interface_attachment.as_ref()),
        vpc_interfaces,
    )
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

    /// The names of the media streams the flow holds — the set every by-name media stream
    /// reference a request carries is resolved against (see
    /// [`unknown_media_stream_rejection`]), because a media stream is addressed by name
    /// within its flow.
    pub fn media_stream_names(&self) -> BTreeSet<String> {
        self.media_streams
            .iter()
            .map(|media_stream| media_stream.stream.media_stream_name.clone())
            .collect()
    }

    /// The names of the VPC interfaces the flow holds — the set every by-name VPC
    /// interface reference a request carries is resolved against (see
    /// [`unknown_vpc_interface_rejection`]), because a VPC interface is addressed by name
    /// within its flow.
    pub fn vpc_interface_names(&self) -> BTreeSet<String> {
        self.vpc_interfaces
            .iter()
            .map(|vpc_interface| vpc_interface.name.clone())
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
    /// A stored source or output references a VPC interface of its flow by name through
    /// EVERY member that carries such a name, which is exactly the set the write paths
    /// resolve against the flow's VPC interfaces before storing it (see
    /// [`source_request_reference_rejection`] and [`output_request_reference_rejection`]):
    /// on a source, `Source.VpcInterfaceName` ("the name of the VPC interface that is used
    /// for this source"), `GatewayBridgeSource.VpcInterfaceAttachment` ("the name of the
    /// VPC interface attachment to use for this bridge source") and the `Interface.Name`
    /// of each `MediaStreamSourceConfigurations[*].InputConfigurations[*]` ("the VPC
    /// interface that is used for the media stream associated with the source or output");
    /// on an output, `Output.VpcInterfaceAttachment` ("the name of the VPC interface
    /// attachment to use for this output") and the `Interface.Name` of each
    /// `MediaStreamOutputConfigurations[*].DestinationConfigurations[*]`. The lookup is
    /// stated once here because every operation whose documented precondition is such a
    /// reference — RemoveFlowVpcInterface ("can be made only on a VPC interface that does
    /// not have a Source or Output associated with it") — must read the SAME references,
    /// so a removal can never leave behind a stored source or output naming a VPC
    /// interface the flow no longer holds.
    pub fn vpc_interface_reference(&self, vpc_interface_name: &str) -> Option<String> {
        let source = self.sources.iter().find(|source| {
            source.vpc_interface_name.as_deref() == Some(vpc_interface_name)
                || source
                    .gateway_bridge_source
                    .as_ref()
                    .and_then(|bridge_source| bridge_source.vpc_interface_attachment.as_ref())
                    .and_then(|attachment| attachment.vpc_interface_name.as_deref())
                    == Some(vpc_interface_name)
                || source
                    .media_stream_source_configurations
                    .iter()
                    .flatten()
                    .flat_map(|configuration| {
                        configuration.input_configurations.iter().flatten()
                    })
                    .any(|configuration| configuration.interface.name == vpc_interface_name)
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
                || output
                    .media_stream_output_configurations
                    .iter()
                    .flatten()
                    .flat_map(|configuration| {
                        configuration.destination_configurations.iter().flatten()
                    })
                    .any(|configuration| configuration.interface.name == vpc_interface_name)
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
