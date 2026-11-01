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
//! manifest configuration's window, URI path type, child-manifest URL encoding and (on
//! the HLS families) child manifest name are materialised ONCE, at create time, from the
//! submitted value or the service's default, and stored — so the create response, a later
//! read and the list projection all report the same concrete value.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aws_smithy_schema_server::{Violation, ViolationKind};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    CdnAuthConfiguration, ConflictException, CreateDashManifestConfiguration,
    CreateHlsManifestConfiguration, CreateLowLatencyHlsManifestConfiguration,
    CreateMssManifestConfiguration, Destination, EncryptionMethod,
    ForceEndpointErrorConfiguration,
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

/// The `OutputLockingMode` a channel has to carry for the origin endpoints under it to
/// carry a segment output-timestamp mode at all: the model's `Segment$OutputTimestampMode`
/// doc says the setting "is only configurable on channels with OutputLockingMode set to
/// NON_EPOCH_LOCKED". One of the two values the `OutputLockingMode` enum publishes.
pub const OUTPUT_TIMESTAMP_MODE_LOCKING_MODE: &str = "NON_EPOCH_LOCKED";

/// The `OutputTimestampMode` an origin endpoint whose channel may carry one is created
/// with when the request omits it: the same member's doc says "If you don't specify a
/// value, the default is PASSTHROUGH".
pub const DEFAULT_OUTPUT_TIMESTAMP_MODE: &str = "PASSTHROUGH";

/// How many ingest endpoints a channel that receives ingest of its own is given.
/// MediaPackage v2 hands such a channel a redundant pair, which is why `IngestEndpoints`
/// is a list rather than a single URL.
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

/// The child manifest name MediaPackage applies to an HLS or low-latency HLS manifest
/// configuration submitted without one.
///
/// The model states the rule on both sides of the wire: the create shapes say "If you
/// don't enter a value, MediaPackage uses the default manifest name, index, with an added
/// suffix to distinguish it from the manifest name", and the `Get*` shapes name the
/// applied value outright — "MediaPackage uses the default child manifest name, index_1".
/// The suffixed form is why the reported member's `ResourceName` pattern admits `_` while
/// the submitted member's `ManifestName` pattern does not.
pub const DEFAULT_CHILD_MANIFEST_NAME: &str = "index_1";

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
    /// The group or the channel the path names does not resolve, or the channel it names
    /// publishes no ingest endpoint the path could belong to; nothing was written.
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
pub const VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB: &str =
    "MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_INVALID_PAGINATION_TOKEN: &str = "INVALID_PAGINATION_TOKEN";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE: &str =
    "ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED: &str =
    "MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_MULTIVIEW_SOURCE_NON_EPOCH_LOCKED: &str =
    "MULTIVIEW_SOURCE_NON_EPOCH_LOCKED";

/// The 409 for a conditional update that lost the race: the entity tag the caller
/// presented is no longer the committed one.
pub fn conflicting_operation() -> ConflictException {
    ConflictException {
        message: Some(CONFLICTING_OPERATION_MESSAGE.to_string()),
        conflict_exception_type: Some("CONFLICTING_OPERATION".to_string()),
    }
}

/// The whole conditional-write precondition, for every operation that binds
/// `x-amzn-update-if-match`: an absent entity tag makes the write unconditional, a
/// presented tag that IS the resource's committed one lets it proceed, and a presented tag
/// that is not means somebody else committed first — the refusal
/// [`conflicting_operation`] words, with nothing written.
///
/// `presented` is the request's optional `ETag` member and `committed` the tag the store
/// holds the addressed resource at. Returns the refusal to answer with, or `None` when the
/// write may proceed, which is the shape this crate's other cross-handler rules
/// ([`channel_input_type_violation`], [`speke_role_arn_rejection`]) hand back — so the
/// rule that SELECTS the refusal lives beside the sentence it selects and every
/// conditional writer resolves through this one copy of it.
pub fn stale_entity_tag_rejection(
    presented: Option<&str>,
    committed: &str,
) -> Option<ConflictException> {
    presented
        .is_some_and(|tag| tag != committed)
        .then(conflicting_operation)
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
/// `member_path` is the wire name of the member the tags arrived in, which differs per
/// operation exactly as it does for [`reserved_tag_key_rejection`], so the sentence
/// points the caller at the member it sent.
///
/// `ValidationExceptionType` enumerates no tag-quota member
/// (`VALIDATION_EXCEPTION_TYPE_VALUES` in `ops::types`), so the discriminator is left
/// unset rather than claiming a reason the service does not name for this case; the
/// message states which bound was exceeded and the bound's value, in the one envelope
/// [`validation_message`] gives every refusal this service makes.
pub fn tag_bounds_rejection(
    member_path: &str,
    tags: &HashMap<String, String>,
) -> Option<ValidationException> {
    tag_bounds_rejection_counted(member_path, tags, |count| {
        format!(
            "A resource carries at most {MAX_TAGS_PER_RESOURCE} tags; the request would \
             leave it with {count}"
        )
    })
}

/// The same bounds, judged on the map the REQUEST carries rather than on the one the
/// resource is left holding.
///
/// What a merging writer ([`merged_tags`]) can be refused for without reading the stored
/// map at all: merging only ever ADDS keys and never shortens one, so a submitted map
/// already past a bound leaves the resource past it whatever it holds. The count clause
/// states what the REQUEST carried, because how many the resource would then hold is not
/// known at this point — the map the resource is left with is judged by
/// [`tag_bounds_rejection`] once that map is in hand.
pub fn submitted_tag_bounds_rejection(
    member_path: &str,
    tags: &HashMap<String, String>,
) -> Option<ValidationException> {
    tag_bounds_rejection_counted(member_path, tags, |count| {
        format!(
            "A resource carries at most {MAX_TAGS_PER_RESOURCE} tags; the request carries \
             {count}"
        )
    })
}

/// Refuse a request whose tag KEYS are past the bound a key carries, judged on the keys
/// ALONE — the whole of the tag bounds for a path that names keys without values.
///
/// A tag key is as long as the request made it whatever the request asks to be done with
/// that key, so the character bound on a key is one rule across the whole tagging surface:
/// every path that takes caller-supplied tag keys judges it, whether the keys arrive in a
/// map to be written or in a list to be removed. The other two clauses of
/// [`tag_bounds_rejection`] have nothing to judge where no values arrive and nothing is
/// added: the value bound is about a value the request did not send, and the entry-count
/// bound is about the map a resource is left HOLDING, which removing keys only shrinks.
///
/// `member_path` is the wire name of the member the keys arrived in, which differs per
/// operation exactly as it does for [`reserved_tag_key_rejection`], so the sentence points
/// the caller at the member it sent. Called before anything is written, so a refused
/// request leaves the resource's tags exactly as they were.
pub fn tag_key_bounds_rejection<'a, I>(member_path: &str, keys: I) -> Option<ValidationException>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter()
        .find_map(|key| over_long_tag_key_rejection(member_path, key))
}

/// The tag-key character bound as ONE key's rejection: the single place this service
/// judges how long a tag key may be, so a key of a given length earns the same refusal
/// wherever it arrives.
fn over_long_tag_key_rejection(member_path: &str, key: &str) -> Option<ValidationException> {
    let characters = key.chars().count();
    (characters > MAX_TAG_KEY_CHARS).then(|| ValidationException {
        message: Some(validation_message(
            Some(member_path),
            &format!(
                "A tag key carries at most {MAX_TAG_KEY_CHARS} characters; one in the \
                 request carries {characters}"
            ),
        )),
        validation_exception_type: None,
    })
}

/// The bounds themselves, shared by the two readings above: `count_detail` words the
/// entry-count clause for the map being judged, while the per-entry clauses are the same
/// either way, because a key and a value are as long as the request made them.
fn tag_bounds_rejection_counted(
    member_path: &str,
    tags: &HashMap<String, String>,
    count_detail: impl Fn(usize) -> String,
) -> Option<ValidationException> {
    let refuse = |detail: String| {
        Some(ValidationException {
            message: Some(validation_message(Some(member_path), &detail)),
            validation_exception_type: None,
        })
    };
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return refuse(count_detail(tags.len()));
    }
    for (key, value) in tags {
        // The key half is the shared builder above, so this path and the key-only path
        // cannot drift in either the bound or the sentence.
        if let Some(rejection) = over_long_tag_key_rejection(member_path, key) {
            return Some(rejection);
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
            TaggedResource::HarvestJob {
                channel,
                endpoint,
                job,
                ..
            } => Some(
                &group
                    .channels
                    .get(channel)?
                    .origin_endpoints
                    .get(endpoint)?
                    .harvest_jobs
                    .get(job)?
                    .tags,
            ),
        }
    }

    /// The channel group a tagging ARN reaches from a request served in `region`.
    ///
    /// `None` when the ARN names a scope this emulator does not hold the resource in
    /// ([`TagTarget::reaches`] — another account, or another Region): the resource it
    /// names lives in that scope's namespace, so from this endpoint there is nothing of
    /// that name to tag, report or untag.
    fn resolved_group(&self, region: &str, target: &TagTarget) -> Option<&ChannelGroup> {
        target.reaches(region).then_some(())?;
        self.channel_group(region, target.resource.group())
    }

    /// Apply `mutate` to the tags of the resource `target` names and commit the change it
    /// made (new `ModifiedAt`, new entity tag), reporting whether the resource was there.
    ///
    /// The tags are part of each record's content, so a tag write that CHANGED them
    /// commits a new version of the record, and one that stored exactly the tags that were
    /// already there commits nothing — the same rule every other write is judged by
    /// ([`VersionedRecord::commit_if_changed`]).
    ///
    /// `false` means the resource the ARN names does not exist in the scope a request
    /// served in `region` reaches (see [`Self::resolved_group`]), so there was nothing to
    /// write to.
    pub fn with_tags_mut(
        &mut self,
        region: &str,
        target: &TagTarget,
        at: ::aws_smithy_types::DateTime,
        mutate: impl FnOnce(&mut HashMap<String, String>),
    ) -> bool {
        if !target.reaches(region) {
            return false;
        }
        let Some(group) = self.channel_group_mut(region, target.resource.group()) else {
            return false;
        };
        match &target.resource {
            TaggedResource::ChannelGroup { .. } => {
                let before = group.configuration_fingerprint();
                mutate(&mut group.tags);
                group.commit_if_changed(&before, at);
                true
            }
            TaggedResource::Channel { channel, .. } => {
                let Some(channel) = group.channels.get_mut(channel) else {
                    return false;
                };
                let before = channel.configuration_fingerprint();
                mutate(&mut channel.tags);
                channel.commit_if_changed(&before, at);
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
                let before = endpoint.configuration_fingerprint();
                mutate(&mut endpoint.tags);
                endpoint.commit_if_changed(&before, at);
                true
            }
            TaggedResource::HarvestJob {
                channel,
                endpoint,
                job,
                ..
            } => {
                let Some(job) = group
                    .channels
                    .get_mut(channel)
                    .and_then(|channel| channel.origin_endpoints.get_mut(endpoint))
                    .and_then(|endpoint| endpoint.harvest_jobs.get_mut(job))
                else {
                    return false;
                };
                let before = job.configuration_fingerprint();
                mutate(&mut job.tags);
                job.commit_if_changed(&before, at);
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
    /// the group or the channel does not resolve there, or when the channel it names
    /// publishes no ingest endpoint for the path to belong to
    /// ([`Channel::receives_ingest`]).
    pub fn channel_at_mut(
        &mut self,
        region: &str,
        target: &IngestTarget,
    ) -> Option<&mut Channel> {
        self.channel_group_mut(region, &target.group)?
            .channels
            .get_mut(&target.channel)
            .filter(|channel| channel.receives_ingest())
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

/// A persisted record whose writes are VERSIONED: every committed change bumps its
/// commit count, restamps `ModifiedAt` and re-derives its entity tag, so the tag a caller
/// holds names exactly one version of the record.
///
/// Implemented by every resource a caller-submitted write can store content into, so the
/// question each of those handlers has to answer before it commits — did this write
/// actually CHANGE the record? — is decided in ONE place instead of once per handler.
pub trait VersionedRecord {
    /// A fingerprint of everything this record IS, except the bookkeeping a commit writes
    /// (its commit count, its entity tag and its modification instant) and the
    /// sub-resources hanging off it, which commit versions of their own.
    ///
    /// Compared only within one request: derived `Debug` output is not a stability
    /// contract, so the value is never stored or returned.
    fn configuration_fingerprint(&self) -> String;

    /// Commit a change: bump the version, stamp `at` on `ModifiedAt`, re-derive the tag.
    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime);

    /// Commit the write that has just run, but only if it CHANGED the record.
    ///
    /// `before` is [`Self::configuration_fingerprint`] taken before the write. A write
    /// that changed ANY member commits a new version of the record, which rotates the
    /// entity tag — so the tag the caller held is necessarily stale, which is what makes a
    /// conditional write that lost a race detectable. A write that stored exactly what was
    /// already there changed nothing, so it commits nothing: neither the tag nor
    /// `ModifiedAt` moves, and the tag the caller holds stays usable as the precondition of
    /// the write it is entitled to make.
    fn commit_if_changed(&mut self, before: &str, at: ::aws_smithy_types::DateTime) {
        if self.configuration_fingerprint() != before {
            self.commit_version(at);
        }
    }
}

/// One record's fingerprint, taken from the record with its bookkeeping and its
/// sub-resources already scrubbed, plus the tags scrubbed out of it.
///
/// Rendered through the record's own `Debug`, so it covers every member BY CONSTRUCTION —
/// including a member added to the struct later, which is exactly what a hand-listed
/// digest gets wrong. The tags come separately, through [`tag_fingerprint`], because a
/// `HashMap`'s `Debug` order is unspecified and a rehash must not look like a change.
fn scrubbed_fingerprint(
    scrubbed: &impl ::std::fmt::Debug,
    tags: &HashMap<String, String>,
) -> String {
    format!("{}\u{1f}{scrubbed:?}", tag_fingerprint(tags))
}

impl VersionedRecord for ChannelGroup {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        // The channels beneath the group are resources in their own right, each with its
        // own version and entity tag, so they are no part of the group's content.
        own.channels = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
}

impl VersionedRecord for Channel {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        // The attached policy, the origin endpoints and what the ingest data plane
        // accepted are not channel members — no `Channel*` shape in the model carries
        // them and they take no part in the channel's entity tag either — so they are no
        // part of its content here.
        own.policy = None;
        own.origin_endpoints = BTreeMap::new();
        own.ingested_objects = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
}

impl VersionedRecord for OriginEndpoint {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        // The harvest jobs are left out for the same reason the attached policy is: a
        // sub-resource commits versions of its own.
        own.harvest_jobs = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
}

impl VersionedRecord for HarvestJobRecord {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
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
    /// service reports a channel's switching state on every projection of it — see
    /// [`materialized_input_switch_configuration`].
    pub input_switch_configuration: InputSwitchConfiguration,
    /// Materialised at create time, for the same reason as
    /// [`Channel::input_switch_configuration`]: see
    /// [`materialized_output_header_configuration`].
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

    /// Whether this channel composites video from the source channels in its multiview
    /// configuration instead of receiving ingest of its own, which is what the model's
    /// `InputType` doc says a [`INPUT_TYPE_MULTIVIEW`] channel does ("The channel receives
    /// no ingest of its own. Instead, it composites video from the source channels in its
    /// MultiviewConfiguration").
    ///
    /// Decided from the stored `InputType`, which is materialised at create time and
    /// immutable, so it is the one answer every operation that has nothing to act on such
    /// a channel branches on — rather than each handler inferring it from a different
    /// member.
    pub fn is_multiview(&self) -> bool {
        is_multiview_input_type(&self.input_type)
    }

    /// Discard everything this channel's ingest data plane has accepted.
    ///
    /// The one place a channel's retained content is dropped short of dropping the whole
    /// channel record, so the content stops being retained AND the budget it occupied —
    /// the per-channel object count ([`MAX_INGESTED_OBJECTS_PER_CHANNEL`]) and this
    /// channel's share of the store-wide retained bytes ([`MAX_INGESTED_BYTES`]), which
    /// [`ComAmazonawsMediapackagev2Mediapackagev2State::retained_ingested_bytes`] sums off
    /// the same map — is free for a later push.
    ///
    /// Touches no member of the channel's CONFIGURATION, so it neither bumps the version
    /// nor rotates the entity tag: ingested content takes no part in either (see
    /// [`Channel::ingested_objects`]).
    pub fn discard_ingested_content(&mut self) {
        self.ingested_objects.clear();
    }

    /// Whether this channel's ingest data plane serves a push.
    ///
    /// Read off the ingest endpoints the channel PUBLISHES rather than decided again from
    /// the input type: the data plane serves exactly the URLs the control plane handed
    /// out, so a channel that publishes none ([`ingest_endpoints`]) has no reachable
    /// ingest path and cannot accumulate content it never received.
    pub fn receives_ingest(&self) -> bool {
        !self.ingest_endpoints.is_empty()
    }

    /// Every harvest job this channel holds, whichever of its origin endpoints owns it.
    ///
    /// A job record is persisted on the endpoint whose content it exports, but the model
    /// gives the job's NAME the CHANNEL's namespace
    /// (`CreateHarvestJobRequest$HarvestJobName`: "This name must be unique within the
    /// channel"), so this walk — not one endpoint's map — is the collection a job name is
    /// judged against.
    pub fn harvest_jobs(&self) -> impl Iterator<Item = &HarvestJobRecord> {
        self.origin_endpoints
            .values()
            .flat_map(|endpoint| endpoint.harvest_jobs.values())
    }

    /// The harvest job of this channel called `name`, whichever of its origin endpoints
    /// persists it — the lookup every decision about a job's identity goes through,
    /// because that identity is the channel's ([`Channel::harvest_jobs`]).
    pub fn harvest_job(&self, name: &str) -> Option<&HarvestJobRecord> {
        self.harvest_jobs().find(|job| job.name == name)
    }

    /// Whether any origin endpoint of this channel already holds a harvest job called
    /// `name` — the question a create naming its job is refused by.
    pub fn holds_harvest_job(&self, name: &str) -> bool {
        self.harvest_job(name).is_some()
    }

    /// Every harvest job of this channel as `match_client_token` takes it: the candidate
    /// set a create's `ClientToken` is resolved in.
    ///
    /// The scope is the CHANNEL, the namespace a job's identity lives in, and NOT the
    /// addressed endpoint's map: a token already spent on a job under a sibling endpoint
    /// of the same channel must be found, or a second create under it would be read as a
    /// first attempt and mint a second job holding one token.
    pub fn harvest_job_token_candidates(
        &self,
    ) -> impl Iterator<Item = (&str, Option<&str>, &str)> {
        self.harvest_jobs().map(|job| {
            (
                job.name.as_str(),
                job.client_token.as_deref(),
                job.fingerprint.as_str(),
            )
        })
    }

    /// A harvest job name no job of this channel holds, derived from `seed` and the
    /// instant the job is created — what the service calls a job the caller left unnamed.
    ///
    /// Each candidate is [`derived_harvest_job_name`] under its own salt, so successive
    /// candidates differ; one more candidate is tried than the channel holds names, which
    /// is enough for one of them to be free unless two digests agree. `None` reports that
    /// impossible case rather than handing back a name already in use. The names the
    /// channel holds are gathered once, so the walk costs one pass over them however many
    /// candidates it has to try.
    pub fn unused_harvest_job_name(
        &self,
        seed: &str,
        at: ::aws_smithy_types::DateTime,
    ) -> Option<String> {
        let taken: ::std::collections::BTreeSet<&str> =
            self.harvest_jobs().map(|job| job.name.as_str()).collect();
        (0..=taken.len())
            .map(|attempt| derived_harvest_job_name(seed, at, taken.len() + attempt))
            .find(|candidate| !taken.contains(candidate.as_str()))
    }
}

/// How many characters of the digest a service-chosen harvest job name carries.
const DERIVED_HARVEST_JOB_NAME_CHARS: usize = 32;

/// One candidate name for a harvest job the caller did not name: a prefix of the digest
/// over the endpoint the job belongs to (`seed`), the instant it was created and `salt`.
///
/// Hex, so the name satisfies the model's `ResourceName` pattern `^[a-zA-Z0-9_-]+$` and
/// its 1..256 length, which is what lets every later request address the job by it.
fn derived_harvest_job_name(
    seed: &str,
    at: ::aws_smithy_types::DateTime,
    salt: usize,
) -> String {
    let digest = etag(&[
        seed.to_string(),
        at.as_secs_f64().to_string(),
        salt.to_string(),
    ]);
    digest[..DERIVED_HARVEST_JOB_NAME_CHARS].to_string()
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
    /// Materialised at create time, but only when the parent channel's input type may
    /// carry a startover window at all: an endpoint of a channel that may not has none
    /// and reports none. See [`materialized_startover_window_seconds`].
    pub startover_window_seconds: Option<i32>,
    pub hls_manifests: Vec<GetHlsManifestConfiguration>,
    pub low_latency_hls_manifests: Vec<GetLowLatencyHlsManifestConfiguration>,
    pub dash_manifests: Vec<GetDashManifestConfiguration>,
    pub mss_manifests: Vec<GetMssManifestConfiguration>,
    pub force_endpoint_error_configuration: Option<ForceEndpointErrorConfiguration>,
    /// Materialised at create time: the service reports the separator it applied on
    /// every projection of the endpoint, the list projection included.
    pub uri_separator: String,
    /// Materialised at create time like [`OriginEndpoint::uri_separator`], but only when
    /// the parent channel's input type may carry the member at all: an endpoint of a
    /// channel that may not has no mode and reports none. See
    /// [`materialized_stream_name_output_mode`].
    pub stream_name_output_mode: Option<String>,
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

    /// The manifest names this endpoint publishes, in the families a harvest job can name
    /// — the set a job's `HarvestedManifests` is resolved against.
    pub fn published_manifests(&self) -> PublishedManifests<'_> {
        PublishedManifests {
            hls: &self.hls_manifests,
            low_latency_hls: &self.low_latency_hls_manifests,
            dash: &self.dash_manifests,
        }
    }
}

/// A set of published manifests, per harvestable family: what a harvest job's
/// `HarvestedManifests` has to resolve against.
///
/// Borrowed rather than owned, so ONE predicate judges both the set an endpoint publishes
/// now ([`OriginEndpoint::published_manifests`]) and the set a write is about to leave it
/// publishing. The model's `HarvestedManifests` carries the HLS, low-latency HLS and DASH
/// families only, so an endpoint's MSS manifests are not part of the set.
#[derive(Debug, Clone, Copy)]
pub struct PublishedManifests<'a> {
    pub hls: &'a [GetHlsManifestConfiguration],
    pub low_latency_hls: &'a [GetLowLatencyHlsManifestConfiguration],
    pub dash: &'a [GetDashManifestConfiguration],
}

/// Whether `requested` names any manifest `published` does not carry.
///
/// A harvested manifest is addressed by the `ManifestName` the endpoint publishes it
/// under, per family: HLS and low-latency HLS and DASH each have their own list, so each
/// requested family is resolved against the matching one.
///
/// A PREDICATE rather than the offending name: what a caller is told is the rule that was
/// broken, so the name it sent is never needed outside this check. The ONE place the
/// reference between a harvest job and its endpoint's manifests is resolved — read by the
/// create that establishes it ([`validation_failed`] with
/// [`VALIDATION_HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT`]) and by the update that would
/// break it ([`harvest_job_harvests_unpublished_manifest`]).
pub fn names_unpublished_manifest(
    requested: &HarvestedManifests,
    published: PublishedManifests<'_>,
) -> bool {
    /// The per-family rule, stated once: a requested name resolves only if the same family
    /// publishes a manifest with that name. Both sides stay iterators, so a family is
    /// walked only until a name fails to resolve.
    fn any_unpublished<'a, Requested, Published>(
        mut requested: Requested,
        published: Published,
    ) -> bool
    where
        Requested: Iterator<Item = &'a str>,
        Published: Iterator<Item = &'a str> + Clone,
    {
        requested.any(|name| !published.clone().any(|known| known == name))
    }
    any_unpublished(
        requested
            .hls_manifests
            .iter()
            .flatten()
            .map(|manifest| manifest.manifest_name.as_str()),
        published
            .hls
            .iter()
            .map(|published| published.manifest_name.as_str()),
    ) || any_unpublished(
        requested
            .low_latency_hls_manifests
            .iter()
            .flatten()
            .map(|manifest| manifest.manifest_name.as_str()),
        published
            .low_latency_hls
            .iter()
            .map(|published| published.manifest_name.as_str()),
    ) || any_unpublished(
        requested
            .dash_manifests
            .iter()
            .flatten()
            .map(|manifest| manifest.manifest_name.as_str()),
        published
            .dash
            .iter()
            .map(|published| published.manifest_name.as_str()),
    )
}

/// Whether a harvest job of `endpoint` that has NOT finished harvests a manifest the
/// `replacement` manifest set would not publish — the stored cross-reference a write that
/// replaces the endpoint's manifests would leave pointing at nothing.
///
/// Only a job that can still harvest holds such a reference: a job in a status it can no
/// longer leave ([`HarvestJobRecord::is_terminal`]) is a record of work that is over, and
/// the manifests it names are the ones it was harvesting rather than ones it still needs.
/// Statuses the clock already owed are expected to have been materialised
/// ([`HarvestJobRecord::materialise_due_transitions`]) before this is asked.
pub fn harvest_job_harvests_unpublished_manifest(
    endpoint: &OriginEndpoint,
    replacement: PublishedManifests<'_>,
) -> bool {
    endpoint
        .harvest_jobs
        .values()
        .filter(|job| !job.is_terminal())
        .any(|job| names_unpublished_manifest(&job.harvested_manifests, replacement))
}

/// The shortest time a harvest job spends in each status it has yet to leave.
///
/// AWS does not publish this latency, so it is the EMULATOR's own span, inside the
/// 250–750 ms band the project fixes for a transition a call starts: a caller that polls
/// straight after `CreateHarvestJob` still observes `QUEUED`, and a job whose scheduled
/// window has already passed by the time it was created is still observed `IN_PROGRESS`
/// before it ends, instead of crossing both transitions at one instant.
const HARVEST_JOB_TRANSITION_MILLIS: i64 = 400;

/// `at`, moved on by one [`HARVEST_JOB_TRANSITION_MILLIS`] span.
fn after_harvest_job_span(at: ::aws_smithy_types::DateTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(
        at.as_secs_f64() + (HARVEST_JOB_TRANSITION_MILLIS as f64 / 1000.0),
    )
}

/// Whichever of two instants comes later.
///
/// The modelled timestamp is compared through its epoch-seconds value, the same way every
/// other instant in this crate is ordered.
fn later_instant(
    one: ::aws_smithy_types::DateTime,
    other: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    if other.as_secs_f64() > one.as_secs_f64() {
        other
    } else {
        one
    }
}

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
    /// When the clock owes this job its `QUEUED` -> `IN_PROGRESS` transition: the start of
    /// the window the caller scheduled, never sooner than one span after the create (see
    /// [`harvest_job_in_progress_at`]).
    pub in_progress_at: ::aws_smithy_types::DateTime,
    /// See [`ChannelGroup::version`].
    pub version: u64,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this job carried, if any — what an SDK's
    /// automatic retry of that create sends again.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this job, so a repeat carrying
    /// the same token can be told apart from a changed one.
    pub fingerprint: String,
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

    /// When the clock owes this job its exit from `IN_PROGRESS`.
    ///
    /// The end of the window the CALLER scheduled, the same half of
    /// `ScheduleConfiguration` that [`harvest_job_in_progress_at`] takes the start from —
    /// but never before the job has been `IN_PROGRESS` for one
    /// [`HARVEST_JOB_TRANSITION_MILLIS`] span, so a job whose window had already ended
    /// when it was created still passes through the status it is leaving.
    fn terminal_at(&self) -> ::aws_smithy_types::DateTime {
        later_instant(
            after_harvest_job_span(self.in_progress_at),
            self.schedule_configuration.end_time,
        )
    }

    /// Materialise the status changes the CLOCK already owed this job by `at`.
    ///
    /// Called by the reporting operations as well as the mutating ones, because each of
    /// these transitions was due at the same instant whether or not anybody read the job.
    /// BOTH are decided by the window the CALLER scheduled: `QUEUED` -> `IN_PROGRESS`
    /// when that window begins ([`harvest_job_in_progress_at`]) and `IN_PROGRESS` ->
    /// `FAILED` when it ends ([`HarvestJobRecord::terminal_at`], and see
    /// [`HARVESTER_UNAVAILABLE_MESSAGE`]), each no sooner than one
    /// [`HARVEST_JOB_TRANSITION_MILLIS`] span after the job entered the status it leaves.
    ///
    /// `at` decides only WHETHER each transition has fallen due; the change is committed
    /// with the instant it fell DUE, so `ModifiedAt` and the version the job reports are a
    /// function of its own schedule rather than of when a client happened to look.
    pub fn materialise_due_transitions(&mut self, at: ::aws_smithy_types::DateTime) {
        let in_progress_at = self.in_progress_at;
        if self.status == HARVEST_JOB_QUEUED && at.as_secs_f64() >= in_progress_at.as_secs_f64() {
            self.status = HARVEST_JOB_IN_PROGRESS.to_string();
            self.commit(in_progress_at);
        }
        let terminal_at = self.terminal_at();
        if self.status == HARVEST_JOB_IN_PROGRESS && at.as_secs_f64() >= terminal_at.as_secs_f64() {
            self.status = HARVEST_JOB_FAILED.to_string();
            self.error_message = Some(HARVESTER_UNAVAILABLE_MESSAGE.to_string());
            self.commit(terminal_at);
        }
    }
}

/// The instant the clock owes `IN_PROGRESS` to a harvest job created at `created_at` for
/// a window that begins at `start_time`.
///
/// The schedule the caller submitted is WHEN THE JOB RUNS
/// (`CreateHarvestJobRequest$ScheduleConfiguration`: "The configuration for when the
/// harvest job should run, including start and end times"), so a job whose window has not
/// begun stays `QUEUED` until it does; a job whose window has already begun waits only the
/// emulator's own [`HARVEST_JOB_TRANSITION_MILLIS`] span, which is never zero. The end of
/// the same window is read by [`HarvestJobRecord::terminal_at`].
pub fn harvest_job_in_progress_at(
    created_at: ::aws_smithy_types::DateTime,
    start_time: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    later_instant(after_harvest_job_span(created_at), start_time)
}

/// The input-switch configuration a channel is stored with.
///
/// MediaPackage materialises the block on the server side, so a channel created without
/// one still has a switching state to report: the submitted block if there is one, with
/// `MQCSInputSwitching` filled in from the model's documented default ("Default is
/// false") when the caller left it out. The recorded service reports the member on a
/// channel of ANY input type, not only on one that may carry a SUBMITTED one, which is
/// what tells this member apart from [`materialized_output_locking_mode`].
/// `PreferredInput` has no documented default and the service reports none, so an omitted
/// one stays omitted.
pub fn materialized_input_switch_configuration(
    submitted: Option<InputSwitchConfiguration>,
) -> InputSwitchConfiguration {
    let mut configuration = submitted.unwrap_or_default();
    configuration.mqcs_input_switching = Some(configuration.mqcs_input_switching.unwrap_or(false));
    configuration
}

/// The output-header configuration a channel is stored with — the same materialisation
/// as [`materialized_input_switch_configuration`], one member wide: CMSD header
/// publication is off unless the caller turned it on. The off state is the one the
/// recorded service reports for a channel created without the block, on a channel of any
/// input type.
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
/// channel that may not has no mode and reports none — which is what the recorded service
/// reports for such a channel. The restriction is read off the same table a SUBMITTED
/// mode is refused by ([`channel_carries_member`]), so a stored mode and a refused one can
/// never disagree about which input types carry the member. The default is also the one
/// value every input type that carries the member may have, so applying it can never
/// store the value [`channel_output_locking_mode_value_violation`] refuses.
pub fn materialized_output_locking_mode(
    submitted: Option<String>,
    input_type: &str,
) -> Option<String> {
    if !channel_carries_member(|members| members.output_locking_mode = true, input_type) {
        return None;
    }
    Some(submitted.unwrap_or_else(|| DEFAULT_OUTPUT_LOCKING_MODE.to_string()))
}

/// Whether an origin endpoint on a channel with this output locking mode may carry a
/// segment output-timestamp mode at all.
///
/// `Segment$OutputTimestampMode` is "only configurable on channels with
/// `OutputLockingMode` set to `NON_EPOCH_LOCKED`", and a channel that may not carry an
/// output locking mode at all ([`materialized_output_locking_mode`]) carries none — so
/// its endpoints may not carry the mode either. The ONE reading of that restriction:
/// both the materialisation of the member's documented default
/// ([`materialized_segment`]) and the refusal of a SUBMITTED mode
/// ([`output_timestamp_mode_violation`]) decide through it, so a stored mode and a
/// refused one can never disagree about which channels carry the member.
pub fn channel_carries_output_timestamp_mode(channel_output_locking_mode: Option<&str>) -> bool {
    channel_output_locking_mode == Some(OUTPUT_TIMESTAMP_MODE_LOCKING_MODE)
}

/// The refusal for a request that names a segment output-timestamp mode on a channel
/// whose output locking mode may not carry one, or `None` when the channel may — the
/// `ValidationExceptionType` the model declares for exactly this combination.
pub fn output_timestamp_mode_violation(
    submitted: Option<&Segment>,
    channel_output_locking_mode: Option<&str>,
) -> Option<ValidationException> {
    let names_mode = submitted.is_some_and(|segment| segment.output_timestamp_mode.is_some());
    if !names_mode || channel_carries_output_timestamp_mode(channel_output_locking_mode) {
        return None;
    }
    Some(validation_failed(
        "segment.outputTimestampMode",
        &format!(
            "OutputTimestampMode can only be configured when the channel OutputLockingMode \
             is {OUTPUT_TIMESTAMP_MODE_LOCKING_MODE}. To use OutputTimestampMode, create an \
             origin endpoint on a channel with {OUTPUT_TIMESTAMP_MODE_LOCKING_MODE} output \
             locking mode"
        ),
        VALIDATION_ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE,
    ))
}

/// The segment configuration an origin endpoint is stored with.
///
/// `Segment` is required on every origin-endpoint response and optional on the request,
/// so the service applies its own segmentation settings to whatever the caller omitted
/// and reports the result: the duration and the base name from
/// [`DEFAULT_SEGMENT_DURATION_SECONDS`] and [`DEFAULT_SEGMENT_NAME`], the TS passthrough
/// toggles on and the extra I-frame-only stream off — the state the recorded service
/// reports for an endpoint created without a segment configuration — and
/// [`DEFAULT_OUTPUT_TIMESTAMP_MODE`] on an endpoint whose channel may carry an
/// output-timestamp mode. `Scte` and `Encryption` are reported only when they were
/// configured, so they pass through untouched.
///
/// The output-timestamp mode is materialised the way a channel's own output locking mode
/// is: the member has a documented service-applied default AND the model restricts which
/// parents may carry it, so the default is applied exactly where the endpoint's channel
/// may carry one ([`channel_carries_output_timestamp_mode`]) and the member stays absent
/// on an endpoint whose channel may not. Every endpoint that can report a mode therefore
/// holds a concrete one from its create onwards, which is what makes a member documented
/// "immutable after endpoint creation" immutable from the first write on.
pub fn materialized_segment(
    submitted: Option<Segment>,
    channel_output_locking_mode: Option<&str>,
) -> Segment {
    let mut segment = submitted.unwrap_or_default();
    let submitted_output_timestamp_mode = segment.output_timestamp_mode.take();
    segment.output_timestamp_mode = channel_carries_output_timestamp_mode(
        channel_output_locking_mode,
    )
    .then(|| {
        submitted_output_timestamp_mode
            .unwrap_or_else(|| DEFAULT_OUTPUT_TIMESTAMP_MODE.to_string())
    });
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
/// the service's own ([`DEFAULT_STARTOVER_WINDOW_SECONDS`]) — on an endpoint whose parent
/// channel's input type may carry a window at all
/// ([`input_type_carries_startover_window`]), and `None` on one whose may not, the way
/// [`materialized_stream_name_output_mode`] withholds the other restricted member with a
/// documented default. Shared by the create and the update so one write cannot leave the
/// endpoint reporting a window the other would not.
pub fn materialized_startover_window_seconds(
    submitted: Option<i32>,
    input_type: &str,
) -> Option<i32> {
    if !input_type_carries_startover_window(input_type) {
        return None;
    }
    Some(submitted.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS))
}

/// The URI separator an origin endpoint is stored with — the submitted one, else
/// [`DEFAULT_URI_SEPARATOR`].
pub fn materialized_uri_separator(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_URI_SEPARATOR.to_string())
}

/// The stream-name output mode an origin endpoint is stored with.
///
/// The model documents a service-applied default for the member ("If you don't specify a
/// value, the default is `INDEX`") AND restricts which channel input types may carry it at
/// all ("valid only when the associated channel's `InputType` is `HLS`"), so the default is
/// materialised only under a channel whose input type may carry the member; an endpoint of
/// a channel that may not has no mode and reports none. The restriction is read off the
/// same place a SUBMITTED mode is refused by
/// ([`stream_name_output_mode_input_type_violation`]), so a stored mode and a refused one
/// can never disagree about which channels carry the member — exactly as
/// [`materialized_output_locking_mode`] does for the channel member documented this way.
pub fn materialized_stream_name_output_mode(
    submitted: Option<String>,
    input_type: &str,
) -> Option<String> {
    if !input_type_carries_stream_name_output_mode(input_type) {
        return None;
    }
    Some(submitted.unwrap_or_else(|| DEFAULT_STREAM_NAME_OUTPUT_MODE.to_string()))
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
/// Every level of the hierarchy this service mints an ARN for and stores a tag map on,
/// in the resource-path form the `*_arn` builders above produce. A level the service
/// tags is a level the tagging operations reach: the deepest of them, the harvest job,
/// carries `Tags` on its create and on its read just as its ancestors do.
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
    HarvestJob {
        group: String,
        channel: String,
        endpoint: String,
        job: String,
    },
}

impl TaggedResource {
    /// The channel group at the root of this resource's path — the record every
    /// taggable level of the hierarchy is reached through.
    pub fn group(&self) -> &str {
        match self {
            Self::ChannelGroup { group }
            | Self::Channel { group, .. }
            | Self::OriginEndpoint { group, .. }
            | Self::HarvestJob { group, .. } => group,
        }
    }
}

/// What a tagging `ResourceArn` names: the account and the Region the ARN is scoped to,
/// and the resource its path names inside that scope.
///
/// The account and the Region are read rather than skipped because both are part of the
/// resource's identity: the same path under another account, or in another Region (see
/// [`ChannelGroupKey`]), names a different resource, so a request can only reach the one
/// its own scope holds — [`TagTarget::reaches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTarget {
    pub account: String,
    pub region: String,
    pub resource: TaggedResource,
}

impl TagTarget {
    /// Whether a tagging request served in `region` can reach the resource this ARN
    /// names at all.
    ///
    /// An ARN addresses one resource in one account's Region-scoped namespace, and every
    /// resource this emulator holds belongs to the single account its own ARNs carry
    /// ([`::engine::emulator_account_id`]) — so an ARN naming any other account, or a
    /// Region other than the one the request is served in, names nothing that is here to
    /// report, tag or untag. The one reading of that scope, so the reporting path
    /// ([`ServiceState::resolved_group`]) and the writing path
    /// ([`ServiceState::with_tags_mut`]) can never disagree about which ARNs reach a
    /// resource.
    pub fn reaches(&self, region: &str) -> bool {
        self.account == ::engine::emulator_account_id() && self.region == region
    }
}

/// The resource a tagging `ResourceArn` names, or `None` when the string is not a
/// syntactically valid ARN for a resource of THIS service.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>` — six
/// colon-separated fields, the first literally `arn` — and this service's resource path
/// is `channelGroup/<g>[/channel/<c>[/originEndpoint/<e>[/harvestJob/<j>]]]`, one
/// `<type>/<name>` pair per level of the containment hierarchy. Every path the `*_arn`
/// builders above can produce parses back, so no ARN this service HANDED a caller is
/// refused as malformed. Anything else cannot name a MediaPackage resource, which is a
/// different failure from naming one that is absent: the caller has to fix the ARN
/// rather than create the resource.
///
/// The SCOPE fields an ARN carries — its account and its Region — are kept rather than
/// dropped, because they are part of what the ARN addresses; whether the scope they name
/// is one this emulator holds a resource in is [`TagTarget::reaches`], and an ARN of a
/// scope that holds nothing is an absent resource rather than a malformed ARN.
pub fn parse_tag_arn(arn: &str) -> Option<TagTarget> {
    // The six fields come from the SHARED ARN parse — the form a service reads its OWN
    // resource ARNs with ([`::service_common::arn::parse_of`]), over the same reader the
    // caller-supplied ARN members of this model are judged by ([`cdn_auth_rejection`]), so
    // this crate reads one ARN grammar rather than two.
    let parsed = ::service_common::arn::parse_of(arn, SIGNING_NAME)?;
    let segments: Vec<&str> = parsed.resource.split('/').collect();
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
        [
            "channelGroup",
            group,
            "channel",
            channel,
            "originEndpoint",
            endpoint,
            "harvestJob",
            job,
        ] => TaggedResource::HarvestJob {
            group: (*group).to_string(),
            channel: (*channel).to_string(),
            endpoint: (*endpoint).to_string(),
            job: (*job).to_string(),
        },
        _ => return None,
    };
    Some(TagTarget {
        account: parsed.account.to_string(),
        region: parsed.region.to_string(),
        resource,
    })
}

/// Whether `policy` lets an UNAUTHENTICATED caller perform `action`.
///
/// An anonymous request is the `*` principal, so a statement GRANTS it only by naming
/// `*` as its principal. `Deny` wins over `Allow`, as in IAM. Used by the egress data
/// plane, where the endpoint's resource policy is the only thing that can authorize a
/// player with no credentials.
///
/// This evaluator understands `Effect`, `Action`, `Principal` and `Resource` only, so it
/// fails CLOSED on anything else — in BOTH directions, which are opposite readings of the
/// same ignorance. A statement carrying a member it cannot evaluate
/// ([`UNEVALUATED_STATEMENT_MEMBERS`] — `Condition`, `NotAction`, `NotPrincipal`,
/// `NotResource`) never GRANTS, because honouring a conditional `Allow` as an
/// unconditional one would authorize a fetch on a condition nothing checked. A `Deny`
/// still DENIES unless a member this evaluator can judge rules this request out
/// ([`statement_excludes_anonymous_request`]), so a `Deny` written in the inverted or
/// conditional form — which reaches everything outside what it names, an anonymous player
/// included — is honoured rather than dropped.
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
    let has_effect = |statement: &::serde_json::Value, effect: &str| {
        statement.get("Effect").and_then(::serde_json::Value::as_str) == Some(effect)
    };
    // The DENY pass: an explicit `Deny` the caller stored applies to this request unless a
    // member this evaluator CAN judge rules the request out of it.
    if statements.iter().any(|statement| {
        has_effect(statement, "Deny")
            && !statement_excludes_anonymous_request(statement, action, resource)
    }) {
        return false;
    }
    // The ALLOW pass: a grant has to be stated in full, in members this evaluator judges,
    // and narrowed by nothing it cannot.
    statements.iter().any(|statement| {
        has_effect(statement, "Allow")
            && !carries_unevaluated_member(statement)
            && statement_names_action(statement, action)
            && statement_names_anonymous_principal(statement)
            && statement_names_resource(statement, resource)
    })
}

/// Whether a member this evaluator CAN judge puts this anonymous request outside the
/// statement — the reading the `Deny` pass of [`policy_allows_anonymous`] needs.
///
/// A statement reaches a request unless one of its dimensions says otherwise, so the
/// question per dimension is not "does a member name this request" but "does a member
/// this evaluator reads name something else": an `Action` list none of whose entries is
/// the action, a `Principal` naming someone other than everyone, or a `Resource` list no
/// pattern of which covers the addressed ARN. A dimension stated only in its inverted
/// form (`NotAction`, `NotPrincipal`, `NotResource`), left unstated, or written in a shape
/// this evaluator cannot read, is judged by nothing and so excludes nothing — the closed
/// side for a `Deny`, since an inverted member matches everything OUTSIDE what it names
/// and an anonymous request can be exactly that.
fn statement_excludes_anonymous_request(
    statement: &::serde_json::Value,
    action: &str,
    resource: &str,
) -> bool {
    statement_excludes_action(statement, action)
        || statement_excludes_anonymous_principal(statement)
        || statement_excludes_resource(statement, resource)
}

/// Whether the statement's `Action` names actions and none of them is `action`.
fn statement_excludes_action(statement: &::serde_json::Value, action: &str) -> bool {
    statement
        .get("Action")
        .and_then(declared_names)
        .is_some_and(|names| !names.iter().any(|name| action_name_matches(name, action)))
}

/// Whether the statement's `Principal` names principals and everyone is not among them.
fn statement_excludes_anonymous_principal(statement: &::serde_json::Value) -> bool {
    statement.get("Principal").is_some_and(|declared| {
        principal_is_readable(declared) && !statement_names_anonymous_principal(statement)
    })
}

/// Whether a `Principal` is written in a form this evaluator reads in FULL, so that a
/// value of it not being `*` is a judgement rather than a gap: the bare string, an array
/// of strings, or a map of either ([`statement_names_anonymous_principal`]).
fn principal_is_readable(declared: &::serde_json::Value) -> bool {
    let names_principals = |value: &::serde_json::Value| match value {
        ::serde_json::Value::String(_) => true,
        ::serde_json::Value::Array(list) => {
            list.iter().all(::serde_json::Value::is_string)
        }
        _ => false,
    };
    match declared {
        ::serde_json::Value::Object(map) => map.values().all(names_principals),
        other => names_principals(other),
    }
}

/// Whether the statement's `Resource` names patterns and none of them covers `resource`.
fn statement_excludes_resource(statement: &::serde_json::Value, resource: &str) -> bool {
    statement
        .get("Resource")
        .and_then(declared_names)
        .is_some_and(|patterns| {
            !patterns
                .iter()
                .any(|pattern| arn_pattern_matches(pattern, resource))
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
    declared_names(declared).is_some_and(|patterns| {
        patterns
            .iter()
            .any(|pattern| arn_pattern_matches(pattern, resource))
    })
}

/// The names a statement member declares, or `None` when the member is written in a shape
/// this evaluator cannot read as a list of names.
///
/// IAM writes `Action` and `Resource` either as one string or as an array of them, and
/// every member of such a list is read the same way whichever member carries it. A list
/// is read only when EVERY entry is a name: one entry this evaluator cannot read could
/// have been the name that decides the statement, so the member as a whole is a gap rather
/// than a shorter list.
fn declared_names(declared: &::serde_json::Value) -> Option<Vec<&str>> {
    match declared {
        ::serde_json::Value::String(one) => Some(vec![one.as_str()]),
        ::serde_json::Value::Array(list) => list.iter().map(::serde_json::Value::as_str).collect(),
        _ => None,
    }
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
    statement
        .get("Action")
        .and_then(declared_names)
        .is_some_and(|names| names.iter().any(|name| action_name_matches(name, action)))
}

/// Whether one declared action name covers `action`, honouring the `*` wildcard IAM
/// allows both on its own and as a trailing one (`service:*`).
fn action_name_matches(name: &str, action: &str) -> bool {
    match name.strip_suffix('*') {
        Some(prefix) => action.starts_with(prefix),
        None => name.eq_ignore_ascii_case(action),
    }
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

/// The ingest endpoints a channel of this input type is created with, derived from the
/// parent group's egress domain and the channel's name: the redundant pair for a channel
/// that receives ingest of its own, and NONE for one that does not (see
/// [`is_multiview_input_type`]) — a channel with no ingest stream of its own has no URL
/// for a contributing encoder to push to, and `IngestEndpoints` carries no `@required` on
/// any channel shape, so reporting none is what the model represents that with.
pub fn ingest_endpoints(
    egress_domain: &str,
    group: &str,
    channel: &str,
    input_type: &str,
) -> Vec<IngestEndpoint> {
    if is_multiview_input_type(input_type) {
        return Vec::new();
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
/// [`DEFAULT_URI_PATH_TYPE`], [`DEFAULT_CHILD_MANIFEST_NAME`], and child-manifest URL
/// encoding off).
pub fn stored_hls_manifest(
    submitted: CreateHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetHlsManifestConfiguration {
    GetHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: Some(
            submitted
                .child_manifest_name
                .unwrap_or_else(|| DEFAULT_CHILD_MANIFEST_NAME.to_string()),
        ),
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
/// manifest-setting defaults, child manifest name included.
pub fn stored_low_latency_hls_manifest(
    submitted: CreateLowLatencyHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetLowLatencyHlsManifestConfiguration {
    GetLowLatencyHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: Some(
            submitted
                .child_manifest_name
                .unwrap_or_else(|| DEFAULT_CHILD_MANIFEST_NAME.to_string()),
        ),
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
///
/// A caller renders each field; an OPTIONAL member is rendered so that ABSENT and
/// present-but-default stay apart (`format!("{:?}", member)` does), because a member the
/// request leaves out is materialised to the service's own default: were the two
/// flattened to one field, a repeat that submitted that default would ask for a different
/// resource under the first attempt's fingerprint.
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

/// The 409 an update answers while a harvest job that has not finished still harvests a
/// manifest the update would stop the origin endpoint publishing.
///
/// `RESOURCE_IN_USE`, the same discriminator [`children_attached`] and
/// [`multiview_source_in_use`] use: a sub-resource that references the manifests is a use
/// of them, and the write would leave that reference pointing at nothing. The sentence
/// names what has to change first and, like every refusal here, neither the caller's names
/// nor how many references there are.
pub fn harvested_manifest_in_use() -> ConflictException {
    ConflictException {
        message: Some(
            "The origin endpoint you tried to update publishes manifests that a harvest \
             job which hasn't finished is harvesting. If you want to update this origin \
             endpoint, you must first cancel the harvest jobs that harvest the manifests \
             you're removing"
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

/// Every `InputType` restriction the model states on the PRESENCE of a channel member,
/// one row per `ValidationExceptionType` that names one. A restriction on a member's
/// VALUE rather than on its presence is stated beside it, in
/// [`channel_output_locking_mode_value_violation`].
///
/// The rows are read off the model and off the refusal the service itself answers, not
/// guessed. `MQCSInputSwitching` / `PreferredInput` / `PublishMQCS` carry "valid only when
/// InputType is CMAF" and `MultiviewConfiguration` "required when InputType is MULTIVIEW,
/// and can't be set for any other input type" in their own member docs. For
/// `OutputLockingMode` the recorded refusal names the input types that may configure the
/// member — "OutputLockingMode can only be configured when the channel InputType is CMAF
/// or MULTIVIEW" — which is also the only reading that leaves the model's
/// `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` discriminator something to refuse: a
/// multiview channel carries the member, at one of its two values only
/// ([`channel_output_locking_mode_value_violation`]). `ValidationExceptionType` carries
/// exactly one discriminator per row.
const CHANNEL_INPUT_TYPE_RULES: &[ChannelInputTypeRule] = &[
    ChannelInputTypeRule {
        member: OUTPUT_LOCKING_MODE_MEMBER,
        allowed: &["CMAF", INPUT_TYPE_MULTIVIEW],
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

/// Whether a channel of this input type composites video from the source channels in its
/// multiview configuration instead of receiving ingest of its own, which is what the
/// model's `InputType` doc says a [`INPUT_TYPE_MULTIVIEW`] channel does ("The channel
/// receives no ingest of its own").
///
/// Takes the input type rather than a stored channel, so the create path — which settles
/// the materialised input type before any channel record exists — decides it from the
/// same place [`Channel::is_multiview`] does.
pub fn is_multiview_input_type(input_type: &str) -> bool {
    input_type == INPUT_TYPE_MULTIVIEW
}

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

/// Whether a channel of this input type may carry a restricted member at all.
///
/// `member` marks the ONE member it is asked about on an otherwise empty
/// [`SubmittedChannelMembers`], so the answer comes off the same table a SUBMITTED member
/// is refused by ([`channel_input_type_violation`]). A materialisation that WITHHOLDS a
/// member from a channel whose input type may not carry it decides through here —
/// [`materialized_output_locking_mode`] does, for the one channel member the model
/// documents with both a default and that restriction — so a stored member and a refused
/// one can never disagree about which input types carry it.
pub fn channel_carries_member(member: fn(&mut SubmittedChannelMembers), input_type: &str) -> bool {
    let mut submitted = SubmittedChannelMembers::default();
    member(&mut submitted);
    channel_input_type_violation(submitted, input_type).is_none()
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

/// The member path of a channel's output locking mode, as a validation message names it
/// (lowerCamel, as on the wire). Written once: the member is restricted twice, on its
/// PRESENCE by [`CHANNEL_INPUT_TYPE_RULES`] and on its VALUE by
/// [`channel_output_locking_mode_value_violation`], and both refusals path it the same.
const OUTPUT_LOCKING_MODE_MEMBER: &str = "outputLockingMode";

/// The `OutputLockingMode` no channel of a multiview composition may have.
///
/// A multiview channel composites tiles from several channels into one output stream,
/// which only lines up while every stream in the composition carries the deterministic,
/// epoch-aligned segment boundaries the other mode does not: the model's
/// `OutputLockingMode` doc gives `EPOCH_LOCKED` "deterministic sequence numbering and
/// fixed segment boundaries aligned to epoch time" and this value "duration-based segment
/// combining and monotonically increasing sequence numbers starting from 0". The model
/// names the refusal once per side of the composition —
/// `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` for the multiview channel itself
/// ([`channel_output_locking_mode_value_violation`]) and `MULTIVIEW_SOURCE_NON_EPOCH_LOCKED`
/// for a tile source it names ([`multiview_source_violation`]) — so both read this one
/// value and cannot disagree about which mode a composition admits.
pub const MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE: &str = "NON_EPOCH_LOCKED";

/// The refusal for a request that asks for an output locking mode the channel's input
/// type may not have, or `None` when it may.
///
/// The VALUE half of the member's restriction: [`channel_input_type_violation`] decides
/// which input types carry the member at all, and a member an input type DOES carry can
/// still be restricted to some of its values, with its own `ValidationExceptionType` —
/// which is the only kind of refusal `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` can
/// name, since a multiview channel carries the member. Decided from the request and the
/// input type alone, so the create settles it before anything is stored; `UpdateChannel`
/// does not bind the member, so no later write can introduce the refused value.
pub fn channel_output_locking_mode_value_violation(
    submitted: Option<&str>,
    input_type: &str,
) -> Option<ValidationException> {
    let refused = submitted == Some(MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE)
        && is_multiview_input_type(input_type);
    refused.then(|| {
        validation_failed(
            OUTPUT_LOCKING_MODE_MEMBER,
            &format!(
                "OutputLockingMode can't be {MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE} when \
                 the channel InputType is {INPUT_TYPE_MULTIVIEW}. {CHECK_THE_VALUE}"
            ),
            VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED,
        )
    })
}

/// Which of an origin endpoint's restricted settings a request carries.
///
/// An endpoint setting can be restricted by its PARENT channel's input type rather than by
/// the endpoint's own members, which is why those restrictions are stated here rather than
/// in [`CHANNEL_INPUT_TYPE_RULES`]: they are read against the input type of the channel the
/// endpoint hangs under. Like [`SubmittedChannelMembers`] one level up, this is about the
/// REQUEST rather than about either operation's input shape, so `CreateOriginEndpoint` and
/// `UpdateOriginEndpoint` reduce their own generated inputs to it and the rule table
/// ([`ORIGIN_ENDPOINT_INPUT_TYPE_RULES`]) is written once.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubmittedOriginEndpointMembers {
    pub stream_name_output_mode: bool,
    pub force_endpoint_error_configuration: bool,
    /// Whether the request names the ISM container type: the restriction is on the VALUE
    /// of a required member rather than on the member being present at all.
    pub ism_container_type: bool,
    pub low_latency_hls_manifests: bool,
    pub mss_manifests: bool,
    pub startover_window_seconds: bool,
}

/// An origin-endpoint setting the service only lets the endpoints of particular parent
/// `InputType`s carry. The mirror of [`ChannelInputTypeRule`] one level down, without the
/// required-on-that-input-type direction: no endpoint setting is documented as required by
/// an input type.
struct OriginEndpointInputTypeRule {
    /// The member, as a validation message names it (lowerCamel, as on the wire).
    member: &'static str,
    /// The `InputType` values whose channels may carry it, from the model's `InputType`
    /// enum.
    allowed: &'static [&'static str],
    /// The `ValidationExceptionType` the model declares for this combination.
    exception_type: &'static str,
    /// What the setting is called in the sentence the refusal states. Written to read
    /// inside a sentence, which is where it lands: [`validation_message`] states the
    /// member path ahead of it.
    label: &'static str,
    /// Whether the request carries the setting at all.
    present: fn(&SubmittedOriginEndpointMembers) -> bool,
}

/// The `InputType` values whose channels are NOT the one that composites its video from
/// other channels ([`INPUT_TYPE_MULTIVIEW`]), from the model's `InputType` enum — the
/// input types a setting a multiview channel's endpoint may not carry is allowed under.
/// Written once, so the rows of [`ORIGIN_ENDPOINT_INPUT_TYPE_RULES`] that share that
/// restriction cannot drift apart.
const INPUT_TYPES_EXCEPT_MULTIVIEW: &[&str] = &["HLS", "CMAF"];

/// Every parent-`InputType` restriction the model states for an origin endpoint's
/// settings, one row per `ValidationExceptionType` that names one.
///
/// The rows are read off the model rather than guessed: `StreamNameOutputMode`'s own doc
/// says the setting "is valid only when the associated channel's `InputType` is `HLS`",
/// and `ValidationExceptionType` names one value per remaining combination —
/// `ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION` for the failover
/// settings, and `MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER`,
/// `MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST`, `MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST`
/// and `MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED` for the four an endpoint of a channel that
/// composites its tiles from other channels may not carry — so those four are allowed
/// under [`INPUT_TYPES_EXCEPT_MULTIVIEW`].
///
/// The rows are walked in this order, so the ISM container type — a required member, and
/// what the endpoint IS — is reported before the MSS manifests only an ISM container
/// serves ([`container_manifest_conflict`]).
const ORIGIN_ENDPOINT_INPUT_TYPE_RULES: &[OriginEndpointInputTypeRule] = &[
    OriginEndpointInputTypeRule {
        member: "streamNameOutputMode",
        allowed: &["HLS"],
        exception_type: "ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE",
        label: "StreamNameOutputMode",
        present: |submitted| submitted.stream_name_output_mode,
    },
    OriginEndpointInputTypeRule {
        member: "forceEndpointErrorConfiguration",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION",
        label: "ForceEndpointErrorConfiguration",
        present: |submitted| submitted.force_endpoint_error_configuration,
    },
    OriginEndpointInputTypeRule {
        member: "containerType",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER",
        label: "an ISM container type",
        present: |submitted| submitted.ism_container_type,
    },
    OriginEndpointInputTypeRule {
        member: "lowLatencyHlsManifests",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST",
        label: "a low-latency HLS manifest",
        present: |submitted| submitted.low_latency_hls_manifests,
    },
    OriginEndpointInputTypeRule {
        member: "mssManifests",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST",
        label: "an MSS manifest",
        present: |submitted| submitted.mss_manifests,
    },
    OriginEndpointInputTypeRule {
        member: "startoverWindowSeconds",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED",
        label: "StartoverWindowSeconds",
        present: |submitted| submitted.startover_window_seconds,
    },
];

/// The first parent-`InputType` restriction a request breaks, as the modeled
/// `ValidationException` to answer it with, or `None` when every setting it carries is
/// allowed under `input_type`.
///
/// The sentence is the one every other input-type restriction of this service states
/// ([`channel_input_type_violation`]), off the same table the settings an endpoint is
/// STORED with are materialised from ([`origin_endpoint_carries_member`]). Shared by the
/// create and the update so the two cannot disagree about which channels carry what.
pub fn origin_endpoint_input_type_violation(
    submitted: SubmittedOriginEndpointMembers,
    input_type: &str,
) -> Option<ValidationException> {
    ORIGIN_ENDPOINT_INPUT_TYPE_RULES
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

/// Whether an endpoint of a channel of this input type may carry a restricted setting at
/// all.
///
/// `member` marks the ONE setting it is asked about on an otherwise empty
/// [`SubmittedOriginEndpointMembers`], so the answer comes off the same table a SUBMITTED
/// setting is refused by ([`origin_endpoint_input_type_violation`]) — exactly as
/// [`channel_carries_member`] does one level up. A materialisation that WITHHOLDS a
/// setting from an endpoint whose parent input type may not carry it decides through here,
/// so a stored setting and a refused one can never disagree.
pub fn origin_endpoint_carries_member(
    member: fn(&mut SubmittedOriginEndpointMembers),
    input_type: &str,
) -> bool {
    let mut submitted = SubmittedOriginEndpointMembers::default();
    member(&mut submitted);
    origin_endpoint_input_type_violation(submitted, input_type).is_none()
}

/// Whether a channel of this input type may carry an origin endpoint's stream-name output
/// mode at all.
pub fn input_type_carries_stream_name_output_mode(input_type: &str) -> bool {
    origin_endpoint_carries_member(
        |members| members.stream_name_output_mode = true,
        input_type,
    )
}

/// Whether an endpoint of a channel of this input type may carry a startover window at
/// all.
pub fn input_type_carries_startover_window(input_type: &str) -> bool {
    origin_endpoint_carries_member(
        |members| members.startover_window_seconds = true,
        input_type,
    )
}

/// The refusal a SUBMITTED stream-name output mode earns under a channel whose input type
/// may not carry the member, or `None` when the request names no mode or the channel may
/// carry one.
///
/// The one row of [`ORIGIN_ENDPOINT_INPUT_TYPE_RULES`] that is about this member, kept as
/// its own entry point because the update reads it beside the member's immutability.
pub fn stream_name_output_mode_input_type_violation(
    submitted: Option<&str>,
    input_type: &str,
) -> Option<ValidationException> {
    origin_endpoint_input_type_violation(
        SubmittedOriginEndpointMembers {
            stream_name_output_mode: submitted.is_some(),
            ..SubmittedOriginEndpointMembers::default()
        },
        input_type,
    )
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
/// the model's not-found discriminator, one whose channel is not of the input type a
/// tile source must have is its invalid-input-type one
/// (`MultiviewConfiguration$AvailableSources`: "Each source channel must be in the same
/// channel group as the multiview channel, and must have an `InputType` of `CMAF`"), and
/// one whose channel is locked the way no channel of a composition may be
/// ([`MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE`]) is the source side of that same rule. The
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
        // The other side of the composition's one locking rule: a tile source whose own
        // output locking mode is the one no channel of a composition may have cannot be
        // composited, which is the refusal the model names for a source.
        if channel.output_locking_mode.as_deref() == Some(MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE) {
            return Some(validation_failed(
                MULTIVIEW_SOURCES_MEMBER,
                &format!(
                    "A source channel of a multiview channel can't have an \
                     OutputLockingMode of {MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE}. \
                     {CHECK_THE_VALUE}"
                ),
                VALIDATION_MULTIVIEW_SOURCE_NON_EPOCH_LOCKED,
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

/// The member a paged list carries its continuation token in, as the wire names it
/// (`httpQuery("nextToken")` on every list's `NextToken`) and therefore as a validation
/// message paths it.
const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The modeled 400 a paged list refuses a token this service did not issue with.
///
/// Every paged list answers this one class, so the member path, the detail and the
/// discriminator are assembled here once rather than per list: four lists refusing one
/// class cannot then word it four ways.
pub fn invalid_pagination_token() -> ValidationException {
    validation_failed(
        NEXT_TOKEN_MEMBER,
        INVALID_PAGINATION_TOKEN_DETAIL,
        VALIDATION_INVALID_PAGINATION_TOKEN,
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

    /// The container format this family's manifests are published in — the suffix their
    /// egress URL ends in. The HLS families share it: both publish an `.m3u8` playlist.
    pub fn kind(self) -> ManifestKind {
        match self {
            Self::Hls | Self::LowLatencyHls => ManifestKind::Hls,
            Self::Dash => ManifestKind::Dash,
            Self::Mss => ManifestKind::Mss,
        }
    }
}

/// How many manifests a request carries per family, in [`ManifestFamily::ALL`] order —
/// the array [`manifest_set_violation`] reads.
///
/// Declared here, beside `ManifestFamily::ALL`, because the array's POSITIONS are what
/// carry the family: the counts are placed by mapping over `ALL` itself, so the order of
/// the array can only ever be the order that enum declares, and a family added to it is a
/// non-exhaustive `match` here rather than a count silently read as another family's.
/// `CreateOriginEndpoint` and `UpdateOriginEndpoint` both write the whole manifest set and
/// both count it through here. A family the request left out carries no manifest, which is
/// the same count as an empty list.
pub fn manifest_counts<Hls, LowLatencyHls, Dash, Mss>(
    hls_manifests: Option<&[Hls]>,
    low_latency_hls_manifests: Option<&[LowLatencyHls]>,
    dash_manifests: Option<&[Dash]>,
    mss_manifests: Option<&[Mss]>,
) -> [usize; ManifestFamily::ALL.len()] {
    ManifestFamily::ALL.map(|family| match family {
        ManifestFamily::Hls => hls_manifests.map_or(0, <[Hls]>::len),
        ManifestFamily::LowLatencyHls => {
            low_latency_hls_manifests.map_or(0, <[LowLatencyHls]>::len)
        }
        ManifestFamily::Dash => dash_manifests.map_or(0, <[Dash]>::len),
        ManifestFamily::Mss => mss_manifests.map_or(0, <[Mss]>::len),
    })
}

/// The manifest names a request carries, in [`ManifestFamily::ALL`] order and, within a
/// family, in the order the request listed them — what [`manifest_name_collision`] reads.
///
/// Declared beside [`manifest_counts`] and built the same way, by mapping over `ALL`
/// itself, so the positions of the array can only ever be the order that enum declares.
fn submitted_manifest_names(
    hls_manifests: Option<&[CreateHlsManifestConfiguration]>,
    low_latency_hls_manifests: Option<&[CreateLowLatencyHlsManifestConfiguration]>,
    dash_manifests: Option<&[CreateDashManifestConfiguration]>,
    mss_manifests: Option<&[CreateMssManifestConfiguration]>,
) -> [Vec<String>; ManifestFamily::ALL.len()] {
    ManifestFamily::ALL.map(|family| match family {
        ManifestFamily::Hls => hls_manifests.map_or_else(Vec::new, |manifests| {
            manifests
                .iter()
                .map(|manifest| manifest.manifest_name.clone())
                .collect()
        }),
        ManifestFamily::LowLatencyHls => {
            low_latency_hls_manifests.map_or_else(Vec::new, |manifests| {
                manifests
                    .iter()
                    .map(|manifest| manifest.manifest_name.clone())
                    .collect()
            })
        }
        ManifestFamily::Dash => dash_manifests.map_or_else(Vec::new, |manifests| {
            manifests
                .iter()
                .map(|manifest| manifest.manifest_name.clone())
                .collect()
        }),
        ManifestFamily::Mss => mss_manifests.map_or_else(Vec::new, |manifests| {
            manifests
                .iter()
                .map(|manifest| manifest.manifest_name.clone())
                .collect()
        }),
    })
}

/// The `ValidationExceptionType` a manifest set that uses one name twice carries, from the
/// model's `ValidationExceptionType` enum.
pub const VALIDATION_MANIFEST_NAME_COLLISION: &str = "MANIFEST_NAME_COLLISION";

/// The second manifest of a submitted set that would be published at a URL another
/// manifest of the same set is already published at, as the modeled `ValidationException`
/// to answer it with, or `None` when every manifest the request carries has an egress URL
/// of its own.
///
/// A manifest name is "a short string that's appended to the endpoint URL" that "creates a
/// unique path to this endpoint", so it is what makes the URL this service computes and
/// persists for a manifest ([`manifest_url`], through [`stored_hls_manifest`] and its
/// siblings) identify THAT manifest. Two manifests collide exactly when that URL is the
/// same one, which is when they share a name AND a container format
/// ([`ManifestFamily::kind`]) — so the namespace a name is taken from is the endpoint's,
/// not one family's: the HLS and low-latency HLS families both publish an `.m3u8`
/// playlist, which is why the model documents that you "can't use the same manifest name
/// if you use HLS manifest and low-latency HLS manifest"
/// (`CreateHlsManifestConfiguration$ManifestName`) and names
/// `MANIFEST_NAME_COLLISION` for it. The set is walked in [`ManifestFamily::ALL`] order,
/// so the refusal is a function of the request and not of which family the service happens
/// to read first. About the request alone, and the same rule for `CreateOriginEndpoint` and
/// `UpdateOriginEndpoint`, which both write the endpoint's whole manifest set.
pub fn manifest_name_collision(
    hls_manifests: Option<&[CreateHlsManifestConfiguration]>,
    low_latency_hls_manifests: Option<&[CreateLowLatencyHlsManifestConfiguration]>,
    dash_manifests: Option<&[CreateDashManifestConfiguration]>,
    mss_manifests: Option<&[CreateMssManifestConfiguration]>,
) -> Option<ValidationException> {
    let names = submitted_manifest_names(
        hls_manifests,
        low_latency_hls_manifests,
        dash_manifests,
        mss_manifests,
    );
    let mut taken: BTreeSet<(ManifestKind, &str)> = BTreeSet::new();
    for (family, names) in ManifestFamily::ALL.into_iter().zip(names.iter()) {
        for (index, name) in names.iter().enumerate() {
            if taken.insert((family.kind(), name.as_str())) {
                continue;
            }
            return Some(validation_failed(
                &format!("{}.manifestName", element_path(family.member(), index)),
                &format!(
                    "Each manifest of an origin endpoint must have a manifest name no \
                     other manifest published in the same format uses. {CHECK_THE_VALUE}"
                ),
                VALIDATION_MANIFEST_NAME_COLLISION,
            ));
        }
    }
    None
}

/// The `ContainerType` whose endpoints package Microsoft Smooth Streaming, from the
/// model's `ContainerType` enum.
///
/// The one container the model singles out by name in its `ValidationExceptionType`
/// values (`ISM_CONTAINER_WITH_KEY_ROTATION`,
/// `ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE`,
/// `MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER`), so the value is written once and the rules
/// that read it cannot drift.
pub const CONTAINER_TYPE_ISM: &str = "ISM";

/// Whether this container type is the Microsoft Smooth Streaming one.
pub fn is_ism_container(container: &str) -> bool {
    container == CONTAINER_TYPE_ISM
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
/// [`ManifestFamily::ALL`] order, as [`manifest_counts`] — the one builder of that array —
/// emits it. `None` means the set is acceptable. This is the same
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
        // Rendered so that an ABSENT window and a present one stay apart: an endpoint of a
        // channel that may not carry the member has none at all.
        format!("{:?}", endpoint.startover_window_seconds),
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
        // Rendered so that an ABSENT mode and a present one stay apart: an endpoint of a
        // channel that may not carry the member has none at all.
        format!("{:?}", endpoint.stream_name_output_mode),
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

// =============================================================================
// Validation: the encryption settings an origin endpoint's segment carries
// =============================================================================

/// `ValidationExceptionType` values a refused `Segment.Encryption` carries, from the
/// model's `ValidationExceptionType` enum.
///
/// The enum names one value per way the block can be a configuration the service could not
/// act on: the encryption METHOD against the endpoint's container format
/// ([`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`]), the constant
/// initialization vector against that method ([`VALIDATION_CENC_IV_INCOMPATIBLE`]), the
/// DRM systems against it ([`VALIDATION_DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE`]), key
/// rotation against the container ([`VALIDATION_ISM_CONTAINER_WITH_KEY_ROTATION`]), the
/// encryption contract's presets against each other and against the container
/// ([`VALIDATION_ENCRYPTION_CONTRACT_UNENCRYPTED`],
/// [`VALIDATION_ENCRYPTION_CONTRACT_SHARED`],
/// [`VALIDATION_ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE`]) and the content-key
/// period timing against the SPEKE version and key rotation it needs
/// ([`VALIDATION_CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1`],
/// [`VALIDATION_CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION`]).
pub const VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH: &str =
    "ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_CENC_IV_INCOMPATIBLE: &str = "CENC_IV_INCOMPATIBLE";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE: &str =
    "DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ISM_CONTAINER_WITH_KEY_ROTATION: &str = "ISM_CONTAINER_WITH_KEY_ROTATION";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ENCRYPTION_CONTRACT_UNENCRYPTED: &str = "ENCRYPTION_CONTRACT_UNENCRYPTED";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ENCRYPTION_CONTRACT_SHARED: &str = "ENCRYPTION_CONTRACT_SHARED";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE: &str =
    "ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1: &str =
    "CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION: &str =
    "CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION";

/// The wire path of the encryption block, under the member the request carries it at.
const ENCRYPTION_MEMBER: &str = "segment.encryption";

/// The `EncryptionMethod` member an endpoint of this container type carries its method in,
/// and the container format that member names.
///
/// `EncryptionMethod` declares exactly one member per container format the service
/// packages — `TsEncryptionMethod`, `CmafEncryptionMethod`, `IsmEncryptionMethod`
/// (`smithy_query(query="shape",shape="EncryptionMethod")`) — so which of them an endpoint
/// may carry is decided by its `ContainerType` (TS | CMAF | ISM), and the model's
/// `ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH` is the refusal of any other. `None` for a
/// container value the model does not name; the boundary validator has already refused one
/// against the `ContainerType` enum.
fn encryption_method_member(container: &str) -> Option<&'static str> {
    match container {
        "TS" => Some("tsEncryptionMethod"),
        "CMAF" => Some("cmafEncryptionMethod"),
        CONTAINER_TYPE_ISM => Some("ismEncryptionMethod"),
        _ => None,
    }
}

/// Every `EncryptionMethod` member the request set, as `(member, value)` pairs in the
/// order the model declares them.
fn submitted_encryption_methods(method: &EncryptionMethod) -> Vec<(&'static str, &str)> {
    [
        ("tsEncryptionMethod", method.ts_encryption_method.as_deref()),
        (
            "cmafEncryptionMethod",
            method.cmaf_encryption_method.as_deref(),
        ),
        (
            "ismEncryptionMethod",
            method.ism_encryption_method.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(member, value)| value.map(|value| (member, value)))
    .collect()
}

/// The `DrmSystem` values an encryption method can protect content with, or an empty slice
/// for a method value the model does not name.
///
/// Each encryption method is a packaging of ONE content-protection scheme, and each DRM
/// system supports the schemes its vendor implements: `AES_128` is HLS AES-128 with a key
/// the player fetches in the clear, which is what `CLEAR_KEY_AES_128` names; `SAMPLE_AES`
/// is Apple's sample encryption, used by `FAIRPLAY`; `CENC` and `CBCS` are the two Common
/// Encryption schemes, and of them FairPlay signals only `cbcs`, while the Common
/// Encryption systems (`PLAYREADY`, `WIDEVINE`, `IRDETO`) carry both. The model's
/// `DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE` is the refusal of a pair outside that, and
/// the values are the model's own (`smithy_query(query="shape",shape="DrmSystem,
/// CmafEncryptionMethod,TsEncryptionMethod,IsmEncryptionMethod")`).
fn drm_systems_for_encryption_method(method: &str) -> &'static [&'static str] {
    match method {
        "AES_128" => &["CLEAR_KEY_AES_128"],
        "SAMPLE_AES" => &["FAIRPLAY"],
        ENCRYPTION_METHOD_CENC => &["PLAYREADY", "WIDEVINE", "IRDETO"],
        "CBCS" => &["FAIRPLAY", "PLAYREADY", "WIDEVINE", "IRDETO"],
        _ => &[],
    }
}

/// The Common Encryption method value both the CMAF and the ISM container name, from the
/// model's `CmafEncryptionMethod` and `IsmEncryptionMethod` enums.
const ENCRYPTION_METHOD_CENC: &str = "CENC";

/// The `PresetSpeke20Audio` / `PresetSpeke20Video` value that asks for ONE content key
/// across every audio and video track ("SHARED - Use the same content key for all of the
/// audio and video tracks in your stream").
const PRESET_SHARED: &str = "SHARED";

/// The preset value that asks for no encryption of its track group ("UNENCRYPTED - Don't
/// encrypt any of the audio tracks in your stream").
const PRESET_UNENCRYPTED: &str = "UNENCRYPTED";

/// The `ContentKeyPeriodTiming` values whose doc states that they signal the start and end
/// times a content key is used for, and therefore "Requires `SpekeVersion` `V2_1`" and key
/// rotation ("Signaling start and end times … also requires key rotation to be enabled",
/// `smithy_query(query="doc",shape="ContentKeyPeriodConfiguration",member="ContentKeyPeriodTiming")`).
const CONTENT_KEY_PERIOD_TIMINGS_WITH_START_END: &[&str] =
    &["START_END_ONLY", "INDEX_WITH_START_END"];

/// The `SpekeVersion` those timings require, from the model's `SpekeVersion` enum.
const SPEKE_VERSION_WITH_KEY_PERIOD_TIMING: &str = "V2_1";

/// The `SpekeVersion` an endpoint's key provider is read as when the request names none
/// ("If you don't specify a value, the default is `V2_0`").
const DEFAULT_SPEKE_VERSION: &str = "V2_0";

/// Refuse a segment whose encryption settings are a configuration the service could not
/// act on, or `None` when the block is one it can.
///
/// `Segment.Encryption` is a cross-member configuration: the method, the initialization
/// vector, the DRM systems, the encryption contract and the key-provider endpoint only make
/// sense together and against the endpoint's `ContainerType`, and the model states each way
/// they can disagree as its own `ValidationExceptionType` (see
/// [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`]) — none of which the boundary
/// validator can decide, since each is about two members at once. Shared by the writers of
/// an endpoint's segment (`CreateOriginEndpoint` and `UpdateOriginEndpoint`) so both read
/// the block the same way, and decided before anything is stored: an endpoint persisted
/// with settings the key provider could never serve publishes manifests no player can
/// decrypt.
///
/// `SpekeKeyProvider$RoleArn` is read by [`speke_role_arn_rejection`], beside the other
/// caller-supplied ARN of this model.
pub fn segment_encryption_rejection(
    segment: Option<&Segment>,
    container: &str,
) -> Option<ValidationException> {
    let encryption = segment?.encryption.as_ref()?;
    let provider = &encryption.speke_key_provider;
    // The member this endpoint's container format configures its method through. `None`
    // for a container value the model does not name — the method rules below then decide
    // nothing, while the rules that are about the block alone still do.
    let expected_member = encryption_method_member(container);
    let submitted = submitted_encryption_methods(&encryption.encryption_method);

    // The method first: every later rule is about the method the endpoint encrypts with.
    if let Some(expected_member) = expected_member {
        for (member, _) in &submitted {
            if *member != expected_member {
                return Some(validation_failed(
                    &format!("{ENCRYPTION_MEMBER}.encryptionMethod.{member}"),
                    &format!(
                        "An encryption method of a {container} container endpoint is \
                         configured through {expected_member}. {CHECK_THE_VALUE}"
                    ),
                    VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH,
                ));
            }
        }
    }
    let method = expected_member.and_then(|expected_member| {
        submitted
            .iter()
            .find_map(|(member, value)| (*member == expected_member).then_some(*value))
    });

    if let Some(method) = method {
        // Common Encryption derives each sample's initialization vector from the content
        // key, so a CENC endpoint has no constant one to be given.
        if encryption.constant_initialization_vector.is_some()
            && method == ENCRYPTION_METHOD_CENC
        {
            return Some(validation_failed(
                &format!("{ENCRYPTION_MEMBER}.constantInitializationVector"),
                &format!(
                    "A constant initialization vector can't be combined with the \
                     {method} encryption method. Remove the vector, or encrypt with a \
                     method that takes one"
                ),
                VALIDATION_CENC_IV_INCOMPATIBLE,
            ));
        }
        let carried = drm_systems_for_encryption_method(method);
        let incompatible = provider
            .drm_systems
            .iter()
            .find(|system| !carried.contains(&system.as_str()));
        // A method value the model does not name constrains nothing here; the boundary
        // validator has already refused one against the method's own enum.
        if let Some(system) = incompatible.filter(|_| !carried.is_empty()) {
            return Some(validation_failed(
                &format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.drmSystems"),
                &format!(
                    "The {system} DRM system can't be combined with the {method} \
                     encryption method. The DRM systems that method supports are {}",
                    carried.join(", ")
                ),
                VALIDATION_DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE,
            ));
        }
    }

    // Key rotation, and then the encryption contract, against the container.
    if is_ism_container(container) && encryption.key_rotation_interval_seconds.is_some() {
        return Some(validation_failed(
            &format!("{ENCRYPTION_MEMBER}.keyRotationIntervalSeconds"),
            &format!(
                "An {container} container endpoint can't rotate content keys. Remove the \
                 rotation interval, or create an origin endpoint whose container type \
                 rotates keys"
            ),
            VALIDATION_ISM_CONTAINER_WITH_KEY_ROTATION,
        ));
    }
    let contract = &provider.encryption_contract_configuration;
    let audio = contract.preset_speke20_audio.as_str();
    let video = contract.preset_speke20_video.as_str();
    let contract_member = format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.encryptionContractConfiguration");
    if audio == PRESET_UNENCRYPTED && video == PRESET_UNENCRYPTED {
        return Some(validation_failed(
            &contract_member,
            &format!(
                "An encryption contract that leaves both the audio and the video tracks \
                 {PRESET_UNENCRYPTED} encrypts nothing. Choose a preset for the audio or \
                 the video tracks, or remove the encryption settings"
            ),
            VALIDATION_ENCRYPTION_CONTRACT_UNENCRYPTED,
        ));
    }
    if (audio == PRESET_SHARED) != (video == PRESET_SHARED) {
        return Some(validation_failed(
            &contract_member,
            &format!(
                "The {PRESET_SHARED} preset uses one content key for all of the audio and \
                 video tracks, so it has to be the preset of both. {CHECK_THE_VALUE}"
            ),
            VALIDATION_ENCRYPTION_CONTRACT_SHARED,
        ));
    }
    if is_ism_container(container) && !(audio == PRESET_SHARED && video == PRESET_SHARED) {
        return Some(validation_failed(
            &contract_member,
            &format!(
                "An {container} container endpoint encrypts every track with one content \
                 key, so its encryption contract must use the {PRESET_SHARED} preset for \
                 the audio and the video tracks. {CHECK_THE_VALUE}"
            ),
            VALIDATION_ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE,
        ));
    }

    // The content-key period timing, against the two things its doc says it needs.
    let timing = provider
        .content_key_period_configuration
        .as_ref()
        .and_then(|configuration| configuration.content_key_period_timing.as_deref());
    if let Some(timing) = timing.filter(|timing| {
        CONTENT_KEY_PERIOD_TIMINGS_WITH_START_END.contains(timing)
    }) {
        let timing_member =
            format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.contentKeyPeriodConfiguration.contentKeyPeriodTiming");
        let speke_version = provider
            .speke_version
            .as_deref()
            .unwrap_or(DEFAULT_SPEKE_VERSION);
        if speke_version != SPEKE_VERSION_WITH_KEY_PERIOD_TIMING {
            return Some(validation_failed(
                &timing_member,
                &format!(
                    "Signaling the times a content key is used for requires SpekeVersion \
                     {SPEKE_VERSION_WITH_KEY_PERIOD_TIMING}. Use SpekeVersion \
                     {SPEKE_VERSION_WITH_KEY_PERIOD_TIMING}, or signal {timing} only as \
                     the content key index"
                ),
                VALIDATION_CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1,
            ));
        }
        if encryption.key_rotation_interval_seconds.is_none() {
            return Some(validation_failed(
                &timing_member,
                &format!(
                    "Signaling the times a content key is used for requires key rotation \
                     to be enabled. Specify KeyRotationIntervalSeconds, or signal \
                     {timing} only as the content key index"
                ),
                VALIDATION_CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION,
            ));
        }
    }

    key_provider_url_rejection(&provider.url)
}

/// `ValidationExceptionType` values a refused `SpekeKeyProvider$Url` carries, from the
/// model's `ValidationExceptionType` enum.
///
/// The enum names one value per way the value can fail to be an endpoint the service could
/// fetch content keys from: what it IS ([`VALIDATION_URL_INVALID`]), the scheme
/// ([`VALIDATION_URL_SCHEME`]), the credentials ([`VALIDATION_URL_USER_INFO`]) and port
/// ([`VALIDATION_URL_PORT`]) it carries, and the host it names
/// ([`VALIDATION_URL_UNKNOWN_HOST`], [`VALIDATION_URL_LOOPBACK_ADDRESS`],
/// [`VALIDATION_URL_LOCAL_ADDRESS`], [`VALIDATION_URL_LINK_LOCAL_ADDRESS`],
/// [`VALIDATION_URL_MULTICAST_ADDRESS`]).
pub const VALIDATION_URL_INVALID: &str = "URL_INVALID";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_SCHEME: &str = "URL_SCHEME";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_USER_INFO: &str = "URL_USER_INFO";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_PORT: &str = "URL_PORT";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_UNKNOWN_HOST: &str = "URL_UNKNOWN_HOST";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_LOOPBACK_ADDRESS: &str = "URL_LOOPBACK_ADDRESS";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_LOCAL_ADDRESS: &str = "URL_LOCAL_ADDRESS";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_LINK_LOCAL_ADDRESS: &str = "URL_LINK_LOCAL_ADDRESS";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_MULTICAST_ADDRESS: &str = "URL_MULTICAST_ADDRESS";

/// The scheme a key-provider URL has to use: `SpekeKeyProvider$Url` is "The URL of the API
/// Gateway proxy that you set up to talk to your key server", which "must start with
/// https://".
const KEY_PROVIDER_URL_SCHEME: &str = "https";

/// The `ValidationExceptionType` for an IP address a service endpoint may not be at, or
/// `None` for an address outside those classes.
///
/// The model names one value per class of address that is not a reachable, public endpoint
/// of a key server: the loopback of whoever resolves it, an address private to a network,
/// a link-local address and a multicast group. Both IP versions, since a URL may carry
/// either.
fn local_address_class(address: ::std::net::IpAddr) -> Option<&'static str> {
    match address {
        ::std::net::IpAddr::V4(address) => {
            if address.is_loopback() {
                Some(VALIDATION_URL_LOOPBACK_ADDRESS)
            } else if address.is_link_local() {
                Some(VALIDATION_URL_LINK_LOCAL_ADDRESS)
            } else if address.is_multicast() {
                Some(VALIDATION_URL_MULTICAST_ADDRESS)
            } else if address.is_private() || address.is_unspecified() {
                Some(VALIDATION_URL_LOCAL_ADDRESS)
            } else {
                None
            }
        }
        ::std::net::IpAddr::V6(address) => {
            if address.is_loopback() {
                Some(VALIDATION_URL_LOOPBACK_ADDRESS)
            } else if address.is_multicast() {
                Some(VALIDATION_URL_MULTICAST_ADDRESS)
            } else if address.segments()[0] & 0xffc0 == 0xfe80 {
                Some(VALIDATION_URL_LINK_LOCAL_ADDRESS)
            } else if address.segments()[0] & 0xfe00 == 0xfc00 || address.is_unspecified() {
                Some(VALIDATION_URL_LOCAL_ADDRESS)
            } else {
                None
            }
        }
    }
}

/// The host NAMES that resolve to the loopback of whoever resolves them, so a URL naming
/// one addresses the resolver itself rather than a key server: `localhost` and anything
/// under it (RFC 6761 reserves the whole `.localhost` tree for exactly this).
///
/// Names only — an address literal is read as an address
/// ([`local_address_class`]) — and a reading of a MODELLED member's value, which is why it
/// is not the framework's inbound-listener guard: that one decides which `Host` header a
/// listener answers, over a whole authority including literals and zone ids.
fn is_loopback_domain_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// The port a key server is reached at: the default port of [`KEY_PROVIDER_URL_SCHEME`],
/// which is the only one a URL that names the scheme's own endpoint can carry.
const KEY_PROVIDER_URL_PORT: u16 = 443;

/// Refuse a SPEKE key-provider URL that is not an endpoint this service could fetch
/// content keys from, or `None` when it is one.
///
/// The service RESOLVES this URL and calls it for every content key, so what it names has
/// to be a reachable key server: the model documents the value as "The URL of the API
/// Gateway proxy that you set up to talk to your key server", which "must reside in the
/// same AWS Region as MediaPackage and must start with https://", and its
/// `ValidationExceptionType` names one value per way the value fails that (see
/// [`VALIDATION_URL_INVALID`]). A URL carrying credentials or a port of its own does not
/// name that endpoint, and one naming a host that is not a public name — the resolver's own
/// loopback, a private, link-local or multicast address, or a name that is not a
/// fully-qualified one — names no key server reachable from the service at all.
///
/// Every class is decided from the VALUE: the emulator serves inbound requests and
/// resolves nothing, so what the host classes cover is what the text itself settles — the
/// address literals, and a single-label name, which is not a name the public DNS this
/// endpoint has to be reachable through can answer. The URI grammar itself is read by the
/// `http` crate's parser, the one every request's own URI is read with, rather than by a
/// hand-rolled split.
pub fn key_provider_url_rejection(url: &str) -> Option<ValidationException> {
    let member = format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.url");
    let refused = |requirement: String, exception_type: &str| {
        Some(validation_failed(&member, &requirement, exception_type))
    };
    let parsed = url.parse::<::http::Uri>().ok().and_then(|parsed| {
        let scheme = parsed.scheme_str()?.to_string();
        let authority = parsed.authority()?.clone();
        Some((scheme, authority))
    });
    // A value the grammar does not admit, or one naming no scheme or no authority, is not
    // an endpoint at all rather than an endpoint of the wrong kind.
    let Some((scheme, authority)) = parsed else {
        return refused(
            format!("Member must be a URL naming a scheme and a host. {CHECK_THE_VALUE}"),
            VALIDATION_URL_INVALID,
        );
    };
    if scheme != KEY_PROVIDER_URL_SCHEME {
        return refused(
            format!(
                "Member must be a {KEY_PROVIDER_URL_SCHEME} URL. {CHECK_THE_VALUE}"
            ),
            VALIDATION_URL_SCHEME,
        );
    }
    // The authority carries any userinfo ahead of the host, which the host itself does not.
    if authority.as_str().contains('@') {
        return refused(
            format!("Member must not carry user information. {CHECK_THE_VALUE}"),
            VALIDATION_URL_USER_INFO,
        );
    }
    if authority
        .port_u16()
        .is_some_and(|port| port != KEY_PROVIDER_URL_PORT)
    {
        return refused(
            format!(
                "Member must not name a port other than {KEY_PROVIDER_URL_PORT}. \
                 {CHECK_THE_VALUE}"
            ),
            VALIDATION_URL_PORT,
        );
    }
    let host = authority.host();
    if is_loopback_domain_name(host) {
        return refused(
            format!(
                "Member must name a host that is not the loopback of the service \
                 resolving it. {CHECK_THE_VALUE}"
            ),
            VALIDATION_URL_LOOPBACK_ADDRESS,
        );
    }
    // An IPv6 literal is bracketed in an authority, which is where its own colons live.
    let literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    match literal.parse::<::std::net::IpAddr>() {
        Ok(address) => local_address_class(address).and_then(|exception_type| {
            refused(
                format!(
                    "Member must name a host that is reachable from the service. \
                     {CHECK_THE_VALUE}"
                ),
                exception_type,
            )
        }),
        // A single-label name is not a name the service can resolve to a key server.
        Err(_) => (!host.trim_end_matches('.').contains('.')).then(|| {
            validation_failed(
                &member,
                &format!("Member must name a host that resolves. {CHECK_THE_VALUE}"),
                VALIDATION_URL_UNKNOWN_HOST,
            )
        }),
    }
}

/// The collection a pagination token was issued for.
///
/// A token names a position in ONE collection, so what it resumes has to be read against
/// the collection the request is paging: every list here walks a name-ordered map of the
/// same shape, so without that binding a cursor issued for one of them is a
/// plausible-looking cursor in each of the others. The variants are the collections the
/// model declares a paged list over, each carrying the names that identify it — the
/// Region included, since the Region is part of every resource's identity here.
#[derive(Debug, Clone, Copy)]
pub enum PageScope<'a> {
    /// The channel groups one Region holds.
    ChannelGroups { region: &'a str },
    /// The channels of one channel group.
    Channels {
        region: &'a str,
        channel_group: &'a str,
    },
    /// The origin endpoints of one channel.
    OriginEndpoints {
        region: &'a str,
        channel_group: &'a str,
        channel: &'a str,
    },
    /// The harvest jobs of one channel group: the cursor keys that group's whole job
    /// order, and the request's channel, endpoint and status members narrow what that
    /// order reports rather than naming a collection of their own.
    HarvestJobs {
        region: &'a str,
        channel_group: &'a str,
    },
}

impl PageScope<'_> {
    /// What identifies this collection: the level it is at, then the names of the
    /// resources above it.
    fn parts(&self) -> Vec<&str> {
        match self {
            Self::ChannelGroups { region } => vec!["channelGroups", region],
            Self::Channels {
                region,
                channel_group,
            } => vec!["channels", region, channel_group],
            Self::OriginEndpoints {
                region,
                channel_group,
                channel,
            } => vec!["originEndpoints", region, channel_group, channel],
            Self::HarvestJobs {
                region,
                channel_group,
            } => vec!["harvestJobs", region, channel_group],
        }
    }
}

/// How many hex characters of the binding digest a pagination token opens with.
const PAGE_TOKEN_DIGEST_LEN: usize = 24;

/// The digest that binds a token to the collection it walks AND to the position it names.
///
/// [`short_digest`] length-prefixes each part, so no two collections — a Region's groups
/// and a group's channels, two groups of the same name in two Regions — share a digest,
/// and no edit of the cursor keeps one: the token reads only as the whole value a list
/// handed out. The digest is not a secret and is not an authentication of the caller —
/// what it decides is whether a token names a page of THIS collection at all.
fn page_token_digest(scope: PageScope<'_>, last: &str) -> String {
    let mut parts = scope.parts();
    parts.push(last);
    short_digest(&parts, PAGE_TOKEN_DIGEST_LEN)
}

/// Whether `text` is the lowercase hexadecimal this service's tokens are written in.
fn is_lowercase_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// The pagination token a list of `scope` that stopped after `last` issues — opaque to
/// the caller, and decoded by [`page_resume_after`].
///
/// The token is that binding digest followed by the hex of the name the page stopped
/// after, so it is self-describing: resuming it needs no server-side cursor record, and
/// a reporting operation never has to write one to serve a page.
pub fn page_token(scope: PageScope<'_>, last: &str) -> String {
    format!(
        "{}{}",
        page_token_digest(scope, last),
        hex::encode(last.as_bytes())
    )
}

/// The name a pagination token resumes after, or `None` when the token is not one this
/// service issued FOR `scope`.
///
/// The strict inverse of [`page_token`]: the token has to carry the lowercase hex of a
/// non-empty name behind the digest that name has for `scope`. Every token this service
/// issues for this collection satisfies that, so anything else — a cursor another of this
/// service's lists issued, a hand-written hex run, a token whose digest or cursor was
/// edited or truncated — resumes nothing, and the list answers the modelled
/// [`invalid_pagination_token`] refusal instead of seeking to a position that was never a
/// page boundary of this collection.
pub fn page_resume_after(scope: PageScope<'_>, token: &str) -> Option<String> {
    let digest = token.get(..PAGE_TOKEN_DIGEST_LEN)?;
    let cursor = token.get(PAGE_TOKEN_DIGEST_LEN..)?;
    if !is_lowercase_hex(cursor) {
        return None;
    }
    let name = String::from_utf8(hex::decode(cursor).ok()?).ok()?;
    if name.is_empty() || digest != page_token_digest(scope, &name) {
        return None;
    }
    Some(name)
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
/// `records` is a name-ordered map's iterator, `scope` the collection being walked (what
/// the issued token is bound to, see [`PageScope`]), `resume_after` the name the previous
/// page stopped at (from [`page_resume_after`]) and `size` the resolved [`page_size`].
/// This page starts strictly beyond `resume_after`, so no record is returned twice and
/// none is skipped, and a token is issued only when a record is actually left over.
/// Shared by every paged list so the two cannot drift.
pub fn page<'a, T>(
    records: impl Iterator<Item = (&'a String, &'a T)>,
    scope: PageScope<'_>,
    resume_after: Option<&str>,
    size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let mut remaining = records
        .filter(|(name, _)| resume_after.is_none_or(|after| name.as_str() > after));
    let page: Vec<(&String, &T)> = remaining.by_ref().take(size).collect();
    let next_token = page
        .last()
        .filter(|_| remaining.next().is_some())
        .map(|(last, _)| page_token(scope, last));
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

    /// Every collection a paged list of this service walks, as a token is bound to it.
    fn page_scopes() -> Vec<PageScope<'static>> {
        vec![
            PageScope::ChannelGroups {
                region: "us-east-1",
            },
            PageScope::ChannelGroups {
                region: "eu-west-1",
            },
            PageScope::Channels {
                region: "us-east-1",
                channel_group: "grp",
            },
            PageScope::OriginEndpoints {
                region: "us-east-1",
                channel_group: "grp",
                channel: "chan",
            },
            PageScope::HarvestJobs {
                region: "us-east-1",
                channel_group: "grp",
            },
        ]
    }

    #[test]
    fn a_page_token_round_trips_the_name_it_stopped_after() {
        for scope in page_scopes() {
            for name in ["a", "endpoint-1", "A_long_Name-with-9-digits"] {
                let token = page_token(scope, name);
                assert_eq!(
                    page_resume_after(scope, &token).as_deref(),
                    Some(name),
                    "case {scope:?}/{name:?}: the token {token:?} must decode back to the name"
                );
            }
        }
    }

    #[test]
    fn a_token_this_service_did_not_issue_does_not_decode() {
        let scope = PageScope::ChannelGroups {
            region: "us-east-1",
        };
        let issued = page_token(scope, "grp");
        // Tokens differing in FORM, none of them one this service issued: prose, non-hex
        // characters, an odd-length hex run, a base64-shaped string, a bare hex run that
        // decodes to a name but carries no binding digest, a digest with no cursor behind
        // it, and an issued token with its cursor truncated.
        for token in [
            "not-a-token",
            "zzzz",
            "abc",
            "eyJrZXkiOiAidmFsdWUifQ==",
            "6d",
            &issued[..PAGE_TOKEN_DIGEST_LEN],
            &issued[..issued.len() - 2],
        ] {
            assert!(
                page_resume_after(scope, token).is_none(),
                "case {token:?}: a token this service did not issue must not decode"
            );
        }
        // And a token this service DID issue, for another of its collections: the digest
        // binds the cursor to the collection, so it is not readable outside it.
        for other in page_scopes().into_iter().skip(1) {
            let token = page_token(other, "grp");
            assert!(
                page_resume_after(scope, &token).is_none(),
                "case {other:?}: a token issued for another collection must not decode, \
                 got {:?}",
                page_resume_after(scope, &token)
            );
        }
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
        // On a channel whose output locking mode carries no output-timestamp mode, so the
        // member stays absent; the carrying case is the table in
        // `tests/all/segment_output_timestamp_mode.rs`.
        let materialized = materialized_segment(Some(submitted), Some(DEFAULT_OUTPUT_LOCKING_MODE));
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
        let endpoints = ingest_endpoints(&egress, "group", "channel", DEFAULT_INPUT_TYPE);
        let suffix = egress.split_once(".egress.").expect("an egress label").1;
        assert!(
            !endpoints.is_empty(),
            "a channel that receives ingest must publish ingest endpoints"
        );
        for endpoint in &endpoints {
            let url = endpoint.url.as_deref().expect("an ingest url");
            assert!(
                url.contains(suffix),
                "ingest url {url} must share the group's DNS suffix {suffix}"
            );
        }
        assert!(
            ingest_endpoints(&egress, "group", "channel", INPUT_TYPE_MULTIVIEW).is_empty(),
            "a channel that receives no ingest of its own must publish no ingest endpoint"
        );
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

    /// A VALUE an input type that CARRIES a member may not take is refused on that input
    /// type and accepted on the ones the restriction does not name — the whole
    /// `InputType` enum against the whole `OutputLockingMode` enum, plus the absent case.
    ///
    /// The catalogues are the model's own
    /// (`smithy_query(query="shape", shape="InputType,OutputLockingMode")`): `InputType`
    /// is HLS | CMAF | MULTIVIEW and `OutputLockingMode` is EPOCH_LOCKED |
    /// NON_EPOCH_LOCKED.
    #[test]
    fn a_value_an_input_type_may_not_take_is_refused_on_that_input_type() {
        for input_type in ["HLS", "CMAF", INPUT_TYPE_MULTIVIEW] {
            for submitted in [None, Some(DEFAULT_OUTPUT_LOCKING_MODE), Some("NON_EPOCH_LOCKED")] {
                let label = format!("{input_type} with {submitted:?}");
                let violation = channel_output_locking_mode_value_violation(submitted, input_type);
                let refused = submitted == Some(MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE)
                    && input_type == INPUT_TYPE_MULTIVIEW;
                assert_eq!(
                    violation.is_some(),
                    refused,
                    "case {label:?}: this combination being refused is {refused}, got \
                     {violation:?}"
                );
                let Some(violation) = violation else { continue };
                assert_eq!(
                    violation.validation_exception_type.as_deref(),
                    Some(VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED),
                    "case {label:?}: expected \
                     {VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED}, got \
                     {violation:?}"
                );
                let message = violation.message.as_deref().unwrap_or_default();
                assert!(
                    message.starts_with(&format!(
                        "Validation error encountered at request.{OUTPUT_LOCKING_MODE_MEMBER}:"
                    )),
                    "case {label:?}: expected this service's message form at the member \
                     the refusal is about, got {message:?}"
                );
            }
        }
    }

    /// An endpoint member the model restricts to particular parent-channel input types is
    /// materialised only under a channel whose input type may carry it: the documented
    /// default is applied there and withheld everywhere else, and a SUBMITTED value is
    /// refused where the member may not be carried — both read off the one allowed-input-type
    /// list, so they cannot disagree.
    ///
    /// The input-type catalogue is the model's own `InputType` enum
    /// (`smithy_query(query="shape", shape="InputType")` — HLS | CMAF | MULTIVIEW), and the
    /// submitted modes are the whole `StreamNameOutputMode` enum.
    #[test]
    fn a_restricted_endpoint_member_is_materialised_only_where_it_may_be_carried() {
        for input_type in ["HLS", "CMAF", "MULTIVIEW"] {
            let carries = input_type_carries_stream_name_output_mode(input_type);
            for submitted in [None, Some("INDEX"), Some("PASSTHROUGH_NAME")] {
                let label = format!("{input_type} with {submitted:?}");
                let stored = materialized_stream_name_output_mode(
                    submitted.map(str::to_string),
                    input_type,
                );
                let expected = if carries {
                    Some(submitted.unwrap_or(DEFAULT_STREAM_NAME_OUTPUT_MODE).to_string())
                } else {
                    None
                };
                assert_eq!(
                    stored, expected,
                    "case {label:?}: expected {expected:?}, got {stored:?}"
                );

                let violation = stream_name_output_mode_input_type_violation(submitted, input_type);
                match (submitted, carries) {
                    (Some(_), false) => {
                        let violation = violation.unwrap_or_else(|| {
                            panic!("case {label:?}: expected a refusal, got none")
                        });
                        assert_eq!(
                            violation.validation_exception_type.as_deref(),
                            Some("ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE"),
                            "case {label:?}: expected the input-type discriminator, got \
                             {violation:?}"
                        );
                    }
                    _ => assert!(
                        violation.is_none(),
                        "case {label:?}: expected no refusal, got {violation:?}"
                    ),
                }
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
            segment: materialized_segment(None, None),
            description: None,
            startover_window_seconds: materialized_startover_window_seconds(
                None,
                DEFAULT_INPUT_TYPE,
            ),
            hls_manifests: Vec::new(),
            low_latency_hls_manifests: Vec::new(),
            dash_manifests: Vec::new(),
            mss_manifests: Vec::new(),
            force_endpoint_error_configuration: None,
            uri_separator: materialized_uri_separator(None),
            stream_name_output_mode: materialized_stream_name_output_mode(
                None,
                DEFAULT_INPUT_TYPE,
            ),
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
        let mutations: [EndpointMutation; 12] = [
            ("container_type", |e| {
                e.container_type = "CMAF".to_string()
            }),
            ("description", |e| e.description = Some("a".to_string())),
            ("startover_window_seconds", |e| {
                e.startover_window_seconds = Some(1)
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
                e.stream_name_output_mode = Some("PASSTHROUGH_NAME".to_string())
            }),
            // An endpoint under a channel that may not carry the member has no mode at
            // all, which is a different record from one that carries the default.
            ("stream_name_output_mode absent", |e| {
                e.stream_name_output_mode = None
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
