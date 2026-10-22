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

use std::collections::{BTreeMap, HashMap};

use aws_smithy_schema_server::{Violation, ViolationKind};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    CreateDashManifestConfiguration, CreateHlsManifestConfiguration,
    CreateLowLatencyHlsManifestConfiguration, CreateMssManifestConfiguration,
    ForceEndpointErrorConfiguration, GetDashManifestConfiguration, GetHlsManifestConfiguration,
    GetLowLatencyHlsManifestConfiguration, GetMssManifestConfiguration, IngestEndpoint,
    InputSwitchConfiguration, MultiviewConfiguration, OutputHeaderConfiguration, Segment,
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
    pub input_switch_configuration: Option<InputSwitchConfiguration>,
    pub output_header_configuration: Option<OutputHeaderConfiguration>,
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
    /// policy a 404 rather than an empty success.
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
    pub startover_window_seconds: Option<i32>,
    pub hls_manifests: Vec<GetHlsManifestConfiguration>,
    pub low_latency_hls_manifests: Vec<GetLowLatencyHlsManifestConfiguration>,
    pub dash_manifests: Vec<GetDashManifestConfiguration>,
    pub mss_manifests: Vec<GetMssManifestConfiguration>,
    pub force_endpoint_error_configuration: Option<ForceEndpointErrorConfiguration>,
    pub uri_separator: Option<String>,
    pub stream_name_output_mode: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    pub reset_at: Option<::aws_smithy_types::DateTime>,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this endpoint carried, if any.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this endpoint.
    pub fingerprint: String,
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
/// the URL the service computes for it.
pub fn stored_hls_manifest(
    submitted: CreateHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetHlsManifestConfiguration {
    GetHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: submitted.child_manifest_name,
        manifest_window_seconds: submitted.manifest_window_seconds,
        program_date_time_interval_seconds: submitted.program_date_time_interval_seconds,
        scte_hls: submitted.scte_hls,
        filter_configuration: submitted.filter_configuration,
        start_tag: submitted.start_tag,
        url_encode_child_manifest: submitted.url_encode_child_manifest,
        uri_path_type: submitted.uri_path_type,
    }
}

/// The stored form of a submitted low-latency HLS manifest. Low-latency HLS publishes
/// the same `.m3u8` playlist as plain HLS, so it shares the URL rule.
pub fn stored_low_latency_hls_manifest(
    submitted: CreateLowLatencyHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetLowLatencyHlsManifestConfiguration {
    GetLowLatencyHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: submitted.child_manifest_name,
        manifest_window_seconds: submitted.manifest_window_seconds,
        program_date_time_interval_seconds: submitted.program_date_time_interval_seconds,
        scte_hls: submitted.scte_hls,
        filter_configuration: submitted.filter_configuration,
        start_tag: submitted.start_tag,
        url_encode_child_manifest: submitted.url_encode_child_manifest,
        uri_path_type: submitted.uri_path_type,
    }
}

/// The stored form of a submitted DASH manifest.
pub fn stored_dash_manifest(
    submitted: CreateDashManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetDashManifestConfiguration {
    GetDashManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Dash),
        manifest_name: submitted.manifest_name,
        manifest_window_seconds: submitted.manifest_window_seconds,
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
        uri_path_type: submitted.uri_path_type,
        availability_start_time_configuration: submitted.availability_start_time_configuration,
    }
}

/// The stored form of a submitted Microsoft Smooth Streaming manifest.
pub fn stored_mss_manifest(
    submitted: CreateMssManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetMssManifestConfiguration {
    GetMssManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Mss),
        manifest_name: submitted.manifest_name,
        filter_configuration: submitted.filter_configuration,
        manifest_window_seconds: submitted.manifest_window_seconds,
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
fn constraint_detail(kind: &ViolationKind) -> String {
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
        ViolationKind::Pattern(pattern) => {
            format!("Member must satisfy regular expression pattern: {pattern}")
        }
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

/// The message a list operation answers a pagination token it did not issue with.
/// Request-derived, and the same sentence whichever list is being paged.
pub fn invalid_pagination_token_message() -> String {
    validation_message(
        Some("nextToken"),
        "You've provided an invalid pagination token in your request. Check your token \
         and then try again",
    )
}

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
        endpoint.startover_window_seconds.unwrap_or_default().to_string(),
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
        endpoint.uri_separator.clone().unwrap_or_default(),
        endpoint.stream_name_output_mode.clone().unwrap_or_default(),
        tag_fingerprint(&endpoint.tags),
    ])
}

/// Whether `policy` is a policy document this service accepts.
///
/// A resource policy is an IAM policy document: a JSON OBJECT carrying a `Statement`.
/// Anything else — text that is not JSON, a JSON array or scalar, an object with no
/// `Statement` — is the model's `INVALID_POLICY` validation case, which is why the
/// service's `ValidationExceptionType` enum names it. Shared by the policy writers
/// (`PutChannelPolicy`, and `PutOriginEndpointPolicy` when it is implemented) so both
/// decide validity the same way.
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
