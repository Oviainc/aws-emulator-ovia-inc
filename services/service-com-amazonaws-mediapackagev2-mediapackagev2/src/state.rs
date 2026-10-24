// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediapackagev2-mediapackagev2 service.
//!
//! MediaPackage v2 is a three-level containment hierarchy — channel group -> channel
//! -> origin endpoint — so the store nests the same way: a channel cannot exist
//! without its group, and deleting a parent that still has children is refused. Every
//! derived value a response carries (ARN, egress domain, ingest URL, manifest URL,
//! ETag, timestamps) is computed by the call that WRITES the record - a create, or an
//! update - and persisted, so every later read reproduces it byte for byte and no
//! response computes a value of its own while being serialized. A resource's `ETag` is a
//! function of its content and of the commit count the store holds it at (see
//! [`ChannelGroup::version`]), so a write that changed the resource rotates it.
//!
//! MediaPackage v2 is a REGIONAL control plane, so the Region a request is served in is
//! part of the identity of every resource it touches: a channel group is stored under
//! its Region AND its name ([`ChannelGroupKey`]), the same name in two Regions is two
//! independent groups with their own channels, endpoints and harvest jobs, and a request
//! served in one Region resolves, lists, mutates and deletes only what was created in
//! it. Every accessor on the store therefore takes the request's Region
//! ([`request_region`]) beside the name.
//!
//! The maps are [`BTreeMap`]s: `ListOriginEndpoints` pages in name order, and an
//! ordered map makes that order a property of the store rather than of each read.
//!
//! The same holds for the members the SERVICE applies rather than the caller: a channel's
//! input type and output locking mode, its switching and CMSD-header blocks, an origin
//! endpoint's segment settings, startover window and URI/stream-name modes, and every
//! manifest configuration's window, URI path type and child-manifest URL encoding are
//! materialised ONCE, at create time, from the submitted value or the service's default,
//! and stored — so the create response, a later read and the list projection all report
//! the same concrete value.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aws_smithy_schema_server::{Violation, ViolationKind};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    CdnAuthConfiguration, ConflictException, CreateDashManifestConfiguration,
    CreateHlsManifestConfiguration, CreateLowLatencyHlsManifestConfiguration,
    CreateMssManifestConfiguration, Destination, ForceEndpointErrorConfiguration,
    GetDashManifestConfiguration, GetHlsManifestConfiguration,
    GetLowLatencyHlsManifestConfiguration, GetMssManifestConfiguration, HarvestedManifests,
    HarvesterScheduleConfiguration, IngestEndpoint, InputSwitchConfiguration,
    MultiviewConfiguration, OutputHeaderConfiguration, ResourceNotFoundException, Segment,
    ServiceQuotaExceededException, ValidationException,
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

/// The `OutputLockingMode` a channel is created with when the request omits it and the
/// channel's input type may carry the member at all: the model's `OutputLockingMode` doc
/// says "If you don't specify a value, the default is EPOCH_LOCKED".
pub const DEFAULT_OUTPUT_LOCKING_MODE: &str = "EPOCH_LOCKED";

/// How many ingest endpoints a channel is given. MediaPackage v2 hands every channel a
/// redundant pair, which is why `IngestEndpoints` is a list rather than a single URL.
const INGEST_ENDPOINTS_PER_CHANNEL: u8 = 2;

/// The most channel groups this emulator holds at once.
///
/// `channel_groups` grows by one record per `CreateChannelGroup` request and shrinks only
/// on an explicit delete, so without a ceiling its entry COUNT is set by how many
/// requests a caller makes. AWS bounds the same thing with a per-account, per-Region
/// service quota and refuses a create past it, so this ceiling is applied per Region too
/// ([`ComAmazonawsMediapackagev2Mediapackagev2State::channel_group_count`]); that quota's value is adjustable and is
/// not in the model, so this number is the EMULATOR's own bound rather than a claim about
/// AWS's, while the refusal it produces is the error the model declares for exactly this
/// case: `ServiceQuotaExceededException` (HTTP 402, "The request would cause a service
/// quota to be exceeded.").
pub const MAX_CHANNEL_GROUPS: usize = 100;

/// The most channel groups this emulator holds at once across EVERY Region together.
///
/// [`MAX_CHANNEL_GROUPS`] bounds one Region's share, and the Region a create is served in
/// is read from that request's own credential scope — so a per-Region ceiling alone would
/// leave the store growing by one record per request again, for a caller that names a
/// fresh Region each time. This is the bound on the whole map, so what the emulator holds
/// is set by this number rather than by how many requests arrive, and it is the same kind
/// of store-wide ceiling [`MAX_INGESTED_BYTES`] is for ingested content. It is a small
/// multiple of the per-Region bound, so a handful of Regions each still reach their own
/// ceiling, and a create past it is refused with the SAME error the per-Region ceiling
/// produces (`ServiceQuotaExceededException`): both are the service's quota refusal.
pub const MAX_CHANNEL_GROUPS_TOTAL: usize = 500;

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
/// on the manifest lists, so the NUMBER here is the emulator's own choice. The REFUSAL
/// is not: the model's `ValidationExceptionType` declares `NUM_MANIFESTS_HIGH` beside
/// `NUM_MANIFESTS_LOW`, i.e. a manifest count outside the service's bounds is an input
/// failure (`ValidationException`), not a quota error.
pub const MAX_MANIFESTS_PER_ORIGIN_ENDPOINT: usize = 100;

/// The fewest manifest configurations an origin endpoint may carry, counted across the
/// four families together.
///
/// Like the ceiling above, this bound is reported as a `ValidationException` carrying the
/// discriminator for the side of the bound that was crossed (`NUM_MANIFESTS_LOW` here),
/// and an endpoint with no manifest can serve nothing at all.
pub const MIN_MANIFESTS_PER_ORIGIN_ENDPOINT: usize = 1;

/// The most harvest jobs one origin endpoint holds at once.
///
/// `harvest_jobs` grows by one record per `CreateHarvestJob` and never shrinks (a
/// finished job stays readable), so without a ceiling its entry COUNT is set by how many
/// requests a caller makes. AWS bounds the same thing with an adjustable service quota
/// that is not in the model, so this number is the EMULATOR's own bound, refused with the
/// `ServiceQuotaExceededException` the model declares for exactly this case.
pub const MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT: usize = 100;

/// The most ingested objects one channel's data plane retains, and the most bytes one of
/// them may carry.
///
/// An ingest `PUT` is caller-driven, so both the object COUNT and each object's SIZE are
/// bounded here rather than by how much an encoder pushes. Past either bound the write is
/// refused rather than recorded.
pub const MAX_INGESTED_OBJECTS_PER_CHANNEL: usize = 64;
/// See [`MAX_INGESTED_OBJECTS_PER_CHANNEL`].
pub const MAX_INGESTED_OBJECT_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes the ingest data plane retains across EVERY channel together.
///
/// The two bounds above are per channel, and the channel count is itself caller-driven
/// ([`MAX_CHANNEL_GROUPS`] x [`MAX_CHANNELS_PER_GROUP`]), so their product is not a bound
/// on the store: this is. It is sized for a developer machine rather than derived from
/// the per-channel ceilings, and a `PUT` that would push the retained total past it is
/// refused rather than recorded — nothing is evicted, because an encoder's content is not
/// this emulator's to drop silently.
pub const MAX_INGESTED_BYTES: usize = 64 * 1024 * 1024;

/// What [`ComAmazonawsMediapackagev2Mediapackagev2State::record_ingested_object`] did, so
/// the data plane answers each outcome without re-deriving it — and cannot reach the
/// stored case without having gone through both bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome {
    /// The object was recorded against the channel.
    Recorded,
    /// The group or the channel the path names does not resolve; nothing was written.
    NoSuchChannel,
    /// The channel already holds [`MAX_INGESTED_OBJECTS_PER_CHANNEL`] distinct objects.
    ObjectCountReached,
    /// Recording this object would push the whole store past [`MAX_INGESTED_BYTES`].
    StoreFull,
}

// ---------------------------------------------------------------------------
// Rejection bodies this service's shared refusal layer does not already own.
//
// The not-found, duplicate-create, children-attached and policy-not-found sentences all
// come from `ResourceKind::not_found`, `children_attached`, `resource_already_exists` and
// `policy_not_found` further down this file, so no handler words one of its own. What is
// declared here is only what those do not cover: the conditional-update conflict, the
// `HarvestJobStatus` values and the `ValidationExceptionType` values the harvest-job and
// pagination rules answer with.
// ---------------------------------------------------------------------------

/// The one sentence for a conditional update whose `x-amzn-update-if-match` entity tag is
/// not the committed one — the loser of a concurrent update.
///
/// This is the text the service itself answers with: fixed per rejection class, naming no
/// resource and carrying nothing from the request, so the sentence is part of the contract
/// rather than a per-run value (the recorded behavioral corpus asserts it for the channel
/// path, and every operation of this class answers with the same one). Note the
/// typographic apostrophes.
pub const CONFLICTING_OPERATION_MESSAGE: &str = "You\u{2019}ve specified an entity tag with \
    your request that does not match the resource\u{2019}s current entity tag. This can \
    occur if the resource has been updated by another request between the time you \
    retrieved the ETag and when you made your update request";

/// `HarvestJobStatus` values, from the model's `HarvestJobStatus` enum
/// (`HARVEST_JOB_STATUS_VALUES` in `ops::types`).
pub const HARVEST_JOB_QUEUED: &str = "QUEUED";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_CANCELLED: &str = "CANCELLED";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_FAILED: &str = "FAILED";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_COMPLETED: &str = "COMPLETED";

/// `ValidationExceptionType` values this crate answers with, from the model's
/// `ValidationExceptionType` enum (`VALIDATION_EXCEPTION_TYPE_VALUES` in `ops::types`).
pub const VALIDATION_END_TIME_EARLIER_THAN_START_TIME: &str = "END_TIME_EARLIER_THAN_START_TIME";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION: &str =
    "HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_INVALID_HARVEST_JOB_DURATION: &str = "INVALID_HARVEST_JOB_DURATION";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT: &str =
    "HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_INVALID_PAGINATION_TOKEN: &str = "INVALID_PAGINATION_TOKEN";

/// The 409 for a conditional update that lost the race: the entity tag the caller
/// presented is no longer the committed one.
pub fn conflicting_operation() -> ConflictException {
    ConflictException {
        message: Some(CONFLICTING_OPERATION_MESSAGE.to_string()),
        conflict_exception_type: Some("CONFLICTING_OPERATION".to_string()),
    }
}

// ---------------------------------------------------------------------------
// The tagging front end's own rejections.
//
// `TagResource` / `UntagResource` / `ListTagsForResource` model only
// `ValidationException`, but MediaPackage answers them from a tagging front end that
// refuses a malformed `ResourceArn` as `BadRequestException` and an absent resource as
// `NotFoundException` — the codes the recorded behaviour shows, and the codes a client
// branches on. Both are 4xx client errors: a caller that passed a bad ARN must be told
// to fix it, never handed a retryable server fault.
// ---------------------------------------------------------------------------

/// The wire error code for a `ResourceArn` that is not a syntactically valid ARN.
pub const INVALID_ARN_CODE: &str = "BadRequestException";

/// The sentence that accompanies [`INVALID_ARN_CODE`] — a property of the ARN's SYNTAX,
/// so it names nothing from the request.
pub const INVALID_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The wire error code for a well-formed `ResourceArn` naming a resource that is absent.
pub const RESOURCE_NOT_FOUND_CODE: &str = "NotFoundException";

/// The sentence that accompanies [`RESOURCE_NOT_FOUND_CODE`].
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str =
    "The resource arn you provided does not name a resource that exists";

/// The most tags one resource carries, and so the most entries a resource's tag map can
/// ever hold.
///
/// A tag map grows by one entry per distinct caller-chosen KEY and shrinks only on an
/// explicit `UntagResource`, so without a ceiling its size is set by how many requests a
/// caller makes. AWS bounds it with the tag quota it publishes for every taggable
/// resource — 50 tags per resource — and refuses a request past it, which is what this
/// constant is. The model puts no `@length` on `TagMap`, so the boundary validator does
/// not enforce it and the handlers do.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most characters a tag KEY may carry — AWS's published tag-key limit of 128
/// characters, which the model's `TagMap` does not express either.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The most characters a tag VALUE may carry — AWS's published tag-value limit of 256
/// characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Refuse `tags` when it is past the tag bounds above, reporting the modelled rejection.
///
/// `tags` is the tag map the resource would be left HOLDING, not just the one the request
/// carried, so a `TagResource` that merges into an existing map is judged on the merged
/// result. Called before anything is written, so a refused request leaves the resource's
/// tags exactly as they were.
///
/// `ValidationExceptionType` enumerates no tag-quota member
/// (`VALIDATION_EXCEPTION_TYPE_VALUES` in `ops::types`), so the discriminator is left
/// unset rather than claiming a reason the service does not name for this case; the
/// message states which bound was exceeded and the bound's value.
pub fn tag_bounds_rejection(tags: &HashMap<String, String>) -> Option<ValidationException> {
    let refuse = |message: String| {
        Some(ValidationException {
            message: Some(message),
            validation_exception_type: None,
        })
    };
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return refuse(format!(
            "A resource carries at most {MAX_TAGS_PER_RESOURCE} tags; the request would \
             leave it with {}",
            tags.len()
        ));
    }
    for (key, value) in tags {
        if key.chars().count() > MAX_TAG_KEY_CHARS {
            return refuse(format!(
                "A tag key carries at most {MAX_TAG_KEY_CHARS} characters; one in the \
                 request carries {}",
                key.chars().count()
            ));
        }
        if value.chars().count() > MAX_TAG_VALUE_CHARS {
            return refuse(format!(
                "A tag value carries at most {MAX_TAG_VALUE_CHARS} characters; one in the \
                 request carries {}",
                value.chars().count()
            ));
        }
    }
    None
}

/// The tag-key prefix AWS reserves for its own system tags.
///
/// AWS's tagging rules, applied uniformly by every taggable service: "Do not use the
/// `aws:` prefix in your tag names or values because it is reserved for AWS use. You
/// can't edit or delete tag names or values with this prefix." A caller may therefore
/// neither CREATE, REPLACE nor DELETE a tag whose key is in this namespace — the service
/// owns it — so every request that names such a key is refused before anything is
/// written. The model expresses nothing about it (`TagKey` is a bare string with no
/// `@pattern`), so the handlers enforce it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// Compared WITHOUT regard to ASCII case: the namespace belongs to the service, and a
/// key spelled `AWS:` or `Aws:` names the same system namespace as `aws:`, so admitting
/// one spelling would leave the rule bypassable. A key shorter than the prefix, or one
/// whose first bytes are not a character boundary, is not in the namespace.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The requirement a reserved-key refusal states, worded as this service words every
/// other member requirement ([`constraint_detail`]): the rule, then the sentence telling
/// the caller to check what it sent.
///
/// It states the RULE and names no key from the request: the refusal is about the
/// namespace, not about which key happened to land in it, and a tag map carries no
/// ordering a message could pick a representative key from.
pub fn reserved_tag_key_requirement() -> String {
    format!(
        "Member must not contain a tag key beginning with {RESERVED_TAG_KEY_PREFIX}, a \
         prefix reserved for Amazon Web Services use. {CHECK_THE_VALUE}"
    )
}

/// Refuse a request whose tag keys reach into the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace, reporting the modelled rejection.
///
/// `keys` are the keys the REQUEST named — the keys it would create, replace or delete —
/// rather than the keys the resource ends up holding: a stored map can never hold a
/// reserved key (this gate is what keeps it out), and it is the request that is refused.
/// `member_path` is the wire name of the member they arrived in, which differs per
/// operation (`tags` on `TagResource`, `CreateChannelGroup` and `CreateChannel` through
/// `@jsonName`, `Tags` on `CreateOriginEndpoint` and `CreateHarvestJob`, `tagKeys` on
/// `UntagResource`), so the sentence points the caller at the member it sent.
///
/// Called before anything is written, so a refused request leaves the resource exactly
/// as it was. The discriminator is `MEMBER_INVALID`: `ValidationExceptionType` names no
/// reserved-tag reason, and `MEMBER_INVALID` is the value the enum carries for a member
/// that is wrong in a way none of its other values names.
pub fn reserved_tag_key_rejection<'a, I>(
    member_path: &str,
    keys: I,
) -> Option<ValidationException>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().any(is_reserved_tag_key).then(|| {
        validation_failed(
            member_path,
            &reserved_tag_key_requirement(),
            "MEMBER_INVALID",
        )
    })
}

/// The tag map a resource holding `current` would be left with after `submitted` is
/// merged into it, which is what [`tag_bounds_rejection`] has to judge.
///
/// Assigning a tag whose key is already there REPLACES that tag's value rather than
/// adding an entry, so the merged map is the union of the keys.
pub fn merged_tags(
    current: &HashMap<String, String>,
    submitted: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = current.clone();
    merged.extend(
        submitted
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    merged
}

/// The key a channel group is stored under: the AWS Region the group was created in,
/// then the group's own name.
///
/// The name is the group's primary identifier, and the model says it "must be unique for
/// your account in the AWS Region" — so the Region is part of the identity, not a
/// decoration on the record: two Regions holding the same name hold two independent
/// groups, each with its own channels, origin endpoints and harvest jobs. A
/// [`BTreeMap`] keyed this way orders by Region first, so one Region's groups are a
/// contiguous range and a listing pages them in name order (see [`region_slice`]).
pub type ChannelGroupKey = (String, String);

/// The key the group `name` is stored under when the request that created it was served
/// in `region`.
pub fn channel_group_key(region: &str, name: &str) -> ChannelGroupKey {
    (region.to_string(), name.to_string())
}

/// The records of ONE Region, as `(name, record)` pairs in name order.
///
/// The scan starts at the Region's smallest possible key and stops at the first key
/// naming another Region, so nothing another Region holds is ever reported.
fn region_slice<'map, V>(
    map: &'map BTreeMap<ChannelGroupKey, V>,
    region: &str,
) -> impl Iterator<Item = (&'map String, &'map V)> {
    let region = region.to_string();
    map.range((region.clone(), String::new())..)
        .take_while(move |((stored, _), _)| *stored == region)
        .map(|((_, name), record)| (name, record))
}

/// Per-service state for com-amazonaws-mediapackagev2-mediapackagev2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackagev2Mediapackagev2State {
    /// Every channel group, keyed by [`ChannelGroupKey`] — its Region and its name.
    ///
    /// PRIVATE, and reached only through the accessors below, so no operation can resolve
    /// a name without naming the Region its request was served in.
    channel_groups: BTreeMap<ChannelGroupKey, ChannelGroup>,
}

impl ComAmazonawsMediapackagev2Mediapackagev2State {
    /// The channel group `name` names IN `region`, or `None` for a name only another
    /// Region holds: that group does not exist here.
    pub fn channel_group(&self, region: &str, name: &str) -> Option<&ChannelGroup> {
        self.channel_groups.get(&channel_group_key(region, name))
    }

    /// The channel group `name` names in `region`, for an operation that writes to the
    /// group or to a resource beneath it.
    pub fn channel_group_mut(&mut self, region: &str, name: &str) -> Option<&mut ChannelGroup> {
        self.channel_groups
            .get_mut(&channel_group_key(region, name))
    }

    /// Whether `region` already holds a channel group under `name`.
    pub fn has_channel_group(&self, region: &str, name: &str) -> bool {
        self.channel_groups
            .contains_key(&channel_group_key(region, name))
    }

    /// Store `group` under its own Region and name.
    pub fn insert_channel_group(&mut self, group: ChannelGroup) {
        let key = channel_group_key(&group.region, &group.name);
        self.channel_groups.insert(key, group);
    }

    /// Remove the channel group `name` names in `region`, reporting what was stored.
    pub fn remove_channel_group(&mut self, region: &str, name: &str) -> Option<ChannelGroup> {
        self.channel_groups.remove(&channel_group_key(region, name))
    }

    /// Every channel group stored in `region`, as `(name, record)` pairs in name order —
    /// what `ListChannelGroups` pages over.
    pub fn channel_groups_in(
        &self,
        region: &str,
    ) -> impl Iterator<Item = (&String, &ChannelGroup)> {
        region_slice(&self.channel_groups, region)
    }

    /// How many channel groups `region` holds — the count the per-Region ceiling is
    /// judged against, since a group of another Region takes nothing from this one.
    pub fn channel_group_count(&self, region: &str) -> usize {
        self.channel_groups_in(region).count()
    }

    /// How many channel groups this emulator holds across every Region — the count the
    /// store-wide ceiling ([`MAX_CHANNEL_GROUPS_TOTAL`]) is judged against.
    pub fn channel_group_total(&self) -> usize {
        self.channel_groups.len()
    }

    /// The tags of the resource `target` names, or `None` when this emulator holds no
    /// such resource in `region`.
    ///
    /// Tags live on the resource itself, so this walks the one containment hierarchy the
    /// store keeps rather than keeping a second index beside it.
    pub fn tags_of(
        &self,
        region: &str,
        target: &TagTarget,
    ) -> Option<&HashMap<String, String>> {
        let group = self.resolved_group(region, target)?;
        match &target.resource {
            TaggedResource::ChannelGroup { .. } => Some(&group.tags),
            TaggedResource::Channel { channel, .. } => Some(&group.channels.get(channel)?.tags),
            TaggedResource::OriginEndpoint {
                channel, endpoint, ..
            } => Some(
                &group
                    .channels
                    .get(channel)?
                    .origin_endpoints
                    .get(endpoint)?
                    .tags,
            ),
        }
    }

    /// The channel group a tagging ARN reaches from a request served in `region`.
    ///
    /// `None` when the ARN names another Region: the resource it names lives in that
    /// Region's namespace, so from this endpoint there is nothing of that name to tag,
    /// report or untag.
    fn resolved_group(&self, region: &str, target: &TagTarget) -> Option<&ChannelGroup> {
        (target.region == region).then_some(())?;
        self.channel_group(region, target.resource.group())
    }

    /// Apply `mutate` to the tags of the resource `target` names and COMMIT the change
    /// (new `ModifiedAt`, new entity tag), reporting whether the resource was there.
    ///
    /// `false` means nothing was written: the resource the ARN names does not exist in
    /// `region` (see [`Self::resolved_group`]).
    pub fn with_tags_mut(
        &mut self,
        region: &str,
        target: &TagTarget,
        at: ::aws_smithy_types::DateTime,
        mutate: impl FnOnce(&mut HashMap<String, String>),
    ) -> bool {
        if target.region != region {
            return false;
        }
        let Some(group) = self.channel_group_mut(region, target.resource.group()) else {
            return false;
        };
        match &target.resource {
            TaggedResource::ChannelGroup { .. } => {
                mutate(&mut group.tags);
                group.commit(at);
                true
            }
            TaggedResource::Channel { channel, .. } => {
                let Some(channel) = group.channels.get_mut(channel) else {
                    return false;
                };
                mutate(&mut channel.tags);
                channel.commit(at);
                true
            }
            TaggedResource::OriginEndpoint {
                channel, endpoint, ..
            } => {
                let Some(endpoint) = group
                    .channels
                    .get_mut(channel)
                    .and_then(|channel| channel.origin_endpoints.get_mut(endpoint))
                else {
                    return false;
                };
                mutate(&mut endpoint.tags);
                endpoint.commit(at);
                true
            }
        }
    }

    /// The origin endpoint an egress request to `region`'s data plane addresses, or
    /// `None` when any level of the path does not resolve there.
    pub fn endpoint_at(&self, region: &str, target: &EgressTarget) -> Option<&OriginEndpoint> {
        self.channel_group(region, &target.group)?
            .channels
            .get(&target.channel)?
            .origin_endpoints
            .get(&target.endpoint)
    }

    /// The channel an ingest request to `region`'s data plane addresses, or `None` when
    /// the group or the channel does not resolve there.
    pub fn channel_at_mut(
        &mut self,
        region: &str,
        target: &IngestTarget,
    ) -> Option<&mut Channel> {
        self.channel_group_mut(region, &target.group)?
            .channels
            .get_mut(&target.channel)
    }

    /// How many bytes the ingest data plane is retaining across every channel.
    ///
    /// Summed from the store rather than tracked in a counter beside it: a counter can
    /// drift from the map it describes (a channel delete drops a whole map of objects at
    /// once), and this walk is bounded by the declared ceilings — at most
    /// [`MAX_CHANNEL_GROUPS`] x [`MAX_CHANNELS_PER_GROUP`] x
    /// [`MAX_INGESTED_OBJECTS_PER_CHANNEL`] lengths to add up — so it cannot be made
    /// expensive by a caller either.
    pub fn retained_ingested_bytes(&self) -> usize {
        self.channel_groups
            .values()
            .flat_map(|group| group.channels.values())
            .flat_map(|channel| channel.ingested_objects.values())
            .map(Vec::len)
            .sum()
    }

    /// Record one ingested object against the channel the path names, reporting what
    /// happened.
    ///
    /// Both store bounds are judged HERE, under the caller's single hold of the store
    /// lock, so the decision and the write cannot be interleaved with another push: the
    /// per-channel object count ([`MAX_INGESTED_OBJECTS_PER_CHANNEL`]) and the store-wide
    /// retained bytes ([`MAX_INGESTED_BYTES`]), counting the bytes a REPLACED object
    /// gives back. Nothing is written unless the answer is [`IngestOutcome::Recorded`].
    ///
    /// `body` is already clamped to [`MAX_INGESTED_OBJECT_BYTES`] by the caller, which is
    /// what keeps a single push from being read into memory unbounded.
    pub fn record_ingested_object(
        &mut self,
        region: &str,
        target: &IngestTarget,
        body: Vec<u8>,
    ) -> IngestOutcome {
        let retained = self.retained_ingested_bytes();
        let Some(channel) = self.channel_at_mut(region, target) else {
            return IngestOutcome::NoSuchChannel;
        };
        let replaced = channel
            .ingested_objects
            .get(&target.object)
            .map_or(0, Vec::len);
        if replaced == 0
            && channel.ingested_objects.len() >= MAX_INGESTED_OBJECTS_PER_CHANNEL
        {
            return IngestOutcome::ObjectCountReached;
        }
        if retained - replaced + body.len() > MAX_INGESTED_BYTES {
            return IngestOutcome::StoreFull;
        }
        channel.ingested_objects.insert(target.object.clone(), body);
        IngestOutcome::Recorded
    }
}

/// A persisted channel group: the top-level resource that owns the DNS every channel
/// and origin endpoint beneath it shares.
#[derive(Debug, Clone)]
pub struct ChannelGroup {
    pub name: String,
    /// The Region this group was created in — the one its ARN and its egress domain
    /// carry, and the only one it resolves, lists and mutates in. See
    /// [`ChannelGroupKey`].
    pub region: String,
    pub arn: String,
    pub egress_domain: String,
    pub description: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    /// How many times this group has been committed, starting at 1 for the create.
    ///
    /// Part of [`ChannelGroup::recompute_etag`]'s input, which is what makes every
    /// commit produce a DIFFERENT entity tag even when it writes the same values: a
    /// conditional update that lost a race must find the tag it presented stale.
    pub version: u64,
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

impl ChannelGroup {
    /// Recompute this group's entity tag from its own content.
    ///
    /// The single place the group's ETag is derived, so the create path and every update
    /// path agree on what the tag is a function of.
    pub fn recompute_etag(&mut self) {
        self.etag = etag(&[
            self.arn.clone(),
            self.egress_domain.clone(),
            self.description.clone().unwrap_or_default(),
            self.version.to_string(),
            tag_fingerprint(&self.tags),
        ]);
    }

    /// Commit a change: bump the version, stamp `modified_at`, and derive the new ETag.
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }
}

/// A persisted channel: the input side of the pipeline, with the ingest endpoints a
/// contributing encoder pushes to.
#[derive(Debug, Clone)]
pub struct Channel {
    pub name: String,
    pub group_name: String,
    pub arn: String,
    pub description: Option<String>,
    /// Materialised at create time from the submitted value or the service's default, so
    /// it is never absent: see [`materialized_input_type`]. The input type is immutable,
    /// so no later write replaces it.
    pub input_type: String,
    pub ingest_endpoints: Vec<IngestEndpoint>,
    /// Materialised at create time rather than only when the caller sends one: the
    /// service reports a channel's switching state on every projection of it.
    pub input_switch_configuration: InputSwitchConfiguration,
    /// Materialised at create time, for the same reason as
    /// [`Channel::input_switch_configuration`].
    pub output_header_configuration: OutputHeaderConfiguration,
    pub multiview_configuration: Option<MultiviewConfiguration>,
    /// Materialised at create time on a channel whose input type may carry the member,
    /// and absent on one that may not: see [`materialized_output_locking_mode`].
    pub output_locking_mode: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    pub reset_at: Option<::aws_smithy_types::DateTime>,
    /// See [`ChannelGroup::version`].
    pub version: u64,
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
    /// What the channel's ingest data plane has accepted, keyed by the request path the
    /// contributing encoder `PUT` it to. Bounded by
    /// [`MAX_INGESTED_OBJECTS_PER_CHANNEL`] / [`MAX_INGESTED_OBJECT_BYTES`].
    ///
    /// Ingested content is not a channel member either, so it stays out of the ETag for
    /// the same reason the policy does.
    pub ingested_objects: BTreeMap<String, Vec<u8>>,
    pub origin_endpoints: BTreeMap<String, OriginEndpoint>,
}

impl Channel {
    /// Recompute this channel's entity tag from its own content. See
    /// [`ChannelGroup::recompute_etag`].
    pub fn recompute_etag(&mut self) {
        self.etag = etag(&[
            self.arn.clone(),
            self.description.clone().unwrap_or_default(),
            self.input_type.clone(),
            self.output_locking_mode.clone().unwrap_or_default(),
            // The APPLIED switching and CMSD-header state is part of the channel's
            // content, so a channel that differs only in it has a different ETag. Each
            // member is rendered explicitly — `mqcs`/`publish` as 0 or 1, an absent
            // preferred input as the empty string — rather than through a derived `Debug`.
            format!(
                "{}/{}/{}",
                u8::from(
                    self.input_switch_configuration
                        .mqcs_input_switching
                        .unwrap_or_default()
                ),
                self.input_switch_configuration
                    .preferred_input
                    .map(|preferred| preferred.to_string())
                    .unwrap_or_default(),
                u8::from(
                    self.output_header_configuration
                        .publish_mqcs
                        .unwrap_or_default()
                ),
            ),
            self.version.to_string(),
            tag_fingerprint(&self.tags),
        ]);
    }

    /// Commit a change. See [`ChannelGroup::commit`].
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }
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
    /// See [`ChannelGroup::version`].
    pub version: u64,
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
    /// no second map can disagree about which policy an endpoint carries. It is also what
    /// gates the endpoint's egress data plane (see `crate::service`), and being a
    /// sub-resource it takes no part in the endpoint's ETag.
    pub policy: Option<EndpointPolicy>,
    /// Every harvest job created against this endpoint, keyed by job name.
    pub harvest_jobs: BTreeMap<String, HarvestJobRecord>,
}

/// A policy attached to an origin endpoint: the policy document, plus the optional CDN
/// authorization settings submitted with it (the two travel together on every
/// `PutOriginEndpointPolicy`/`GetOriginEndpointPolicy` pair).
#[derive(Debug, Clone)]
pub struct EndpointPolicy {
    pub policy: String,
    pub cdn_auth_configuration: Option<CdnAuthConfiguration>,
}

impl OriginEndpoint {
    /// Commit a change. See [`ChannelGroup::commit`].
    ///
    /// The tag is derived by [`origin_endpoint_etag`], the ONE place an endpoint's ETag
    /// comes from, so the tag a create reports, the tag an update rotates and the tag a
    /// tag change rotates are the same function of the same resource.
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.etag = origin_endpoint_etag(self);
    }

    /// A fingerprint of everything this endpoint IS, except the bookkeeping a commit
    /// writes (its commit count, its entity tag and its modification instant).
    ///
    /// Taken over the record's own `Debug` rendering, so it covers every member BY
    /// CONSTRUCTION — including a member added to the struct later, which is exactly what
    /// a hand-listed digest gets wrong. `tags` is rendered through [`tag_fingerprint`]
    /// and then taken out of that rendering, because a `HashMap`'s `Debug` order is
    /// unspecified and a rehash must not look like a change; `harvest_jobs` is left out
    /// for the same reason the policy is — a sub-resource commits versions of its own.
    ///
    /// `UpdateOriginEndpoint` captures it before it writes and compares afterwards, so a
    /// write that changed ANY member commits (and rotates the tag) while one that stored
    /// exactly what was there does not. Compared only within one request: derived `Debug`
    /// output is not a stability contract, so the value is never stored or returned.
    pub fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = tag_fingerprint(&own.tags);
        own.tags = HashMap::new();
        own.harvest_jobs = BTreeMap::new();
        format!("{tags}\u{1f}{own:?}")
    }
}

/// How long a harvest job stays `QUEUED` before the clock owes it `IN_PROGRESS`.
///
/// AWS does not publish this latency, so it is the EMULATOR's own span, inside the
/// 250–750 ms band the project fixes for a transition a call starts: a caller that polls
/// straight after `CreateHarvestJob` still observes `QUEUED`.
const HARVEST_JOB_QUEUED_MILLIS: i64 = 400;

/// What a harvest job reports once its scheduled window has passed.
///
/// This emulator runs no harvester: nothing reads the endpoint's segments and nothing is
/// written to the S3 destination, so the job cannot complete. It therefore ends in the
/// model's own failure status with a reason that says exactly that, rather than reporting
/// a `COMPLETED` export that never happened.
pub const HARVESTER_UNAVAILABLE_MESSAGE: &str = "This emulator does not run the MediaPackage \
    harvester, so no content was exported to the harvest destination";

/// A persisted harvest job: an export of an origin endpoint's content to S3.
#[derive(Debug, Clone)]
pub struct HarvestJobRecord {
    pub name: String,
    pub group_name: String,
    pub channel_name: String,
    pub endpoint_name: String,
    pub arn: String,
    pub description: Option<String>,
    pub harvested_manifests: HarvestedManifests,
    pub schedule_configuration: HarvesterScheduleConfiguration,
    pub destination: Destination,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    /// When the clock owes this job its `QUEUED` -> `IN_PROGRESS` transition.
    pub in_progress_at: ::aws_smithy_types::DateTime,
    /// See [`ChannelGroup::version`].
    pub version: u64,
    pub etag: String,
    pub tags: HashMap<String, String>,
}

impl HarvestJobRecord {
    /// Recompute this job's entity tag from its own content.
    pub fn recompute_etag(&mut self) {
        self.etag = etag(&[
            self.arn.clone(),
            self.status.clone(),
            self.description.clone().unwrap_or_default(),
            self.version.to_string(),
            tag_fingerprint(&self.tags),
        ]);
    }

    /// Commit a change. See [`ChannelGroup::commit`].
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }

    /// Whether this job has reached a status it can no longer leave.
    pub fn is_terminal(&self) -> bool {
        self.status == HARVEST_JOB_CANCELLED
            || self.status == HARVEST_JOB_FAILED
            || self.status == HARVEST_JOB_COMPLETED
    }

    /// Materialise the status changes the CLOCK already owed this job by `at`.
    ///
    /// Called by the reporting operations as well as the mutating ones, because each of
    /// these transitions was due at the same instant whether or not anybody read the job:
    /// `QUEUED` -> `IN_PROGRESS` once [`HARVEST_JOB_QUEUED_MILLIS`] has passed, and
    /// `IN_PROGRESS` -> `FAILED` once the window the CALLER scheduled has ended (see
    /// [`HARVESTER_UNAVAILABLE_MESSAGE`]).
    pub fn materialise_due_transitions(&mut self, at: ::aws_smithy_types::DateTime) {
        if self.status == HARVEST_JOB_QUEUED
            && at.as_secs_f64() >= self.in_progress_at.as_secs_f64()
        {
            self.status = HARVEST_JOB_IN_PROGRESS.to_string();
            self.commit(at);
        }
        if self.status == HARVEST_JOB_IN_PROGRESS
            && at.as_secs_f64() >= self.schedule_configuration.end_time.as_secs_f64()
        {
            self.status = HARVEST_JOB_FAILED.to_string();
            self.error_message = Some(HARVESTER_UNAVAILABLE_MESSAGE.to_string());
            self.commit(at);
        }
    }
}

/// The instant a harvest job created at `created_at` is owed `IN_PROGRESS`.
pub fn harvest_job_in_progress_at(
    created_at: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(
        created_at.as_secs_f64() + (HARVEST_JOB_QUEUED_MILLIS as f64 / 1000.0),
    )
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

/// The input type a channel is stored with: the one the request named, else the HLS the
/// model's `InputType` doc declares for an unprovided value ("If unprovided, the value
/// defaults to HLS").
///
/// Materialised rather than held as the raw optional, because the service applies the
/// default at create time: every projection of a channel — the create response, a later
/// read, an update's response and the list item — reports a concrete input type.
pub fn materialized_input_type(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_INPUT_TYPE.to_string())
}

/// The output locking mode a channel is stored with.
///
/// The model documents a service-applied default for the member ("If you don't specify a
/// value, the default is `EPOCH_LOCKED`") AND restricts which input types may carry it at
/// all, so the default is materialised only on a channel whose input type may carry it; a
/// channel that may not has no mode and reports none. The restriction is read off the
/// same table a SUBMITTED mode is refused by ([`channel_input_type_violation`]), so a
/// stored mode and a refused one can never disagree about which input types carry the
/// member.
pub fn materialized_output_locking_mode(
    submitted: Option<String>,
    input_type: &str,
) -> Option<String> {
    if channel_input_type_violation(
        SubmittedChannelMembers {
            output_locking_mode: true,
            ..SubmittedChannelMembers::default()
        },
        input_type,
    )
    .is_some()
    {
        return None;
    }
    Some(submitted.unwrap_or_else(|| DEFAULT_OUTPUT_LOCKING_MODE.to_string()))
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

// The fixed not-found sentence for each resource type lives with the rest of this
// service's refusal bodies, as `ResourceKind::not_found`.

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

/// The ARN of a harvest job, which nests under its origin endpoint's resource path.
pub fn harvest_job_arn(
    region: &str,
    group: &str,
    channel: &str,
    endpoint: &str,
    job: &str,
) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!(
            "channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}/harvestJob/{job}"
        ),
    )
}

/// Which resource a `TagArn` names.
///
/// The three taggable levels of the hierarchy, in the resource-path form the `*_arn`
/// builders above produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    ChannelGroup {
        group: String,
    },
    Channel {
        group: String,
        channel: String,
    },
    OriginEndpoint {
        group: String,
        channel: String,
        endpoint: String,
    },
}

impl TaggedResource {
    /// The channel group at the root of this resource's path — the record every
    /// taggable level of the hierarchy is reached through.
    pub fn group(&self) -> &str {
        match self {
            Self::ChannelGroup { group }
            | Self::Channel { group, .. }
            | Self::OriginEndpoint { group, .. } => group,
        }
    }
}

/// What a tagging `ResourceArn` names: the Region the ARN is scoped to, and the resource
/// its path names inside that Region.
///
/// The Region is read rather than skipped because it is part of the resource's identity
/// (see [`ChannelGroupKey`]): the same path in two Regions names two different
/// resources, so a request can only reach the one its own Region holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTarget {
    pub region: String,
    pub resource: TaggedResource,
}

/// The resource a tagging `ResourceArn` names, or `None` when the string is not a
/// syntactically valid ARN for a resource of THIS service.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>` — six
/// colon-separated fields, the first literally `arn` — and this service's resource path
/// is `channelGroup/<g>[/channel/<c>[/originEndpoint/<e>]]`. Anything else cannot name a
/// MediaPackage resource, which is a different failure from naming one that is absent:
/// the caller has to fix the ARN rather than create the resource.
pub fn parse_tag_arn(arn: &str) -> Option<TagTarget> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if partition.is_empty() || service != SIGNING_NAME {
        return None;
    }
    let segments: Vec<&str> = resource.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let resource = match segments.as_slice() {
        ["channelGroup", group] => TaggedResource::ChannelGroup {
            group: (*group).to_string(),
        },
        ["channelGroup", group, "channel", channel] => TaggedResource::Channel {
            group: (*group).to_string(),
            channel: (*channel).to_string(),
        },
        ["channelGroup", group, "channel", channel, "originEndpoint", endpoint] => {
            TaggedResource::OriginEndpoint {
                group: (*group).to_string(),
                channel: (*channel).to_string(),
                endpoint: (*endpoint).to_string(),
            }
        }
        _ => return None,
    };
    Some(TagTarget {
        region: region.to_string(),
        resource,
    })
}

/// Whether `policy` lets an UNAUTHENTICATED caller perform `action`.
///
/// An anonymous request is the `*` principal, so a statement reaches it only by naming
/// `*` as its principal. `Deny` wins over `Allow`, as in IAM. Used by the egress data
/// plane, where the endpoint's resource policy is the only thing that can authorize a
/// player with no credentials.
///
/// This evaluator understands `Effect`, `Action`, `Principal` and `Resource` only, so it
/// fails CLOSED on anything else: a statement carrying a member it cannot evaluate
/// ([`UNEVALUATED_STATEMENT_MEMBERS`] — `Condition`, `NotAction`, `NotPrincipal`,
/// `NotResource`) never GRANTS, because honouring a conditional `Allow` as an
/// unconditional one would authorize a fetch on a condition nothing checked. A `Deny`
/// carrying one still denies, which is the closed side there too.
///
/// `resource` is the ARN of the resource the request addresses, which a statement's
/// `Resource` is matched against: a statement that names no `Resource` applies to the
/// resource its policy is attached to, which is what a resource policy means.
pub fn policy_allows_anonymous(policy: &str, action: &str, resource: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let statements: Vec<&::serde_json::Value> = match parsed.get("Statement") {
        Some(::serde_json::Value::Array(list)) => list.iter().collect(),
        Some(single @ ::serde_json::Value::Object(_)) => vec![single],
        _ => return false,
    };
    let matching = |statement: &::serde_json::Value, effect: &str| {
        statement.get("Effect").and_then(::serde_json::Value::as_str) == Some(effect)
            && statement_names_action(statement, action)
            && statement_names_anonymous_principal(statement)
            && statement_names_resource(statement, resource)
    };
    if statements
        .iter()
        .any(|statement| matching(statement, "Deny"))
    {
        return false;
    }
    statements.iter().any(|statement| {
        matching(statement, "Allow") && !carries_unevaluated_member(statement)
    })
}

/// The statement members [`policy_allows_anonymous`] cannot evaluate, and so refuses to
/// grant past.
///
/// Each of them narrows or inverts what a statement reaches, so ignoring one would widen
/// the grant: a `Condition` makes the `Allow` conditional, and the `Not*` forms match
/// everything OUTSIDE the value they name.
const UNEVALUATED_STATEMENT_MEMBERS: &[&str] =
    &["Condition", "NotAction", "NotPrincipal", "NotResource"];

/// Whether a statement carries a member this evaluator does not understand.
fn carries_unevaluated_member(statement: &::serde_json::Value) -> bool {
    UNEVALUATED_STATEMENT_MEMBERS
        .iter()
        .any(|member| statement.get(member).is_some())
}

/// Whether a statement's `Resource` covers `resource`.
///
/// A statement that names no `Resource` applies to the resource its policy is attached
/// to, so it covers the addressed one. A statement that DOES name one covers the request
/// only when a named pattern matches the addressed ARN, under the wildcards IAM allows in
/// a resource pattern ([`arn_pattern_matches`]).
fn statement_names_resource(statement: &::serde_json::Value, resource: &str) -> bool {
    let Some(declared) = statement.get("Resource") else {
        return true;
    };
    let patterns = match declared {
        ::serde_json::Value::String(one) => vec![one.as_str()],
        ::serde_json::Value::Array(list) => {
            list.iter().filter_map(::serde_json::Value::as_str).collect()
        }
        _ => return false,
    };
    patterns
        .iter()
        .any(|pattern| arn_pattern_matches(pattern, resource))
}

/// Whether an IAM resource pattern matches `value`.
///
/// IAM resource patterns take two wildcards: `*` for any run of characters (including
/// none) and `?` for exactly one. The match is anchored at both ends, and `*` backtracks,
/// so a pattern with several of them matches the way IAM's does.
fn arn_pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut at_pattern, mut at_value) = (0usize, 0usize);
    // Where the most recent `*` was, and how much of the value it had consumed, so a
    // failed match can give it one more character and try again.
    let mut star: Option<(usize, usize)> = None;
    while at_value < value.len() {
        let literal = at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == value[at_value]);
        if literal {
            at_pattern += 1;
            at_value += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some((at_pattern, at_value));
            at_pattern += 1;
        } else if let Some((star_pattern, star_value)) = star {
            at_pattern = star_pattern + 1;
            at_value = star_value + 1;
            star = Some((star_pattern, star_value + 1));
        } else {
            return false;
        }
    }
    // The value is exhausted, so what is left of the pattern may only be wildcards that
    // match nothing.
    pattern[at_pattern..].iter().all(|glob| *glob == '*')
}

/// Whether a statement's `Action` covers `action`, honouring the `*` wildcard IAM allows
/// both on its own and as a trailing one (`service:*`).
fn statement_names_action(statement: &::serde_json::Value, action: &str) -> bool {
    let Some(declared) = statement.get("Action") else {
        return false;
    };
    let names = match declared {
        ::serde_json::Value::String(one) => vec![one.as_str()],
        ::serde_json::Value::Array(list) => {
            list.iter().filter_map(::serde_json::Value::as_str).collect()
        }
        _ => return false,
    };
    names.iter().any(|name| match name.strip_suffix('*') {
        Some(prefix) => action.starts_with(prefix),
        None => name.eq_ignore_ascii_case(action),
    })
}

/// Whether a statement's `Principal` names everyone — the only principal an anonymous
/// request can match. IAM writes it either as the bare string `"*"` or as a map whose
/// values are `"*"` (`{"AWS": "*"}`).
fn statement_names_anonymous_principal(statement: &::serde_json::Value) -> bool {
    let is_everyone = |value: &::serde_json::Value| match value {
        ::serde_json::Value::String(one) => one == "*",
        ::serde_json::Value::Array(list) => list
            .iter()
            .any(|entry| entry.as_str() == Some("*")),
        _ => false,
    };
    match statement.get("Principal") {
        Some(::serde_json::Value::Object(map)) => map.values().any(is_everyone),
        Some(value) => is_everyone(value),
        None => false,
    }
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
    egress_domain.replacen(EGRESS_HOST_LABEL, INGEST_HOST_LABEL, 1)
}

/// The label that marks a hostname as this service's EGRESS data plane, as
/// [`egress_domain`] builds it.
const EGRESS_HOST_LABEL: &str = ".egress.";

/// The label that marks a hostname as this service's INGEST data plane, as
/// [`ingest_domain`] builds it.
const INGEST_HOST_LABEL: &str = ".ingest.";

/// Which half of this service's data plane a hostname addresses.
///
/// The control plane is reached at `mediapackagev2.<region>.amazonaws.com`, whose first
/// label names the service; the data plane is reached at the per-group hostnames
/// [`egress_domain`] and [`ingest_domain`] build, whose first label is opaque. Those
/// hostnames are the ones this service PUBLISHES in `EgressDomain` and in a channel's
/// `IngestEndpoints[].Url`, so recognising them is how a request to a published URL
/// reaches the service that published it instead of the control-plane resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPlane {
    /// A contributing encoder pushing content to a channel.
    Ingest,
    /// A player fetching content from an origin endpoint.
    Egress,
}

/// The half of the data plane `host` addresses, or `None` for any other hostname
/// (the control-plane endpoint included).
pub fn data_plane_of_host(host: &str) -> Option<DataPlane> {
    let host = host_name(host);
    if !host.contains(&format!(".{SIGNING_NAME}.")) {
        return None;
    }
    if host.contains(INGEST_HOST_LABEL) {
        return Some(DataPlane::Ingest);
    }
    if host.contains(EGRESS_HOST_LABEL) {
        return Some(DataPlane::Egress);
    }
    None
}

/// The Region a published data-plane hostname is served in, or `None` when the host
/// carries no region label.
///
/// MediaPackage's data plane is regional exactly as its control plane is: the hostnames
/// [`egress_domain`] and [`ingest_domain`] publish end in `<service>.<region>.amazonaws.com`,
/// so the published URL itself names the Region whose resources it serves — which is how
/// a push or a fetch resolves the group it addresses without a credential scope to read.
pub fn data_plane_region(host: &str) -> Option<String> {
    let host = host_name(host);
    let (_, after_service) = host.split_once(&format!(".{SIGNING_NAME}."))?;
    let region = after_service.split('.').next()?;
    (!region.is_empty()).then(|| region.to_string())
}

/// The bare hostname of a `Host` header or URI authority: no userinfo, no port.
fn host_name(host: &str) -> &str {
    let host = host.rsplit('@').next().unwrap_or(host);
    host.split(':').next().unwrap_or(host)
}

/// What an egress request path names: the endpoint whose policy gates it, plus the
/// object beneath that endpoint.
///
/// The path is the one [`manifest_url`] builds: `/out/v1/<group>/<channel>/<endpoint>/…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressTarget {
    pub group: String,
    pub channel: String,
    pub endpoint: String,
    /// Everything after the endpoint — the manifest or segment being fetched.
    pub object: String,
}

/// The endpoint and object an egress path names, or `None` when the path is not one this
/// service publishes.
pub fn parse_egress_path(path: &str) -> Option<EgressTarget> {
    let rest = path.strip_prefix("/out/v1/")?;
    let mut segments = rest.splitn(4, '/');
    let (Some(group), Some(channel), Some(endpoint), Some(object)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if [group, channel, endpoint, object]
        .iter()
        .any(|segment| segment.is_empty())
    {
        return None;
    }
    Some(EgressTarget {
        group: group.to_string(),
        channel: channel.to_string(),
        endpoint: endpoint.to_string(),
        object: object.to_string(),
    })
}

/// What an ingest request path names: the channel the content is being pushed to.
///
/// The path is the one [`ingest_endpoints`] builds:
/// `/in/v1/<group>/<index>/<channel>/…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestTarget {
    pub group: String,
    pub channel: String,
    /// Everything after the channel — what the encoder is writing.
    pub object: String,
}

/// The channel an ingest path names, or `None` when the path is not one this service
/// publishes.
pub fn parse_ingest_path(path: &str) -> Option<IngestTarget> {
    let rest = path.strip_prefix("/in/v1/")?;
    let mut segments = rest.splitn(4, '/');
    let (Some(group), Some(_index), Some(channel), Some(object)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if [group, channel, object]
        .iter()
        .any(|segment| segment.is_empty())
    {
        return None;
    }
    Some(IngestTarget {
        group: group.to_string(),
        channel: channel.to_string(),
        object: object.to_string(),
    })
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

/// The names of the multiview channels in `group` that list `channel_name` among their
/// `MultiviewConfiguration.AvailableSources`.
///
/// The model calls `AttachedMultiviewChannels` a read-only field: it is the inverse of
/// the available-source lists the group's other channels were created or updated with, so
/// every channel reader DERIVES it from the store rather than storing it on the channel.
/// `None` when nothing references the channel, so the member is reported only once there
/// is an attachment to report.
pub fn attached_multiview_channels(group: &ChannelGroup, channel_name: &str) -> Option<Vec<String>> {
    // `channels` is a BTreeMap, so the names come out in name order.
    let attached: Vec<String> = group
        .channels
        .values()
        .filter(|candidate| {
            candidate
                .multiview_configuration
                .as_ref()
                .is_some_and(|multiview| {
                    multiview
                        .available_sources
                        .iter()
                        .any(|source| source == channel_name)
                })
        })
        .map(|candidate| candidate.name.clone())
        .collect();
    (!attached.is_empty()).then_some(attached)
}

/// A length-prefixed digest over the fields the caller supplies.
///
/// The one hashing primitive this module has: each resource's `recompute_etag` (and
/// [`origin_endpoint_etag`]) builds an entity tag with it, and [`request_fingerprint`] a
/// create's idempotency fingerprint. The length prefix is what makes two different field
/// sequences unable to collide, so what the digest means is decided entirely by which
/// fields the caller passes.
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

/// The conflict a repeat create under a token that was used for a DIFFERENT request is
/// refused with: 409, `IDEMPOTENT_PARAMETER_MISMATCH`, and the service's fixed sentence.
///
/// The sentence names no value from the request — least of all the `ClientToken` itself,
/// which is the caller's own credential-like identifier for the attempt. It follows the
/// rule the refusal builders below state: a refusal's words depend only on WHAT was
/// refused, never on the identifiers the caller sent.
pub fn idempotent_parameter_mismatch() -> ConflictException {
    ConflictException {
        message: Some(
            "You've specified a client token with your request using parameter values \
             that differ from a previous request that used the same client token. Check \
             your parameters and then try again"
                .to_string(),
        ),
        conflict_exception_type: Some("IDEMPOTENT_PARAMETER_MISMATCH".to_string()),
    }
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
/// channel -> origin endpoint -> harvest job.
///
/// The variants are the `ResourceTypeNotFound` enum values the model declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// The top-level resource, which owns channels.
    ChannelGroup,
    /// A channel inside a channel group, which owns origin endpoints.
    Channel,
    /// An origin endpoint on a channel.
    OriginEndpoint,
    /// A harvest job on an origin endpoint.
    HarvestJob,
}

impl ResourceKind {
    /// This kind's `ResourceTypeNotFound` value (model shape `ResourceTypeNotFound`,
    /// whose values are `CHANNEL_GROUP`, `CHANNEL`, `ORIGIN_ENDPOINT`, `HARVEST_JOB`).
    pub fn resource_type_not_found(self) -> &'static str {
        match self {
            Self::ChannelGroup => "CHANNEL_GROUP",
            Self::Channel => "CHANNEL",
            Self::OriginEndpoint => "ORIGIN_ENDPOINT",
            Self::HarvestJob => "HARVEST_JOB",
        }
    }

    /// How a mediapackagev2 message names this kind in prose.
    pub fn noun(self) -> &'static str {
        match self {
            Self::ChannelGroup => "channel group",
            Self::Channel => "channel",
            Self::OriginEndpoint => "origin endpoint",
            Self::HarvestJob => "harvest job",
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

/// The 409 a delete answers while another resource still REFERENCES the one it names —
/// a channel that a multiview channel of the same group lists among its available
/// sources.
///
/// `RESOURCE_IN_USE`, the same discriminator [`children_attached`] uses: a reference is
/// a use of the resource just as a child is, and the deletion would leave the referrer
/// pointing at nothing. The sentence names the kind that has to be updated first and,
/// like every refusal here, neither the caller's names nor how many referrers there are.
pub fn multiview_source_in_use() -> ConflictException {
    ConflictException {
        message: Some(
            "The channel you tried to delete is listed as an available source by a \
             multiview channel in the same channel group. If you want to delete this \
             channel, you must first update the multiview channels that list it as an \
             available source"
                .to_string(),
        ),
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

/// The 402 a create answers when the collection it would grow is already at its ceiling.
///
/// `ServiceQuotaExceededException` declares no discriminator member, so the sentence is
/// the whole refusal: it names the KIND whose quota is reached and, like every refusal
/// here, neither the caller's names nor the quota's value — which is a fact about the
/// account rather than about the refusal.
pub fn quota_exceeded(kind: ResourceKind) -> ServiceQuotaExceededException {
    let plural = kind.plural();
    ServiceQuotaExceededException {
        message: Some(format!(
            "MediaPackage can't create the resource you requested because you've reached \
             the quota for {plural}. Delete {plural} you no longer need, or request a \
             quota increase, and then try again"
        )),
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

/// Which of a channel's restricted members a request carries.
///
/// The `InputType` restrictions below are about the REQUEST, not about any one
/// operation's input shape, so each handler reduces its own generated input to this and
/// the rule table is written once. `CreateChannel` and `UpdateChannel` therefore cannot
/// disagree about which input type may carry which member.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubmittedChannelMembers {
    pub output_locking_mode: bool,
    pub mqcs_input_switching: bool,
    pub preferred_input: bool,
    pub publish_mqcs: bool,
    pub multiview_configuration: bool,
}

/// A channel member the service only lets a particular set of input types carry.
struct ChannelInputTypeRule {
    /// The member, as a validation message names it (lowerCamel, as on the wire).
    member: &'static str,
    /// The `InputType` values that may carry it, from the model's `InputType` enum.
    allowed: &'static [&'static str],
    /// The `ValidationExceptionType` the model declares for this combination.
    exception_type: &'static str,
    /// The `ValidationExceptionType` for the OPPOSITE direction, on a member whose doc
    /// also calls it REQUIRED on the input types that may carry it: the one the model
    /// names for the member being absent on such an input type. `None` on a member the
    /// docs only permit, never require.
    required_as: Option<&'static str>,
    /// What the member is called in the sentence the refusal states.
    label: &'static str,
    /// Whether the request carries the member at all.
    present: fn(&SubmittedChannelMembers) -> bool,
}

/// Every `InputType` restriction the model states for a channel member, one row per
/// `ValidationExceptionType` that names one.
///
/// The rows are read off the model, not guessed: each member's own doc says which input
/// types may carry it (`OutputLockingMode` — "only configurable when the channel
/// InputType is CMAF or MULTIVIEW"; `MQCSInputSwitching` / `PreferredInput` /
/// `PublishMQCS` — "valid only when InputType is CMAF"; `MultiviewConfiguration` —
/// "required when InputType is MULTIVIEW, and can't be set for any other input type"),
/// and `ValidationExceptionType` carries exactly one discriminator per row.
const CHANNEL_INPUT_TYPE_RULES: &[ChannelInputTypeRule] = &[
    ChannelInputTypeRule {
        member: "outputLockingMode",
        allowed: &["CMAF", "MULTIVIEW"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE",
        // The member has a documented DEFAULT, so an absent one is materialised rather
        // than refused: see `materialized_output_locking_mode`.
        required_as: None,
        label: "OutputLockingMode",
        present: |submitted| submitted.output_locking_mode,
    },
    ChannelInputTypeRule {
        member: "inputSwitchConfiguration.mqcsInputSwitching",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING",
        required_as: None,
        label: "MQCS input switching",
        present: |submitted| submitted.mqcs_input_switching,
    },
    ChannelInputTypeRule {
        member: "inputSwitchConfiguration.preferredInput",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_PREFERRED_INPUT_CONFIGURATION",
        required_as: None,
        label: "PreferredInput",
        present: |submitted| submitted.preferred_input,
    },
    ChannelInputTypeRule {
        member: "outputHeaderConfiguration.publishMqcs",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION",
        required_as: None,
        label: "PublishMQCS",
        present: |submitted| submitted.publish_mqcs,
    },
    ChannelInputTypeRule {
        member: "multiviewConfiguration",
        allowed: &[INPUT_TYPE_MULTIVIEW],
        exception_type: "MULTIVIEW_CONFIGURATION_NOT_ALLOWED",
        // The only member the docs state in BOTH directions: "required when InputType is
        // MULTIVIEW, and can't be set for any other input type".
        required_as: Some("MULTIVIEW_CONFIGURATION_REQUIRED"),
        label: "MultiviewConfiguration",
        present: |submitted| submitted.multiview_configuration,
    },
];

/// The `InputType` whose channels are the only ones that carry a multiview
/// configuration, and which require one, from the model's `InputType` enum.
pub const INPUT_TYPE_MULTIVIEW: &str = "MULTIVIEW";

/// The first `InputType` restriction a request breaks, as the modeled
/// `ValidationException` to answer it with, or `None` when every member it carries is
/// allowed on `input_type`.
pub fn channel_input_type_violation(
    submitted: SubmittedChannelMembers,
    input_type: &str,
) -> Option<ValidationException> {
    CHANNEL_INPUT_TYPE_RULES
        .iter()
        .find(|rule| (rule.present)(&submitted) && !rule.allowed.contains(&input_type))
        .map(|rule| {
            validation_failed(
                rule.member,
                &format!(
                    "{} can only be configured when the channel InputType is {}. To use \
                     {}, create a channel with {} input type",
                    rule.label,
                    rule.allowed.join(" or "),
                    rule.label,
                    rule.allowed.join(" or ")
                ),
                rule.exception_type,
            )
        })
}

/// The member an input type REQUIRES that the request does not carry, as the modeled
/// `ValidationException` to answer it with, or `None` when every member `input_type`
/// requires is there.
///
/// The other direction of [`channel_input_type_violation`], off the SAME rule table: a
/// member whose doc both restricts it to particular input types and calls it required on
/// them is refused when it is MISSING on one of those types, with the
/// `ValidationExceptionType` the model names for that case.
///
/// Only a request that describes a WHOLE channel can be judged this way — the create. On
/// an update, an absent member means "leave what is stored", not "the channel has none",
/// so an update checks the present-but-not-allowed direction alone.
pub fn channel_required_member_violation(
    submitted: SubmittedChannelMembers,
    input_type: &str,
) -> Option<ValidationException> {
    CHANNEL_INPUT_TYPE_RULES
        .iter()
        .find_map(|rule| {
            let exception_type = rule.required_as?;
            let missing = rule.allowed.contains(&input_type) && !(rule.present)(&submitted);
            missing.then_some((rule, exception_type))
        })
        .map(|(rule, exception_type)| {
            validation_failed(
                rule.member,
                &format!(
                    "{} is required when the channel InputType is {}. Specify {}, or use \
                     another input type, and then try again",
                    rule.label,
                    rule.allowed.join(" or "),
                    rule.label
                ),
                exception_type,
            )
        })
}

/// The `InputType` a multiview channel's source channels must have, from
/// `MultiviewConfiguration$AvailableSources`' doc ("must have an `InputType` of `CMAF`").
pub const MULTIVIEW_SOURCE_INPUT_TYPE: &str = "CMAF";

/// The member path of a submitted multiview configuration's source list, as a validation
/// message names it (lowerCamel, as on the wire).
const MULTIVIEW_SOURCES_MEMBER: &str = "multiviewConfiguration.availableSources";

/// The member path of a submitted multiview configuration's layout list.
const MULTIVIEW_LAYOUTS_MEMBER: &str = "multiviewConfiguration.availableLayouts";

/// The first entry a list repeats, or `None` when every entry is distinct.
fn first_repeated_entry(entries: &[String]) -> Option<&str> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    entries
        .iter()
        .find(|entry| !seen.insert(entry.as_str()))
        .map(String::as_str)
}

/// The first repetition a submitted multiview configuration carries, as the modeled
/// `ValidationException` to answer it with.
///
/// Each of the two lists names a SET — the sources a player may be given as tiles and the
/// layouts it may request — so "each must appear at most once"
/// (`MultiviewConfiguration$AvailableLayouts`' doc), and each list has its own
/// `ValidationExceptionType` for a repeat. About the request alone, so it is decided
/// before the store is consulted.
pub fn multiview_repetition_violation(
    multiview: &MultiviewConfiguration,
) -> Option<ValidationException> {
    if first_repeated_entry(&multiview.available_sources).is_some() {
        return Some(validation_failed(
            MULTIVIEW_SOURCES_MEMBER,
            &format!(
                "Each source channel can appear at most once in a multiview channel's \
                 available sources. {CHECK_THE_VALUE}"
            ),
            "MULTIVIEW_DUPLICATE_SOURCE",
        ));
    }
    first_repeated_entry(&multiview.available_layouts).map(|_| {
        validation_failed(
            MULTIVIEW_LAYOUTS_MEMBER,
            &format!(
                "Each layout can appear at most once in a multiview channel's available \
                 layouts. {CHECK_THE_VALUE}"
            ),
            "MULTIVIEW_DUPLICATE_LAYOUT",
        )
    })
}

/// The first available source a submitted multiview configuration names that `group`
/// cannot serve as a tile, as the modeled `ValidationException` to answer it with.
///
/// `AvailableSources` names other CHANNELS, so it is resolved against the store the way
/// every other reference to a resource is: a name no channel of the SAME group carries is
/// the model's not-found discriminator, and one whose channel is not of the input type a
/// tile source must have is its invalid-input-type one
/// (`MultiviewConfiguration$AvailableSources`: "Each source channel must be in the same
/// channel group as the multiview channel, and must have an `InputType` of `CMAF`"). The
/// sources are walked in the order the request listed them, so the refusal is a function
/// of the request and not of the store's iteration.
pub fn multiview_source_violation(
    group: &ChannelGroup,
    multiview: &MultiviewConfiguration,
) -> Option<ValidationException> {
    for source in &multiview.available_sources {
        let Some(channel) = group.channels.get(source) else {
            return Some(validation_failed(
                MULTIVIEW_SOURCES_MEMBER,
                &format!(
                    "Each source channel of a multiview channel must be a channel of the \
                     same channel group. {CHECK_THE_VALUE}"
                ),
                "MULTIVIEW_SOURCE_NOT_FOUND",
            ));
        };
        if channel.input_type != MULTIVIEW_SOURCE_INPUT_TYPE {
            return Some(validation_failed(
                MULTIVIEW_SOURCES_MEMBER,
                &format!(
                    "Each source channel of a multiview channel must have an InputType of \
                     {MULTIVIEW_SOURCE_INPUT_TYPE}. {CHECK_THE_VALUE}"
                ),
                "MULTIVIEW_SOURCE_INVALID_INPUT_TYPE",
            ));
        }
    }
    None
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

/// Which operation is checking a manifest set — the two differ only in what a
/// manifest-count sentence tells the caller to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestSetOn {
    Create,
    Update,
}

/// Check the manifest set an origin endpoint is being created or updated with: its total
/// count against both ends of the permitted range, and each present family against the
/// container type.
///
/// `counts` is how many manifests the request carries per family, in
/// [`ManifestFamily::ALL`] order. `None` means the set is acceptable. This is the same
/// rule for `CreateOriginEndpoint` and `UpdateOriginEndpoint`: both carry the whole
/// manifest set and the same required `ContainerType`, so an endpoint's stored manifest
/// count stays inside the range whichever operation last wrote it.
///
/// A count outside the range is an input failure, reported on `ValidationException` with
/// the discriminator for the end that was crossed — `NUM_MANIFESTS_LOW` below
/// [`MIN_MANIFESTS_PER_ORIGIN_ENDPOINT`], `NUM_MANIFESTS_HIGH` above
/// [`MAX_MANIFESTS_PER_ORIGIN_ENDPOINT`] — not a quota refusal.
pub fn manifest_set_violation(
    container: &str,
    counts: [usize; ManifestFamily::ALL.len()],
    on: ManifestSetOn,
) -> Option<ManifestSetViolation> {
    let retry = match on {
        ManifestSetOn::Create => "create",
        ManifestSetOn::Update => "update",
    };
    let total: usize = counts.iter().sum();
    if total < MIN_MANIFESTS_PER_ORIGIN_ENDPOINT {
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
    if total > MAX_MANIFESTS_PER_ORIGIN_ENDPOINT {
        return Some(ManifestSetViolation {
            message: validation_message(
                None,
                &format!(
                    "You've requested an origin endpoint with {total} manifests. Your \
                     origin endpoint can contain at most \
                     {MAX_MANIFESTS_PER_ORIGIN_ENDPOINT} manifests. Remove a manifest and \
                     try to {retry} your origin endpoint again"
                ),
            ),
            validation_exception_type: "NUM_MANIFESTS_HIGH",
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
        // The commit count, so a commit that writes the same values still rotates the
        // tag — which is what a conditional update that lost a race needs. See
        // [`ChannelGroup::version`].
        endpoint.version.to_string(),
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

// =============================================================================
// Caller-supplied ARNs: what a member documented as another service's resource names
// =============================================================================

/// What an input member this model DOCUMENTS as the ARN of another AWS resource has to
/// name.
///
/// Every such member is a plain string whose model states a `@length` and nothing else
/// (`smithy_query(query="shape",shape="CdnAuthConfiguration")`, `shape="SpekeKeyProvider"`),
/// so the boundary validator admits any text of the right size and reading the ARN form is
/// the handler's job. It HAS to be read: the service assumes the role and reads the secret
/// those members name, so a value that is not an ARN of the documented service and resource
/// type names neither and the request is an input failure rather than a configuration that
/// happens not to work.
///
/// The type and the constants are the SHARED ones ([`::service_common::arn::Requirement`]),
/// so this service and every other that binds a caller-supplied resource ARN read it
/// against one requirement rather than against a copy each. What stays here is which of
/// THIS model's members are ARNs ([`cdn_auth_rejection`], [`speke_role_arn_rejection`]) and
/// the envelope its refusal travels in.
pub type ArnRequirement = ::service_common::arn::Requirement;

/// A secret in AWS Secrets Manager: `CdnAuthConfiguration$CdnIdentifierSecretArns` is
/// "The ARN for the secret in Secrets Manager that your CDN uses for authorization to
/// access the endpoint".
pub const SECRETS_MANAGER_SECRET_ARN: ArnRequirement =
    ::service_common::arn::SECRETS_MANAGER_SECRET;

/// An IAM role: `CdnAuthConfiguration$SecretsRoleArn` is "The ARN for the IAM role that
/// gives MediaPackage read access to Secrets Manager and KMS for CDN authorization", and
/// `SpekeKeyProvider$RoleArn` "The ARN for the IAM role granted by the key provider that
/// provides access to the key provider API" — whose doc also states the form,
/// `arn:aws:iam::{accountID}:role/{name}`.
pub const IAM_ROLE_ARN: ArnRequirement = ::service_common::arn::IAM_ROLE;

/// `ValidationExceptionType` values a refused caller-supplied ARN carries, from the
/// model's `ValidationExceptionType` enum.
///
/// The enum names one value per way such a value can be wrong, which is what a caller
/// branches on: the ARN's FORM ([`VALIDATION_MALFORMED_SECRET_ARN`],
/// [`VALIDATION_ROLE_ARN_INVALID_FORMAT`]), the scope it names
/// ([`VALIDATION_SECRET_FROM_DIFFERENT_REGION`],
/// [`VALIDATION_SECRET_FROM_DIFFERENT_ACCOUNT`]) and a list that names one secret twice
/// ([`VALIDATION_DUPLICATED_SECRET`]).
pub const VALIDATION_MALFORMED_SECRET_ARN: &str = "MALFORMED_SECRET_ARN";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_SECRET_FROM_DIFFERENT_REGION: &str = "SECRET_FROM_DIFFERENT_REGION";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_SECRET_FROM_DIFFERENT_ACCOUNT: &str = "SECRET_FROM_DIFFERENT_ACCOUNT";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_DUPLICATED_SECRET: &str = "DUPLICATED_SECRET";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_ROLE_ARN_INVALID_FORMAT: &str = "ROLE_ARN_INVALID_FORMAT";

/// The requirement an ARN member that names the wrong KIND of thing failed, worded the way
/// this service words every other requirement: the rule about the member, then the sentence
/// telling the caller to look at what it sent (see [`constraint_detail`]).
fn arn_requirement_detail(requirement: ArnRequirement) -> String {
    format!("Member {}. {CHECK_THE_VALUE}", requirement.phrase())
}

/// The requirement a well-formed ARN naming another `scope` (a Region, an account) failed.
fn arn_scope_detail(scope: &str, expected: &str) -> String {
    format!("Member must name a resource in {scope} {expected}. {CHECK_THE_VALUE}")
}

/// The member path a list element is at, spelled as the framework's own constraint walk
/// spells it: the list's path, the element's 1-BASED position, then the collection step
/// (`aws_smithy_schema_server::constraints::path`). One spelling, so a refusal from a
/// handler names a position the same way a refusal from the boundary does.
fn element_path(list_path: &str, index: usize) -> String {
    format!("{list_path}.{}.member", index + 1)
}

/// Refuse a `CdnAuthConfiguration` whose ARNs are not ones this service could ever read.
///
/// CDN authorization makes MediaPackage fetch the named secrets through the named role, so
/// each entry of `CdnIdentifierSecretArns` has to be the ARN of a Secrets Manager secret in
/// the SAME account and Region as the endpoint the policy is attached to (the account every
/// resource here belongs to, [`::engine::emulator_account_id`], and `region`, the Region the
/// request is served in), the list must name each secret once, and `SecretsRoleArn` has to
/// be the ARN of an IAM role. A value outside that is an input failure whose
/// `ValidationExceptionType` names which of those it is, and `None` means the block is one
/// the service could act on.
///
/// How MANY secrets the list may carry is the model's own `@length 1..100` on
/// `CdnIdentifierSecretArns`, so the boundary validator refuses a list outside it before any
/// handler runs; this reads only what the model says nothing about.
pub fn cdn_auth_rejection(
    configuration: &CdnAuthConfiguration,
    region: &str,
) -> Option<ValidationException> {
    /// The wire path of the block's members, under the member the request carries it at.
    const SECRETS: &str = "cdnAuthConfiguration.cdnIdentifierSecretArns";
    const ROLE: &str = "cdnAuthConfiguration.secretsRoleArn";

    let account = ::engine::emulator_account_id();
    for (index, secret) in configuration.cdn_identifier_secret_arns.iter().enumerate() {
        let path = element_path(SECRETS, index);
        let Some(arn) = ::service_common::arn::parse(secret)
            .filter(|_| SECRETS_MANAGER_SECRET_ARN.admits(secret))
        else {
            return Some(validation_failed(
                &path,
                &arn_requirement_detail(SECRETS_MANAGER_SECRET_ARN),
                VALIDATION_MALFORMED_SECRET_ARN,
            ));
        };
        if arn.region != region {
            return Some(validation_failed(
                &path,
                &arn_scope_detail("the Region", region),
                VALIDATION_SECRET_FROM_DIFFERENT_REGION,
            ));
        }
        if arn.account != account {
            return Some(validation_failed(
                &path,
                &arn_scope_detail("the account", account),
                VALIDATION_SECRET_FROM_DIFFERENT_ACCOUNT,
            ));
        }
    }

    // Repetition is judged after the forms are, so a list of junk is told what an entry has
    // to BE before it is told it sent one twice. The model puts no `@uniqueItems` on the
    // list, so this is the handler's to read.
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for (index, secret) in configuration.cdn_identifier_secret_arns.iter().enumerate() {
        if !named.insert(secret.as_str()) {
            let requirement = format!(
                "Member must name a secret the list does not already name. {CHECK_THE_VALUE}"
            );
            return Some(validation_failed(
                &element_path(SECRETS, index),
                &requirement,
                VALIDATION_DUPLICATED_SECRET,
            ));
        }
    }

    (!IAM_ROLE_ARN.admits(&configuration.secrets_role_arn)).then(|| {
        validation_failed(
            ROLE,
            &arn_requirement_detail(IAM_ROLE_ARN),
            VALIDATION_ROLE_ARN_INVALID_FORMAT,
        )
    })
}

/// Refuse a segment whose SPEKE key provider names something other than an IAM role.
///
/// `SpekeKeyProvider$RoleArn` is the role MediaPackage ASSUMES to reach the key provider, so
/// the same rule as the CDN authorization role applies to it: a value that is not the ARN of
/// an IAM role names no role to assume. Shared by the writers of an endpoint's segment
/// (`CreateOriginEndpoint` and `UpdateOriginEndpoint`) so both read it the same way.
/// `SpekeKeyProvider$CertificateArn` states its own `@pattern` in the model, so the boundary
/// validator reads that one and this does not.
pub fn speke_role_arn_rejection(segment: Option<&Segment>) -> Option<ValidationException> {
    let role_arn = &segment?.encryption.as_ref()?.speke_key_provider.role_arn;
    (!IAM_ROLE_ARN.admits(role_arn)).then(|| {
        validation_failed(
            "segment.encryption.spekeKeyProvider.roleArn",
            &arn_requirement_detail(IAM_ROLE_ARN),
            VALIDATION_ROLE_ARN_INVALID_FORMAT,
        )
    })
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
    fn an_arn_pattern_matches_the_way_iam_resource_patterns_do() {
        // `*` is any run of characters (including none) and `?` is exactly one; the match
        // is anchored at both ends.
        let arn = "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c\
                   /originEndpoint/e";
        for pattern in [
            arn,
            "*",
            "arn:aws:mediapackagev2:*",
            "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c*",
            "arn:*:mediapackagev2:*:*:channelGroup/g/*/originEndpoint/e",
            "arn:aws:mediapackagev2:us-west-?:000000000000:channelGroup/g/channel/c\
             /originEndpoint/e",
        ] {
            assert!(
                arn_pattern_matches(pattern, arn),
                "case {pattern:?}: expected it to match {arn}"
            );
        }
        for pattern in [
            "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c\
             /originEndpoint/other",
            "arn:aws:s3:*",
            "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g",
            // `?` is exactly one character, so it cannot swallow a whole Region.
            "arn:aws:mediapackagev2:?:000000000000:channelGroup/g/channel/c\
             /originEndpoint/e",
        ] {
            assert!(
                !arn_pattern_matches(pattern, arn),
                "case {pattern:?}: expected it NOT to match {arn}"
            );
        }
    }

    #[test]
    fn an_anonymous_grant_needs_an_effect_an_action_a_principal_and_a_reachable_resource() {
        let arn = "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c\
                   /originEndpoint/e";
        let action = "mediapackagev2:GetObject";
        let statement = |members: &str| format!("{{\"Statement\":[{members}]}}");

        for granting in [
            r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject"}"#,
            r#"{"Effect":"Allow","Principal":{"AWS":"*"},"Action":["mediapackagev2:*"]}"#,
            r#"{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}"#,
        ] {
            assert!(
                policy_allows_anonymous(&statement(granting), action, arn),
                "case {granting:?}: expected an anonymous grant"
            );
        }
        for refusing in [
            // A named principal is not the anonymous one.
            r#"{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},
                 "Action":"mediapackagev2:GetObject"}"#,
            // A different action.
            r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:PutObject"}"#,
            // A resource this request does not address.
            r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject",
                 "Resource":"arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/other"}"#,
            // A member the evaluator cannot judge: it must not grant past it.
            r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject",
                 "Condition":{"Bool":{"aws:SecureTransport":"true"}}}"#,
            r#"{"Effect":"Deny","Principal":"*","Action":"mediapackagev2:GetObject"}"#,
        ] {
            assert!(
                !policy_allows_anonymous(&statement(refusing), action, arn),
                "case {refusing:?}: expected no anonymous grant"
            );
        }
        // Deny beats Allow, whichever order they are written in.
        let both = r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject"},
             {"Effect":"Deny","Principal":"*","Action":"mediapackagev2:GetObject"}"#;
        assert!(
            !policy_allows_anonymous(&statement(both), action, arn),
            "a Deny of the same action must beat the Allow"
        );
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

    /// One row of the input-type restriction table: which member a request carries, the
    /// channel's input type, and the `ValidationExceptionType` the combination is refused
    /// with (`None` when the input type may carry it).
    type RestrictionCase = (
        &'static str,
        &'static str,
        fn(&mut SubmittedChannelMembers),
        Option<&'static str>,
    );

    /// A member the model restricts to particular `InputType`s is refused on every other
    /// input type, with that member's own discriminator — and `CreateChannel` and
    /// `UpdateChannel` read the SAME table, so the two cannot disagree about it.
    ///
    /// One row per restriction the model states, on an input type that forbids it and on
    /// one that allows it.
    #[test]
    fn a_member_is_refused_on_an_input_type_that_may_not_carry_it() {
        let cases: &[RestrictionCase] = &[
            (
                "output locking mode on HLS",
                "HLS",
                |present| present.output_locking_mode = true,
                Some("ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE"),
            ),
            (
                "output locking mode on CMAF",
                "CMAF",
                |present| present.output_locking_mode = true,
                None,
            ),
            (
                "output locking mode on MULTIVIEW",
                "MULTIVIEW",
                |present| present.output_locking_mode = true,
                None,
            ),
            (
                "MQCS switching on HLS",
                "HLS",
                |present| present.mqcs_input_switching = true,
                Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING"),
            ),
            (
                "MQCS switching on CMAF",
                "CMAF",
                |present| present.mqcs_input_switching = true,
                None,
            ),
            (
                "preferred input on MULTIVIEW",
                "MULTIVIEW",
                |present| present.preferred_input = true,
                Some("ONLY_CMAF_INPUT_TYPE_ALLOW_PREFERRED_INPUT_CONFIGURATION"),
            ),
            (
                "CMSD publication on HLS",
                "HLS",
                |present| present.publish_mqcs = true,
                Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION"),
            ),
            (
                "multiview configuration on CMAF",
                "CMAF",
                |present| present.multiview_configuration = true,
                Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
            ),
            (
                "multiview configuration on MULTIVIEW",
                "MULTIVIEW",
                |present| present.multiview_configuration = true,
                None,
            ),
            (
                "no restricted member at all on HLS",
                "HLS",
                |_| {},
                None,
            ),
        ];

        for (label, input_type, carry, expected) in cases {
            let mut submitted = SubmittedChannelMembers::default();
            carry(&mut submitted);
            let violation = channel_input_type_violation(submitted, input_type);
            match expected {
                Some(expected) => {
                    let violation = violation.unwrap_or_else(|| {
                        panic!("case {label:?}: expected a refusal, got none")
                    });
                    assert_eq!(
                        violation.validation_exception_type.as_deref(),
                        Some(*expected),
                        "case {label:?}: expected {expected}, got {violation:?}"
                    );
                    assert!(
                        violation
                            .message
                            .as_deref()
                            .unwrap_or_default()
                            .starts_with("Validation error encountered at request"),
                        "case {label:?}: expected this service's message form, got {:?}",
                        violation.message
                    );
                }
                None => assert!(
                    violation.is_none(),
                    "case {label:?}: expected no refusal, got {violation:?}"
                ),
            }
        }
    }

    /// A stored origin-endpoint record, as the baseline of the table below.
    fn sample_endpoint() -> OriginEndpoint {
        let at = now();
        OriginEndpoint {
            name: "endpoint".to_string(),
            channel_name: "channel".to_string(),
            group_name: "group".to_string(),
            arn: origin_endpoint_arn("us-west-2", "group", "channel", "endpoint"),
            container_type: "TS".to_string(),
            segment: materialized_segment(None),
            description: None,
            startover_window_seconds: materialized_startover_window_seconds(None),
            hls_manifests: Vec::new(),
            low_latency_hls_manifests: Vec::new(),
            dash_manifests: Vec::new(),
            mss_manifests: Vec::new(),
            force_endpoint_error_configuration: None,
            uri_separator: materialized_uri_separator(None),
            stream_name_output_mode: materialized_stream_name_output_mode(None),
            created_at: at,
            modified_at: at,
            reset_at: None,
            version: 1,
            etag: String::new(),
            tags: HashMap::new(),
            client_token: None,
            fingerprint: String::new(),
            policy: None,
            harvest_jobs: BTreeMap::new(),
        }
    }

    /// One row of the per-member ETag table: the member's name, and the mutation that
    /// changes it on an origin-endpoint record. A `fn` pointer rather than a closure, so
    /// a row carries no captured state.
    type EndpointMutation = (&'static str, fn(&mut OriginEndpoint));

    /// One row per member an `Update*` or a `Create*` writes on an origin endpoint.
    #[test]
    fn a_change_to_any_member_of_an_origin_endpoint_moves_its_entity_tag() {
        let baseline = sample_endpoint();
        let mutations: [EndpointMutation; 11] = [
            ("container_type", |e| {
                e.container_type = "CMAF".to_string()
            }),
            ("description", |e| e.description = Some("a".to_string())),
            ("startover_window_seconds", |e| {
                e.startover_window_seconds = 1
            }),
            ("segment", |e| {
                e.segment = Segment {
                    segment_duration_seconds: Some(6),
                    ..Segment::default()
                }
            }),
            ("hls_manifests", |e| {
                e.hls_manifests = vec![stored_hls_manifest(
                    CreateHlsManifestConfiguration {
                        manifest_name: "m".to_string(),
                        ..CreateHlsManifestConfiguration::default()
                    },
                    ManifestLocation {
                        egress_domain: "host.example",
                        group: "group",
                        channel: "channel",
                        endpoint: "endpoint",
                    },
                )]
            }),
            ("low_latency_hls_manifests", |e| {
                e.low_latency_hls_manifests = vec![GetLowLatencyHlsManifestConfiguration {
                    manifest_name: "m".to_string(),
                    ..GetLowLatencyHlsManifestConfiguration::default()
                }]
            }),
            ("dash_manifests", |e| {
                e.dash_manifests = vec![GetDashManifestConfiguration {
                    manifest_name: "m".to_string(),
                    ..GetDashManifestConfiguration::default()
                }]
            }),
            ("mss_manifests", |e| {
                e.mss_manifests = vec![GetMssManifestConfiguration {
                    manifest_name: "m".to_string(),
                    ..GetMssManifestConfiguration::default()
                }]
            }),
            ("force_endpoint_error_configuration", |e| {
                e.force_endpoint_error_configuration = Some(ForceEndpointErrorConfiguration {
                    endpoint_error_conditions: Some(vec!["STALE_MANIFEST".to_string()]),
                })
            }),
            ("uri_separator", |e| {
                e.uri_separator = "HYPHEN".to_string()
            }),
            ("stream_name_output_mode", |e| {
                e.stream_name_output_mode = "PASSTHROUGH_NAME".to_string()
            }),
        ];
        for (member, mutate) in mutations {
            let mut mutated = baseline.clone();
            mutate(&mut mutated);
            assert_ne!(
                origin_endpoint_etag(&baseline),
                origin_endpoint_etag(&mutated),
                "case {member:?}: changing an endpoint's {member} must move its entity tag"
            );
        }
        // The tag is a function of the CONTENT: the same content, built again, hashes
        // the same, so a create and a later update of an unchanged record agree on it.
        let twin = OriginEndpoint {
            created_at: baseline.created_at,
            modified_at: baseline.modified_at,
            ..sample_endpoint()
        };
        assert_eq!(
            origin_endpoint_etag(&baseline),
            origin_endpoint_etag(&twin),
            "a record rebuilt with the same content must keep its entity tag"
        );
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
