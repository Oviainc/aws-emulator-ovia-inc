// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediapackage-mediapackage service.
//!
//! The records here are the single source of truth for every MediaPackage response:
//! a create stores a record, and every later read (`Describe*`, `List*`) is rendered
//! from the stored record, never from the request that asked for it.
//!
//! Modelled shapes are reused as-is from [`crate::ops::types`] rather than mirrored
//! into parallel structs, so a stored packaging configuration is byte-for-byte what a
//! later read renders.

use crate::ops::types::{
    Authorization, Channel, CmafPackage, CmafPackageCreateOrUpdateParameters, DashPackage,
    EgressAccessLogs, HlsIngest, HlsManifest, HlsPackage, IngestEndpoint, IngressAccessLogs,
    MssPackage, OriginEndpoint,
};
use std::collections::BTreeMap;

/// This service's SigV4 signing name and ARN namespace (`OPERATIONS.md`, "Wire protocol").
pub const SERVICE_NAME: &str = "mediapackage";

/// The region an ARN or an endpoint hostname carries when the caller's request is
/// unsigned, so no SigV4 credential scope names one. `emulator_arn`'s documentation
/// names `us-east-1` as the emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ManifestName`'s documented default: "A short string appended to the end of the
/// OriginEndpoint URL" — MediaPackage appends `index` when the caller sends none.
pub const DEFAULT_MANIFEST_NAME: &str = "index";

/// How many ingest endpoints a Channel's `HlsIngest` carries. MediaPackage provisions a
/// redundant pair, so an encoder can push the same source to two ingest URLs.
pub const INGEST_ENDPOINT_COUNT: usize = 2;

/// Page size used when `MaxResults` is absent: the maximum the model allows
/// (`MaxResults` range 1..1000), so an unpaged caller sees everything it can.
pub const MAX_PAGE_SIZE: usize = 1000;

// ---------------------------------------------------------------------------
// Caller-driven growth bounds
//
// Both stores below live for the life of the process and a create is the only thing
// that adds to them, so each needs a maximum ENTRY COUNT: neither a TTL nor an eviction
// policy is available to this service, because `DescribeChannel`/`DescribeOriginEndpoint`
// must still answer what the matching create created. The two operations that grow a
// store refuse past its cap with the service's own modelled `TooManyRequestsException`
// (HTTP 429, "The client has exceeded their resource or throttling limits"), which
// MediaPackage declares on both of them and on no read.
//
// The per-record caller-sized collections and strings are bounded too, with
// `UnprocessableEntityException` (HTTP 422, "The parameters sent in the request are not
// valid"), so one request cannot decide how much memory its record occupies. The product
// is finite: at most MAX_CHANNELS channels and MAX_ORIGIN_ENDPOINTS endpoints, each with
// at most MAX_TAGS tags, MAX_WHITELIST_ENTRIES whitelist entries and
// MAX_CMAF_HLS_MANIFESTS child CMAF manifests, every caller-supplied string capped.
// ---------------------------------------------------------------------------

/// The most channels this store holds. The emulator's own resource guard, not a
/// published AWS quota: AWS publishes "Channels per account per Region" as an adjustable
/// quota, so no single value is the contract, and this is set well above any ordinary
/// workflow.
pub const MAX_CHANNELS: usize = 5_000;

/// The most origin endpoints this store holds — see [`MAX_CHANNELS`] for why a maximum
/// entry count is the bound available, and why the number is the emulator's own.
pub const MAX_ORIGIN_ENDPOINTS: usize = 20_000;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// AWS's tagging limit: a tag key is at most 128 characters.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// AWS's tagging limit: a tag value is at most 256 characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Emulator resource guard, NOT an AWS quota: the most `Whitelist` CIDR entries stored
/// for one origin endpoint.
pub const MAX_WHITELIST_ENTRIES: usize = 100;

/// Emulator resource guard, NOT an AWS quota: the most child HLS manifests stored for
/// one `CmafPackage`.
pub const MAX_CMAF_HLS_MANIFESTS: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied string this
/// service stores for a member whose length the model publishes no limit for.
pub const MAX_STRING_CHARS: usize = 1024;

/// Refuse a caller-sized collection that would grow a record past `max` entries.
///
/// `Err` carries the message, naming the member and the bound it broke, so the caller can
/// fix the request from the text alone.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!(
            "{member} may carry at most {max} entries, got {count}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} may be at most {max} characters, got {length}"
        ));
    }
    Ok(())
}

/// Refuse a tag map that breaks AWS's tagging limits, by entry count and by the length of
/// each key and value.
pub fn bound_tags(tags: &Option<std::collections::HashMap<String, String>>) -> Result<(), String> {
    let Some(tags) = tags else { return Ok(()) };
    bound_entries("Tags", tags.len(), MAX_TAGS)?;
    // Sorted so the member named in the refusal does not depend on hash order.
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    for key in keys {
        bound_chars(&format!("the Tags key {key:?}"), key, MAX_TAG_KEY_CHARS)?;
        bound_chars(
            &format!("the Tags value of {key:?}"),
            &tags[key],
            MAX_TAG_VALUE_CHARS,
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Server-filled origin-endpoint defaults
//
// MediaPackage does not store an origin endpoint as the request wrote it: every optional
// configuration member the request leaves unset is filled with that member's service
// default, stored, and returned on EVERY projection of the endpoint (the create response,
// `DescribeOriginEndpoint`, each entry of `ListOriginEndpoints`). The model declares no
// `@default` on any of them, so each value below carries its own source:
//
// * RECORDED for this service (trace set `simparity-wire-aws-v1`, fixtures
//   `mediapackage-origin-endpoint-lifecycle`, `mediapackage-conflict-error-paths`,
//   `mediapackage-harvest-job-workflow`): `origination` ALLOW, `startoverWindowSeconds`
//   and `timeDelaySeconds` 0, and in `hlsPackage` `adMarkers` NONE, the five-entry
//   `adTriggers` list, `adsOnDeliveryRestrictions` RESTRICTED, `playlistType` EVENT,
//   `playlistWindowSeconds` 60, `programDateTimeIntervalSeconds` 0 and the three booleans
//   `includeIframeOnlyStream` / `useAudioRenditionGroup` / `includeDvbSubtitles` false.
// * RECORDED for the sibling MediaPackage VOD service, whose packaging shapes carry the
//   same member names and defaults (same trace set, fixture
//   `mediapackage-vod-packaging-configuration-multi-format`): a CMAF child `HlsManifest`
//   takes `adMarkers` NONE, `programDateTimeIntervalSeconds` 0 and
//   `includeIframeOnlyStream` false, and a DASH package takes `profile` NONE,
//   `minBufferTimeSeconds` 30, `segmentTemplateFormat` NUMBER_WITH_TIMELINE,
//   `periodTriggers` [] and `includeIframeOnlyStream` false.
//
// The defaults are keyed by MEMBER, so the same member takes the same default wherever it
// appears (`HlsPackage`, a CMAF child `HlsManifest`, `DashPackage`). A member whose
// default neither the model nor a recording publishes is left exactly as the request sent
// it — `SegmentDurationSeconds`, `StreamSelection`, `Encryption`, the DASH window/timing
// members and every `MssPackage` member — because inventing a value for it would be a
// fabrication, not a default.
// ---------------------------------------------------------------------------

/// `Origination`'s default: a new endpoint may be requested unless the caller denies it.
pub const DEFAULT_ORIGINATION: &str = "ALLOW";

/// `StartoverWindowSeconds`' default. The model documents "If not specified, startover
/// playback will be disabled for the OriginEndpoint" — a window of zero seconds.
pub const DEFAULT_STARTOVER_WINDOW_SECONDS: i32 = 0;

/// `TimeDelaySeconds`' default. The model documents "If not specified, there will be no
/// time delay in effect for the OriginEndpoint" — a delay of zero seconds.
pub const DEFAULT_TIME_DELAY_SECONDS: i32 = 0;

/// `AdMarkers`' default: no SCTE-35 ad markers are written into the manifest.
pub const DEFAULT_AD_MARKERS: &str = "NONE";

/// `AdsOnDeliveryRestrictions`' default: only SCTE-35 messages that carry delivery
/// restrictions become ads.
pub const DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS: &str = "RESTRICTED";

/// `AdTriggers`' default: the SCTE-35 message types MediaPackage treats as ad markers when
/// the request names none. Every entry is a value `__AdTriggersElement` accepts
/// (`crate::ops::types::__AD_TRIGGERS_ELEMENT_VALUES`).
pub const DEFAULT_AD_TRIGGERS: &[&str] = &[
    "SPLICE_INSERT",
    "PROVIDER_ADVERTISEMENT",
    "DISTRIBUTOR_ADVERTISEMENT",
    "PROVIDER_PLACEMENT_OPPORTUNITY",
    "DISTRIBUTOR_PLACEMENT_OPPORTUNITY",
];

/// `PlaylistType`'s default: the media playlist carries `EXT-X-PLAYLIST-TYPE:EVENT`.
pub const DEFAULT_PLAYLIST_TYPE: &str = "EVENT";

/// `PlaylistWindowSeconds`' default: the time window each parent manifest contains.
pub const DEFAULT_PLAYLIST_WINDOW_SECONDS: i32 = 60;

/// `ProgramDateTimeIntervalSeconds`' default: no `EXT-X-PROGRAM-DATE-TIME` tags are
/// inserted, which the member reports as an interval of zero.
pub const DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS: i32 = 0;

/// `IncludeIframeOnlyStream`'s default: no I-Frame-only stream is added to the output.
pub const DEFAULT_INCLUDE_IFRAME_ONLY_STREAM: bool = false;

/// `UseAudioRenditionGroup`'s default: audio streams are not placed in rendition groups.
pub const DEFAULT_USE_AUDIO_RENDITION_GROUP: bool = false;

/// `IncludeDvbSubtitles`' default: DVB subtitles are not passed through.
pub const DEFAULT_INCLUDE_DVB_SUBTITLES: bool = false;

/// `DashPackage.Profile`'s default: no profile-specific (HbbTV, DVB-DASH) output.
pub const DEFAULT_DASH_PROFILE: &str = "NONE";

/// `DashPackage.MinBufferTimeSeconds`' default.
pub const DEFAULT_DASH_MIN_BUFFER_TIME_SECONDS: i32 = 30;

/// `DashPackage.SegmentTemplateFormat`'s default.
pub const DEFAULT_DASH_SEGMENT_TEMPLATE_FORMAT: &str = "NUMBER_WITH_TIMELINE";

/// `DashPackage.PeriodTriggers`' default: the MPD is not partitioned into periods, which
/// the member reports as the empty list.
pub const DEFAULT_DASH_PERIOD_TRIGGERS: &[&str] = &[];

/// An owned list for one of the `&[&str]` default lists above.
fn default_list(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// Fill the server defaults of the HLS packaging members `package` left unset, leaving
/// every supplied value — including an explicitly empty `AdTriggers` list — alone.
fn fill_hls_package(package: &mut HlsPackage) {
    package.ad_markers = package
        .ad_markers
        .take()
        .or_else(|| Some(DEFAULT_AD_MARKERS.to_string()));
    package.ad_triggers = package
        .ad_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_AD_TRIGGERS)));
    package.ads_on_delivery_restrictions = package
        .ads_on_delivery_restrictions
        .take()
        .or_else(|| Some(DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS.to_string()));
    package.include_dvb_subtitles = package
        .include_dvb_subtitles
        .or(Some(DEFAULT_INCLUDE_DVB_SUBTITLES));
    package.include_iframe_only_stream = package
        .include_iframe_only_stream
        .or(Some(DEFAULT_INCLUDE_IFRAME_ONLY_STREAM));
    package.playlist_type = package
        .playlist_type
        .take()
        .or_else(|| Some(DEFAULT_PLAYLIST_TYPE.to_string()));
    package.playlist_window_seconds = package
        .playlist_window_seconds
        .or(Some(DEFAULT_PLAYLIST_WINDOW_SECONDS));
    package.program_date_time_interval_seconds = package
        .program_date_time_interval_seconds
        .or(Some(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS));
    package.use_audio_rendition_group = package
        .use_audio_rendition_group
        .or(Some(DEFAULT_USE_AUDIO_RENDITION_GROUP));
}

/// Fill the server defaults of the manifest members a CMAF child `HlsManifest` left unset.
/// The member set it shares with [`HlsPackage`] takes the same defaults.
fn fill_hls_manifest(manifest: &mut HlsManifest) {
    manifest.ad_markers = manifest
        .ad_markers
        .take()
        .or_else(|| Some(DEFAULT_AD_MARKERS.to_string()));
    manifest.ad_triggers = manifest
        .ad_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_AD_TRIGGERS)));
    manifest.ads_on_delivery_restrictions = manifest
        .ads_on_delivery_restrictions
        .take()
        .or_else(|| Some(DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS.to_string()));
    manifest.include_iframe_only_stream = manifest
        .include_iframe_only_stream
        .or(Some(DEFAULT_INCLUDE_IFRAME_ONLY_STREAM));
    manifest.playlist_type = manifest
        .playlist_type
        .take()
        .or_else(|| Some(DEFAULT_PLAYLIST_TYPE.to_string()));
    manifest.playlist_window_seconds = manifest
        .playlist_window_seconds
        .or(Some(DEFAULT_PLAYLIST_WINDOW_SECONDS));
    manifest.program_date_time_interval_seconds = manifest
        .program_date_time_interval_seconds
        .or(Some(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS));
}

/// Fill the server defaults of the DASH packaging members `package` left unset. The
/// SCTE-35 members it shares with [`HlsPackage`] take the same defaults.
fn fill_dash_package(package: &mut DashPackage) {
    package.ad_triggers = package
        .ad_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_AD_TRIGGERS)));
    package.ads_on_delivery_restrictions = package
        .ads_on_delivery_restrictions
        .take()
        .or_else(|| Some(DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS.to_string()));
    package.include_iframe_only_stream = package
        .include_iframe_only_stream
        .or(Some(DEFAULT_INCLUDE_IFRAME_ONLY_STREAM));
    package.min_buffer_time_seconds = package
        .min_buffer_time_seconds
        .or(Some(DEFAULT_DASH_MIN_BUFFER_TIME_SECONDS));
    package.period_triggers = package
        .period_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_DASH_PERIOD_TRIGGERS)));
    package.profile = package
        .profile
        .take()
        .or_else(|| Some(DEFAULT_DASH_PROFILE.to_string()));
    package.segment_template_format = package
        .segment_template_format
        .take()
        .or_else(|| Some(DEFAULT_DASH_SEGMENT_TEMPLATE_FORMAT.to_string()));
}

/// Materialise the server-filled members of an origin endpoint built from a create or
/// update request, BEFORE it is stored.
///
/// Every write of an endpoint goes through this, so the stored record — and therefore
/// every projection of it — carries the same member set whatever the request omitted.
pub fn apply_origin_endpoint_defaults(record: &mut OriginEndpointRecord) {
    record.origination = record
        .origination
        .take()
        .or_else(|| Some(DEFAULT_ORIGINATION.to_string()));
    record.startover_window_seconds = record
        .startover_window_seconds
        .or(Some(DEFAULT_STARTOVER_WINDOW_SECONDS));
    record.time_delay_seconds = record.time_delay_seconds.or(Some(DEFAULT_TIME_DELAY_SECONDS));
    if let Some(package) = record.hls_package.as_mut() {
        fill_hls_package(package);
    }
    if let Some(package) = record.dash_package.as_mut() {
        fill_dash_package(package);
    }
    if let Some(package) = record.cmaf_package.as_mut() {
        for manifest in package.hls_manifests.iter_mut().flatten() {
            fill_hls_manifest(manifest);
        }
    }
}

/// A Channel, exactly as `CreateChannel` stored it.
#[derive(Debug, Clone, Default)]
pub struct ChannelRecord {
    pub id: String,
    pub arn: String,
    pub description: Option<String>,
    /// ISO-8601, minted once at creation and never recomputed on a read.
    pub created_at: String,
    pub ingest_endpoints: Vec<IngestEndpoint>,
    pub egress_log_group_name: Option<String>,
    pub ingress_log_group_name: Option<String>,
    pub tags: BTreeMap<String, String>,
}

impl ChannelRecord {
    /// The modelled `Channel` shape for this record. Every channel-returning operation
    /// renders its output from this one projection, so they cannot disagree.
    pub fn to_channel(&self) -> Channel {
        Channel {
            arn: Some(self.arn.clone()),
            created_at: Some(self.created_at.clone()),
            description: self.description.clone(),
            egress_access_logs: Some(EgressAccessLogs {
                log_group_name: self.egress_log_group_name.clone(),
            }),
            hls_ingest: Some(HlsIngest {
                ingest_endpoints: Some(self.ingest_endpoints.clone()),
            }),
            id: Some(self.id.clone()),
            ingress_access_logs: Some(IngressAccessLogs {
                log_group_name: self.ingress_log_group_name.clone(),
            }),
            tags: Some(tags_to_wire(&self.tags)),
        }
    }
}

/// An OriginEndpoint, exactly as `CreateOriginEndpoint` stored it.
#[derive(Debug, Clone, Default)]
pub struct OriginEndpointRecord {
    pub id: String,
    pub arn: String,
    pub channel_id: String,
    pub description: Option<String>,
    pub manifest_name: Option<String>,
    /// The playback URL, derived once at creation and stored, so every read returns it.
    pub url: String,
    pub created_at: String,
    pub hls_package: Option<HlsPackage>,
    pub dash_package: Option<DashPackage>,
    pub cmaf_package: Option<CmafPackage>,
    pub mss_package: Option<MssPackage>,
    pub authorization: Option<Authorization>,
    pub origination: Option<String>,
    pub startover_window_seconds: Option<i32>,
    pub time_delay_seconds: Option<i32>,
    pub whitelist: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

impl OriginEndpointRecord {
    /// The modelled `OriginEndpoint` shape for this record.
    pub fn to_origin_endpoint(&self) -> OriginEndpoint {
        OriginEndpoint {
            arn: Some(self.arn.clone()),
            authorization: self.authorization.clone(),
            channel_id: Some(self.channel_id.clone()),
            cmaf_package: self.cmaf_package.clone(),
            created_at: Some(self.created_at.clone()),
            dash_package: self.dash_package.clone(),
            description: self.description.clone(),
            hls_package: self.hls_package.clone(),
            id: Some(self.id.clone()),
            manifest_name: self.manifest_name.clone(),
            mss_package: self.mss_package.clone(),
            origination: self.origination.clone(),
            startover_window_seconds: self.startover_window_seconds,
            tags: Some(tags_to_wire(&self.tags)),
            time_delay_seconds: self.time_delay_seconds,
            url: Some(self.url.clone()),
            whitelist: Some(self.whitelist.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-mediapackage-mediapackage. A handler reaches it through [`service_state`].
///
/// Both collections are `BTreeMap`s keyed by the resource id the API itself keys on, so
/// `List*` output and its paging tokens are in a stable, caller-independent order. Each
/// is bounded by an entry count — [`MAX_CHANNELS`] and [`MAX_ORIGIN_ENDPOINTS`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackageMediapackageState {
    pub channels: BTreeMap<String, ChannelRecord>,
    pub origin_endpoints: BTreeMap<String, OriginEndpointRecord>,
    /// How many ids this store has minted. Part of every minted id's seed, so no two
    /// minted ids collide even when two calls land in the same nanosecond.
    pub minted: u64,
}

impl ComAmazonawsMediapackageMediapackageState {
    /// A fresh lowercase-hex id of `bytes` bytes (so `2 * bytes` characters).
    ///
    /// `minted` advances on every call and is hashed into the seed together with the wall
    /// clock and the process id, so repeated calls never return the same value. This is
    /// how the sibling services in this workspace mint a system-generated id or secret.
    pub fn mint_hex(&mut self, label: &str, bytes: usize) -> String {
        use sha2::Digest;
        self.minted = self.minted.wrapping_add(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!("{label}|{}|{}|{nanos}", self.minted, std::process::id());
        let digest = sha2::Sha256::digest(seed.as_bytes());
        hex::encode(&digest[..bytes.min(digest.len())])
    }

    /// The redundant pair of ingest endpoints MediaPackage provisions for a new Channel.
    ///
    /// Each endpoint's id and its ingest credentials — the model documents `Username` and
    /// `Password` as "system generated" — are minted once, here, and stored on the channel
    /// record, so every later read of the channel returns exactly these values. The URL
    /// embeds the endpoint id, so it stays valid for exactly that endpoint.
    pub fn mint_ingest_endpoints(&mut self, region: &str, channel_id: &str) -> Vec<IngestEndpoint> {
        let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);
        let channel_label = stable_label(&[channel_id, region]);
        (0..INGEST_ENDPOINT_COUNT)
            .map(|_| {
                let id = self.mint_hex("ingest-endpoint", 16);
                IngestEndpoint {
                    url: Some(format!(
                        "https://{account_label}.mediapackage.{region}.amazonaws.com/in/v2/{channel_label}/{id}/channel"
                    )),
                    id: Some(id),
                    username: Some(self.mint_hex("ingest-username", 12)),
                    password: Some(self.mint_hex("ingest-password", 24)),
                }
            })
            .collect()
    }

    /// How many stored origin endpoints package `channel_id`. `DeleteChannel` reads it:
    /// a channel that still has endpoints cannot be deleted, which is what keeps the two
    /// stores consistent (no endpoint ever names a channel this service no longer holds).
    pub fn origin_endpoints_of(&self, channel_id: &str) -> usize {
        self.origin_endpoints
            .values()
            .filter(|record| record.channel_id == channel_id)
            .count()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediapackageMediapackageState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediapackageMediapackageState>)
}

/// The modelled `Tags` map (`map<string,string>`) for a stored tag set.
pub fn tags_to_wire(tags: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// A stored tag set from the modelled `Tags` map on an input.
pub fn tags_from_wire(
    tags: &Option<std::collections::HashMap<String, String>>,
) -> BTreeMap<String, String> {
    tags.iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The region this request is addressed to: the SigV4 credential scope's region when the
/// request is signed, else [`DEFAULT_REGION`].
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the ISO-8601 instant MediaPackage reports in `CreatedAt`.
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).unwrap_or(0))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// `arn:aws:mediapackage:<region>:<account>:channels/<id>`.
pub fn channel_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("channels/{id}"))
}

/// `arn:aws:mediapackage:<region>:<account>:origin_endpoints/<id>`.
pub fn origin_endpoint_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("origin_endpoints/{id}"))
}

/// A stable, opaque 32-hex-character label derived from `parts`, used for the
/// account/resource labels in an ingest or playback hostname and path. Derived from
/// content (never from the clock or a random source) so the same resource always renders
/// the same URL on every later read.
pub fn stable_label(parts: &[&str]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// Which packaging configuration an OriginEndpoint was created with, and so which
/// manifest file its playback URL ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackagingKind {
    Hls,
    Dash,
    Cmaf,
    Mss,
}

impl PackagingKind {
    /// The manifest path a packaging type's playback URL ends in, for `manifest_name`.
    ///
    /// The extensions are the manifest formats each packager emits: HLS and CMAF serve an
    /// HLS master playlist (`.m3u8`), DASH an MPD (`.mpd`), and Microsoft Smooth a
    /// server manifest (`.ism/Manifest`).
    pub fn manifest_path(self, manifest_name: &str) -> String {
        match self {
            Self::Hls => format!("{manifest_name}.m3u8"),
            Self::Dash => format!("{manifest_name}.mpd"),
            Self::Cmaf => format!("{manifest_name}/index.m3u8"),
            Self::Mss => format!("{manifest_name}.ism/Manifest"),
        }
    }
}

/// The playback URL for an OriginEndpoint: the egress hostname for this account and
/// region, then the endpoint's own path label, then the manifest its packaging type
/// serves under `manifest_name`.
pub fn playback_url(
    region: &str,
    channel_id: &str,
    endpoint_id: &str,
    manifest_name: &str,
    kind: PackagingKind,
) -> String {
    let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);
    let endpoint_label = stable_label(&[channel_id, endpoint_id, region]);
    let manifest = kind.manifest_path(manifest_name);
    format!(
        "https://{account_label}.mediapackage.{region}.amazonaws.com/out/v1/{endpoint_label}/{manifest}"
    )
}

/// The playback URL of one HLS manifest inside a CMAF package: the endpoint's own
/// playback path, then that child manifest's own label and name.
pub fn cmaf_manifest_url(
    region: &str,
    channel_id: &str,
    endpoint_id: &str,
    manifest_id: &str,
    manifest_name: &str,
) -> String {
    let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);
    let endpoint_label = stable_label(&[channel_id, endpoint_id, region]);
    let manifest_label = stable_label(&[channel_id, endpoint_id, manifest_id, region]);
    format!(
        "https://{account_label}.mediapackage.{region}.amazonaws.com/out/v1/{endpoint_label}/{manifest_label}/{manifest_name}/index.m3u8"
    )
}

/// The `CmafPackage` to store for the `CmafPackageCreateOrUpdateParameters` an input
/// carried.
///
/// The create/update parameter shape differs from the stored shape in exactly two ways,
/// both of them values the SERVICE supplies: each child HLS manifest gains the playback
/// `Url` derived for it, and an absent `SegmentPrefix` takes its documented default,
/// the ChannelId.
pub fn stored_cmaf_package(
    params: &CmafPackageCreateOrUpdateParameters,
    region: &str,
    channel_id: &str,
    endpoint_id: &str,
) -> CmafPackage {
    let hls_manifests = params
        .hls_manifests
        .iter()
        .flatten()
        .map(|manifest| {
            let manifest_name = manifest
                .manifest_name
                .clone()
                .unwrap_or_else(|| DEFAULT_MANIFEST_NAME.to_string());
            HlsManifest {
                ad_markers: manifest.ad_markers.clone(),
                ad_triggers: manifest.ad_triggers.clone(),
                ads_on_delivery_restrictions: manifest.ads_on_delivery_restrictions.clone(),
                include_iframe_only_stream: manifest.include_iframe_only_stream,
                playlist_type: manifest.playlist_type.clone(),
                playlist_window_seconds: manifest.playlist_window_seconds,
                program_date_time_interval_seconds: manifest.program_date_time_interval_seconds,
                url: Some(cmaf_manifest_url(
                    region,
                    channel_id,
                    endpoint_id,
                    &manifest.id,
                    &manifest_name,
                )),
                manifest_name: Some(manifest_name),
                id: manifest.id.clone(),
            }
        })
        .collect();
    CmafPackage {
        encryption: params.encryption.clone(),
        hls_manifests: Some(hls_manifests),
        segment_duration_seconds: params.segment_duration_seconds,
        segment_prefix: Some(
            params
                .segment_prefix
                .clone()
                .unwrap_or_else(|| channel_id.to_string()),
        ),
        stream_selection: params.stream_selection.clone(),
    }
}

/// Encode a paging token: the key the NEXT page starts at.
///
/// Opaque to the caller (hex of the key's bytes), and resolved against the `BTreeMap`
/// ordering, so a token taken from one call resumes exactly where that call stopped even
/// if records were added or removed in between.
pub fn encode_page_token(next_key: &str) -> String {
    hex::encode(next_key.as_bytes())
}

/// Decode a paging token back to the key its page starts at.
///
/// `None` for a token this service did not issue, which the caller is told about rather
/// than silently served page one for.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// How many items one `List*` call serves, for the `MaxResults` it was given.
///
/// `MaxResults`' modelled range is 1..1000 and the schema server enforces it BEFORE this
/// handler runs, so a value reaching here is already inside the range; the clamp states
/// that invariant locally instead of leaving it implicit, and an absent `MaxResults`
/// takes the modelled maximum so an unpaged caller sees everything it can.
pub fn page_size(max_results: Option<i32>) -> usize {
    match max_results {
        None => MAX_PAGE_SIZE,
        Some(max) => usize::try_from(max).unwrap_or(1).clamp(1, MAX_PAGE_SIZE),
    }
}

/// One page of `BTreeMap` keys: up to `page_size` keys at or after `start`, plus the key
/// the next page would start at when (and only when) more remain.
pub fn paginate<'a, I>(
    keys: I,
    start: Option<&str>,
    page_size: usize,
) -> (Vec<&'a str>, Option<String>)
where
    I: IntoIterator<Item = &'a str>,
{
    let mut remaining = keys
        .into_iter()
        .filter(|key| start.is_none_or(|from| *key >= from));
    let page: Vec<&'a str> = remaining.by_ref().take(page_size).collect();
    let next_token = remaining.next().map(encode_page_token);
    (page, next_token)
}
