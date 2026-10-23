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
    EgressAccessLogs, HarvestJob, HlsIngest, HlsManifest, HlsPackage, IngestEndpoint,
    IngressAccessLogs, MssPackage, OriginEndpoint, S3Destination,
};
use crate::scaffolded::ServiceError;
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

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

/// Width, in BYTES, of every value MediaPackage system-generates for an ingest endpoint —
/// its `Id`, its `Username` and its `Password` — rendered as lowercase hex, so each is
/// `2 * INGEST_GENERATED_BYTES` = 32 characters.
///
/// `IngestEndpoint.Id` / `Username` / `Password` are documented as "system generated"
/// and carry no `length` or `pattern` constraint in the Smithy model
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#IngestEndpoint")`
/// reports only `@sensitive` on the two credentials), so the shape is the one recorded
/// MediaPackage responses show: a 32-character lowercase-hexadecimal token for each of
/// the three generated members. One constant, because the service mints all three with
/// the same generator and width.
pub const INGEST_GENERATED_BYTES: usize = 16;

/// The largest page MediaPackage serves, and the largest `MaxResults` it accepts.
///
/// NARROWER than the modelled `MaxResults` range (`@range` min 1 max 1000 — see
/// `smithy_query(query="shape",shape="MaxResults",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
/// because the service enforces its own page cap on top of the modelled range: a
/// recorded `ListChannels` refuses an out-of-range `maxResults` with
/// "maxResults must be an integer between 1 and 50". The modelled range is what a
/// client-side validator checks; THIS is what the service applies, so it is the bound
/// [`page_size_violation`] refuses past and the page an absent `MaxResults` takes.
pub const MAX_PAGE_SIZE: usize = 50;

/// The smallest `MaxResults` MediaPackage accepts — the modelled `@range` minimum, which
/// the service's own bound shares.
pub const MIN_PAGE_SIZE: usize = 1;

/// The query parameter the paginated operations bind their page size to: `MaxResults` is
/// `@httpQuery("maxResults")` and the `@paginated` trait's `pageSize` on every `List*`
/// operation (`smithy_query(query="operation",operation="ListChannels",…)`).
pub const PAGE_SIZE_QUERY: &str = "maxResults";

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

/// The most harvest jobs this store holds — see [`MAX_CHANNELS`]. `CreateHarvestJob` is
/// the only thing that adds one and `DescribeHarvestJob` must still answer what it
/// created, so the bound is an entry count, refused past with `TooManyRequestsException`.
pub const MAX_HARVEST_JOBS: usize = 20_000;

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

// ---------------------------------------------------------------------------
// How MediaPackage states a refusal
//
// Every MediaPackage error body carries `message` AND an `errors` object keyed by the
// thing that was wrong — a RESOURCE TYPE when the request addressed a resource that is
// missing or in the way, a REQUEST MEMBER when a value is one the service will not
// accept — reporting that key's reasons (`crate::scaffolded::Reasons` describes the two
// shapes a recorded body uses). The sentence is built from the same reasons
// (`<key> <reason>`), so it is derived from the request rather than written twice; see
// [`crate::scaffolded::ServiceError`], which owns the envelope.
//
// Every constructor below returns one of those envelopes, so this is the one home of
// this service's refusal text: a handler names the rule, never the sentence.
// ---------------------------------------------------------------------------

/// How a MediaPackage error names a Channel in `errors` — the resource type, lowerCamel,
/// as the wire names a member.
pub const CHANNEL_KIND: &str = "channel";

/// How a MediaPackage error names an OriginEndpoint in `errors`.
pub const ORIGIN_ENDPOINT_KIND: &str = "originEndpoint";

/// How a MediaPackage error names a HarvestJob in `errors`.
pub const HARVEST_JOB_KIND: &str = "harvestJob";

/// The Smithy shape name of the resource an OriginEndpoint is, as the dependency refusal
/// below names the children blocking a delete.
pub const ORIGIN_ENDPOINT_SHAPE: &str = "OriginEndpoint";

/// How a MediaPackage error names an IngestEndpoint in `errors` — the shape name,
/// lowerCamel, the way every other resource type is keyed here
/// (`com.amazonaws.mediapackage#IngestEndpoint`).
pub const INGEST_ENDPOINT_KIND: &str = "ingestEndpoint";

/// `NotFoundException` (404) for a resource the request addressed BY ID that this service
/// does not hold: keyed by the resource type, because what is missing is the resource and
/// not a value in the request.
pub fn resource_not_found(kind: &str, id: &str) -> ServiceError {
    ServiceError::not_found(kind, format!("with id={id} not found"))
}

/// [`resource_not_found`] for a Channel.
pub fn channel_not_found(id: &str) -> ServiceError {
    resource_not_found(CHANNEL_KIND, id)
}

/// [`resource_not_found`] for an OriginEndpoint.
pub fn origin_endpoint_not_found(id: &str) -> ServiceError {
    resource_not_found(ORIGIN_ENDPOINT_KIND, id)
}

/// [`resource_not_found`] for a HarvestJob.
pub fn harvest_job_not_found(id: &str) -> ServiceError {
    resource_not_found(HARVEST_JOB_KIND, id)
}

/// [`resource_not_found`] for an IngestEndpoint.
///
/// 404 and not a 422 reference refusal: the id travels as an `@httpLabel`, so the request
/// ADDRESSES that ingest endpoint rather than referring to it from a body member — the
/// same reason a channel or an origin endpoint the service does not hold is a 404.
pub fn ingest_endpoint_not_found(id: &str) -> ServiceError {
    resource_not_found(INGEST_ENDPOINT_KIND, id)
}

/// `UnprocessableEntityException` (422) for a create whose id is already in use: the
/// request cannot be processed against the state this service holds, and the resource
/// type is what is in the way.
pub fn resource_already_exists(kind: &str, id: &str) -> ServiceError {
    ServiceError::unprocessable_state(kind, format!("with id={id} already exists"))
}

/// `UnprocessableEntityException` (422) for a REQUEST MEMBER naming a resource this
/// service does not hold.
///
/// Keyed by the member, not by the resource type: on a create the unresolvable reference
/// is a value in the request the caller has to fix, which is a different answer from
/// "the resource you addressed is gone" ([`resource_not_found`], 404). The reason is
/// reported the way everything decided against STATE is, because resolving the reference
/// is what failed — not reading it.
pub fn member_not_found(member: &str) -> ServiceError {
    ServiceError::unprocessable_state(member, "not found")
}

/// `UnprocessableEntityException` (422) for a rule about the resource AS A WHOLE — a
/// combination of members no single member can be keyed by — reported like every other
/// reason the service decides for that resource.
pub fn resource_invalid(kind: &str, reason: impl Into<String>) -> ServiceError {
    ServiceError::unprocessable_state(kind, reason)
}

/// `UnprocessableEntityException` (422) from the request validator: the requirement
/// `member` failed, reported as that member's single requirement.
pub fn member_invalid(member: &str, requirement: impl Into<String>) -> ServiceError {
    ServiceError::unprocessable_member(member, requirement)
}

/// [`member_invalid`] for one constraint the model declares and the request violated, as
/// the framework's own constraint walk reported it: keyed by the member PATH the walk
/// names (lowerCamel, as the wire names it), stating the requirement that member failed.
///
/// This is how a violation of a declared constraint reaches the same envelope as a rule
/// the service enforces itself, so one service does not answer its two input-failure paths
/// in two different bodies.
pub fn member_violation(violation: &Violation) -> ServiceError {
    member_invalid(&violation.path, constraint_requirement(&violation.kind))
}

/// The requirement a violated constraint states, worded the way every other refused
/// member here is worded: the rule, with the member supplied by the envelope's key.
pub fn constraint_requirement(kind: &ViolationKind) -> String {
    match kind {
        ViolationKind::MissingRequiredMember => "is required".to_string(),
        ViolationKind::MinLength(min) => {
            format!("must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => format!("must have length less than or equal to {max}"),
        ViolationKind::MinValue(min) => format!("must be greater than or equal to {min}"),
        ViolationKind::MaxValue(max) => format!("must be less than or equal to {max}"),
        ViolationKind::Pattern(pattern) => {
            format!("must satisfy the pattern {pattern}")
        }
        ViolationKind::EnumValue(values) => {
            format!("must be one of {}", values.join(", "))
        }
        ViolationKind::UniqueItems => "must not carry a repeated entry".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class added to the
        // framework later says what little the envelope can say about it, rather than
        // failing to report the refusal at all.
        _ => "is not valid".to_string(),
    }
}

/// `UnprocessableEntityException` (422) for a delete refused because children are still
/// attached: the blocked resource type, how many children hold it, and what they are.
pub fn has_attached_children(kind: &str, child_shape: &str, count: usize) -> ServiceError {
    let children = if count == 1 {
        format!("{count} {child_shape} is")
    } else {
        format!("{count} {child_shape}s are")
    };
    ServiceError::unprocessable_state(
        kind,
        format!("cannot be deleted because {children} attached to it"),
    )
}

/// `TooManyRequestsException` (429) for a store that has reached its entry cap.
pub fn resource_limit_reached(kind: &str, max: usize) -> ServiceError {
    ServiceError::too_many_requests(
        kind,
        format!("limit of {max} for this account has been reached"),
    )
}

/// `UnprocessableEntityException` (422) for a `nextToken` this service did not issue.
pub fn invalid_page_token() -> ServiceError {
    member_invalid(
        "nextToken",
        "is not a pagination token this service issued",
    )
}

/// The requirement an out-of-bound page size failed, worded as the service words it.
///
/// One sentence for every rejected value — too small, too large, or not a number at all —
/// because all three fail the same requirement.
pub fn page_size_requirement() -> String {
    format!("must be an integer between {MIN_PAGE_SIZE} and {MAX_PAGE_SIZE}")
}

/// `Some(error)` when `query` carries a page size outside the bound this service enforces
/// ([`MIN_PAGE_SIZE`]..=[`MAX_PAGE_SIZE`]), `None` when it carries an acceptable one or
/// none at all.
///
/// Reads the parameter AS SENT (decoded), so a value the modelled `@range` would never
/// admit — and a value that is not a number at all — is refused with the bound the service
/// applies rather than with the wider modelled one. The decoded value is read exactly as
/// the framework's HTTP binding reads it afterwards, so the gate refuses precisely what
/// the operation could not have read as a page size.
pub fn page_size_violation(query: &str) -> Option<ServiceError> {
    let sent = query_value(query, PAGE_SIZE_QUERY)?;
    let accepted = sent
        .parse::<usize>()
        .is_ok_and(|size| (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&size));
    (!accepted).then(|| member_invalid(PAGE_SIZE_QUERY, page_size_requirement()))
}

/// The query string of `uri` — everything after the first `?`, empty when it has none.
pub fn request_query(uri: &str) -> &str {
    uri.split_once('?').map_or("", |(_, query)| query)
}

/// The value `query` carries for the parameter `name`, read the way the framework's HTTP
/// binding reads one, so a rule applied here and the value the handler later sees cannot
/// disagree:
///
/// * the value is percent-decoded (`smithy_server_runtime::percent_decode`), so the rule
///   is applied to what the caller sent rather than to its encoding;
/// * a parameter sent with no `=` carries the EMPTY value, exactly as the binding's pair
///   reader does (`aws_smithy_schema_server::http_binding`), rather than being read as
///   absent;
/// * a parameter sent more than once answers with the FIRST value, which is the one the
///   binding hands a non-list member.
pub fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (!key.is_empty() && ::smithy_server_runtime::percent_decode(key) == name)
            .then(|| ::smithy_server_runtime::percent_decode(value))
    })
}

/// Refuse a caller-sized collection that would grow a record past `max` entries.
///
/// `Err` is the service's own envelope, keyed by the member and stating the bound it
/// broke, so the caller can fix the request from the error alone.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), ServiceError> {
    if count > max {
        return Err(member_invalid(
            member,
            format!("may carry at most {max} entries, got {count}"),
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), ServiceError> {
    bound_part_chars(member, "", value, max)
}

/// [`bound_chars`] for a string INSIDE a member — one entry of a list, one tag key — where
/// `part` names which part of the member the bound is about.
pub fn bound_part_chars(
    member: &str,
    part: &str,
    value: &str,
    max: usize,
) -> Result<(), ServiceError> {
    let length = value.chars().count();
    if length > max {
        let subject = if part.is_empty() {
            String::new()
        } else {
            format!("{part} ")
        };
        return Err(member_invalid(
            member,
            format!("{subject}may be at most {max} characters, got {length}"),
        ));
    }
    Ok(())
}

/// Refuse a tag set that breaks AWS's tagging limits, by entry count and by the length of
/// each key and value.
///
/// This is the one place those limits are decided, so a create's request-shaped tag map
/// and a `TagResource`'s merged result cannot be judged differently. Entries are sorted
/// first, so the part named in the refusal does not depend on map ordering.
pub fn bound_tag_entries<'a, I>(tags: I) -> Result<(), ServiceError>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut entries: Vec<(&str, &str)> = tags.into_iter().collect();
    entries.sort_unstable();
    bound_entries("tags", entries.len(), MAX_TAGS)?;
    for (key, value) in entries {
        bound_part_chars("tags", &format!("key {key:?}"), key, MAX_TAG_KEY_CHARS)?;
        bound_part_chars(
            "tags",
            &format!("the value of {key:?}"),
            value,
            MAX_TAG_VALUE_CHARS,
        )?;
    }
    Ok(())
}

/// Refuse a tag map on an input that breaks AWS's tagging limits; see
/// [`bound_tag_entries`].
pub fn bound_tags(
    tags: &Option<std::collections::HashMap<String, String>>,
) -> Result<(), ServiceError> {
    let Some(tags) = tags else { return Ok(()) };
    bound_tag_entries(tags.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}

/// Refuse the caller-sized members an OriginEndpoint create and update share.
///
/// Both handlers call this, so the bounds one enforces cannot drift from the other's: an
/// update must not be able to grow a record past what the create would have refused. Each
/// refusal is keyed by the member path the wire names, as every other refused member is.
pub fn bound_origin_endpoint_members(
    description: Option<&str>,
    manifest_name: Option<&str>,
    whitelist: Option<&[String]>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
) -> Result<(), ServiceError> {
    for (member, value) in [
        ("description", description),
        ("manifestName", manifest_name),
    ] {
        if let Some(value) = value {
            bound_chars(member, value, MAX_STRING_CHARS)?;
        }
    }
    if let Some(whitelist) = whitelist {
        bound_entries("whitelist", whitelist.len(), MAX_WHITELIST_ENTRIES)?;
        for entry in whitelist {
            bound_part_chars("whitelist", "an entry", entry, MAX_STRING_CHARS)?;
        }
    }
    if let Some(manifests) = cmaf_package.and_then(|package| package.hls_manifests.as_ref()) {
        bound_entries(
            "cmafPackage.hlsManifests",
            manifests.len(),
            MAX_CMAF_HLS_MANIFESTS,
        )?;
        for manifest in manifests {
            bound_part_chars(
                "cmafPackage.hlsManifests",
                "an id",
                &manifest.id,
                MAX_STRING_CHARS,
            )?;
            if let Some(name) = &manifest.manifest_name {
                bound_part_chars(
                    "cmafPackage.hlsManifests",
                    "a manifestName",
                    name,
                    MAX_STRING_CHARS,
                )?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Durations in seconds
//
// Several members of an OriginEndpoint are a NUMBER OF SECONDS: a window of content to
// retain or to publish, a segment or buffer length, a delay, an interval between
// rotations or tags. Each is a count of seconds of media, so none of them can be
// negative — a window of minus one hundred seconds is not a window — and MediaPackage
// refuses a request carrying one with `UnprocessableEntityException` (422), storing
// nothing. The model targets every one of them at the plain `__integer` shape and
// declares no `@range`
// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#HlsPackage",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
// so the floor is not a modelled constraint the framework's own walk could refuse: it is
// the service's rule, enforced here for EVERY such member rather than for the one a
// client happened to get wrong.
// ---------------------------------------------------------------------------

/// The smallest value a duration-in-seconds member may carry. Zero is a real value for
/// each of them — `StartoverWindowSeconds` 0 is startover playback disabled, and
/// `TimeDelaySeconds` 0 no delay, which is what [`apply_origin_endpoint_defaults`] fills
/// an absent one with — so the floor is zero and not one.
pub const MIN_DURATION_SECONDS: i32 = 0;

/// The refusal a negative duration-in-seconds member answers with: the floor it broke,
/// keyed by that member, in the wording every other refused numeric member here uses.
fn duration_refusal(member: &str) -> ServiceError {
    member_invalid(
        member,
        format!("must be greater than or equal to {MIN_DURATION_SECONDS}"),
    )
}

/// Refuse a negative value in any duration-in-seconds member of an OriginEndpoint create
/// or update request, keyed by the member path the wire names.
///
/// Both handlers call this, so the floor one enforces cannot drift from the other's: an
/// update must not be able to store a window a create of it would have refused. Members
/// are judged in a fixed order (the request's own member order, outermost first), so one
/// request is always refused by the same member — and a member's PATH is built only when
/// that member is the one being refused, so an accepted request allocates nothing.
pub fn bound_origin_endpoint_durations(
    startover_window_seconds: Option<i32>,
    time_delay_seconds: Option<i32>,
    hls_package: Option<&HlsPackage>,
    dash_package: Option<&DashPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&MssPackage>,
) -> Result<(), ServiceError> {
    /// One group of members whose paths are fixed: refuse the first negative one.
    fn judge(members: &[(&str, Option<i32>)]) -> Result<(), ServiceError> {
        for (member, seconds) in members {
            if seconds.is_some_and(|seconds| seconds < MIN_DURATION_SECONDS) {
                return Err(duration_refusal(member));
            }
        }
        Ok(())
    }

    judge(&[
        ("startoverWindowSeconds", startover_window_seconds),
        ("timeDelaySeconds", time_delay_seconds),
    ])?;
    if let Some(package) = hls_package {
        judge(&[
            (
                "hlsPackage.playlistWindowSeconds",
                package.playlist_window_seconds,
            ),
            (
                "hlsPackage.programDateTimeIntervalSeconds",
                package.program_date_time_interval_seconds,
            ),
            (
                "hlsPackage.segmentDurationSeconds",
                package.segment_duration_seconds,
            ),
            (
                "hlsPackage.encryption.keyRotationIntervalSeconds",
                package
                    .encryption
                    .as_ref()
                    .and_then(|encryption| encryption.key_rotation_interval_seconds),
            ),
        ])?;
    }
    if let Some(package) = dash_package {
        judge(&[
            (
                "dashPackage.manifestWindowSeconds",
                package.manifest_window_seconds,
            ),
            (
                "dashPackage.minBufferTimeSeconds",
                package.min_buffer_time_seconds,
            ),
            (
                "dashPackage.minUpdatePeriodSeconds",
                package.min_update_period_seconds,
            ),
            (
                "dashPackage.segmentDurationSeconds",
                package.segment_duration_seconds,
            ),
            (
                "dashPackage.suggestedPresentationDelaySeconds",
                package.suggested_presentation_delay_seconds,
            ),
            (
                "dashPackage.encryption.keyRotationIntervalSeconds",
                package
                    .encryption
                    .as_ref()
                    .and_then(|encryption| encryption.key_rotation_interval_seconds),
            ),
        ])?;
    }
    if let Some(package) = cmaf_package {
        judge(&[
            (
                "cmafPackage.segmentDurationSeconds",
                package.segment_duration_seconds,
            ),
            (
                "cmafPackage.encryption.keyRotationIntervalSeconds",
                package
                    .encryption
                    .as_ref()
                    .and_then(|encryption| encryption.key_rotation_interval_seconds),
            ),
        ])?;
        // A CMAF package's child HLS manifests carry the same two window members an
        // `HlsPackage` does, so the same floor holds for each of them, named by its index
        // the way the wire indexes a list member — rendered only for the one refused.
        for (index, manifest) in package.hls_manifests.iter().flatten().enumerate() {
            for (member, seconds) in [
                ("playlistWindowSeconds", manifest.playlist_window_seconds),
                (
                    "programDateTimeIntervalSeconds",
                    manifest.program_date_time_interval_seconds,
                ),
            ] {
                if seconds.is_some_and(|seconds| seconds < MIN_DURATION_SECONDS) {
                    return Err(duration_refusal(&format!(
                        "cmafPackage.hlsManifests[{index}].{member}"
                    )));
                }
            }
        }
    }
    if let Some(package) = mss_package {
        judge(&[
            (
                "mssPackage.manifestWindowSeconds",
                package.manifest_window_seconds,
            ),
            (
                "mssPackage.segmentDurationSeconds",
                package.segment_duration_seconds,
            ),
        ])?;
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
    /// Where in [`Self::ingest_endpoints`] the endpoint whose `Id` is `id` sits, or `None`
    /// when this channel carries no such endpoint.
    ///
    /// A POSITION rather than a reference, so a caller that has to mint through the store
    /// (which needs the store mutably) can resolve the target first and write to it after.
    /// An ingest endpoint exists only as part of its channel, so this is the only way it
    /// is ever addressed.
    pub fn ingest_endpoint_position(&self, id: &str) -> Option<usize> {
        self.ingest_endpoints
            .iter()
            .position(|endpoint| endpoint.id.as_deref() == Some(id))
    }

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
    /// Which packaging configuration this endpoint is stored with, and so which manifest
    /// its playback URL ends in.
    ///
    /// Exactly one of the four is stored (a create that names none or several is
    /// refused), so this answers `Some` for every record this service holds.
    pub fn packaging_kind(&self) -> Option<PackagingKind> {
        if self.hls_package.is_some() {
            Some(PackagingKind::Hls)
        } else if self.dash_package.is_some() {
            Some(PackagingKind::Dash)
        } else if self.cmaf_package.is_some() {
            Some(PackagingKind::Cmaf)
        } else if self.mss_package.is_some() {
            Some(PackagingKind::Mss)
        } else {
            None
        }
    }

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

// ---------------------------------------------------------------------------
// A harvest job's status, and the clock that moves it
//
// `CreateHarvestJob` submits a job that exports a window of an origin endpoint's
// packaged content to an S3 bucket. The submitted job is `IN_PROGRESS` — a NON-TERMINAL
// status — so the service owes it a later status without anybody calling again. That
// owed transition is stored as a DUE INSTANT on the record and materialised by
// [`ComAmazonawsMediapackageMediapackageState::settle_harvest_jobs`], which every
// harvest-job handler calls first: a read reports what the clock has already made due
// and never advances a job because it was polled.
// ---------------------------------------------------------------------------

/// The `Status` values the model declares for a HarvestJob, so no handler spells one as a
/// bare literal. The full set is `ops::types::STATUS_VALUES`
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#Status",…)` reports
/// `IN_PROGRESS | SUCCEEDED | FAILED`).
pub const HARVEST_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`HARVEST_IN_PROGRESS`].
pub const HARVEST_SUCCEEDED: &str = "SUCCEEDED";
/// See [`HARVEST_IN_PROGRESS`].
pub const HARVEST_FAILED: &str = "FAILED";

/// The statuses a harvest job never leaves: the harvest finished, or it failed.
pub const HARVEST_TERMINAL_STATUSES: &[&str] = &[HARVEST_SUCCEEDED, HARVEST_FAILED];

/// How long after `CreateHarvestJob` the clock owes the submitted job its terminal
/// status.
///
/// AWS documents no duration — how long a harvest takes depends on the window harvested
/// — so this span is CHOSEN, not modelled: 500 ms, inside the project's 250-750 ms band
/// for a transition the clock owes. It is deliberately non-zero, so a caller reading
/// straight after the create still observes the submitted job `IN_PROGRESS`. Nothing
/// sleeps for it: the due instant is stored and compared against the clock.
pub const HARVEST_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The terminal status a harvest job submitted to THIS emulator reaches.
///
/// A harvest exports the packaged content of an origin endpoint to the caller's S3
/// bucket. This emulator serves no MediaPackage data plane — a channel ingests nothing
/// and an endpoint's playback URL answers no segments — and it writes nothing to the
/// caller's bucket, so no harvest runs and `SUCCEEDED` would claim content that is not
/// there. The job therefore ends in the service's own modelled failure status, which is
/// what the API has to say about a harvest that produced nothing (the explanation AWS
/// publishes for a failure travels as a CloudWatch event, not as a member of this
/// service's `HarvestJob` shape, so there is no field to carry a reason).
pub const HARVEST_OUTCOME: &str = HARVEST_FAILED;

/// A HarvestJob, exactly as `CreateHarvestJob` stored it.
#[derive(Debug, Clone, Default)]
pub struct HarvestJobRecord {
    pub id: String,
    pub arn: String,
    /// The channel the harvested endpoint packages, read from the STORED endpoint at
    /// submission time rather than from the request, which carries no channel id.
    pub channel_id: String,
    pub origin_endpoint_id: String,
    pub s3_destination: S3Destination,
    pub start_time: String,
    pub end_time: String,
    /// ISO-8601, minted once at submission and never recomputed on a read.
    pub created_at: String,
    /// One of [`HARVEST_IN_PROGRESS`] or [`HARVEST_TERMINAL_STATUSES`].
    pub status: String,
    /// The status the clock owes this job, and the instant it falls due. `None` for a job
    /// that has reached a terminal status, which owes nothing.
    pub next_status_due: Option<(SystemTime, &'static str)>,
}

impl HarvestJobRecord {
    /// Whether this job has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        HARVEST_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// The modelled `HarvestJob` shape for this record. Every harvest-job-returning
    /// operation renders its output from this one projection, so they cannot disagree.
    pub fn to_harvest_job(&self) -> HarvestJob {
        HarvestJob {
            arn: Some(self.arn.clone()),
            channel_id: Some(self.channel_id.clone()),
            created_at: Some(self.created_at.clone()),
            end_time: Some(self.end_time.clone()),
            id: Some(self.id.clone()),
            origin_endpoint_id: Some(self.origin_endpoint_id.clone()),
            s3_destination: Some(self.s3_destination.clone()),
            start_time: Some(self.start_time.clone()),
            status: Some(self.status.clone()),
        }
    }

    /// Apply the owed status transition when the clock has already made it due, and
    /// nothing else: the job moves at the instant it was owed whether or not anybody
    /// called.
    fn settle(&mut self, now: SystemTime) {
        let Some((due, next)) = self.next_status_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = next.to_string();
        self.next_status_due = None;
    }
}

/// Per-service state for com-amazonaws-mediapackage-mediapackage. A handler reaches it through [`service_state`].
///
/// Every collection is a `BTreeMap` keyed by the resource id the API itself keys on, so
/// `List*` output and its paging tokens are in a stable, caller-independent order. Each
/// is bounded by an entry count — [`MAX_CHANNELS`], [`MAX_ORIGIN_ENDPOINTS`] and
/// [`MAX_HARVEST_JOBS`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackageMediapackageState {
    pub channels: BTreeMap<String, ChannelRecord>,
    pub origin_endpoints: BTreeMap<String, OriginEndpointRecord>,
    pub harvest_jobs: BTreeMap<String, HarvestJobRecord>,
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
    /// Each endpoint's id and its ingest credentials — the model documents `Id`,
    /// `Username` and `Password` as "system generated" — are minted once, here, with
    /// MediaPackage's generated width ([`INGEST_GENERATED_BYTES`], i.e. 32 lowercase-hex
    /// characters each), and stored on the channel record, so every later read of the
    /// channel returns exactly these values. The URL embeds the endpoint id, so it stays
    /// valid for exactly that endpoint.
    pub fn mint_ingest_endpoints(&mut self, region: &str, channel_id: &str) -> Vec<IngestEndpoint> {
        let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);
        let channel_label = stable_label(&[channel_id, region]);
        (0..INGEST_ENDPOINT_COUNT)
            .map(|_| {
                let id = self.mint_hex("ingest-endpoint", INGEST_GENERATED_BYTES);
                let (username, password) = self.mint_ingest_credentials();
                IngestEndpoint {
                    url: Some(format!(
                        "https://{account_label}.mediapackage.{region}.amazonaws.com/in/v2/{channel_label}/{id}/channel"
                    )),
                    id: Some(id),
                    username: Some(username),
                    password: Some(password),
                }
            })
            .collect()
    }

    /// One freshly minted ingest credential pair — `(username, password)` — for an ingest
    /// endpoint to carry.
    ///
    /// The generator and the width are the ones a brand-new endpoint's credentials are
    /// minted with ([`INGEST_GENERATED_BYTES`]), because a rotated credential is a
    /// system-generated ingest credential like any other: the model documents `Username`
    /// and `Password` as "system generated" and constrains neither. Minting goes through
    /// [`Self::mint_hex`], so a rotation can never hand back the pair it replaced.
    ///
    /// Shared by the credential-rotating operations, so a rotated endpoint is
    /// indistinguishable from a newly provisioned one.
    pub fn mint_ingest_credentials(&mut self) -> (String, String) {
        let username = self.mint_hex("ingest-username", INGEST_GENERATED_BYTES);
        let password = self.mint_hex("ingest-password", INGEST_GENERATED_BYTES);
        (username, password)
    }

    /// The stored tag set of the resource a tagging operation's `ResourceArn` named, for
    /// a REPORTING operation to render. `None` when this service holds no such resource,
    /// which is what tells `ListTagsForResource` to report a 404 instead of a tag set.
    ///
    /// The same lookup as [`tags_mut`](Self::tags_mut), without the write capability: a
    /// read must not be able to change what a later caller sees.
    pub fn tags(&self, target: &TaggedResource) -> Option<&BTreeMap<String, String>> {
        match target {
            TaggedResource::Channel(id) => self.channels.get(id).map(|record| &record.tags),
            TaggedResource::OriginEndpoint(id) => {
                self.origin_endpoints.get(id).map(|record| &record.tags)
            }
        }
    }

    /// The stored tag set of the resource a tagging operation's `ResourceArn` named, for
    /// that operation to change in place. `None` when this service holds no such
    /// resource, which is what tells `TagResource`/`UntagResource` to report a 404
    /// instead of writing.
    pub fn tags_mut(&mut self, target: &TaggedResource) -> Option<&mut BTreeMap<String, String>> {
        match target {
            TaggedResource::Channel(id) => {
                self.channels.get_mut(id).map(|record| &mut record.tags)
            }
            TaggedResource::OriginEndpoint(id) => self
                .origin_endpoints
                .get_mut(id)
                .map(|record| &mut record.tags),
        }
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

    /// Materialise every harvest-job status transition the clock has already made due,
    /// and nothing else.
    ///
    /// Called FIRST by every harvest-job handler, a read included: it applies only a
    /// change that was owed at `now` whether or not anybody called, so what a reader sees
    /// is decided by the clock and never by the read itself.
    pub fn settle_harvest_jobs(&mut self, now: SystemTime) {
        for job in self.harvest_jobs.values_mut() {
            job.settle(now);
        }
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

// ---------------------------------------------------------------------------
// What `CreateHarvestJob` decides about a submission before it stores one
//
// Both rules below are about the REQUEST, not about the record: the window has to name an
// interval of media, and the destination role has to be one this service could write as.
// They live here, beside the store the handler guards, rather than inside the harvest-job
// record's own section, because neither is a property of a stored job.
// ---------------------------------------------------------------------------

/// The service principal MediaPackage assumes a harvest job's `S3Destination.RoleArn` as.
///
/// A harvest writes to the caller's bucket as that role, so the role's trust policy has to
/// let this principal assume it — the same up-front check every service that acts as a
/// caller-supplied role makes.
pub const HARVEST_SERVICE_PRINCIPAL: &str = "mediapackage.amazonaws.com";

/// The instant an ISO-8601 date-time names, in NANOSECONDS since the epoch.
///
/// `StartTime` and `EndTime` are modelled as `__string`, so the FRAMEWORK does not parse
/// them: the handler does, because the service compares them. Both zone spellings
/// RFC 3339 admits are read — a `Z` instant and a numeric offset, which is what
/// `datetime.isoformat()` produces — so two requests naming the same instant are judged
/// the same way; a bound carrying NO zone suffix is read as UTC (see the body). The offset
/// is resolved through the ONE shared reader every service with a string-typed timestamp
/// uses (`service_common::rfc3339`), so nothing about reading a zone is written here; the
/// calendar conversion stays in the maintained library.
///
/// The unit is nanoseconds and not seconds because RFC 3339 admits a fractional second
/// and the comparison this feeds is a STRICT one: truncating to whole seconds would make
/// a half-second window compare as empty and refuse a window the caller can harvest.
///
/// `None` for text that is not an ISO-8601 instant at all — which the caller is told
/// about ([`instant_invalid`]) rather than silently accepted — for EVERY input, including
/// arbitrary non-ASCII text: the text comes straight off the wire, so this must never
/// panic.
pub fn parse_instant(text: &str) -> Option<i128> {
    // A bound carrying no zone suffix at all is read as UTC rather than refused: the
    // members are modelled plain `__string` with no `@pattern`, nothing published says a
    // zone is required, and a client sending `datetime.isoformat()` output for a naive
    // datetime sends exactly this form. BOTH bounds are read the same way, so the ordering
    // comparison stays well defined. Text that is not a date-time at all still fails the
    // parse below and is refused.
    let (civil, offset) = ::service_common::rfc3339::split_utc_offset(text).unwrap_or((text, 0));
    let instant = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    let secs = i128::from(instant.secs().checked_sub(offset)?);
    Some(secs * 1_000_000_000 + i128::from(instant.subsec_nanos()))
}

/// `UnprocessableEntityException` (422) for a timestamp member this service cannot read as
/// an instant.
pub fn instant_invalid(member: &str) -> ServiceError {
    member_invalid(member, "must be an ISO 8601 date-time")
}

/// `UnprocessableEntityException` (422) for a harvest window whose end does not come after
/// its start.
///
/// A harvest window is an interval of media, so its end has to be LATER than its start: an
/// empty or inverted window names no content to harvest. Keyed by `endTime`, which is the
/// member the caller has to fix (`startTime` names a real instant).
pub fn harvest_window_invalid() -> ServiceError {
    member_invalid("endTime", "must be later than the start time")
}

/// The IAM store a harvest job's destination role is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so MediaPackage reads the very records `CreateRole` wrote
/// rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied. `None` has the same
/// meaning as for [`service_state`]: the engine carries no such store, which a handler
/// reports as a server fault.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// `None` when this emulator has no reason to think MediaPackage could not write to the
/// destination as `role_arn`; otherwise the refusal that answers the submission.
///
/// A harvest job writes the exported content to the caller's bucket AS that role, so the
/// service checks up front that the role's trust policy lets
/// [`HARVEST_SERVICE_PRINCIPAL`] assume it and refuses the submission when it does not.
/// The role is resolved against the account's IAM records — the same records `CreateRole`
/// wrote — by its ARN. An ARN the IAM store holds no role for is NOT evidence of a missing
/// trust (the role may belong to another account, or to a client that never told this
/// emulator about it), so the submission goes ahead; only a role this emulator holds and
/// whose trust policy withholds the grant is refused.
pub fn harvest_role_refusal(role_arn: &str, iam: &IamState) -> Option<ServiceError> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    match ::service_common::iam_trust_policy::admits_service_principal(
        document,
        HARVEST_SERVICE_PRINCIPAL,
    ) {
        // A document this emulator cannot read as a policy is no evidence either way.
        None | Some(true) => None,
        Some(false) => Some(member_invalid(
            "s3Destination.roleArn",
            format!(
                "names a role {HARVEST_SERVICE_PRINCIPAL} cannot assume: check the trust \
                 policy grants it sts:AssumeRole"
            ),
        )),
    }
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

/// The resource a tagging operation's `ResourceArn` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    Channel(String),
    OriginEndpoint(String),
}

impl TaggedResource {
    /// The resource's id, as the ARN spelled it.
    pub fn id(&self) -> &str {
        match self {
            Self::Channel(id) | Self::OriginEndpoint(id) => id,
        }
    }

    /// What to call this resource in a message to the caller.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Channel(_) => "Channel",
            Self::OriginEndpoint(_) => "OriginEndpoint",
        }
    }

    /// How an error body's `errors` map keys this resource — the resource type, as
    /// [`CHANNEL_KIND`] and [`ORIGIN_ENDPOINT_KIND`] name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Channel(_) => CHANNEL_KIND,
            Self::OriginEndpoint(_) => ORIGIN_ENDPOINT_KIND,
        }
    }
}

/// The resource `arn` names, for a `ResourceArn` this service can tag.
///
/// A MediaPackage resource ARN is
/// `arn:<partition>:mediapackage:<region>:<account>:<type>/<id>` with `<type>` either
/// `channels` or `origin_endpoints` — the two forms [`channel_arn`] and
/// [`origin_endpoint_arn`] mint, and the only resources this service holds. `None` for
/// anything else: that is a malformed request, which is a different failure from an ARN
/// naming a resource the service does not hold.
///
/// The partition, region and account fields are not matched: a resource is keyed by its
/// id, so an ARN minted for this resource in the caller's own account and region names
/// the same channel or endpoint this store holds.
pub fn parse_tag_arn(arn: &str) -> Option<TaggedResource> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != SERVICE_NAME {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, id) = fields.next()?.split_once('/')?;
    if id.is_empty() {
        return None;
    }
    match resource_type {
        "channels" => Some(TaggedResource::Channel(id.to_string())),
        "origin_endpoints" => Some(TaggedResource::OriginEndpoint(id.to_string())),
        _ => None,
    }
}

/// The region an ARN this service minted names (an ARN's fourth field).
///
/// A value derived for a stored resource — a playback URL — is re-derived from the
/// resource's OWN region, not from the region the request that changed it happened to be
/// signed for, so an update cannot move an existing endpoint's URL to another region.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// `arn:aws:mediapackage:<region>:<account>:channels/<id>`.
pub fn channel_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("channels/{id}"))
}

/// `arn:aws:mediapackage:<region>:<account>:origin_endpoints/<id>`.
pub fn origin_endpoint_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("origin_endpoints/{id}"))
}

/// `arn:aws:mediapackage:<region>:<account>:harvest_jobs/<id>` — the ARN form
/// MediaPackage assigns a HarvestJob, whose resource segment is the operation's own URI
/// collection (`smithy_query(query="operation",operation="CreateHarvestJob",…)` binds
/// `POST /harvest_jobs`), as `channels` and `origin_endpoints` are above.
pub fn harvest_job_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("harvest_jobs/{id}"))
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

/// The single packaging configuration a create or update request carries.
///
/// An OriginEndpoint packages its channel in exactly ONE format — its playback URL ends
/// in that format's manifest — so a request naming several cannot be stored. `Ok(None)`
/// means the request named none: a create must refuse that, an update leaves the stored
/// packaging in place. `Err` carries the names supplied, for [`packaging_refusal`].
pub fn sole_packaging_kind(
    hls: bool,
    dash: bool,
    cmaf: bool,
    mss: bool,
) -> Result<Option<PackagingKind>, Vec<&'static str>> {
    let supplied: Vec<(PackagingKind, &'static str)> = [
        (PackagingKind::Hls, "hlsPackage", hls),
        (PackagingKind::Dash, "dashPackage", dash),
        (PackagingKind::Cmaf, "cmafPackage", cmaf),
        (PackagingKind::Mss, "mssPackage", mss),
    ]
    .into_iter()
    .filter_map(|(kind, name, present)| present.then_some((kind, name)))
    .collect();
    match supplied[..] {
        [] => Ok(None),
        [(kind, _)] => Ok(Some(kind)),
        _ => Err(supplied.iter().map(|(_, name)| *name).collect()),
    }
}

/// The refusal for a request that does not name exactly one packaging configuration,
/// naming the ones it did supply so the caller can fix the request from the error alone.
///
/// A rule about a COMBINATION of members, so it is keyed by the resource the request is
/// about rather than by one member ([`resource_invalid`]).
pub fn packaging_refusal(supplied: &[&str]) -> ServiceError {
    resource_invalid(
        ORIGIN_ENDPOINT_KIND,
        format!(
            "requires exactly one of hlsPackage, dashPackage, cmafPackage or mssPackage; got {}",
            if supplied.is_empty() {
                "none".to_string()
            } else {
                supplied.join(", ")
            }
        ),
    )
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
/// A `MaxResults` outside the bound this service enforces is refused before the handler
/// runs ([`page_size_violation`]), so a value reaching here is already inside
/// [`MIN_PAGE_SIZE`]..=[`MAX_PAGE_SIZE`]; the clamp states that invariant locally instead
/// of leaving it implicit, and an absent `MaxResults` takes the service's page cap, which
/// is the most it would serve for any value the caller could send.
pub fn page_size(max_results: Option<i32>) -> usize {
    match max_results {
        None => MAX_PAGE_SIZE,
        Some(max) => usize::try_from(max)
            .unwrap_or(MIN_PAGE_SIZE)
            .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
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
