// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-medicalimaging-ahigatewayservice service.
//!
//! One data store map is the whole control plane: a data store owns its DICOM import
//! jobs and its image sets, so every handler reaches one record rather than joining
//! two parallel maps.
//!
//! # The clock, not the reader, moves a status
//!
//! AWS HealthImaging answers `CreateDatastore` with `CREATING` and `DeleteDatastore`
//! with `DELETING`, `StartDICOMImportJob` with `SUBMITTED`, and `CopyImageSet` with a
//! `LOCKED` source and destination — non-terminal statuses the service itself advances
//! afterwards. This crate stores the instant each
//! transition falls DUE and materialises it in [`ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`],
//! which every handler calls first. Nothing sleeps, and a reporting operation
//! (`GetDatastore`, `ListDatastores`, `GetDICOMImportJob`, `GetImageSet`) only ever
//! writes a change the clock already owed at that instant — so a read never moves a
//! status that was not already due.
//!
//! # Pixel data belongs to the image set
//!
//! An image set owns the image frames it holds ([`ImageFrame`]), keyed by image frame
//! id, because `GetImageFrame` takes no version: every version of an image set reaches
//! the same frames. A frame is readable only once something has DELIVERED it — the copy
//! that completes carries its source's frames to its destination — so nothing is served
//! that was never stored.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::ImportConfiguration;

/// How long after `CreateDatastore` the data store leaves `CREATING` for `ACTIVE`.
///
/// AWS publishes no duration for it, so this is a chosen span: long enough that a
/// caller polling straight after the create still observes `CREATING`, short enough
/// that a poll loop terminates promptly.
pub const DATASTORE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteDatastore` the data store leaves `DELETING` and is gone.
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const DATASTORE_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How long after `StartDICOMImportJob` the job leaves `SUBMITTED` for `IN_PROGRESS`.
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const IMPORT_JOB_IN_PROGRESS_DELAY: Duration = Duration::from_millis(300);

/// How long after `StartDICOMImportJob` the job reaches `COMPLETED` and gets `endedAt`.
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const IMPORT_JOB_COMPLETION_DELAY: Duration = Duration::from_millis(600);

/// How long after `CopyImageSet` the copy completes: the source image set leaves
/// `COPYING_WITH_READ_ONLY_ACCESS` and the destination leaves `COPYING`, both returning
/// to `ACTIVE`. A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`] —
/// AWS publishes no duration for a copy, and a caller reading either image set straight
/// after the call still observes the `LOCKED` copy in flight.
pub const IMAGE_SET_COPY_DELAY: Duration = Duration::from_millis(400);

/// The external version identifier an image set's FIRST version carries. AWS numbers
/// image set versions from 1 (`ImageSetExternalVersionId` is `^\d+$`), and each later
/// version is one past the one it supersedes.
pub const FIRST_IMAGE_SET_VERSION_ID: u32 = 1;

/// The smallest page `ListDatastores` will serve — the model's `maxResults` minimum
/// (`smithy_query(query="operation",operation="ListDatastores")` reports range 1..50).
pub const MIN_LIST_PAGE_SIZE: usize = 1;

/// The largest page `ListDatastores` will serve — the model's `maxResults` maximum, and
/// also the page size used when the caller names no `maxResults`. Every page is clamped
/// into `MIN_LIST_PAGE_SIZE..=MAX_LIST_PAGE_SIZE` in the handler rather than trusting
/// the boundary validator alone, so one request can never materialise an unbounded
/// response.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// How many data stores this emulator will hold at once.
///
/// A memory bound, not an AWS quota: `CreateDatastore` persists caller-supplied data
/// (name, ARNs, the whole tag map) in process memory, so without a cap a caller could
/// grow it without limit. Past the cap the call is refused with the operation's own
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_DATASTORES: usize = 128;

/// How many import jobs one data store will hold at once — the same memory bound as
/// [`MAX_DATASTORES`], applied per data store so the product of the two is bounded too.
/// Past the cap `StartDICOMImportJob` is refused with `ServiceQuotaExceededException`.
pub const MAX_IMPORT_JOBS_PER_DATASTORE: usize = 128;

/// How many image sets one data store will hold at once — the same memory bound as
/// [`MAX_IMPORT_JOBS_PER_DATASTORE`], applied per data store. Past the cap an operation
/// that would mint another image set (`CopyImageSet` without a destination image set)
/// is refused with `ServiceQuotaExceededException`.
pub const MAX_IMAGE_SETS_PER_DATASTORE: usize = 128;

/// How many versions one image set will hold at once.
///
/// A memory bound, not an AWS quota: every copy into an existing image set keeps the
/// version it superseded, so without a cap a caller could grow one image set's version
/// list without limit. Past the cap the call is refused with the operation's own
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_IMAGE_SET_VERSIONS: usize = 128;

/// How many `@idempotencyToken` entries the ledger keeps.
///
/// The ledger outlives the resources it names (a token must stay honoured for a while
/// after its resource is gone), so it is bounded two ways: entries whose resource has
/// been retired are dropped in
/// [`ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`], and the
/// remainder is capped here with FIFO eviction of the oldest token.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// The service's `arnNamespace` / sigv4 signing name, used to build data store ARNs.
pub const ARN_SERVICE: &str = "medical-imaging";

/// The region an ARN names when the request carries no SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// How this service WORDS a refusal
//
// Three wording rules live here because more than one handler — and the
// `override_refusal` seam in `crate::service` — states them, and a text spelled
// twice drifts:
//
// 1. A missing resource is stated by its KIND, as a constant: the text carries no
//    identifier from the request, so one caller probing ids cannot read them back
//    out of the 404 body ([`DATASTORE_NOT_FOUND_MESSAGE`],
//    [`IMPORT_JOB_NOT_FOUND_MESSAGE`]).
// 2. A continuation token this service did not issue is refused with ONE fixed
//    sentence that does not vary with the token supplied
//    ([`INVALID_NEXT_TOKEN_MESSAGE`]).
// 3. An input-constraint violation is stated in the AWS envelope over the framework's
//    own clauses, with every `@pattern` quoted WITHOUT the anchors the model spells it
//    with — and with the rejected value withheld for the members this service does not
//    reflect ([`constraint_clause`], [`VALUE_WITHHOLDING_MEMBERS`],
//    [`validation_envelope`]).
//
// A malformed resource ARN is NOT one of them: the ARN parse runs before constraint
// validation, so `crate::service` answers it with the modelled `BadRequestException`
// and [`MALFORMED_ARN_MESSAGE`] instead.
// ---------------------------------------------------------------------------

/// The text this service answers a request naming a data store it does not hold with.
///
/// A constant of the service, not of the request: the recorded corpus shows the same
/// sentence from `GetDatastore`, `DeleteDatastore`, `GetDICOMImportJob` and
/// `StartDICOMImportJob`, with no data store identifier in it.
pub const DATASTORE_NOT_FOUND_MESSAGE: &str = "Datastore resource is not found.";

/// The text this service answers a request naming an import job a data store does not
/// hold with.
///
/// The resource KIND is what the sentence states, exactly as
/// [`DATASTORE_NOT_FOUND_MESSAGE`] does for a data store; AWS's own wording for an
/// absent import job is not recorded, so this spelling is chosen by analogy. What is
/// NOT chosen is the shape: no identifier the caller supplied appears in it.
pub const IMPORT_JOB_NOT_FOUND_MESSAGE: &str = "ImportJob resource is not found.";

/// The text this service answers a request naming an image set a data store does not
/// hold with — the resource KIND, by the same rule as [`DATASTORE_NOT_FOUND_MESSAGE`],
/// with no identifier from the request in it.
pub const IMAGE_SET_NOT_FOUND_MESSAGE: &str = "ImageSet resource is not found.";

/// The text this service answers a request naming a VERSION of an image set it holds no
/// such version of with. Stated by kind, like [`IMAGE_SET_NOT_FOUND_MESSAGE`].
pub const IMAGE_SET_VERSION_NOT_FOUND_MESSAGE: &str =
    "ImageSet version resource is not found.";

/// The text this service refuses a copy whose caller named a version that is no longer
/// the image set's latest with. A `latestVersionId` is an optimistic-concurrency check:
/// a stale one means the caller's view of the image set is out of date, which is what
/// `ConflictException` (HTTP 409) reports. Constant, echoing neither version.
pub const IMAGE_SET_VERSION_CONFLICT_MESSAGE: &str =
    "The latest image set version identifier provided does not match the image set's latest version.";

/// The text this service refuses a copy of (or into) an image set that is not `ACTIVE`
/// with — one still `LOCKED` by a copy in flight, or already `DELETED`. Constant, and
/// stated by rule rather than by which image set it was.
pub const IMAGE_SET_NOT_ACTIVE_MESSAGE: &str =
    "The image set is not in the ACTIVE state, so it cannot take part in a copy.";

/// The text this service answers a request naming an image frame an image set does not
/// hold with — the resource KIND, by the same rule as [`IMAGE_SET_NOT_FOUND_MESSAGE`],
/// with no identifier from the request in it.
pub const IMAGE_FRAME_NOT_FOUND_MESSAGE: &str = "ImageFrame resource is not found.";

/// The storage tier every image set this emulator holds reports.
///
/// `StorageTier`'s other value, `ARCHIVE_INSTANT_ACCESS`, is reached by archiving, and
/// this service's model binds no operation that archives an image set
/// (`smithy_query(query="list_operations")`) — so an image set here never leaves the
/// tier a new one is created in.
pub const DEFAULT_STORAGE_TIER: &str = "FREQUENT_ACCESS";

/// The text this service refuses a `nextToken` it did not issue with — fixed, and
/// independent of the token supplied, so the refusal reveals nothing about how a real
/// token is built.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str =
    "Failed to perform requested operation due to invalid NextToken. Please try again.";

/// The words that join a constraint clause's subject to its requirement. The framework
/// renders `Value '<v>' at '<path>'` before it and the requirement after it, so it is
/// also where [`constraint_clause`] cuts the framework's clause in two.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// One constraint violation, worded the way this service words it.
///
/// The framework already states every violation class in the AWS sentence
/// (`Value '<v>' at '<path>' failed to satisfy constraint: <requirement>`), so that
/// rendering is REUSED — a constraint class this service has never seen is still phrased
/// like the rest — and exactly two things are changed:
///
/// * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` the model
///   spells it with. AWS advertises the expression a value must satisfy, not the
///   anchoring that makes the match total, and a client reconstructing the advertised
///   pattern from the message must get the one the service really emits.
/// * a violation on one of [`VALUE_WITHHOLDING_MEMBERS`] (or a DICOM attribute) is
///   stated as `<path> failed to satisfy constraint: <requirement>`, with no value at
///   all: this service does not reflect free text a caller composed, a key reference or
///   patient detail back into an error body. Every other member — including one that
///   names another AWS resource — keeps the echoed value. Both forms are in the
///   recorded corpus, from the same operation: the clauses about the data store's name
///   and its KMS key carry no value where the one about the authorizer's function ARN
///   does. See [`withholds_value`].
///
/// The framework's clause is split at its LAST separator, and a `@pattern` is replaced
/// in the requirement half only: the echoed value precedes it and is caller-controlled,
/// so a caller who sends the separator text — or the pattern itself — as the value
/// cannot have its own value rewritten or read as a requirement.
pub fn constraint_clause(violation: &Violation) -> String {
    let rendered = violation.message();
    let Some((subject, requirement)) = rendered.rsplit_once(CONSTRAINT_SEPARATOR) else {
        // A clause the framework states in some other shape: keep it verbatim rather
        // than guessing at its parts.
        return rendered;
    };
    let requirement = match &violation.kind {
        ViolationKind::Pattern(pattern) => {
            requirement.replace(pattern.as_str(), unanchored(pattern))
        }
        _ => requirement.to_string(),
    };
    if withholds_value(&violation.path) {
        return format!("{}{CONSTRAINT_SEPARATOR}{requirement}", violation.path);
    }
    format!("{subject}{CONSTRAINT_SEPARATOR}{requirement}")
}

/// The members whose value a refusal WITHHOLDS, spelled out so the rule's scope is
/// visible — every one of them is an input member this service's model constrains, in
/// the lowerCamel spelling a validation path uses, lowercased for comparison:
///
/// * `datastoreName` and `jobName` carry a caller-chosen NAME: free text a caller
///   composed, which in a clinical service can carry patient detail.
/// * `kmsKeyArn` names an encryption key, which is credential-adjacent.
/// * `copiableAttributes` carries a caller-composed JSON document naming the SOP
///   instances a copy takes, which the model marks `@sensitive`
///   (`smithy_query(query="shape",shape="MetadataCopies")`) — the model itself saying
///   the value must not be reproduced.
///
/// A member that names another AWS resource (`lambdaAuthorizerArn`, `dataAccessRoleArn`)
/// is in neither class and keeps its echoed value — which is what the recorded corpus
/// shows: the clauses about the data store's name and its KMS key carry no value where
/// the one about the authorizer's function ARN carries it.
const VALUE_WITHHOLDING_MEMBERS: &[&str] = &[
    "datastorename",
    "jobname",
    "kmskeyarn",
    "copiableattributes",
];

/// The DICOM attribute members, in the same lowercased spelling. The model marks every
/// one of them `@sensitive`
/// (`smithy_query(query="shape",shape="SearchByAttributeValue,DICOMStudyDateAndTime")`:
/// the five search attributes plus the study date and time), which is the model itself
/// saying the value must not be reproduced — so a refusal about one withholds it, by the
/// same rule. Listed one by one rather than matched by prefix, so the scope is exactly
/// the `@sensitive` set the model declares and a future non-sensitive `DICOM*` member
/// does not silently join it.
const SENSITIVE_DICOM_MEMBERS: &[&str] = &[
    "dicompatientid",
    "dicomaccessionnumber",
    "dicomstudyid",
    "dicomstudyinstanceuid",
    "dicomseriesinstanceuid",
    "dicomstudydate",
    "dicomstudytime",
];

/// Whether a refusal about `path` withholds the value it refused.
///
/// Keyed on the MEMBER — the last segment of the validation path — never on what the
/// caller sent: the same member withholds its value whatever the violation and whatever
/// the value was.
fn withholds_value(path: &str) -> bool {
    let member = path.rsplit('.').next().unwrap_or(path).to_ascii_lowercase();
    VALUE_WITHHOLDING_MEMBERS.contains(&member.as_str())
        || SENSITIVE_DICOM_MEMBERS.contains(&member.as_str())
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$` escaped
/// by a backslash is a literal dollar sign inside the expression, not the end anchor, so
/// it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The `ValidationException` message for one or more violations: the AWS envelope
/// (`N validation error(s) detected: `) over one [`constraint_clause`] each, in the
/// order they were accumulated. `None` when there is nothing to state.
pub fn validation_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations.iter().map(constraint_clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The envelope for a single violation a HANDLER found itself, so a refusal raised past
/// the boundary validator reads exactly like one the boundary stated. Delegates to
/// [`validation_envelope`], so the envelope text is spelled in one place.
pub fn one_validation_error(violation: &Violation) -> String {
    validation_envelope(::std::slice::from_ref(violation)).unwrap_or_default()
}

/// The `ValidationException` message for a requirement no [`ViolationKind`] states —
/// one a HANDLER checks itself, such as a member the model documents as holding a JSON
/// document, or two members that cannot hold together. Worded through the same envelope
/// and the same `<subject> failed to satisfy constraint: <requirement>` clause as every
/// refusal the boundary validator states, so the two read alike.
///
/// `value` is the rejected value, and is echoed under exactly the rule the boundary
/// refusals follow: withheld for a member [`withholds_value`] covers, and withheld when
/// the caller passes `None` because the requirement is about the member's presence or
/// its relation to another member rather than about what it held.
pub fn handler_requirement_error(path: &str, value: Option<&str>, requirement: &str) -> String {
    let clause = match value {
        Some(shown) if !withholds_value(path) => {
            format!("Value '{shown}' at '{path}'{CONSTRAINT_SEPARATOR}{requirement}")
        }
        _ => format!("{path}{CONSTRAINT_SEPARATOR}{requirement}"),
    };
    format!("1 validation error detected: {clause}")
}

/// The `@pattern` of `com.amazonaws.medicalimaging#Arn`
/// (`smithy_query(query="shape",shape="Arn")`), the shape every `resourceArn` path
/// label targets, copied verbatim.
///
/// It is the single source for two things: [`parse_resource_arn`] below, and the way
/// [`crate::service`] recognises a malformed RESOURCE ARN — a refused value is one
/// when the `@pattern` it failed is this one. No other member targets this shape:
/// `kmsKeyArn`, `lambdaAuthorizerArn` and `dataAccessRoleArn` each target their own
/// ARN shape with its own pattern, so they keep the generic constraint refusal.
pub const ARN_PATTERN: &str = "^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$";

/// The ARN partitions [`ARN_PATTERN`] admits: `aws` plus the four suffixed ones
/// (`(-us-gov)|(-iso)|(-iso-b)|(-cn))?`).
pub const ARN_PARTITIONS: [&str; 5] = ["aws", "aws-us-gov", "aws-iso", "aws-iso-b", "aws-cn"];

/// How many tags one resource will hold at once.
///
/// A memory bound, not an AWS quota: the tagging operations persist caller-supplied
/// keys and values in process memory, so without a cap one caller could grow a
/// resource's tag map without limit. Past the cap EVERY operation that installs tags
/// — `CreateDatastore` as much as `TagResource` — is refused with its own modelled
/// `ValidationException` (HTTP 400). The value matches AWS's general per-resource tag
/// limit of 50.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The text this service refuses a request that would leave one resource holding more
/// than [`MAX_TAGS_PER_RESOURCE`] tags with. `requested` is how many the resource would
/// end up with — a count computed from the request, so the sentence says which side of
/// the bound the caller landed on without echoing any key.
///
/// One sentence for both write paths: the create path and the tag path enforce one
/// bound, so they state it with one text.
pub fn too_many_tags_message(requested: usize) -> String {
    format!(
        "this emulator holds at most {MAX_TAGS_PER_RESOURCE} tags per resource; this \
         request would leave {requested}"
    )
}

// ---------------------------------------------------------------------------
// What makes a TAG KEY legal
//
// Every rule about a tag KEY is applied here, by every handler that accepts one,
// because the boundary validator applies NONE of them: the validating walk captures a
// collection's ELEMENT constraints only ("Map keys are not captured" —
// `vendor/smithy-macros/src/lib.rs`), so a `TagMap` key is never visited at all, and
// `TagKey`'s `@pattern` opens with a negative lookahead the `regex` crate cannot
// compile, which `pattern_matches`
// (`vendor/smithy-schema-server/src/constraints.rs`) treats as matching. A tag VALUE
// needs nothing here: it IS the map's element, so `TagValue`'s `@length` reaches it
// through the `ElementConstraintsTrait` the boundary already runs.
// ---------------------------------------------------------------------------

/// The `@pattern` the model declares on `TagKey`, the key shape of `TagMap`
/// (`smithy_query(query="shape",shape="TagMap,TagKey")`), copied verbatim: it is the
/// text a refusal quotes, and the two halves applied below are
/// [`TAG_KEY_RESERVED_PREFIX`] and the character class [`is_tag_key_character`] tests
/// membership of.
pub const TAG_KEY_PATTERN: &str = "^(?!aws:)[a-zA-Z+-=._:/]+$";

/// The prefix [`TAG_KEY_PATTERN`]'s leading `(?!aws:)` refuses: the tag namespace AWS
/// reserves for itself. A negative lookahead anchored at the start of an expression says
/// exactly "the value does not start with this", so it is applied as a `starts_with` —
/// and in the one case the model spells it with, not case-insensitively, because the
/// pattern is what the service advertises.
pub const TAG_KEY_RESERVED_PREFIX: &str = "aws:";

/// `TagKey`'s `@length` minimum (len 1..128). Also what [`TAG_KEY_PATTERN`]'s `+`
/// quantifier says: a key is a non-empty run of admitted characters.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// `TagKey`'s `@length` maximum (len 1..128) — the one tag-key rule the pattern does not
/// also state.
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// Whether `character` is one the class `[a-zA-Z+-=._:/]` of [`TAG_KEY_PATTERN`] admits.
///
/// That class admits the ASCII letters, `_`, and — because `+-=` inside a character
/// class is the RANGE from `+` (0x2B) to `=` (0x3D) — every character in that range: the
/// digits, `+`, `,`, `-`, `.`, `/`, `:`, `;`, `<` and `=`. The `.`, `:` and `/` the class
/// also lists one by one are inside that range too, so the three terms below are the
/// whole class. A character class is a MEMBERSHIP test rather than a pattern language, so
/// it is written out: the check then costs one pass over the key with no regex engine on
/// a path that reads request text.
fn is_tag_key_character(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || ('+'..='=').contains(&character)
}

/// Whether `key` satisfies [`TAG_KEY_PATTERN`]: it lies outside AWS's reserved namespace
/// AND it is a non-empty run of characters the class admits — which is exactly what the
/// pattern's lookahead, class and `+` quantifier say between the `^` and the `$`.
pub fn tag_key_matches_pattern(key: &str) -> bool {
    !key.starts_with(TAG_KEY_RESERVED_PREFIX)
        && !key.is_empty()
        && key.chars().all(is_tag_key_character)
}

/// The violation `key` commits as a `TagKey` at `path`, or `None` when it satisfies
/// every constraint the model puts on one.
///
/// The bounds are checked before the pattern, in the order the boundary validator's own
/// leaf walk runs them (`run_leaf_traits`), so a key that breaks both is reported the way
/// the framework would have reported it. The caller words the result through
/// [`one_validation_error`], so a refusal raised here reads like one the boundary stated.
pub fn tag_key_violation(path: &str, key: &str) -> Option<Violation> {
    let length = key.chars().count();
    let kind = if length < TAG_KEY_MIN_LENGTH {
        ViolationKind::MinLength(TAG_KEY_MIN_LENGTH as i64)
    } else if length > TAG_KEY_MAX_LENGTH {
        ViolationKind::MaxLength(TAG_KEY_MAX_LENGTH as i64)
    } else if !tag_key_matches_pattern(key) {
        ViolationKind::Pattern(TAG_KEY_PATTERN.to_string())
    } else {
        return None;
    };
    Some(Violation {
        path: path.to_string(),
        kind,
        value: Some(echoed_value(key)),
    })
}

/// The violation the keys of a supplied `TagMap` commit, or `None` when every key is
/// legal. `member` is the map member's own path (`tags`), which is where the refusal is
/// reported: a key is not a member of the map, so it has no path of its own to be
/// reported at, and the key that failed is carried as the clause's echoed VALUE.
///
/// The keys are examined in sorted order so that a map with more than one illegal key is
/// refused with the same clause every time — a `HashMap`'s iteration order is not
/// stable, and a refusal that varies run to run is not a contract.
pub fn tag_map_violation(member: &str, tags: &HashMap<String, String>) -> Option<Violation> {
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    keys.into_iter()
        .find_map(|key| tag_key_violation(member, key))
}

/// The rejected value as a MESSAGE carries it: itself when short, a
/// character-boundary-safe prefix with an elision when not.
///
/// Mirrors the framework's own elision (`echoed` in
/// `vendor/smithy-schema-server/src/constraints.rs`), which is private to that crate, cap
/// and rendering included. A tag key is a value NO boundary check bounds first — the walk
/// never visits a map key — so without this a caller could turn a one-megabyte key into a
/// one-megabyte message. Rendering ONLY: every check above runs on the original key.
fn echoed_value(value: &str) -> String {
    /// The longest rejected value a clause carries, as the framework caps it.
    const MAX_ECHOED_VALUE: usize = 256;

    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    // Truncate on a char boundary, never mid-UTF-8.
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// `DatastoreStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="DatastoreStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatastoreStatus {
    Creating,
    CreateFailed,
    Active,
    Deleting,
    Deleted,
}

impl DatastoreStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::CreateFailed => "CREATE_FAILED",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "CREATING" => Some(Self::Creating),
            "CREATE_FAILED" => Some(Self::CreateFailed),
            "ACTIVE" => Some(Self::Active),
            "DELETING" => Some(Self::Deleting),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// `JobStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="JobStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Submitted,
    InProgress,
    Completed,
    Failed,
}

impl JobStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InProgress => "IN_PROGRESS",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "SUBMITTED" => Some(Self::Submitted),
            "IN_PROGRESS" => Some(Self::InProgress),
            "COMPLETED" => Some(Self::Completed),
            "FAILED" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One persisted DICOM import job, owned by the data store it imports into.
#[derive(Debug, Clone)]
pub struct ImportJob {
    pub job_id: String,
    /// `StartDICOMImportJob` leaves `jobName` optional; `DICOMImportJobProperties`
    /// makes it required, so [`ImportJob::reported_name`] supplies the fallback.
    pub job_name: Option<String>,
    pub job_status: JobStatus,
    pub datastore_id: String,
    pub data_access_role_arn: String,
    pub input_s3_uri: String,
    pub output_s3_uri: String,
    pub input_owner_account_id: Option<String>,
    pub import_configuration: Option<ImportConfiguration>,
    pub submitted_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub message: Option<String>,
    /// When the job is due to leave `SUBMITTED`.
    pub in_progress_due_at: SystemTime,
    /// When the job is due to reach `COMPLETED`.
    pub completed_due_at: SystemTime,
}

impl ImportJob {
    /// The job name `DICOMImportJobProperties` reports. The properties shape makes
    /// `jobName` `@required` while the start request leaves it optional, so a job
    /// submitted without one reports its own identifier — which satisfies the member's
    /// `^[A-Za-z0-9._/#-]+$` pattern.
    pub fn reported_name(&self) -> String {
        self.job_name.clone().unwrap_or_else(|| self.job_id.clone())
    }

    /// Materialise every status change the clock owed this job by `now`.
    fn advance_to(&mut self, now: SystemTime) {
        if matches!(self.job_status, JobStatus::Completed | JobStatus::Failed) {
            return;
        }
        if now >= self.in_progress_due_at && self.job_status == JobStatus::Submitted {
            self.job_status = JobStatus::InProgress;
        }
        if now >= self.completed_due_at {
            self.job_status = JobStatus::Completed;
            // The due instant, not `now`: `endedAt` must not change between reads.
            self.ended_at = Some(self.completed_due_at);
        }
    }
}

/// The `contentType` `GetImageFrame` reports for pixel data whose stored transfer
/// syntax the response documentation does not list — the documented default
/// ("Default is `application/octet-stream`").
pub const DEFAULT_IMAGE_FRAME_CONTENT_TYPE: &str = "application/octet-stream";

/// The `contentType` `GetImageFrame` reports when the stored transfer syntax is
/// MISSING, which the documentation gives the same answer as UID
/// `1.2.840.10008.1.2.4.202`.
pub const MISSING_TRANSFER_SYNTAX_CONTENT_TYPE: &str = "image/jph";

/// Every stored DICOM transfer syntax UID the `GetImageFrame` response documents a
/// `contentType` for, with the content type it documents for it.
///
/// Transcribed from the member's own documentation —
/// `smithy_query(query="doc",shape="GetImageFrameResponse",member="contentType")` —
/// including `video/H256` for HEVC/H.265, which is how AWS spells it there. A UID this
/// table does not list falls back to [`DEFAULT_IMAGE_FRAME_CONTENT_TYPE`].
pub const IMAGE_FRAME_CONTENT_TYPES: [(&str, &str); 22] = [
    ("1.2.840.10008.1.2.1", "application/octet-stream"),
    ("1.2.840.10008.1.2.4.50", "image/jpeg"),
    ("1.2.840.10008.1.2.4.91", "image/j2c"),
    // MPEG2.
    ("1.2.840.10008.1.2.4.100", "video/mpeg"),
    ("1.2.840.10008.1.2.4.100.1", "video/mpeg"),
    ("1.2.840.10008.1.2.4.101", "video/mpeg"),
    ("1.2.840.10008.1.2.4.101.1", "video/mpeg"),
    // MPEG-4 AVC/H.264.
    ("1.2.840.10008.1.2.4.102", "video/mp4"),
    ("1.2.840.10008.1.2.4.102.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.103", "video/mp4"),
    ("1.2.840.10008.1.2.4.103.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.104", "video/mp4"),
    ("1.2.840.10008.1.2.4.104.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.105", "video/mp4"),
    ("1.2.840.10008.1.2.4.105.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.106", "video/mp4"),
    ("1.2.840.10008.1.2.4.106.1", "video/mp4"),
    // HEVC/H.265.
    ("1.2.840.10008.1.2.4.107", "video/H256"),
    ("1.2.840.10008.1.2.4.108", "video/H256"),
    // High-Throughput JPEG 2000.
    ("1.2.840.10008.1.2.4.202", "image/jph"),
    ("1.2.840.10008.1.2.4.203", "image/jphc"),
    // JPEG XL.
    ("1.2.840.10008.1.2.4.112", "image/jxl"),
];

/// The `contentType` `GetImageFrame` reports for pixel data STORED in
/// `transfer_syntax_uid`: what [`IMAGE_FRAME_CONTENT_TYPES`] documents for it, the
/// missing-transfer-syntax answer when the frame declares none, and the documented
/// default for a UID the table does not list.
pub fn image_frame_content_type(transfer_syntax_uid: Option<&str>) -> &'static str {
    let Some(uid) = transfer_syntax_uid else {
        return MISSING_TRANSFER_SYNTAX_CONTENT_TYPE;
    };
    IMAGE_FRAME_CONTENT_TYPES
        .iter()
        .find(|(documented, _)| *documented == uid)
        .map_or(DEFAULT_IMAGE_FRAME_CONTENT_TYPE, |(_, content_type)| {
            *content_type
        })
}

/// One image frame — the pixel data of one frame of one image set, as it was stored.
///
/// Pixel data belongs to the image SET rather than to one of its versions:
/// `GetImageFrame` takes no version, so every version of an image set reaches the same
/// frames.
#[derive(Debug, Clone)]
pub struct ImageFrame {
    /// `ImageFrameId` (`^[0-9a-z]{32}$`) — the identifier this frame is reached by.
    pub image_frame_id: String,
    /// The DICOM transfer syntax UID the pixel data is STORED in, `None` when the
    /// instance it came from declared none. `GetImageFrame`'s `contentType` is computed
    /// from it (see [`image_frame_content_type`]); the bytes themselves are never
    /// transcoded.
    pub transfer_syntax_uid: Option<String>,
    /// The pixel data, byte for byte as it was stored.
    pub pixel_data: Vec<u8>,
}

/// `ImageSetState`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="ImageSetState")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSetState {
    Active,
    Locked,
    Deleted,
}

impl ImageSetState {
    /// The wire form of this state.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Locked => "LOCKED",
            Self::Deleted => "DELETED",
        }
    }

    /// Parse a wire state, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ACTIVE" => Some(Self::Active),
            "LOCKED" => Some(Self::Locked),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// `ImageSetWorkflowStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="ImageSetWorkflowStatus")`) — what last happened
/// to an image set, as against [`ImageSetState`], which is whether it can be read and
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSetWorkflowStatus {
    Created,
    Copied,
    Copying,
    CopyingWithReadOnlyAccess,
    CopyFailed,
    Updating,
    UpdatingForStudyConsistency,
    Updated,
    UpdateFailed,
    Deleting,
    Deleted,
    Importing,
    Imported,
    ImportFailed,
}

impl ImageSetWorkflowStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Copied => "COPIED",
            Self::Copying => "COPYING",
            Self::CopyingWithReadOnlyAccess => "COPYING_WITH_READ_ONLY_ACCESS",
            Self::CopyFailed => "COPY_FAILED",
            Self::Updating => "UPDATING",
            Self::UpdatingForStudyConsistency => "UPDATING_FOR_STUDY_CONSISTENCY",
            Self::Updated => "UPDATED",
            Self::UpdateFailed => "UPDATE_FAILED",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
            Self::Importing => "IMPORTING",
            Self::Imported => "IMPORTED",
            Self::ImportFailed => "IMPORT_FAILED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "CREATED" => Some(Self::Created),
            "COPIED" => Some(Self::Copied),
            "COPYING" => Some(Self::Copying),
            "COPYING_WITH_READ_ONLY_ACCESS" => Some(Self::CopyingWithReadOnlyAccess),
            "COPY_FAILED" => Some(Self::CopyFailed),
            "UPDATING" => Some(Self::Updating),
            "UPDATING_FOR_STUDY_CONSISTENCY" => Some(Self::UpdatingForStudyConsistency),
            "UPDATED" => Some(Self::Updated),
            "UPDATE_FAILED" => Some(Self::UpdateFailed),
            "DELETING" => Some(Self::Deleting),
            "DELETED" => Some(Self::Deleted),
            "IMPORTING" => Some(Self::Importing),
            "IMPORTED" => Some(Self::Imported),
            "IMPORT_FAILED" => Some(Self::ImportFailed),
            _ => None,
        }
    }
}

/// One VERSION of an image set — what `GetImageSet`, `ListImageSetVersions` and the
/// copy properties report.
///
/// State and workflow status live HERE and nowhere else: an image set's current state
/// is its latest version's, so the two can never disagree.
#[derive(Debug, Clone)]
pub struct ImageSetVersion {
    /// The external version identifier, `1` for an image set's first version. The wire
    /// form is `ImageSetExternalVersionId` (`^\d+$`), rendered by
    /// [`ImageSetVersion::wire_version_id`].
    pub version_id: u32,
    pub image_set_state: ImageSetState,
    pub workflow_status: ImageSetWorkflowStatus,
    /// When this version last changed. Set from the instant the change fell DUE, never
    /// from `now`, so it does not move between reads.
    pub updated_at: SystemTime,
    /// `Overrides.forced`: whether the `force` flag was used when creating this
    /// version, which is exactly what the model says the member reports.
    pub forced: bool,
    /// `message` — the error message an image set action failed with, absent otherwise.
    pub message: Option<String>,
}

impl ImageSetVersion {
    /// The version identifier as `ImageSetExternalVersionId` spells it (`^\d+$`).
    pub fn wire_version_id(&self) -> String {
        self.version_id.to_string()
    }
}

/// A change to an image set the CLOCK owes — the copy a `CopyImageSet` call started,
/// which lands on the latest version once [`IMAGE_SET_COPY_DELAY`] has passed.
#[derive(Debug, Clone)]
pub struct PendingImageSetTransition {
    pub due_at: SystemTime,
    pub image_set_state: ImageSetState,
    pub workflow_status: ImageSetWorkflowStatus,
    /// The pixel data this transition DELIVERS when it lands, keyed by image frame id —
    /// the frames a copy in flight is writing into its destination. `None` for a
    /// transition that delivers none, such as the release of the copy's read-only lock
    /// on its source. Nothing is readable through `GetImageFrame` before the transition
    /// that delivers it falls due, because nothing holds it yet.
    pub delivered_image_frames: Option<HashMap<String, ImageFrame>>,
}

/// One persisted image set, with every version of it the data store holds.
#[derive(Debug, Clone)]
pub struct ImageSet {
    /// The tags `TagResource` / `ListTagsForResource` keep for this image set, which
    /// an ARN reaches through its optional `/imageset/<id>` suffix (see
    /// [`ARN_PATTERN`]).
    pub tags: HashMap<String, String>,
    pub image_set_id: String,
    pub image_set_arn: String,
    /// The newest version. Held apart from [`ImageSet::earlier_versions`] so that
    /// "an image set has at least one version" is a TYPE invariant: no reader has to
    /// handle an empty version list, and no handler can create an image set without
    /// one.
    pub latest_version: ImageSetVersion,
    /// Every superseded version, oldest first.
    pub earlier_versions: Vec<ImageSetVersion>,
    /// The pixel data this image set holds, keyed by `ImageFrameId` — what
    /// `GetImageFrame` serves. Empty for an image set no frame has been delivered to
    /// yet.
    pub image_frames: HashMap<String, ImageFrame>,
    /// Whether this image set is in the primary DICOM hierarchy (`isPrimary`).
    pub is_primary: bool,
    pub created_at: SystemTime,
    pub deleted_at: Option<SystemTime>,
    /// What the clock owes the latest version, if anything.
    pub pending: Option<PendingImageSetTransition>,
}

impl ImageSet {
    /// A freshly minted image set at version [`FIRST_IMAGE_SET_VERSION_ID`], in the
    /// state and workflow status its creator puts it in.
    ///
    /// The ONE place an image set record is built, so every creator — the copy's
    /// destination today, a DICOM import later — agrees on the invariants.
    pub fn new(
        image_set_id: String,
        image_set_arn: String,
        image_set_state: ImageSetState,
        workflow_status: ImageSetWorkflowStatus,
        is_primary: bool,
        forced: bool,
        now: SystemTime,
    ) -> Self {
        Self {
            tags: HashMap::new(),
            image_set_id,
            image_set_arn,
            latest_version: ImageSetVersion {
                version_id: FIRST_IMAGE_SET_VERSION_ID,
                image_set_state,
                workflow_status,
                updated_at: now,
                forced,
                message: None,
            },
            earlier_versions: Vec::new(),
            // A freshly minted image set holds no pixel data: a frame is readable only
            // once something has delivered it (the copy that completes, an import).
            image_frames: HashMap::new(),
            is_primary,
            created_at: now,
            deleted_at: None,
            pending: None,
        }
    }

    /// Every version this image set holds, oldest first — the order
    /// `ListImageSetVersions` reports them in.
    pub fn versions(&self) -> impl Iterator<Item = &ImageSetVersion> {
        self.earlier_versions
            .iter()
            .chain(::std::iter::once(&self.latest_version))
    }

    /// The version `wire_version_id` (`^\d+$`) names, or `None` when this image set
    /// holds no such version.
    pub fn version(&self, wire_version_id: &str) -> Option<&ImageSetVersion> {
        self.versions()
            .find(|version| version.wire_version_id() == wire_version_id)
    }

    /// The image frame `image_frame_id` names, or `None` when this image set holds no
    /// pixel data under that identifier.
    pub fn image_frame(&self, image_frame_id: &str) -> Option<&ImageFrame> {
        self.image_frames.get(image_frame_id)
    }

    /// The latest version identifier, as the wire spells it.
    pub fn latest_version_id(&self) -> String {
        self.latest_version.wire_version_id()
    }

    /// Supersede the latest version with a new one, numbered one past it, and answer
    /// the new version's identifier. The old latest becomes the newest earlier version,
    /// so a reader naming it still gets what it reported before.
    pub fn push_version(
        &mut self,
        image_set_state: ImageSetState,
        workflow_status: ImageSetWorkflowStatus,
        forced: bool,
        now: SystemTime,
    ) -> u32 {
        let version_id = self.latest_version.version_id.saturating_add(1);
        let superseded = ::std::mem::replace(
            &mut self.latest_version,
            ImageSetVersion {
                version_id,
                image_set_state,
                workflow_status,
                updated_at: now,
                forced,
                message: None,
            },
        );
        self.earlier_versions.push(superseded);
        version_id
    }

    /// Materialise the transition the clock owed this image set by `now`.
    fn advance_to(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.latest_version.image_set_state = pending.image_set_state;
        self.latest_version.workflow_status = pending.workflow_status;
        // The due instant, not `now`: `updatedAt` must not move between reads.
        self.latest_version.updated_at = pending.due_at;
        // The pixel data the transition owed lands with it, so a frame becomes readable
        // exactly when the copy that delivers it completes. A frame already held under
        // the same identifier is replaced by the delivered one: an image frame id names
        // its pixel data, so the two cannot disagree.
        if let Some(delivered) = pending.delivered_image_frames {
            self.image_frames.extend(delivered);
        }
        self.pending = None;
    }
}

/// One persisted data store, with the import jobs and image sets it owns.
#[derive(Debug, Clone)]
pub struct Datastore {
    pub datastore_id: String,
    /// `CreateDatastore` leaves `datastoreName` optional; `DatastoreProperties` and
    /// `DatastoreSummary` make it required, so [`Datastore::reported_name`] supplies
    /// the fallback.
    pub datastore_name: Option<String>,
    pub datastore_status: DatastoreStatus,
    pub datastore_arn: String,
    pub kms_key_arn: Option<String>,
    pub lambda_authorizer_arn: Option<String>,
    pub lossless_storage_format: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the next status change this data store owes falls due, if any.
    pub status_due_at: Option<SystemTime>,
    pub import_jobs: HashMap<String, ImportJob>,
    pub image_sets: HashMap<String, ImageSet>,
}

impl Datastore {
    /// The data store name `DatastoreProperties` / `DatastoreSummary` report. Both
    /// shapes make `datastoreName` `@required` while the create request leaves it
    /// optional, so a store created without one reports its own identifier — which
    /// satisfies the member's `^[A-Za-z0-9._/#-]+$` pattern.
    pub fn reported_name(&self) -> String {
        self.datastore_name
            .clone()
            .unwrap_or_else(|| self.datastore_id.clone())
    }

    /// Materialise every change the clock owed this data store by `now`.
    ///
    /// Returns `false` when the deletion it owed has fallen due, i.e. the record must
    /// be dropped from the store.
    fn advance_to(&mut self, now: SystemTime) -> bool {
        if let Some(due) = self.status_due_at {
            if now >= due {
                match self.datastore_status {
                    DatastoreStatus::Creating => {
                        self.datastore_status = DatastoreStatus::Active;
                        // The due instant, not `now`: `updatedAt` must be stable
                        // across reads.
                        self.updated_at = due;
                        self.status_due_at = None;
                    }
                    DatastoreStatus::Deleting => {
                        self.datastore_status = DatastoreStatus::Deleted;
                        self.updated_at = due;
                        self.status_due_at = None;
                        return false;
                    }
                    _ => self.status_due_at = None,
                }
            }
        }
        for job in self.import_jobs.values_mut() {
            job.advance_to(now);
        }
        for image_set in self.image_sets.values_mut() {
            image_set.advance_to(now);
        }
        true
    }
}

/// Per-service state for com-amazonaws-medicalimaging-ahigatewayservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMedicalimagingAhigatewayserviceState {
    /// Every live data store, keyed by its 32-character lowercase-alphanumeric id.
    pub datastores: HashMap<String, Datastore>,
    /// `@idempotencyToken` ledger: a scoped client token (see [`client_token_key`])
    /// mapped to the id it minted, so a repeat call returns the SAME resource. Bounded
    /// by [`MAX_CLIENT_TOKENS`]; write it only through
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::remember_client_token`].
    pub client_tokens: HashMap<String, String>,
    /// The ledger's keys in insertion order, which is what makes the eviction in
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::remember_client_token`] FIFO.
    pub client_token_order: VecDeque<String>,
    /// Monotonic counter behind [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`].
    pub next_id: u64,
}

impl ComAmazonawsMedicalimagingAhigatewayserviceState {
    /// Mint a fresh 32-character lowercase-hex id, which satisfies both
    /// `DatastoreId` (`^[0-9a-z]{32}$`) and `JobId` (`^[0-9a-z]+$`, 1..32).
    ///
    /// Deterministic: the n-th id of a fresh store is always the same, because the
    /// bits come from SplitMix64 over a counter rather than from the system RNG.
    /// SplitMix64 is a bijection on `u64`, so two counter values never collide.
    pub fn mint_id(&mut self) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let high = splitmix64(self.next_id);
        let low = splitmix64(self.next_id ^ 0x5DEE_CE66_D1B5_7A4F);
        format!("{high:016x}{low:016x}")
    }

    /// Record that `key` minted `minted_id`, evicting the oldest token once the ledger
    /// is at [`MAX_CLIENT_TOKENS`], so repeated creates cannot grow it without limit.
    pub fn remember_client_token(&mut self, key: String, minted_id: String) {
        if self.client_tokens.insert(key.clone(), minted_id).is_none() {
            self.client_token_order.push_back(key);
        }
        while self.client_token_order.len() > MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                None => break,
            }
        }
    }

    /// The tag map of the resource `arn` names — a data store, or one image set it
    /// owns — or `None` when this emulator holds no such resource.
    ///
    /// One lookup for both resource kinds, so the three tagging operations agree on
    /// what exists: tags live ON the resource record rather than in a map beside it.
    /// The ARN must be the resource's OWN: a data store is reached by its id, and
    /// then only when the ARN it was named by is the one the data store reports.
    pub fn resource_tags(&self, arn: &ResourceArn) -> Option<&HashMap<String, String>> {
        let datastore = self.datastores.get(&arn.datastore_id)?;
        if datastore.datastore_arn != arn.datastore_arn {
            return None;
        }
        match &arn.image_set_id {
            None => Some(&datastore.tags),
            Some(image_set_id) => Some(&datastore.image_sets.get(image_set_id)?.tags),
        }
    }

    /// [`Self::resource_tags`], for a writer.
    pub fn resource_tags_mut(&mut self, arn: &ResourceArn) -> Option<&mut HashMap<String, String>> {
        let datastore = self.datastores.get_mut(&arn.datastore_id)?;
        if datastore.datastore_arn != arn.datastore_arn {
            return None;
        }
        match &arn.image_set_id {
            None => Some(&mut datastore.tags),
            Some(image_set_id) => Some(&mut datastore.image_sets.get_mut(image_set_id)?.tags),
        }
    }

    /// Materialise every status change the clock owed by `now`, dropping the data
    /// stores whose deletion has fallen due — and, with them, the ledger entries that
    /// named the data store or any of its import jobs, so a create/delete loop cannot
    /// leave entries behind forever. Every handler calls this first, so each request
    /// sees the state the clock already made true.
    pub fn advance_clock(&mut self, now: SystemTime) {
        let mut retired: HashSet<String> = HashSet::new();
        self.datastores.retain(|_, datastore| {
            if datastore.advance_to(now) {
                return true;
            }
            retired.insert(datastore.datastore_id.clone());
            retired.extend(datastore.import_jobs.keys().cloned());
            false
        });
        if retired.is_empty() {
            return;
        }
        self.client_tokens
            .retain(|_, minted_id| !retired.contains(minted_id));
        let live = &self.client_tokens;
        self.client_token_order.retain(|key| live.contains_key(key));
    }
}

/// The ledger key for one `@idempotencyToken`.
///
/// Scoped by operation (and, for an import job, by the data store it runs against), so
/// the same token reused on a different operation mints a different resource rather
/// than returning the first one.
pub fn client_token_key(scope: &str, client_token: &str) -> String {
    format!("{scope}\u{0}{client_token}")
}

/// SplitMix64's finalizer — a bijection on `u64`, which is what makes
/// [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`] collision-free.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The region this request is signed for, used to build ARNs.
///
/// Read from the SigV4 credential scope (`<akid>/<date>/<region>/<name>/aws4_request`)
/// through the one parser the core vends; an unsigned request falls back to
/// [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a data store in `region`:
/// `arn:aws:medical-imaging:<region>:<account>:datastore/<datastoreId>`.
pub fn datastore_arn(region: &str, datastore_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("datastore/{datastore_id}"))
}

/// The ARN of one image set inside a data store: that data store's OWN ARN with the
/// `/imageset/<imageSetId>` suffix [`ARN_PATTERN`] admits — which is what makes the
/// tagging operations reach the image set through [`ResourceArn`].
pub fn image_set_arn(datastore_arn: &str, image_set_id: &str) -> String {
    format!("{datastore_arn}/imageset/{image_set_id}")
}

/// What a `resourceArn` label names: a data store, or ONE image set inside it —
/// the two resources [`ARN_PATTERN`] can spell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceArn {
    /// The ARN of the data store, i.e. the label minus any `/imageset/<id>` suffix.
    ///
    /// Carried, not just the id, because an ARN names a resource only when it is
    /// THAT resource's ARN: one differing in partition, region or account names a
    /// resource of another account or region, which this emulator does not hold.
    pub datastore_arn: String,
    pub datastore_id: String,
    /// `Some` only when the ARN carries the optional `/imageset/<id>` suffix.
    pub image_set_id: Option<String>,
}

/// Parse a medical-imaging resource ARN, or `None` when the string is not one.
///
/// Accepts exactly what [`ARN_PATTERN`] does — partition, this service's
/// `arnNamespace`, a region, a 12-digit account id, and a `datastore/<id>` resource
/// with an optional `/imageset/<id>` suffix — so a value the boundary validator let
/// through always parses, and a caller that reaches the handler by another route is
/// still refused rather than mis-read.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceArn> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if !ARN_PARTITIONS.contains(&partition) {
        return None;
    }
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    // `[a-z0-9-]+`: the pattern constrains the region's alphabet, not its shape.
    let region = fields.next()?;
    if region.is_empty()
        || !region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    // `[0-9]{12}`: an account id.
    let account_id = fields.next()?;
    if account_id.len() != 12 || !account_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut segments = fields.next()?.split('/');
    if segments.next()? != "datastore" {
        return None;
    }
    let datastore_id = segments.next().filter(|id| is_resource_id(id))?.to_string();
    let image_set_id = match segments.next() {
        None => None,
        Some("imageset") => Some(segments.next().filter(|id| is_resource_id(id))?.to_string()),
        Some(_) => return None,
    };
    if segments.next().is_some() {
        return None;
    }
    Some(ResourceArn {
        datastore_arn: format!(
            "arn:{partition}:{ARN_SERVICE}:{region}:{account_id}:datastore/{datastore_id}"
        ),
        datastore_id,
        image_set_id,
    })
}

/// `[0-9a-z]{32}`: the id form [`ARN_PATTERN`] gives both `datastore/` and
/// `/imageset/`, and the form [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`]
/// produces.
fn is_resource_id(value: &str) -> bool {
    value.len() == 32
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// A modelled timestamp for `instant`, kept to full nanosecond precision rather than
/// routed through an `f64` number of seconds.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(seconds, since_epoch.subsec_nanos())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMedicalimagingAhigatewayserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMedicalimagingAhigatewayserviceState>)
}
