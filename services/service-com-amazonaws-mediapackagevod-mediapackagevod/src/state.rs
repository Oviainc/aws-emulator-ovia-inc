// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediapackagevod-mediapackagevod service.
//!
//! Three resource maps, each keyed by a [`ResourceKey`] — the AWS Region the
//! resource was created in, then the resource's own `Id` (the identifier the REST
//! routes address it by) — plus ONE tag map keyed by resource ARN. Tags live only
//! in `tags_by_arn`: a resource record carries its ARN, and an ARN carries its
//! Region, so every operation — the creates here and the later `TagResource` /
//! `ListTagsForResource` slice — reads and writes the one copy rather than two that
//! can drift.
//!
//! MediaPackage VOD is a REGIONAL control plane (`aws.api#service` gives it the
//! `mediapackage-vod` endpoint prefix, arn namespace and SigV4 signing name), so
//! the Region a request is served in is part of the identity of every resource it
//! touches: the same `Id` in two Regions is two independent resources, and a
//! request served in one Region resolves, lists, mutates and deletes only the
//! records created in it. Every accessor on this struct therefore takes the
//! request's Region ([`request_region`]) beside the `Id`.
//!
//! `ApproximateAssetCount` and `EgressEndpoints` are never stored: they are
//! derived from these maps on every read ([`Self::asset_count`],
//! [`Self::egress_endpoints`]), so adding a packaging configuration or an asset
//! shows up in the next describe without rewriting any stored record.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use crate::ops::types::{
    Authorization, CmafPackage, DashManifest, DashPackage, EgressAccessLogs, EgressEndpoint,
    HlsManifest, HlsPackage, MssPackage,
};

/// The region an unsigned request is served in. A signed request names its own
/// region in the SigV4 credential scope; see [`request_region`].
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's SigV4 / ARN namespace, from the model's `aws.api#service`
/// (`arnNamespace: mediapackage-vod`).
pub const ARN_NAMESPACE: &str = "mediapackage-vod";

/// How long after both the asset and the packaging configuration exist the
/// packaging job the service owes is due, so the endpoint reports
/// [`EGRESS_STATUS_PLAYABLE`] instead of [`EGRESS_STATUS_QUEUED`].
///
/// AWS documents no duration for VOD ingest, so this span is CHOSEN: it is long
/// enough that a caller acting straight after `CreateAsset` still observes the
/// non-terminal status, and short enough not to slow a test down.
pub const INGEST_PLAYABLE_AFTER: Duration = Duration::from_millis(500);

/// The non-terminal egress-endpoint status: the packaging job is not due yet.
///
/// From `EgressEndpoint$Status`: "The current processing status of the asset used
/// for the packaging configuration. The status can be either QUEUED, PROCESSING,
/// PLAYABLE, or FAILED."
pub const EGRESS_STATUS_QUEUED: &str = "QUEUED";

/// The terminal egress-endpoint status for a successfully packaged asset. Same
/// `EgressEndpoint$Status` value list as [`EGRESS_STATUS_QUEUED`].
pub const EGRESS_STATUS_PLAYABLE: &str = "PLAYABLE";

/// The manifest file name a packaging configuration uses when its manifest block
/// names none. `HlsManifest$ManifestName` (and the Dash/Mss equivalents) is "An
/// optional string to include in the name of the manifest".
pub const DEFAULT_MANIFEST_NAME: &str = "index";

/// The ad-marker treatment an `HlsManifest` is packaged with when the request names
/// none: `AdMarkers` value `NONE`, which "will omit all SCTE-35 ad markers from the
/// output" (`HlsManifest$AdMarkers`) — the treatment of content carrying no markers.
pub const DEFAULT_AD_MARKERS: &str = "NONE";

/// The `EXT-X-PROGRAM-DATE-TIME` interval an `HlsManifest` is packaged with when the
/// request names none. `HlsManifest$ProgramDateTimeIntervalSeconds` is "The interval
/// (in seconds) between each EXT-X-PROGRAM-DATE-TIME tag inserted into manifests";
/// zero is the interval that inserts none.
pub const DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS: i32 = 0;

/// The segment-template form a `DashPackage` is packaged with when the request names
/// none, from `SegmentTemplateFormat`: "When set to NUMBER_WITH_TIMELINE, a full
/// timeline is presented in each SegmentTemplate, with $Number$ media URLs".
pub const DEFAULT_SEGMENT_TEMPLATE_FORMAT: &str = "NUMBER_WITH_TIMELINE";

/// The DASH profile a `DashManifest` is packaged with when the request names none,
/// from `Profile`: the value that enables no profile-specific output (the other,
/// `HBBTV_1_5`, "enables HbbTV 1.5 compliant output").
pub const DEFAULT_DASH_PROFILE: &str = "NONE";

/// The SCTE-marker source a `DashManifest` is packaged with when the request names
/// none, from `ScteMarkersSource`: "When set to SEGMENTS, the scte markers are sourced
/// from the segments of the ingested content."
pub const DEFAULT_SCTE_MARKERS_SOURCE: &str = "SEGMENTS";

/// The buffer a `DashManifest` advertises when the request names none —
/// `DashManifest$MinBufferTimeSeconds`, "Minimum duration (in seconds) that a player
/// will buffer media before starting the presentation".
///
/// The value is the one the service materialises, as the recorded create responses of
/// this service show; the model declares no `@default`.
pub const DEFAULT_MIN_BUFFER_TIME_SECONDS: i32 = 30;

/// The most PackagingGroups this emulator holds.
///
/// A CAPACITY BOUND, not an AWS-published quota: every create grows the in-process
/// store and the only removal path is an explicit delete, so without a ceiling a
/// caller looping creates with fresh ids grows this process without limit. Past the
/// bound a create answers the modelled `TooManyRequestsException` (HTTP 429), whose
/// own documentation is "The client has exceeded their resource or throttling
/// limits."
pub const MAX_PACKAGING_GROUPS: usize = 1_000;

/// The most PackagingConfigurations this emulator holds in total. Same kind of
/// capacity bound as [`MAX_PACKAGING_GROUPS`].
pub const MAX_PACKAGING_CONFIGURATIONS: usize = 1_000;

/// The most PackagingConfigurations this emulator holds in ONE PackagingGroup.
///
/// This also bounds the derived fan-out: `EgressEndpoints` is one entry per
/// configuration in the asset's group, so it caps both the work a single read does
/// and the size of the response it returns, however many assets are stored.
pub const MAX_PACKAGING_CONFIGURATIONS_PER_GROUP: usize = 100;

/// The most Assets this emulator holds. Same kind of capacity bound as
/// [`MAX_PACKAGING_GROUPS`].
pub const MAX_ASSETS: usize = 10_000;

/// Length of the checksum prefix a pagination cursor carries, so a cursor this
/// service did not issue is detectable rather than merely malformed.
const PAGE_TOKEN_CHECK_LEN: usize = 8;

/// The listing name [`encode_page_token`] binds a `ListAssets` cursor to.
pub const LISTING_ASSETS: &str = "assets";

/// The listing name [`encode_page_token`] binds a `ListPackagingConfigurations`
/// cursor to.
pub const LISTING_PACKAGING_CONFIGURATIONS: &str = "packaging-configurations";

/// The listing name [`encode_page_token`] binds a `ListPackagingGroups` cursor to.
pub const LISTING_PACKAGING_GROUPS: &str = "packaging-groups";

/// The smallest page a listing returns: the minimum of the `MaxResults` `range`
/// trait every paginated operation in this model shares (`min: 1, max: 1000`).
pub const MIN_PAGE_SIZE: usize = 1;

/// The largest page a listing returns, and the page size a request that names no
/// `MaxResults` gets: the maximum of that same `range` trait.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How every resource map in this state is keyed: the AWS Region the resource was
/// created in, then the caller-chosen `Id` the REST routes address it by.
///
/// The Region comes FIRST so one Region's records are a contiguous range of the
/// `BTreeMap`, read in `Id` order ([`region_slice`]) \u2014 which is what lets a listing
/// page one Region's resources in one stable order. Two Regions holding the same
/// `Id` are two distinct keys, so the same `Id` can be created independently in
/// each, exactly as a regional AWS control plane allows.
pub type ResourceKey = (String, String);

/// The key `id` is stored under when the request that created it was served in
/// `region`.
pub fn resource_key(region: &str, id: &str) -> ResourceKey {
    (region.to_string(), id.to_string())
}

/// The records of ONE Region, in `Id` order.
///
/// A `BTreeMap` keyed by [`ResourceKey`] orders by Region first, so a Region's
/// records are the contiguous range that starts at its smallest possible key; the
/// scan stops at the first key naming another Region.
fn region_slice<'map, V>(
    map: &'map BTreeMap<ResourceKey, V>,
    region: &str,
) -> impl Iterator<Item = &'map V> {
    let region = region.to_string();
    map.range(resource_key(&region, "")..)
        .take_while(move |((stored_region, _), _)| *stored_region == region)
        .map(|(_, record)| record)
}

/// A stored PackagingGroup. `ApproximateAssetCount` is NOT a field: it is counted
/// from [`ComAmazonawsMediapackagevodMediapackagevodState::assets`] on each read.
#[derive(Debug, Clone)]
pub struct PackagingGroupRecord {
    /// The caller-chosen `Id` this group is addressed by.
    pub id: String,
    /// The Region this group was created in — the one its ARN and its egress domain
    /// name carry, and the only one it resolves in. See [`ResourceKey`].
    pub region: String,
    /// `arn:aws:mediapackage-vod:<region>:<account>:packaging-groups/<Id>`.
    pub arn: String,
    /// The egress domain name assets in this group are played back from.
    pub domain_name: String,
    /// ISO-8601 instant this group was created, as returned by every read.
    pub created_at: String,
    /// CDN authorization credentials, exactly as supplied.
    pub authorization: Option<Authorization>,
    /// Egress access logging configuration, exactly as supplied.
    pub egress_access_logs: Option<EgressAccessLogs>,
}

/// A stored PackagingConfiguration. The package block is kept verbatim so a read
/// returns what was written.
#[derive(Debug, Clone)]
pub struct PackagingConfigurationRecord {
    /// The caller-chosen `Id` this configuration is addressed by.
    pub id: String,
    /// The Region this configuration was created in; see [`ResourceKey`].
    pub region: String,
    /// `arn:aws:mediapackage-vod:<region>:<account>:packaging-configurations/<Id>`.
    pub arn: String,
    /// ISO-8601 instant this configuration was created.
    pub created_at: String,
    /// The `Id` of the PackagingGroup this configuration packages for.
    pub packaging_group_id: String,
    /// Exactly one of these four is `Some`; whichever it is decides the playback
    /// manifest the egress endpoints expose.
    pub hls_package: Option<HlsPackage>,
    /// See [`Self::hls_package`].
    pub dash_package: Option<DashPackage>,
    /// See [`Self::hls_package`].
    pub cmaf_package: Option<CmafPackage>,
    /// See [`Self::hls_package`].
    pub mss_package: Option<MssPackage>,
    /// Monotonic instant this configuration was created, for the packaging job the
    /// clock owes each asset in the group.
    pub created_instant: Instant,
}

/// A stored Asset. `EgressEndpoints` is NOT a field: it is derived from the
/// packaging configurations attached to this asset's group on each read.
#[derive(Debug, Clone)]
pub struct AssetRecord {
    /// The caller-chosen `Id` this asset is addressed by.
    pub id: String,
    /// The Region this asset was ingested in; see [`ResourceKey`].
    pub region: String,
    /// `arn:aws:mediapackage-vod:<region>:<account>:assets/<Id>`.
    pub arn: String,
    /// ISO-8601 instant this asset was submitted for ingest.
    pub created_at: String,
    /// The `Id` of the PackagingGroup this asset was ingested into.
    pub packaging_group_id: String,
    /// The resource id to include in SPEKE key requests, exactly as supplied — and
    /// `None` when the caller supplied none, because the service reports this member
    /// only where the caller stated it.
    pub resource_id: Option<String>,
    /// ARN of the source object in S3.
    pub source_arn: String,
    /// The IAM role ARN used to read the source object.
    pub source_role_arn: String,
    /// Monotonic instant ingest started, for the packaging job the clock owes.
    pub created_instant: Instant,
}

/// Per-service state for com-amazonaws-mediapackagevod-mediapackagevod. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackagevodMediapackagevodState {
    /// PackagingGroups by (Region, `Id`).
    pub packaging_groups: BTreeMap<ResourceKey, PackagingGroupRecord>,
    /// PackagingConfigurations by (Region, `Id`).
    pub packaging_configurations: BTreeMap<ResourceKey, PackagingConfigurationRecord>,
    /// Assets by (Region, `Id`). A `BTreeMap` so `ListAssets` pages one Region's
    /// assets in one stable `Id` order.
    pub assets: BTreeMap<ResourceKey, AssetRecord>,
    /// Every resource's tags, keyed by that resource's ARN — the single tag store.
    ///
    /// It holds at most one entry per stored resource (a create inserts one, a
    /// delete removes it), so the three capacity bounds above bound this map too.
    pub tags_by_arn: BTreeMap<String, BTreeMap<String, String>>,
}

impl ComAmazonawsMediapackagevodMediapackagevodState {
    /// The PackagingGroup `id` names IN `region`, for a reporting operation. `None`
    /// for an `Id` only another Region holds: that group does not exist here.
    pub fn packaging_group(&self, region: &str, id: &str) -> Option<&PackagingGroupRecord> {
        self.packaging_groups.get(&resource_key(region, id))
    }

    /// The PackagingGroup `id` names in `region`, for an operation the spec says
    /// mutates it (`UpdatePackagingGroup`, `ConfigureLogs`).
    pub fn packaging_group_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut PackagingGroupRecord> {
        self.packaging_groups.get_mut(&resource_key(region, id))
    }

    /// Whether `region` holds a PackagingGroup under `id`.
    pub fn has_packaging_group(&self, region: &str, id: &str) -> bool {
        self.packaging_groups.contains_key(&resource_key(region, id))
    }

    /// Store `record` as the PackagingGroup `id` of `region`.
    pub fn insert_packaging_group(&mut self, record: PackagingGroupRecord) {
        let key = resource_key(&record.region, &record.id);
        self.packaging_groups.insert(key, record);
    }

    /// Remove the PackagingGroup `id` names in `region`, returning what was stored.
    pub fn remove_packaging_group(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<PackagingGroupRecord> {
        self.packaging_groups.remove(&resource_key(region, id))
    }

    /// Every PackagingGroup stored in `region`, in `Id` order.
    pub fn packaging_groups_in(&self, region: &str) -> impl Iterator<Item = &PackagingGroupRecord> {
        region_slice(&self.packaging_groups, region)
    }

    /// The PackagingConfiguration `id` names in `region`.
    pub fn packaging_configuration(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&PackagingConfigurationRecord> {
        self.packaging_configurations.get(&resource_key(region, id))
    }

    /// Whether `region` holds a PackagingConfiguration under `id`.
    pub fn has_packaging_configuration(&self, region: &str, id: &str) -> bool {
        self.packaging_configurations
            .contains_key(&resource_key(region, id))
    }

    /// Store `record` as the PackagingConfiguration `id` of `region`.
    pub fn insert_packaging_configuration(&mut self, record: PackagingConfigurationRecord) {
        let key = resource_key(&record.region, &record.id);
        self.packaging_configurations.insert(key, record);
    }

    /// Remove the PackagingConfiguration `id` names in `region`.
    pub fn remove_packaging_configuration(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<PackagingConfigurationRecord> {
        self.packaging_configurations.remove(&resource_key(region, id))
    }

    /// Every PackagingConfiguration stored in `region`, in `Id` order.
    pub fn packaging_configurations_in(
        &self,
        region: &str,
    ) -> impl Iterator<Item = &PackagingConfigurationRecord> {
        region_slice(&self.packaging_configurations, region)
    }

    /// The Asset `id` names in `region`.
    pub fn asset(&self, region: &str, id: &str) -> Option<&AssetRecord> {
        self.assets.get(&resource_key(region, id))
    }

    /// Whether `region` holds an Asset under `id`.
    pub fn has_asset(&self, region: &str, id: &str) -> bool {
        self.assets.contains_key(&resource_key(region, id))
    }

    /// Store `record` as the Asset `id` of `region`.
    pub fn insert_asset(&mut self, record: AssetRecord) {
        let key = resource_key(&record.region, &record.id);
        self.assets.insert(key, record);
    }

    /// Remove the Asset `id` names in `region`.
    pub fn remove_asset(&mut self, region: &str, id: &str) -> Option<AssetRecord> {
        self.assets.remove(&resource_key(region, id))
    }

    /// Every Asset stored in `region`, in `Id` order.
    pub fn assets_in(&self, region: &str) -> impl Iterator<Item = &AssetRecord> {
        region_slice(&self.assets, region)
    }

    /// Replace the tags stored for `arn`. An absent `Tags` member and an empty map
    /// are the same request: the resource carries no tags.
    pub fn set_tags(&mut self, arn: &str, tags: Option<&HashMap<String, String>>) {
        let stored: BTreeMap<String, String> = tags
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.tags_by_arn.insert(arn.to_string(), stored);
    }

    /// Forget the tags stored for `arn`, so a deleted resource leaves none behind.
    pub fn forget_tags(&mut self, arn: &str) {
        self.tags_by_arn.remove(arn);
    }

    /// The tags stored for `arn`, in the modelled `Tags` map shape. A resource with
    /// no tags reports an empty map, which is what it was created with.
    pub fn tags_of(&self, arn: &str) -> HashMap<String, String> {
        self.tags_by_arn
            .get(arn)
            .map(|tags| {
                tags.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<HashMap<String, String>>()
            })
            .unwrap_or_default()
    }

    /// How many assets are currently stored in `group_id` IN `region` —
    /// `ApproximateAssetCount`, counted rather than stored. An asset of the same
    /// group `Id` in another Region belongs to another group, so it is not counted.
    pub fn asset_count(&self, region: &str, group_id: &str) -> i32 {
        let count = self
            .assets_in(region)
            .filter(|asset| asset.packaging_group_id == group_id)
            .count();
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// How many packaging configurations are currently stored in `group_id` in
    /// `region`, which is also the length of the `EgressEndpoints` list every asset
    /// in that group derives. Bounded by
    /// [`MAX_PACKAGING_CONFIGURATIONS_PER_GROUP`].
    pub fn configuration_count(&self, region: &str, group_id: &str) -> usize {
        self.packaging_configurations_in(region)
            .filter(|cfg| cfg.packaging_group_id == group_id)
            .count()
    }

    /// One egress endpoint per packaging configuration attached to this asset's
    /// group, in configuration-`Id` order.
    ///
    /// Derived, never stored: a configuration added after the asset shows up here on
    /// the next read, which is how the real service behaves. Only the group and the
    /// configurations of the asset's OWN Region take part, since those are the only
    /// ones that exist where the asset does. `now` is the reading instant, so the
    /// packaging status the clock owes is materialised without writing anything.
    pub fn egress_endpoints(&self, asset: &AssetRecord, now: Instant) -> Vec<EgressEndpoint> {
        let Some(group) = self.packaging_group(&asset.region, &asset.packaging_group_id) else {
            return Vec::new();
        };
        self.packaging_configurations_in(&asset.region)
            .filter(|cfg| cfg.packaging_group_id == asset.packaging_group_id)
            .map(|cfg| EgressEndpoint {
                packaging_configuration_id: Some(cfg.id.clone()),
                status: Some(egress_status(asset, cfg, now).to_string()),
                url: Some(playback_url(&group.domain_name, asset, cfg)),
            })
            .collect()
    }
}

/// The processing status of one asset packaged by one configuration.
///
/// Packaging cannot start before BOTH the asset and the configuration exist, so the
/// job is due [`INGEST_PLAYABLE_AFTER`] past the later of the two.
fn egress_status(
    asset: &AssetRecord,
    cfg: &PackagingConfigurationRecord,
    now: Instant,
) -> &'static str {
    let started = asset.created_instant.max(cfg.created_instant);
    if now.saturating_duration_since(started) >= INGEST_PLAYABLE_AFTER {
        EGRESS_STATUS_PLAYABLE
    } else {
        EGRESS_STATUS_QUEUED
    }
}

/// The parent-manifest URL one packaging configuration exposes one asset at.
///
/// It is rooted at the group's `DomainName` (which carries its own scheme — see
/// [`packaging_group_domain_name`]), the path segment is derived from the (group,
/// asset, configuration) triple so it is stable across reads and distinct per
/// endpoint, and the file name is the configuration's manifest name plus the
/// extension that configuration's packaging format uses.
pub fn playback_url(
    domain_name: &str,
    asset: &AssetRecord,
    cfg: &PackagingConfigurationRecord,
) -> String {
    let path = digest_hex(
        &format!(
            "{}/{}/{}",
            asset.packaging_group_id, asset.id, cfg.id
        ),
        32,
    );
    let (manifest_name, suffix) = manifest_name_and_suffix(cfg);
    format!("{domain_name}/out/v1/{path}/{manifest_name}.{suffix}")
}

/// The manifest name and file-name suffix a packaging configuration's format uses.
///
/// The suffix follows the packaging format, which is the package block the
/// configuration was created with: HLS and CMAF expose an HLS parent manifest
/// (`.m3u8`), DASH an MPD (`.mpd`), and Microsoft Smooth Streaming a server
/// manifest (`.ism/Manifest`). The name is the manifest block's own
/// `ManifestName`, or [`DEFAULT_MANIFEST_NAME`] when it names none.
fn manifest_name_and_suffix(cfg: &PackagingConfigurationRecord) -> (String, &'static str) {
    if let Some(pkg) = &cfg.hls_package {
        let name = pkg.hls_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "m3u8");
    }
    if let Some(pkg) = &cfg.cmaf_package {
        let name = pkg.hls_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "m3u8");
    }
    if let Some(pkg) = &cfg.dash_package {
        let name = pkg.dash_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "mpd");
    }
    if let Some(pkg) = &cfg.mss_package {
        let name = pkg.mss_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "ism/Manifest");
    }
    // Unreachable through the API: `CreatePackagingConfiguration` refuses a request
    // that carries no package block. Kept total rather than panicking in a handler.
    (DEFAULT_MANIFEST_NAME.to_string(), "m3u8")
}

/// A manifest block's `ManifestName`, or [`DEFAULT_MANIFEST_NAME`].
fn manifest_name(name: Option<&str>) -> String {
    name.filter(|n| !n.is_empty())
        .unwrap_or(DEFAULT_MANIFEST_NAME)
        .to_string()
}

/// The package block a `CreatePackagingConfiguration` request stores: the one the
/// caller supplied, with every member the service materialises a default for filled
/// in.
///
/// The service does not store a packaging configuration "as sent": it resolves the
/// optional settings of the chosen packaging format to the values it will package
/// with, and every later read of the configuration reports those resolved values. The
/// defaults are therefore applied ONCE, here, on the way into state — so the create
/// response, a describe and a list all project the identical block.
///
/// `SegmentDurationSeconds`, the encryption blocks, `StreamSelection`,
/// `ManifestName` and `DashManifest$ManifestLayout` are deliberately left as sent:
/// the recorded responses of this service report them only when the request
/// supplied them, so materialising a value for them would invent one the service
/// does not report.
pub fn hls_package_with_defaults(mut package: HlsPackage) -> HlsPackage {
    package.include_dvb_subtitles.get_or_insert(false);
    package.use_audio_rendition_group.get_or_insert(false);
    for manifest in &mut package.hls_manifests {
        hls_manifest_apply_defaults(manifest);
    }
    package
}

/// See [`hls_package_with_defaults`]: the same resolution for a CMAF package, whose
/// manifests are the same `HlsManifest` shape.
pub fn cmaf_package_with_defaults(mut package: CmafPackage) -> CmafPackage {
    package
        .include_encoder_configuration_in_segments
        .get_or_insert(false);
    for manifest in &mut package.hls_manifests {
        hls_manifest_apply_defaults(manifest);
    }
    package
}

/// See [`hls_package_with_defaults`]: the same resolution for a DASH package.
pub fn dash_package_with_defaults(mut package: DashPackage) -> DashPackage {
    package
        .include_encoder_configuration_in_segments
        .get_or_insert(false);
    package.include_iframe_only_stream.get_or_insert(false);
    // `PeriodTriggers` is "a list of triggers that controls when the outgoing
    // [MPD] will be partitioned into multiple periods. If empty, the content will
    // not be partitioned" — so the resolved value of an unset list is the empty one,
    // reported rather than omitted.
    package.period_triggers.get_or_insert_with(Vec::new);
    package
        .segment_template_format
        .get_or_insert_with(|| DEFAULT_SEGMENT_TEMPLATE_FORMAT.to_string());
    for manifest in &mut package.dash_manifests {
        dash_manifest_apply_defaults(manifest);
    }
    package
}

/// See [`hls_package_with_defaults`]: a Microsoft Smooth package carries no member
/// the service materialises a default for — `MssPackage` and `MssManifest` declare
/// only `SegmentDurationSeconds`, `Encryption`, `ManifestName` and
/// `StreamSelection`, each reported only as sent — so the stored block is the block
/// supplied. The function exists so every packaging format is resolved through one
/// named seam rather than three plus a silent case.
pub fn mss_package_with_defaults(package: MssPackage) -> MssPackage {
    package
}

/// The defaults the service materialises on one HLS manifest configuration (used by
/// both the HLS and the CMAF packaging formats).
fn hls_manifest_apply_defaults(manifest: &mut HlsManifest) {
    manifest
        .ad_markers
        .get_or_insert_with(|| DEFAULT_AD_MARKERS.to_string());
    manifest.include_iframe_only_stream.get_or_insert(false);
    manifest
        .program_date_time_interval_seconds
        .get_or_insert(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS);
    manifest.repeat_ext_x_key.get_or_insert(false);
}

/// The defaults the service materialises on one DASH manifest configuration.
fn dash_manifest_apply_defaults(manifest: &mut DashManifest) {
    manifest
        .min_buffer_time_seconds
        .get_or_insert(DEFAULT_MIN_BUFFER_TIME_SECONDS);
    manifest
        .profile
        .get_or_insert_with(|| DEFAULT_DASH_PROFILE.to_string());
    manifest
        .scte_markers_source
        .get_or_insert_with(|| DEFAULT_SCTE_MARKERS_SOURCE.to_string());
}

/// The scheme-and-separator prefix a packaging group's `DomainName` carries.
///
/// `DomainName` is documented as "The fully qualified domain name for Assets in the
/// PackagingGroup", but what this service puts on the wire for it is the egress
/// ORIGIN a player fetches from — scheme included — as the recorded create and
/// describe responses of this service show, and as every `EgressEndpoint$Url` built
/// from it needs. Playback is TLS-only, so the prefix is always this one. It carries
/// the `://` separator, so a caller building a URL beneath the origin (see
/// [`playback_url`]) appends a path and never re-adds a scheme.
pub const EGRESS_ORIGIN_PREFIX: &str = "https://";

/// The egress origin assets in a packaging group are played back from, which is what
/// this service reports as the group's `DomainName` — an ORIGIN, not a bare hostname,
/// so it is used as the root of a URL rather than as a host.
///
/// Shaped like the real one: [`EGRESS_ORIGIN_PREFIX`] followed by an opaque per-group
/// host label, the `egress` data-plane label, this service's own name, the region and
/// `amazonaws.com`. The label is DERIVED from the group's identity, never minted per
/// call, so the value is stable across reads and distinct per group.
pub fn packaging_group_domain_name(group_id: &str, region: &str) -> String {
    let label = digest_hex(
        &format!("{}/{region}/{group_id}", engine::emulator_account_id()),
        16,
    );
    format!("{EGRESS_ORIGIN_PREFIX}{label}.egress.{ARN_NAMESPACE}.{region}.amazonaws.com")
}

/// The first `len` hex characters of the SHA-256 of `input`.
fn digest_hex(input: &str, len: usize) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(input.as_bytes());
    let mut hex = hex::encode(digest);
    hex.truncate(len);
    hex
}

/// The ARN of a resource of `kind` (the ARN's resource-type segment) named `id`.
pub fn resource_arn(kind: &str, region: &str, id: &str) -> String {
    engine::emulator_arn(ARN_NAMESPACE, region, &format!("{kind}/{id}"))
}

/// The region this request is served in: the SigV4 credential scope's region, or
/// [`DEFAULT_REGION`] when the request carries no scope.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The current wall-clock instant as an ISO-8601 `YYYY-MM-DDThh:mm:ssZ` string, the
/// form this model's `CreatedAt` members (plain `__string`) carry.
pub fn now_iso8601() -> String {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = i64::try_from(unix.as_secs()).unwrap_or(i64::MAX);
    aws_smithy_types::DateTime::from_secs(secs)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Encode a pagination cursor for `listing`: the id the page ended on, which the
/// next page of THAT listing resumes strictly after.
///
/// Opaque to the caller, and carrying a checksum over both the listing and the id it
/// names, so a cursor that is merely malformed — or one another listing of this
/// service issued — is REFUSED rather than silently resumed from. The checksum is an
/// unkeyed digest over public text, so it detects those two cases rather than
/// resisting a deliberately forged cursor; a resume point only picks where in this
/// same store the listing starts, so there is nothing a forged one reaches that the
/// unpaged listing does not already report. Decoded only by [`decode_page_token`].
pub fn encode_page_token(listing: &str, last_id: &str) -> String {
    format!(
        "{}{}",
        page_token_check(listing, last_id),
        hex::encode(last_id.as_bytes())
    )
}

/// Decode a cursor produced by [`encode_page_token`] for `listing`. `None` for
/// anything that is not a cursor this service issued for that listing — a malformed
/// one, one whose checksum does not match the id it carries, or one issued by a
/// different listing — which the caller answers as an invalid parameter.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    // Both halves are ASCII hex when this service issued the token; the ASCII check
    // is also what makes the byte split below a safe char boundary.
    if !token.is_ascii() || token.len() < PAGE_TOKEN_CHECK_LEN {
        return None;
    }
    let (check, payload) = token.split_at(PAGE_TOKEN_CHECK_LEN);
    let last_id = String::from_utf8(hex::decode(payload).ok()?).ok()?;
    (page_token_check(listing, &last_id) == check).then_some(last_id)
}

/// The checksum prefix a cursor over `last_id` in `listing` carries.
fn page_token_check(listing: &str, last_id: &str) -> String {
    digest_hex(&format!("{listing}-cursor/{last_id}"), PAGE_TOKEN_CHECK_LEN)
}

/// The id a `NextToken` names as the resume point of `listing`, or the refusal a
/// cursor this listing did not issue gets.
///
/// `None` for a request that carries no `NextToken`: the listing starts at its
/// first record.
pub fn resume_after(
    listing: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, crate::scaffolded::UnprocessableEntity> {
    match next_token {
        None => Ok(None),
        Some(token) => decode_page_token(listing, token).map(Some).ok_or_else(|| {
            crate::scaffolded::UnprocessableEntity::member(
                "nextToken",
                "is not a cursor this service issued",
            )
        }),
    }
}

/// How many records one page of a listing carries, from the request's `MaxResults`.
///
/// `MaxResults` is modelled `range(min: 1, max: 1000)`, so the boundary validator
/// already refuses anything outside it; this enforces the same bound itself rather
/// than depending on that, and a request that names no `MaxResults` gets the largest
/// page the model allows.
pub fn page_size(
    max_results: Option<i32>,
) -> Result<usize, crate::scaffolded::UnprocessableEntity> {
    match max_results {
        None => Ok(MAX_PAGE_SIZE),
        Some(max) => match usize::try_from(max) {
            Ok(max) if (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&max) => Ok(max),
            _ => Err(crate::scaffolded::UnprocessableEntity::member(
                "maxResults",
                format!("must be between {MIN_PAGE_SIZE} and {MAX_PAGE_SIZE}"),
            )),
        },
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediapackagevodMediapackagevodState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediapackagevodMediapackagevodState>)
}
