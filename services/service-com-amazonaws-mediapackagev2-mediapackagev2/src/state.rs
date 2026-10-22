// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediapackagev2-mediapackagev2 service.
//!
//! MediaPackage v2 is a three-level containment hierarchy — channel group -> channel
//! -> origin endpoint — so the store nests the same way: a channel cannot exist
//! without its group, and deleting a parent that still has children is refused. Every
//! derived value a response carries (ARN, egress domain, ingest URL, manifest URL,
//! ETag, timestamps) is computed ONCE, when the resource is created, and persisted, so
//! a later read reproduces it byte for byte.
//!
//! The maps are [`BTreeMap`]s: `ListOriginEndpoints` pages in name order, and an
//! ordered map makes that order a property of the store rather than of each read.
//!
//! The same holds for the members the SERVICE applies rather than the caller: a channel's
//! switching and CMSD-header blocks, an origin endpoint's segment settings, startover
//! window and URI/stream-name modes, and every manifest configuration's window, URI path
//! type and child-manifest URL encoding are materialised ONCE, at create time, from the
//! submitted value or the service's default, and stored — so the create response, a later
//! read and the list projection all report the same concrete value.

use std::collections::{BTreeMap, HashMap};

use aws_smithy_schema_server::{Violation, ViolationKind};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    CdnAuthConfiguration, ConflictException, CreateDashManifestConfiguration,
    CreateHlsManifestConfiguration, CreateLowLatencyHlsManifestConfiguration,
    CreateMssManifestConfiguration, ForceEndpointErrorConfiguration,
    GetDashManifestConfiguration, GetHlsManifestConfiguration,
    GetLowLatencyHlsManifestConfiguration, GetMssManifestConfiguration, IngestEndpoint,
    InputSwitchConfiguration, MultiviewConfiguration, OutputHeaderConfiguration,
    ResourceNotFoundException, Segment, ValidationException,
};

/// The signing name (and ARN service namespace) of this service, from the model's
/// `awsService.sigv4Name`.
pub const SIGNING_NAME: &str = "mediapackagev2";

/// The region used when a request carries no SigV4 credential scope to read one from.
///
/// The emulator's own shared config writes `region = us-east-1`, so an unsigned request
/// lands in the same region a signed default-profile request does.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `InputType` a channel is created with when the request omits it: the model's
/// `InputType` doc says "If unprovided, the value defaults to HLS".
pub const DEFAULT_INPUT_TYPE: &str = "HLS";

/// How many ingest endpoints a channel is given. MediaPackage v2 hands every channel a
/// redundant pair, which is why `IngestEndpoints` is a list rather than a single URL.
const INGEST_ENDPOINTS_PER_CHANNEL: u8 = 2;

/// The most channel groups this emulator holds at once.
///
/// `channel_groups` grows by one record per `CreateChannelGroup` request and shrinks only
/// on an explicit delete, so without a ceiling its entry COUNT is set by how many
/// requests a caller makes. AWS bounds the same thing with a per-account, per-Region
/// service quota and refuses a create past it; that quota's value is adjustable and is
/// not in the model, so this number is the EMULATOR's own bound rather than a claim about
/// AWS's, while the refusal it produces is the error the model declares for exactly this
/// case: `ServiceQuotaExceededException` (HTTP 402, "The request would cause a service
/// quota to be exceeded.").
pub const MAX_CHANNEL_GROUPS: usize = 100;

/// The most channels one channel group holds at once — the same bound, one level down:
/// `channels` grows per `CreateChannel` and shrinks only on an explicit delete.
pub const MAX_CHANNELS_PER_GROUP: usize = 100;

/// The most origin endpoints one channel holds at once — the same bound, one level
/// further down: `origin_endpoints` grows per `CreateOriginEndpoint` and shrinks only on
/// an explicit delete.
pub const MAX_ORIGIN_ENDPOINTS_PER_CHANNEL: usize = 100;

/// The startover window MediaPackage applies to an origin endpoint created without one.
///
/// The model declares no `@default` and documents none, so this is RECORDED behaviour:
/// the service reports a fifteen-minute startover window on an endpoint whose create
/// request named none, and reports the window the request named when it named one. The
/// value is inside the member's modelled `@range 0..1209600`.
pub const DEFAULT_STARTOVER_WINDOW_SECONDS: i32 = 900;

/// The segment duration MediaPackage applies when a create request sends no
/// `SegmentDurationSeconds` — six seconds, within the member's modelled `@range 1..30`
/// and the value the recorded service returns for an endpoint created without a segment
/// configuration.
pub const DEFAULT_SEGMENT_DURATION_SECONDS: i32 = 6;

/// The segment base name MediaPackage applies when a create request sends no
/// `SegmentName` — the value the recorded service returns, and one the member's
/// `^[a-zA-Z0-9_-]+$` pattern and `@length 1..256` admit.
pub const DEFAULT_SEGMENT_NAME: &str = "segment";

/// The manifest window MediaPackage applies when a manifest configuration sends no
/// `ManifestWindowSeconds` — sixty seconds, the value the recorded service returns, and
/// at or above the smallest window the create shapes accept (`@range` min 30).
pub const DEFAULT_MANIFEST_WINDOW_SECONDS: i32 = 60;

/// The URI path type MediaPackage applies when a manifest configuration sends no
/// `UriPathType`. The model states the default on every create manifest shape that
/// carries the member: "If you don't specify a value, the default is `LEAF`."
pub const DEFAULT_URI_PATH_TYPE: &str = "LEAF";

/// The URI separator MediaPackage applies to an origin endpoint created without one —
/// the value the recorded service reports for such an endpoint, from the modelled
/// `UriSeparator` enum (`UNDERSCORE` | `HYPHEN`).
pub const DEFAULT_URI_SEPARATOR: &str = "UNDERSCORE";

/// The stream-name output mode MediaPackage applies to an origin endpoint created
/// without one — the value the recorded service reports, from the modelled
/// `StreamNameOutputMode` enum (`INDEX` | `PASSTHROUGH_NAME`).
pub const DEFAULT_STREAM_NAME_OUTPUT_MODE: &str = "INDEX";

/// Whether MediaPackage bundles the audio tracks of an endpoint created without a
/// segment configuration into a rendition group — recorded behaviour, like
/// [`DEFAULT_TS_INCLUDE_DVB_SUBTITLES`] and [`DEFAULT_INCLUDE_IFRAME_ONLY_STREAMS`]: the
/// model declares no `@default` for any of the three.
pub const DEFAULT_TS_USE_AUDIO_RENDITION_GROUP: bool = true;

/// Whether MediaPackage passes DVB subtitles through for an endpoint created without a
/// segment configuration — recorded behaviour.
pub const DEFAULT_TS_INCLUDE_DVB_SUBTITLES: bool = true;

/// Whether the stream set of an endpoint created without a segment configuration carries
/// the extra I-frame-only stream — recorded behaviour: the service reports it off.
pub const DEFAULT_INCLUDE_IFRAME_ONLY_STREAMS: bool = false;

/// Whether MediaPackage URL-encodes the child-manifest query string of a manifest
/// configuration that does not ask for it — recorded behaviour: the service reports the
/// encoding off.
pub const DEFAULT_URL_ENCODE_CHILD_MANIFEST: bool = false;

/// The most manifest configurations one origin endpoint stores, counted across the HLS,
/// low-latency HLS, DASH and MSS families together.
///
/// Each submitted manifest is persisted with its computed URL, so a single create's
/// record size is caller-chosen unless the count is bounded; the model puts no `@length`
/// on the manifest lists, so this is the emulator's own ceiling, refused with the same
/// `ServiceQuotaExceededException` the count ceilings use.
pub const MAX_MANIFESTS_PER_ORIGIN_ENDPOINT: usize = 100;

/// The fewest manifest configurations an origin endpoint may carry, counted across the
/// four families together.
///
/// Unlike the ceilings above this is NOT the emulator's own bound: the model's
/// `ValidationExceptionType` declares `NUM_MANIFESTS_LOW` beside `NUM_MANIFESTS_HIGH`,
/// i.e. the service enforces a minimum manifest count, and an endpoint with no manifest
/// can serve nothing at all. A request below it is refused with `ValidationException`.
pub const MIN_MANIFESTS_PER_ORIGIN_ENDPOINT: usize = 1;

/// Per-service state for com-amazonaws-mediapackagev2-mediapackagev2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackagev2Mediapackagev2State {
    /// Every channel group, keyed by its name — the group's primary identifier.
    pub channel_groups: BTreeMap<String, ChannelGroup>,
}

/// A persisted channel group: the top-level resource that owns the DNS every channel
/// and origin endpoint beneath it shares.
#[derive(Debug, Clone)]
pub struct ChannelGroup {
    pub name: String,
    pub arn: String,
    pub egress_domain: String,
    pub description: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this group carried, if any — what an
    /// SDK's automatic retry of that create sends again.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this group, so a repeat
    /// carrying the same token can be told apart from a changed one.
    pub fingerprint: String,
    pub channels: BTreeMap<String, Channel>,
}

/// A persisted channel: the input side of the pipeline, with the ingest endpoints a
/// contributing encoder pushes to.
#[derive(Debug, Clone)]
pub struct Channel {
    pub name: String,
    pub group_name: String,
    pub arn: String,
    pub description: Option<String>,
    pub input_type: Option<String>,
    pub ingest_endpoints: Vec<IngestEndpoint>,
    /// Materialised at create time rather than only when the caller sends one: the
    /// service reports a channel's switching state on every projection of it.
    pub input_switch_configuration: InputSwitchConfiguration,
    /// Materialised at create time, for the same reason as
    /// [`Channel::input_switch_configuration`].
    pub output_header_configuration: OutputHeaderConfiguration,
    pub multiview_configuration: Option<MultiviewConfiguration>,
    pub output_locking_mode: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    pub reset_at: Option<::aws_smithy_types::DateTime>,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this channel carried, if any.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this channel.
    pub fingerprint: String,
    /// The resource policy attached to this channel by `PutChannelPolicy`, or `None`
    /// while none has ever been attached.
    ///
    /// The policy is a SUB-RESOURCE of the channel, not one of the channel's own
    /// members: no `Channel*` shape in the model carries it, so it takes no part in the
    /// channel's ETag, and because it is stored inside the channel record it disappears
    /// with the channel. `None` is what makes `GetChannelPolicy` on a never-attached
    /// policy a 404, and `DeleteChannelPolicy` what puts it back.
    pub policy: Option<String>,
    pub origin_endpoints: BTreeMap<String, OriginEndpoint>,
}

/// A persisted origin endpoint: the output side, holding the manifests whose URLs
/// players fetch.
#[derive(Debug, Clone)]
pub struct OriginEndpoint {
    pub name: String,
    pub channel_name: String,
    pub group_name: String,
    pub arn: String,
    pub container_type: String,
    pub segment: Segment,
    pub description: Option<String>,
    /// Materialised at create time: every endpoint has a startover window, whether or
    /// not the create request named one.
    pub startover_window_seconds: i32,
    pub hls_manifests: Vec<GetHlsManifestConfiguration>,
    pub low_latency_hls_manifests: Vec<GetLowLatencyHlsManifestConfiguration>,
    pub dash_manifests: Vec<GetDashManifestConfiguration>,
    pub mss_manifests: Vec<GetMssManifestConfiguration>,
    pub force_endpoint_error_configuration: Option<ForceEndpointErrorConfiguration>,
    /// Materialised at create time: the service reports the separator it applied on
    /// every projection of the endpoint, the list projection included.
    pub uri_separator: String,
    /// Materialised at create time, for the same reason as
    /// [`OriginEndpoint::uri_separator`].
    pub stream_name_output_mode: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    pub reset_at: Option<::aws_smithy_types::DateTime>,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this endpoint carried, if any.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this endpoint.
    pub fingerprint: String,
    /// The resource policy attached to this origin endpoint, or `None` while none is.
    ///
    /// The policy is a sub-resource of the endpoint rather than a resource of its own —
    /// `Put*` replaces it wholesale, `Get*` reports it and `Delete*` detaches it — so it
    /// is persisted ON the endpoint: deleting the endpoint takes its policy with it, and
    /// no second map can disagree about which policy an endpoint carries.
    pub policy: Option<EndpointPolicy>,
}

/// A policy attached to an origin endpoint: the policy document, plus the optional CDN
/// authorization settings submitted with it (the two travel together on every
/// `PutOriginEndpointPolicy`/`GetOriginEndpointPolicy` pair).
#[derive(Debug, Clone)]
pub struct EndpointPolicy {
    pub policy: String,
    pub cdn_auth_configuration: Option<CdnAuthConfiguration>,
}

/// The input-switch configuration a channel is stored with.
///
/// MediaPackage materialises the block on the server side, so a channel created without
/// one still has a switching state to report: the submitted block if there is one, with
/// `MQCSInputSwitching` filled in from the model's documented default ("Default is
/// false") when the caller left it out. `PreferredInput` has no documented default and
/// the service reports none, so an omitted one stays omitted.
pub fn materialized_input_switch_configuration(
    submitted: Option<InputSwitchConfiguration>,
) -> InputSwitchConfiguration {
    let mut configuration = submitted.unwrap_or_default();
    configuration.mqcs_input_switching = Some(configuration.mqcs_input_switching.unwrap_or(false));
    configuration
}

/// The output-header configuration a channel is stored with — the same materialisation
/// as [`materialized_input_switch_configuration`], one member wide: CMSD header
/// publication is off unless the caller turned it on.
pub fn materialized_output_header_configuration(
    submitted: Option<OutputHeaderConfiguration>,
) -> OutputHeaderConfiguration {
    let mut configuration = submitted.unwrap_or_default();
    configuration.publish_mqcs = Some(configuration.publish_mqcs.unwrap_or(false));
    configuration
}

/// The segment configuration an origin endpoint is stored with.
///
/// `Segment` is required on every origin-endpoint response and optional on the request,
/// so the service applies its own segmentation settings to whatever the caller omitted
/// and reports the result: the duration and the base name from
/// [`DEFAULT_SEGMENT_DURATION_SECONDS`] and [`DEFAULT_SEGMENT_NAME`], the TS passthrough
/// toggles on and the extra I-frame-only stream off — the state the recorded service
/// reports for an endpoint created without a segment configuration. `Scte`, `Encryption`
/// and `OutputTimestampMode` are reported only when they were configured, so they pass
/// through untouched.
pub fn materialized_segment(submitted: Option<Segment>) -> Segment {
    let mut segment = submitted.unwrap_or_default();
    segment.segment_duration_seconds = Some(
        segment
            .segment_duration_seconds
            .unwrap_or(DEFAULT_SEGMENT_DURATION_SECONDS),
    );
    segment.segment_name = Some(
        segment
            .segment_name
            .unwrap_or_else(|| DEFAULT_SEGMENT_NAME.to_string()),
    );
    segment.ts_use_audio_rendition_group = Some(
        segment
            .ts_use_audio_rendition_group
            .unwrap_or(DEFAULT_TS_USE_AUDIO_RENDITION_GROUP),
    );
    segment.include_iframe_only_streams = Some(
        segment
            .include_iframe_only_streams
            .unwrap_or(DEFAULT_INCLUDE_IFRAME_ONLY_STREAMS),
    );
    segment.ts_include_dvb_subtitles = Some(
        segment
            .ts_include_dvb_subtitles
            .unwrap_or(DEFAULT_TS_INCLUDE_DVB_SUBTITLES),
    );
    segment
}

/// The startover window an origin endpoint is stored with: the one the request named, or
/// the service's own ([`DEFAULT_STARTOVER_WINDOW_SECONDS`]). Shared by the create and the
/// update so one write cannot leave the endpoint reporting a window the other would not.
pub fn materialized_startover_window_seconds(submitted: Option<i32>) -> i32 {
    submitted.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS)
}

/// The URI separator an origin endpoint is stored with — the submitted one, else
/// [`DEFAULT_URI_SEPARATOR`].
pub fn materialized_uri_separator(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_URI_SEPARATOR.to_string())
}

/// The stream-name output mode an origin endpoint is stored with — the submitted one,
/// else [`DEFAULT_STREAM_NAME_OUTPUT_MODE`].
pub fn materialized_stream_name_output_mode(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_STREAM_NAME_OUTPUT_MODE.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediapackagev2Mediapackagev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediapackagev2Mediapackagev2State>)
}

/// The region this request is addressed to, read from its SigV4 credential scope.
///
/// Every signed AWS SDK request carries the region in `Credential=.../<region>/...`;
/// an unsigned one falls back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `now`, as the modelled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Millisecond resolution: restJson1 renders a timestamp as epoch seconds with a
    // fractional part, and keeping it coarser than the f64 it is serialized through
    // means the value a client reads back is exactly the value that was stored.
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_millis() as f64 / 1000.0)
}

/// The ARN of a channel group.
pub fn channel_group_arn(region: &str, group: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("channelGroup/{group}"))
}

/// The ARN of a channel, which nests under its group's resource path.
pub fn channel_arn(region: &str, group: &str, channel: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("channelGroup/{group}/channel/{channel}"),
    )
}

/// The ARN of an origin endpoint, which nests under its channel's resource path.
pub fn origin_endpoint_arn(region: &str, group: &str, channel: &str, endpoint: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}"),
    )
}

/// The egress domain a channel group is given: the host every manifest URL beneath it
/// is served from, so all of the group's channels and endpoints share one DNS name.
///
/// The two opaque labels are derived from the account and from the group's identity, so
/// the domain is a function of the resource (stable across reads, distinct per group)
/// rather than of the moment it was created.
pub fn egress_domain(region: &str, group: &str) -> String {
    let account = ::engine::emulator_account_id();
    let group_label = short_digest(&[account, region, group], 8);
    let account_label = short_digest(&[account], 12);
    format!("{group_label}.egress.{account_label}.{SIGNING_NAME}.{region}.amazonaws.com")
}

/// The ingest host for a group, derived from the SAME domain egress is served from —
/// the guarantee the service makes is that both sides of a group share its DNS.
fn ingest_domain(egress_domain: &str) -> String {
    egress_domain.replacen(".egress.", ".ingest.", 1)
}

/// The redundant pair of ingest endpoints a channel is created with, derived from the
/// parent group's egress domain and the channel's name.
pub fn ingest_endpoints(egress_domain: &str, group: &str, channel: &str) -> Vec<IngestEndpoint> {
    let host = ingest_domain(egress_domain);
    (1..=INGEST_ENDPOINTS_PER_CHANNEL)
        .map(|index| IngestEndpoint {
            id: Some(index.to_string()),
            url: Some(format!("https://{host}/in/v1/{group}/{index}/{channel}/index")),
        })
        .collect()
}

/// The container-format suffix a manifest's URL ends in.
///
/// One entry per manifest family MediaPackage serves; the family decides the suffix,
/// not the endpoint's container type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    /// HLS and low-latency HLS both publish an `.m3u8` playlist.
    Hls,
    /// DASH publishes an `.mpd` media presentation description.
    Dash,
    /// Microsoft Smooth Streaming publishes an `.ism` presentation manifest.
    Mss,
}

impl ManifestKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Hls => ".m3u8",
            Self::Dash => ".mpd",
            Self::Mss => ".ism/Manifest",
        }
    }
}

/// The egress URL of one manifest: the group's egress domain plus the path that names
/// the group, the channel, the endpoint and the manifest.
pub fn manifest_url(
    egress_domain: &str,
    group: &str,
    channel: &str,
    endpoint: &str,
    manifest_name: &str,
    kind: ManifestKind,
) -> String {
    format!(
        "https://{egress_domain}/out/v1/{group}/{channel}/{endpoint}/{manifest_name}{}",
        kind.suffix()
    )
}

/// Where a manifest's URL is rooted: the parent group's egress domain plus the names of
/// the group, the channel and the origin endpoint the manifest belongs to.
///
/// Passed to the `Create*` -> `Get*` conversions below so every manifest family builds
/// its URL from the same four values.
#[derive(Debug, Clone, Copy)]
pub struct ManifestLocation<'a> {
    pub egress_domain: &'a str,
    pub group: &'a str,
    pub channel: &'a str,
    pub endpoint: &'a str,
}

impl ManifestLocation<'_> {
    fn url(&self, manifest_name: &str, kind: ManifestKind) -> String {
        manifest_url(
            self.egress_domain,
            self.group,
            self.channel,
            self.endpoint,
            manifest_name,
            kind,
        )
    }
}

/// The stored form of a submitted HLS manifest: everything the caller asked for, plus
/// the URL the service computes for it and the manifest settings the service applies to
/// what the caller left out ([`DEFAULT_MANIFEST_WINDOW_SECONDS`],
/// [`DEFAULT_URI_PATH_TYPE`], and child-manifest URL encoding off).
pub fn stored_hls_manifest(
    submitted: CreateHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetHlsManifestConfiguration {
    GetHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: submitted.child_manifest_name,
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        program_date_time_interval_seconds: submitted.program_date_time_interval_seconds,
        scte_hls: submitted.scte_hls,
        filter_configuration: submitted.filter_configuration,
        start_tag: submitted.start_tag,
        url_encode_child_manifest: Some(
            submitted
                .url_encode_child_manifest
                .unwrap_or(DEFAULT_URL_ENCODE_CHILD_MANIFEST),
        ),
        uri_path_type: Some(
            submitted
                .uri_path_type
                .unwrap_or_else(|| DEFAULT_URI_PATH_TYPE.to_string()),
        ),
    }
}

/// The stored form of a submitted low-latency HLS manifest. Low-latency HLS publishes
/// the same `.m3u8` playlist as plain HLS, so it shares the URL rule and the same
/// manifest-setting defaults.
pub fn stored_low_latency_hls_manifest(
    submitted: CreateLowLatencyHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetLowLatencyHlsManifestConfiguration {
    GetLowLatencyHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: submitted.child_manifest_name,
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        program_date_time_interval_seconds: submitted.program_date_time_interval_seconds,
        scte_hls: submitted.scte_hls,
        filter_configuration: submitted.filter_configuration,
        start_tag: submitted.start_tag,
        url_encode_child_manifest: Some(
            submitted
                .url_encode_child_manifest
                .unwrap_or(DEFAULT_URL_ENCODE_CHILD_MANIFEST),
        ),
        uri_path_type: Some(
            submitted
                .uri_path_type
                .unwrap_or_else(|| DEFAULT_URI_PATH_TYPE.to_string()),
        ),
    }
}

/// The stored form of a submitted DASH manifest. The window and the URI path type are
/// the same server-applied settings the HLS families carry; everything else is reported
/// only when it was configured.
pub fn stored_dash_manifest(
    submitted: CreateDashManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetDashManifestConfiguration {
    GetDashManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Dash),
        manifest_name: submitted.manifest_name,
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        filter_configuration: submitted.filter_configuration,
        min_update_period_seconds: submitted.min_update_period_seconds,
        min_buffer_time_seconds: submitted.min_buffer_time_seconds,
        suggested_presentation_delay_seconds: submitted.suggested_presentation_delay_seconds,
        segment_template_format: submitted.segment_template_format,
        period_triggers: submitted.period_triggers,
        scte_dash: submitted.scte_dash,
        drm_signaling: submitted.drm_signaling,
        utc_timing: submitted.utc_timing,
        profiles: submitted.profiles,
        base_urls: submitted.base_urls,
        program_information: submitted.program_information,
        dvb_settings: submitted.dvb_settings,
        compactness: submitted.compactness,
        audio_timeline_pattern: submitted.audio_timeline_pattern,
        subtitle_configuration: submitted.subtitle_configuration,
        uri_path_type: Some(
            submitted
                .uri_path_type
                .unwrap_or_else(|| DEFAULT_URI_PATH_TYPE.to_string()),
        ),
        availability_start_time_configuration: submitted.availability_start_time_configuration,
    }
}

/// The stored form of a submitted Microsoft Smooth Streaming manifest. MSS models no
/// URI path type, so the window is the only server-applied setting it carries.
pub fn stored_mss_manifest(
    submitted: CreateMssManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetMssManifestConfiguration {
    GetMssManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Mss),
        manifest_name: submitted.manifest_name,
        filter_configuration: submitted.filter_configuration,
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        manifest_layout: submitted.manifest_layout,
    }
}

/// An entity tag over a resource's own content.
///
/// Derived from the fields the resource is made of, so it is stable for as long as the
/// resource is unchanged and differs once any of them does — which is what a caller
/// doing a conditional update needs from it. `fields` must include the resource's ARN,
/// so two different resources with equal contents still differ.
pub fn etag(fields: &[String]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        // Length-prefixed, so ["a", "bc"] and ["ab", "c"] cannot hash alike.
        hasher.update(field.len().to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// A tag map rendered in a canonical order, for feeding to [`etag`]. A `HashMap`
/// iterates in an arbitrary order, so hashing it directly would make the ETag depend on
/// the iteration rather than on the tags.
pub fn tag_fingerprint(tags: &HashMap<String, String>) -> String {
    let mut pairs: Vec<(&String, &String)> = tags.iter().collect();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

// =============================================================================
// Idempotency: a create's `ClientToken`
// =============================================================================

/// What a create request's `ClientToken` matched among the resources already stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenMatch {
    /// Nothing stored carries this token (or the request carried none), so the create
    /// goes ahead as a first attempt.
    Fresh,
    /// THIS request already created the named resource: answer with what was stored
    /// then, and create nothing.
    Replay(String),
    /// A stored resource carries this token but was created by a DIFFERENT request.
    Mismatch,
}

/// Match a create request's idempotency token against the resources already stored at
/// the same level of the hierarchy.
///
/// `stored` yields one `(name, the token that create carried, the fingerprint of that
/// create's request)` per sibling. Every create in this model binds `ClientToken` with
/// `@idempotencyToken` and is `@idempotent`, and the model's `ConflictExceptionType`
/// carries both `RESOURCE_ALREADY_EXISTS` and `IDEMPOTENT_PARAMETER_MISMATCH`: a repeat
/// of the SAME request under the same token replays the first result, while a repeat
/// that changed any member is a conflict. The name is part of the fingerprint, so
/// reusing a token for a differently-named resource is a mismatch too.
pub fn match_client_token<'a>(
    token: Option<&str>,
    fingerprint: &str,
    stored: impl Iterator<Item = (&'a str, Option<&'a str>, &'a str)>,
) -> TokenMatch {
    let Some(token) = token else {
        return TokenMatch::Fresh;
    };
    for (name, stored_token, stored_fingerprint) in stored {
        if stored_token == Some(token) {
            return if stored_fingerprint == fingerprint {
                TokenMatch::Replay(name.to_string())
            } else {
                TokenMatch::Mismatch
            };
        }
    }
    TokenMatch::Fresh
}

/// The fingerprint of a create request: a digest of every member it carried EXCEPT its
/// `ClientToken`, which is the identity of the request rather than part of it.
///
/// Two requests share a fingerprint exactly when they ask for the same resource with the
/// same configuration, which is what distinguishes an SDK's retry of one create from a
/// second, different create sent under the same token.
pub fn request_fingerprint(fields: &[String]) -> String {
    // The same length-prefixed digest an ETag is built with, for the same reason: no two
    // different field sequences may collide.
    etag(fields)
}

/// The message a repeat create under a token that was used for a different request
/// carries. The requirement is the 409 and the `IDEMPOTENT_PARAMETER_MISMATCH`
/// discriminator; this sentence says what the caller has to change.
pub fn idempotent_parameter_mismatch_message(client_token: &str) -> String {
    format!(
        "The ClientToken {client_token} was already used for a request with different \
         parameters. Repeat the original request, or use a new ClientToken"
    )
}

// =============================================================================
// Refusals: the MODELED error bodies this service answers with
//
// Every constructor here builds one of the model's own client errors
// (`ResourceNotFoundException`, `ConflictException`, `ValidationException`), so a
// refusal carries the members the model declares rather than a bare sentence.
//
// The rule they all follow: mediapackagev2 states a refusal in words that depend only
// on WHAT was refused — the kind of resource, the constraint that was violated — and
// never on the identifiers the caller sent. The discriminator member
// (`ResourceTypeNotFound`, `ConflictExceptionType`, `ValidationExceptionType`) and the
// sentence are two readings of that same fact, so they are chosen together, here, and
// no handler words a refusal of its own.
// =============================================================================

/// One kind of resource in mediapackagev2's containment hierarchy: channel group ->
/// channel -> origin endpoint.
///
/// The variants are the `ResourceTypeNotFound` enum values the model declares for the
/// three resources this emulator persists (the model also declares `HARVEST_JOB`, whose
/// operations are still scaffolded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// The top-level resource, which owns channels.
    ChannelGroup,
    /// A channel inside a channel group, which owns origin endpoints.
    Channel,
    /// An origin endpoint on a channel.
    OriginEndpoint,
}

impl ResourceKind {
    /// This kind's `ResourceTypeNotFound` value (model shape `ResourceTypeNotFound`,
    /// whose values are `CHANNEL_GROUP`, `CHANNEL`, `ORIGIN_ENDPOINT`, `HARVEST_JOB`).
    pub fn resource_type_not_found(self) -> &'static str {
        match self {
            Self::ChannelGroup => "CHANNEL_GROUP",
            Self::Channel => "CHANNEL",
            Self::OriginEndpoint => "ORIGIN_ENDPOINT",
        }
    }

    /// How a mediapackagev2 message names this kind in prose.
    pub fn noun(self) -> &'static str {
        match self {
            Self::ChannelGroup => "channel group",
            Self::Channel => "channel",
            Self::OriginEndpoint => "origin endpoint",
        }
    }

    /// The plural of [`noun`](Self::noun), for a sentence about this kind's children.
    pub fn plural(self) -> String {
        format!("{}s", self.noun())
    }

    /// The indefinite article [`noun`](Self::noun) takes — `an` before a vowel.
    fn article(self) -> &'static str {
        match self.noun().starts_with(['a', 'e', 'i', 'o', 'u']) {
            true => "an",
            false => "a",
        }
    }

    /// The 404 for a resource of this kind the service cannot find.
    ///
    /// The sentence names the KIND and nothing else: a caller's resource names are not
    /// echoed back, so two callers asking for two different absent channels read the
    /// same message.
    pub fn not_found(self) -> ResourceNotFoundException {
        let noun = self.noun();
        let article = self.article();
        ResourceNotFoundException {
            message: Some(format!(
                "MediaPackage can't process your request because we can't find your {noun}. \
                 Verify your {noun} name or add {article} {noun} and then try again"
            )),
            resource_type_not_found: Some(self.resource_type_not_found().to_string()),
        }
    }
}

/// The 409 a delete answers while the resource it names still holds children.
///
/// `RESOURCE_IN_USE`: the deletion would orphan the children, so the service names the
/// child kind that has to go first — not how many of them there are, which is a fact
/// about the caller's state rather than about the refusal.
pub fn children_attached(parent: ResourceKind, child: ResourceKind) -> ConflictException {
    let parent_noun = parent.noun();
    let children = child.plural();
    ConflictException {
        message: Some(format!(
            "The {parent_noun} you tried to delete has {children} attached to it. \
             If you want to delete this {parent_noun}, you must first delete the \
             {children} attached to it"
        )),
        conflict_exception_type: Some("RESOURCE_IN_USE".to_string()),
    }
}

/// The 409 a create answers when the name it was given is already taken.
///
/// `RESOURCE_ALREADY_EXISTS`, and resource-agnostic: mediapackagev2 states the collision
/// without naming either the resource kind or the name that collided, so a client's
/// idempotency logic matches one sentence for every create path.
pub fn resource_already_exists() -> ConflictException {
    ConflictException {
        message: Some(
            "MediaPackage can't create the resource you requested because a resource with \
             that name already exists. Verify the name of the resource and then try again"
                .to_string(),
        ),
        conflict_exception_type: Some("RESOURCE_ALREADY_EXISTS".to_string()),
    }
}

/// The 404 for a policy the service cannot find because none is attached to the resource
/// it would belong to — a channel or origin endpoint that exists with no policy on it.
///
/// The resource itself is present, and `ResourceTypeNotFound` enumerates only resource
/// types (`CHANNEL_GROUP`, `CHANNEL`, `ORIGIN_ENDPOINT`, `HARVEST_JOB`), so the
/// discriminator is left unset rather than naming a resource that is NOT missing. AWS
/// publishes no wording for this case: what the service requires is the 404 and the
/// error code, and the sentence follows the same rule as every other refusal here — one
/// fixed sentence, naming neither the resource the caller addressed nor its kind, since
/// what is missing is the policy either way.
pub fn policy_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(
            "MediaPackage can't process your request because we can't find your policy. \
             Attach a policy to your resource and then try again"
                .to_string(),
        ),
        resource_type_not_found: None,
    }
}

/// The modeled 400 for one constraint the request violated.
///
/// Both members are set, and both are derived from the violation rather than from the
/// request's values: `ValidationExceptionType` is the discriminator for the violated
/// constraint CLASS, and `Message` names the member path that failed and restates the
/// constraint it failed ([`constraint_detail`], which also words the framework's own
/// refusal fallback, so one violation cannot be worded two ways). A caller can therefore
/// branch on the type and repair the named member from the sentence alone.
pub fn member_validation_failed(violation: &Violation) -> ValidationException {
    validation_failed(
        &violation.path,
        &constraint_detail(&violation.kind),
        validation_exception_type(&violation.kind),
    )
}

/// The modeled 400 for a member this service refuses, in the envelope every
/// mediapackagev2 validation message uses ([`validation_message`]): the member path the
/// request named it at, then the requirement it failed.
///
/// A rule the service enforces itself (a policy document it cannot parse, a pagination
/// token it did not issue) is therefore stated exactly like a constraint the model
/// declares, so a caller reads one form for both.
pub fn validation_failed(
    member_path: &str,
    requirement: &str,
    exception_type: &str,
) -> ValidationException {
    ValidationException {
        message: Some(validation_message(Some(member_path), requirement)),
        validation_exception_type: Some(exception_type.to_string()),
    }
}

/// The `ValidationExceptionType` value for a violated constraint class.
///
/// The model's `ValidationExceptionType` enum carries one `MEMBER_*` value per Smithy
/// constraint class — `MEMBER_MISSING`, `MEMBER_MIN_LENGTH`, `MEMBER_MAX_LENGTH`,
/// `MEMBER_MIN_VALUE`, `MEMBER_MAX_VALUE`, `MEMBER_DOES_NOT_MATCH_PATTERN`,
/// `MEMBER_INVALID_ENUM_VALUE` — plus `MEMBER_INVALID` for a member that is wrong in a
/// way none of the others names. This is that mapping and nothing more.
fn validation_exception_type(kind: &ViolationKind) -> &'static str {
    match kind {
        ViolationKind::MissingRequiredMember => "MEMBER_MISSING",
        ViolationKind::MinLength(_) => "MEMBER_MIN_LENGTH",
        ViolationKind::MaxLength(_) => "MEMBER_MAX_LENGTH",
        ViolationKind::MinValue(_) => "MEMBER_MIN_VALUE",
        ViolationKind::MaxValue(_) => "MEMBER_MAX_VALUE",
        ViolationKind::Pattern(_) => "MEMBER_DOES_NOT_MATCH_PATTERN",
        ViolationKind::EnumValue(_) => "MEMBER_INVALID_ENUM_VALUE",
        // `@uniqueItems`, and any constraint class added to the framework later: the
        // member is invalid, which is all the enum can say about it.
        _ => "MEMBER_INVALID",
    }
}

// =============================================================================
// Validation: how this service words a refused request
// =============================================================================

/// The sentence mediapackagev2 appends when the caller has to look at a value it sent.
const CHECK_THE_VALUE: &str =
    "Check the value you specified in your request and then try again";

/// How mediapackagev2 states an input failure: the fixed prefix, the member path the
/// failure is at (in lowerCamel, as the wire names it), then what is wrong with it.
///
/// Every `ValidationException` this service raises goes through here, so the whole
/// surface reports a refusal in one form — `Validation error encountered at
/// request.<member>: <rule>` — rather than one form at the request boundary and another
/// from the handlers. `member_path` is `None` for a rule that is about the request as a
/// whole rather than about one member.
pub fn validation_message(member_path: Option<&str>, detail: &str) -> String {
    match member_path {
        Some(path) => format!("Validation error encountered at request.{path}: {detail}"),
        None => format!("Validation error encountered at request: {detail}"),
    }
}

/// The requirement a declared-constraint violation states, worded as this service words
/// it: the violated bound, then the sentence telling the caller to check what it sent.
///
/// Reached from both refusal paths — the modelled `ValidationException` this service
/// serializes for a constraint violation ([`member_validation_failed`])
/// and the framework's own refusal wording ([`refusal_message`]) — so one violation
/// cannot be worded two ways.
pub fn constraint_detail(kind: &ViolationKind) -> String {
    let requirement = match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // A `@pattern` that is one character class is stated as the characters it
        // ADMITS, which is what the caller has to act on; a pattern with no listable
        // character set is stated as the pattern itself.
        ViolationKind::Pattern(pattern) => match pattern_character_set(pattern) {
            Some(supported) => format!(
                "Member must contain only supported characters. Supported characters are \
                 {supported}"
            ),
            None => format!("Member must satisfy regular expression pattern: {pattern}"),
        },
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class added later still
        // gets this service's form rather than falling back to the framework's.
        _ => "Member must satisfy the constraints specified by the AWS service".to_string(),
    };
    format!("{requirement}. {CHECK_THE_VALUE}")
}

/// The characters a `@pattern` of the form `^[<class>]+$` admits, listed the way a
/// MediaPackage validation message lists them — `^[a-zA-Z0-9_-]+$` becomes
/// `A-Z, a-z, 0-9, _ (underscore), and - (hyphen)`.
///
/// `None` when the pattern is not a single, non-negated character class (the model's
/// `^[\S]+$`, say): there is no set of characters to list, so the caller states the
/// pattern itself instead.
///
/// Deliberately a RENDERER for message wording and nothing else: it reads the model's
/// own `@pattern` string, never caller data, and must never be used to decide whether a
/// value is acceptable — that is the framework's constraint walk, which compiles the
/// pattern with the `regex` crate. Anything it cannot describe it declines.
fn pattern_character_set(pattern: &str) -> Option<String> {
    let body = pattern.strip_prefix('^')?.strip_suffix('$')?;
    let class = body
        .strip_suffix('+')
        .or_else(|| body.strip_suffix('*'))
        .unwrap_or(body);
    let class = class.strip_prefix('[')?.strip_suffix(']')?;
    // A negated class names what is FORBIDDEN, and an escape (`\S`, `\w`) names a set
    // with no listable members, so neither can be rendered as "supported characters".
    if class.starts_with('^') || class.contains('\\') || class.is_empty() {
        return None;
    }
    let chars: Vec<char> = class.chars().collect();
    let mut ranges: Vec<String> = Vec::new();
    let mut literals: Vec<char> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        // `X-Y` is a range; a `-` with nothing after it is the literal hyphen.
        if index + 2 < chars.len() && chars[index + 1] == '-' {
            // A range whose ends are not one run of letters or digits (`[_-x]`, which
            // regex reads as the code points between them) has no listing a message
            // could state, so the whole pattern is left to its own rendering rather
            // than described as something narrower than it is.
            if !is_listable_range(chars[index], chars[index + 2]) {
                return None;
            }
            ranges.push(format!("{}-{}", chars[index], chars[index + 2]));
            index += 3;
        } else {
            literals.push(chars[index]);
            index += 1;
        }
    }
    // The message lists the ranges in a canonical order — upper case, then lower case,
    // then digits — rather than in the order the pattern happens to write them.
    ranges.sort_by_key(|range| match range.chars().next() {
        Some(first) if first.is_ascii_uppercase() => 0,
        Some(first) if first.is_ascii_lowercase() => 1,
        Some(first) if first.is_ascii_digit() => 2,
        _ => 3,
    });
    let items: Vec<String> = ranges
        .into_iter()
        .chain(literals.into_iter().map(named_character))
        .collect();
    Some(join_with_and(&items))
}

/// Whether `start`-`end` is a range a message can state as itself: one run of upper-case
/// letters, lower-case letters or digits, in ascending order. Anything else is a range
/// over code points that happens to be writable in a class, and listing it would claim
/// more or less than the pattern admits.
fn is_listable_range(start: char, end: char) -> bool {
    let same_run = (start.is_ascii_uppercase() && end.is_ascii_uppercase())
        || (start.is_ascii_lowercase() && end.is_ascii_lowercase())
        || (start.is_ascii_digit() && end.is_ascii_digit());
    same_run && start <= end
}

/// One literal character as a message names it: the character, plus the word for it
/// where it has one, because `_` and `-` are not legible on their own.
fn named_character(character: char) -> String {
    match character {
        '_' => "_ (underscore)".to_string(),
        '-' => "- (hyphen)".to_string(),
        '.' => ". (period)".to_string(),
        ' ' => "a space".to_string(),
        other => other.to_string(),
    }
}

/// Join `items` as prose: `a`, `a and b`, `a, b, and c`.
fn join_with_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [leading @ .., last] => format!("{}, and {last}", leading.join(", ")),
    }
}

/// This service's wording for a refusal the request boundary made from the model's own
/// constraints — a `@range`, `@length`, `@pattern` or enum violation, or an absent
/// required member — before any handler ran.
///
/// mediapackagev2 names ONE member path per message, so the first violation is the one
/// stated; the framework's default lists them all in its own `N validation errors
/// detected: ...` form, which no mediapackagev2 response carries.
pub fn refusal_message(violations: &[Violation]) -> String {
    match violations.first() {
        Some(violation) => validation_message(
            Some(&violation.path),
            &constraint_detail(&violation.kind),
        ),
        // The boundary refused the request without naming a violation; there is no
        // member path to state, so the message is about the request as a whole.
        None => validation_message(
            None,
            "The input failed to meet the constraints specified by the AWS service",
        ),
    }
}

/// Why a pagination token this service did not issue is refused — the detail
/// [`validation_message`] states after the member path. The same sentence whichever list
/// is being paged, and about the token rather than about the caller's data.
pub const INVALID_PAGINATION_TOKEN_DETAIL: &str = "You've provided an invalid pagination \
     token in your request. Check your token and then try again";

// =============================================================================
// Validation: which manifests an origin endpoint's container type can serve
// =============================================================================

/// One of the four manifest families an origin endpoint's request members carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFamily {
    Hls,
    LowLatencyHls,
    Dash,
    Mss,
}

impl ManifestFamily {
    /// Every family, in the order the request declares their members.
    pub const ALL: [Self; 4] = [Self::Hls, Self::LowLatencyHls, Self::Dash, Self::Mss];

    /// The request member that carries this family, as a validation message names it.
    pub fn member(self) -> &'static str {
        match self {
            Self::Hls => "hlsManifests",
            Self::LowLatencyHls => "lowLatencyHlsManifests",
            Self::Dash => "dashManifests",
            Self::Mss => "mssManifests",
        }
    }

    /// How a validation message names the family.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hls => "HLS",
            Self::LowLatencyHls => "low-latency HLS",
            Self::Dash => "DASH",
            Self::Mss => "MSS",
        }
    }
}

/// The `ValidationExceptionType` that names the refusal of `family` on a `container`
/// endpoint, or `None` when that container serves that family.
///
/// The combinations are exactly the ones the model's `ValidationExceptionType` enum
/// names — `TS_CONTAINER_TYPE_WITH_DASH_MANIFEST`, `TS_CONTAINER_TYPE_WITH_MSS_MANIFEST`,
/// `CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST`, `ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST`,
/// `ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST`, `ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST` —
/// so the table is read off the model rather than guessed: a TS container serves the HLS
/// families, CMAF adds DASH, and ISM serves MSS alone. A container value the model does
/// not name constrains nothing here; the boundary validator has already refused it
/// against the `ContainerType` enum.
pub fn container_manifest_conflict(
    container: &str,
    family: ManifestFamily,
) -> Option<&'static str> {
    match (container, family) {
        ("TS", ManifestFamily::Dash) => Some("TS_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        ("TS", ManifestFamily::Mss) => Some("TS_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        ("CMAF", ManifestFamily::Mss) => Some("CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        ("ISM", ManifestFamily::Hls) => Some("ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST"),
        ("ISM", ManifestFamily::LowLatencyHls) => {
            Some("ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST")
        }
        ("ISM", ManifestFamily::Dash) => Some("ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        _ => None,
    }
}

/// A refused manifest set, as the `(message, ValidationExceptionType)` pair the
/// operation puts on its `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSetViolation {
    pub message: String,
    pub validation_exception_type: &'static str,
}

/// Check the manifest set an origin endpoint is being created or updated with against
/// its container type: at least one manifest, and no family the container cannot serve.
///
/// `counts` is how many manifests the request carries per family, in
/// [`ManifestFamily::ALL`] order. `None` means the set is acceptable. This is the same
/// rule for `CreateOriginEndpoint` and `UpdateOriginEndpoint`: both carry the whole
/// manifest set and the same required `ContainerType`.
/// Which operation is checking a manifest set — the two differ only in what the
/// "too few manifests" sentence tells the caller to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestSetOn {
    Create,
    Update,
}

pub fn manifest_set_violation(
    container: &str,
    counts: [usize; ManifestFamily::ALL.len()],
    on: ManifestSetOn,
) -> Option<ManifestSetViolation> {
    if counts.iter().sum::<usize>() < MIN_MANIFESTS_PER_ORIGIN_ENDPOINT {
        let retry = match on {
            ManifestSetOn::Create => "create",
            ManifestSetOn::Update => "update",
        };
        return Some(ManifestSetViolation {
            message: validation_message(
                None,
                &format!(
                    "You've requested an origin endpoint without a manifest. Your origin \
                     endpoint needs to contain at least one manifest. Add a manifest and \
                     try to {retry} your origin endpoint again"
                ),
            ),
            validation_exception_type: "NUM_MANIFESTS_LOW",
        });
    }
    for (family, count) in ManifestFamily::ALL.into_iter().zip(counts) {
        if count == 0 {
            continue;
        }
        if let Some(exception_type) = container_manifest_conflict(container, family) {
            return Some(ManifestSetViolation {
                message: validation_message(
                    Some(family.member()),
                    &format!(
                        "A {container} container type can't be combined with a {} \
                         manifest. Remove the manifest, or create an origin endpoint \
                         whose container type serves it",
                        family.label()
                    ),
                ),
                validation_exception_type: exception_type,
            });
        }
    }
    None
}

/// The quota refusal for a manifest set past [`MAX_MANIFESTS_PER_ORIGIN_ENDPOINT`], or
/// `None` when it fits.
///
/// Every operation that WRITES a manifest set — `CreateOriginEndpoint` and
/// `UpdateOriginEndpoint` — checks it here before anything is stored, so the record size
/// of an endpoint is bounded by this ceiling whichever operation last wrote it rather
/// than by how many manifests a request carries. The model puts no `@length` on the
/// manifest lists, so the request boundary does not clamp the count and this is the only
/// bound; the refusal is the `ServiceQuotaExceededException` the counting ceilings use.
pub fn manifest_ceiling_exceeded(
    counts: [usize; ManifestFamily::ALL.len()],
) -> Option<String> {
    let total: usize = counts.iter().sum();
    (total > MAX_MANIFESTS_PER_ORIGIN_ENDPOINT).then(|| {
        format!(
            "An origin endpoint accepts at most {MAX_MANIFESTS_PER_ORIGIN_ENDPOINT} \
             manifests; the request has {total}"
        )
    })
}

/// The ETag of an origin endpoint: a digest over everything the endpoint is made of, so
/// an update that changes any of it rotates the tag and one that changes none keeps it.
///
/// `CreateOriginEndpoint` and `UpdateOriginEndpoint` both build it here, so the tag a
/// create reports and the tag an update rotates are the same function of the same
/// resource.
pub fn origin_endpoint_etag(endpoint: &OriginEndpoint) -> String {
    let manifest_names = |names: Vec<&str>| names.join("\u{1f}");
    etag(&[
        endpoint.arn.clone(),
        endpoint.container_type.clone(),
        endpoint.description.clone().unwrap_or_default(),
        endpoint.startover_window_seconds.to_string(),
        format!("{:?}", endpoint.segment),
        manifest_names(
            endpoint
                .hls_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        manifest_names(
            endpoint
                .low_latency_hls_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        manifest_names(
            endpoint
                .dash_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        manifest_names(
            endpoint
                .mss_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        format!("{:?}", endpoint.force_endpoint_error_configuration),
        endpoint.uri_separator.clone(),
        endpoint.stream_name_output_mode.clone(),
        tag_fingerprint(&endpoint.tags),
    ])
}

// =============================================================================
// Resource policies: the shared document check and the wording around it
// =============================================================================

/// Whether `policy` is a policy document this service accepts.
///
/// A resource policy is an IAM policy document: a JSON OBJECT carrying a `Statement`.
/// Anything else — text that is not JSON, a JSON array or scalar, an object with no
/// `Statement` — is the model's `INVALID_POLICY` validation case, which is why the
/// service's `ValidationExceptionType` enum names it. Shared by the policy writers
/// (`PutChannelPolicy` and `PutOriginEndpointPolicy`) so both decide validity the same
/// way.
pub fn is_policy_document(policy: &str) -> bool {
    let Ok(::serde_json::Value::Object(document)) = ::serde_json::from_str(policy) else {
        return false;
    };
    document.contains_key("Statement")
}

/// The pagination token for a list that stopped after `last` — opaque to the caller,
/// and decoded by [`page_resume_after`].
pub fn page_token(last: &str) -> String {
    hex::encode(last.as_bytes())
}

/// The name a pagination token resumes after, or `None` when the token is not one this
/// service issued.
pub fn page_resume_after(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// The smallest `MaxResults` the model's `ListResourceMaxResults` range allows.
const MAX_RESULTS_MIN: usize = 1;
/// The largest `MaxResults` the model's `ListResourceMaxResults` range allows.
const MAX_RESULTS_MAX: usize = 100;
/// The page size the model declares as `MaxResults`'s default.
const MAX_RESULTS_DEFAULT: usize = 10;

/// The page size a list serves, from the request's `MaxResults`.
///
/// The model bounds `MaxResults` to 1..=100 and defaults it to 10, and the request
/// boundary has already refused anything outside that range, so a value outside it here
/// means the member was absent: fall back to the modelled default. Every paged list in
/// this service resolves its page size here, so a caller cannot force a larger response
/// from one list than from another.
pub fn page_size(max_results: i32) -> usize {
    match usize::try_from(max_results) {
        Ok(size) if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&size) => size,
        _ => MAX_RESULTS_DEFAULT,
    }
}

/// One page of a name-keyed collection: the records this page reports, and the token the
/// next page resumes from (`None` once nothing is left).
///
/// `records` is a name-ordered map's iterator, `resume_after` the name the previous page
/// stopped at (from [`page_resume_after`]), and `size` the resolved [`page_size`]. This
/// page starts strictly beyond `resume_after`, so no record is returned twice and none is
/// skipped, and a token is issued only when a record is actually left over. Shared by
/// every paged list so the two cannot drift.
pub fn page<'a, T>(
    records: impl Iterator<Item = (&'a String, &'a T)>,
    resume_after: Option<&str>,
    size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let mut remaining = records
        .filter(|(name, _)| resume_after.is_none_or(|after| name.as_str() > after));
    let page: Vec<(&String, &T)> = remaining.by_ref().take(size).collect();
    let next_token = page
        .last()
        .filter(|_| remaining.next().is_some())
        .map(|(last, _)| page_token(last));
    (page.into_iter().map(|(_, record)| record).collect(), next_token)
}

/// The first `len` hex characters of the digest of `parts` — a short, stable label for
/// a name that must look opaque to a client but has to be reproducible.
fn short_digest(parts: &[&str], len: usize) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hex::encode(hasher.finalize());
    digest[..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every not-found sentence names the KIND of resource and never a resource name,
    /// and carries the discriminator for that kind.
    #[test]
    fn a_not_found_names_the_resource_kind_and_no_identifier() {
        for (kind, expected_type, noun) in [
            (ResourceKind::ChannelGroup, "CHANNEL_GROUP", "channel group"),
            (ResourceKind::Channel, "CHANNEL", "channel"),
            (
                ResourceKind::OriginEndpoint,
                "ORIGIN_ENDPOINT",
                "origin endpoint",
            ),
        ] {
            let error = kind.not_found();
            let message = error.message.clone().unwrap_or_default();
            assert!(
                message.contains(noun),
                "case {kind:?}: expected a message naming {noun:?}, got {message:?}"
            );
            assert_eq!(
                error.resource_type_not_found.as_deref(),
                Some(expected_type),
                "case {kind:?}: wrong ResourceTypeNotFound, got {:?}",
                error.resource_type_not_found
            );
        }
        // "an origin endpoint", not "a origin endpoint".
        let endpoint = ResourceKind::OriginEndpoint
            .not_found()
            .message
            .unwrap_or_default();
        assert!(
            endpoint.contains("add an origin endpoint"),
            "case OriginEndpoint: expected the `an` article, got {endpoint:?}"
        );
    }

    /// Each constraint class maps to its own `ValidationExceptionType`, and the message
    /// names the member path that failed without echoing the value that failed it.
    #[test]
    fn a_violation_sets_both_modeled_members() {
        let cases = [
            (
                ViolationKind::Pattern("^[a-zA-Z0-9_-]+$".to_string()),
                "MEMBER_DOES_NOT_MATCH_PATTERN",
            ),
            (ViolationKind::MissingRequiredMember, "MEMBER_MISSING"),
            (ViolationKind::MinLength(1), "MEMBER_MIN_LENGTH"),
            (ViolationKind::MaxLength(256), "MEMBER_MAX_LENGTH"),
            (
                ViolationKind::MinValue("1".to_string()),
                "MEMBER_MIN_VALUE",
            ),
            (
                ViolationKind::MaxValue("100".to_string()),
                "MEMBER_MAX_VALUE",
            ),
            (
                ViolationKind::EnumValue(vec!["CMAF".to_string()]),
                "MEMBER_INVALID_ENUM_VALUE",
            ),
            (ViolationKind::UniqueItems, "MEMBER_INVALID"),
        ];
        for (kind, expected_type) in cases {
            let violation = Violation {
                path: "channelGroupName".to_string(),
                kind: kind.clone(),
                value: Some("bad name!".to_string()),
            };
            let error = member_validation_failed(&violation);
            assert_eq!(
                error.validation_exception_type.as_deref(),
                Some(expected_type),
                "case {kind:?}: wrong ValidationExceptionType, got {:?}",
                error.validation_exception_type
            );
            let message = error.message.clone().unwrap_or_default();
            assert!(
                message.starts_with("Validation error encountered at request.channelGroupName: "),
                "case {kind:?}: expected the member path in the message, got {message:?}"
            );
            assert!(
                !message.contains("bad name!"),
                "case {kind:?}: the rejected value must not be echoed, got {message:?}"
            );
        }
    }

    /// The character-set rendering a `@pattern` refusal states, over the pattern forms
    /// this service's model uses plus the ones it must decline to describe.
    #[test]
    fn a_character_class_pattern_is_stated_as_its_supported_characters() {
        let cases = [
            // `ResourceName`, the shape every resource-name member targets.
            (
                "^[a-zA-Z0-9_-]+$",
                Some("A-Z, a-z, 0-9, _ (underscore), and - (hyphen)"),
            ),
            // A two-item class, which joins with `and` and no comma.
            ("^[a-z0-9]+$", Some("a-z and 0-9")),
            // A one-item class.
            ("^[A-Z]+$", Some("A-Z")),
            // A leading `-` is the literal hyphen, and the ranges are listed first.
            ("^[-a-z]+$", Some("a-z and - (hyphen)")),
            // A range that is not one run of letters or digits (regex reads `_-x` as the
            // code points between them) has no listing, so the whole pattern declines.
            ("^[a-z_-x]+$", None),
            // A descending range is not a range a message can state either.
            ("^[z-a]+$", None),
            // `EntityTag` / `IdempotencyToken`: an escape names no listable set.
            (r"^[\S]+$", None),
            // A negated class states what is forbidden, not what is supported.
            ("^[^a-z]+$", None),
            // Not a single character class at all.
            ("^arn:aws:mediapackagev2:", None),
        ];
        for (pattern, expected) in cases {
            let rendered = pattern_character_set(pattern);
            assert_eq!(
                rendered.as_deref(),
                expected,
                "case {pattern:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// A `@pattern` violation is stated as the characters the member may hold, and every
    /// other constraint class as the bound it broke — each followed by the sentence
    /// telling the caller to check the value it sent.
    #[test]
    fn a_constraint_detail_states_the_bound_that_was_broken() {
        let cases = [
            (
                ViolationKind::Pattern("^[a-zA-Z0-9_-]+$".to_string()),
                "Member must contain only supported characters. Supported characters are \
                 A-Z, a-z, 0-9, _ (underscore), and - (hyphen). Check the value you \
                 specified in your request and then try again",
            ),
            (
                ViolationKind::Pattern(r"^[\S]+$".to_string()),
                "Member must satisfy regular expression pattern: ^[\\S]+$. Check the \
                 value you specified in your request and then try again",
            ),
            (
                ViolationKind::MaxLength(256),
                "Member must have length less than or equal to 256. Check the value you \
                 specified in your request and then try again",
            ),
            (
                ViolationKind::MissingRequiredMember,
                "Member must not be null. Check the value you specified in your request \
                 and then try again",
            ),
        ];
        for (kind, expected) in cases {
            let detail = constraint_detail(&kind);
            assert_eq!(
                detail, expected,
                "case {kind:?}: expected {expected:?}, got {detail:?}"
            );
        }
    }

    #[test]
    fn a_page_token_round_trips_the_name_it_stopped_after() {
        for name in ["a", "endpoint-1", "A_long_Name-with-9-digits"] {
            assert_eq!(
                page_resume_after(&page_token(name)).as_deref(),
                Some(name),
                "token for {name:?} must decode back to it"
            );
        }
    }

    #[test]
    fn a_token_this_service_did_not_issue_does_not_decode() {
        assert!(
            page_resume_after("not-a-token").is_none(),
            "a non-hex token must be rejected, not silently treated as a name"
        );
    }

    #[test]
    fn a_policy_document_is_a_json_object_with_a_statement() {
        for accepted in [
            r#"{"Statement":[]}"#,
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow"}]}"#,
            "{\n  \"Statement\": {\"Effect\": \"Deny\"}\n}",
        ] {
            assert!(
                is_policy_document(accepted),
                "case {accepted:?}: expected a valid policy document"
            );
        }
        for refused in [
            "",
            "not json at all",
            r#"{"Statement":[]"#,
            r#"[{"Statement":[]}]"#,
            r#""{\"Statement\":[]}""#,
            r#"{"Version":"2012-10-17"}"#,
        ] {
            assert!(
                !is_policy_document(refused),
                "case {refused:?}: expected to be refused as a policy document"
            );
        }
    }

    #[test]
    fn an_etag_separates_fields() {
        assert_ne!(
            etag(&["a".to_string(), "bc".to_string()]),
            etag(&["ab".to_string(), "c".to_string()]),
            "field boundaries must be part of the digest"
        );
    }

    #[test]
    fn materialization_fills_only_what_the_caller_left_out() {
        let submitted = Segment {
            segment_duration_seconds: Some(12),
            ts_include_dvb_subtitles: Some(false),
            ..Segment::default()
        };
        let materialized = materialized_segment(Some(submitted));
        assert_eq!(
            materialized.segment_duration_seconds,
            Some(12),
            "a submitted duration must survive materialization"
        );
        assert_eq!(
            materialized.ts_include_dvb_subtitles,
            Some(false),
            "a submitted toggle must survive materialization, even when it is off"
        );
        assert_eq!(
            materialized.segment_name.as_deref(),
            Some(DEFAULT_SEGMENT_NAME),
            "an omitted segment name must come out as the service's default"
        );
        assert!(
            materialized.scte.is_none() && materialized.output_timestamp_mode.is_none(),
            "materialization must not invent members the service reports only when configured"
        );
    }

    #[test]
    fn the_ingest_host_shares_the_groups_dns() {
        let egress = egress_domain("us-west-2", "group");
        let endpoints = ingest_endpoints(&egress, "group", "channel");
        let suffix = egress.split_once(".egress.").expect("an egress label").1;
        for endpoint in &endpoints {
            let url = endpoint.url.as_deref().expect("an ingest url");
            assert!(
                url.contains(suffix),
                "ingest url {url} must share the group's DNS suffix {suffix}"
            );
        }
    }

    #[test]
    fn a_token_match_tells_a_retry_from_a_changed_request() {
        let stored = [("group-a", Some("token-1"), "fingerprint-a")];
        assert_eq!(
            match_client_token(Some("token-1"), "fingerprint-a", stored.into_iter()),
            TokenMatch::Replay("group-a".to_string()),
            "the same token with the same request must replay the stored resource"
        );
        assert_eq!(
            match_client_token(Some("token-1"), "fingerprint-b", stored.into_iter()),
            TokenMatch::Mismatch,
            "the same token with a changed request must be a parameter mismatch"
        );
        assert_eq!(
            match_client_token(Some("token-2"), "fingerprint-a", stored.into_iter()),
            TokenMatch::Fresh,
            "another token must not match a stored resource"
        );
        assert_eq!(
            match_client_token(None, "fingerprint-a", stored.into_iter()),
            TokenMatch::Fresh,
            "a request with no token can never be a replay"
        );
    }

    #[test]
    fn a_container_serves_only_its_own_manifest_families() {
        // Read off the model's `ValidationExceptionType`: TS serves the HLS families,
        // CMAF adds DASH, ISM serves MSS alone.
        for (container, family, refused) in [
            ("TS", ManifestFamily::Hls, false),
            ("TS", ManifestFamily::LowLatencyHls, false),
            ("TS", ManifestFamily::Dash, true),
            ("TS", ManifestFamily::Mss, true),
            ("CMAF", ManifestFamily::Hls, false),
            ("CMAF", ManifestFamily::LowLatencyHls, false),
            ("CMAF", ManifestFamily::Dash, false),
            ("CMAF", ManifestFamily::Mss, true),
            ("ISM", ManifestFamily::Hls, true),
            ("ISM", ManifestFamily::LowLatencyHls, true),
            ("ISM", ManifestFamily::Dash, true),
            ("ISM", ManifestFamily::Mss, false),
        ] {
            let conflict = container_manifest_conflict(container, family);
            assert_eq!(
                conflict.is_some(),
                refused,
                "case {container} + {family:?}: expected refused={refused}, got {conflict:?}"
            );
        }
    }
}
