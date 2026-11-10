// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3vectors-s3vectors service.
//!
//! Holds the vector bucket -> vector index -> vector hierarchy every handler in
//! `src/ops/` reads and writes, together with the lookup, validation, distance
//! and metadata-filter helpers they share. `src/lib.rs` is generated and
//! `src/ops/mod.rs` is an `include!` of generated code, so this module is the
//! one place a helper several operations need can live.

pub mod authorization;
pub mod key_access;
pub mod metadata_budget;

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime};

use aws_smithy_types::{Document, Number};

use crate::ops::types::{
    EncryptionConfiguration, Index, IndexSummary, MetadataConfiguration, NotFoundException,
    ValidationException, ValidationExceptionField, VectorBucket, VectorBucketSummary,
};

/// Region used for a generated ARN when the request carries no SigV4 credential
/// scope to read one from (an unsigned request). Every signed request's ARNs use
/// the region the caller signed for.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service segment, from the model's `arnNamespace` / `endpointPrefix`.
pub const ARN_SERVICE: &str = "s3vectors";

/// `SseType` applied when a create request names no encryption configuration:
/// the model defaults `EncryptionConfiguration$sseType` to `AES256`, and
/// CreateVectorBucket documents that "by default, if you don't specify, all new
/// vectors in Amazon S3 vector buckets use server-side encryption with Amazon S3
/// managed keys (SSE-S3), specifically AES256".
pub const DEFAULT_SSE_TYPE: &str = "AES256";

/// The `SseType` value that `EncryptionConfiguration$kmsKeyArn` is allowed with:
/// that member documents "this parameter is allowed if and only if `sseType` is
/// set to `aws:kms`".
pub const KMS_SSE_TYPE: &str = "aws:kms";

/// The `fieldList` path the service names a broken encryption configuration by: the
/// CONFIGURATION, spelled as the recorded response spells it, rather than either of
/// the two members whose combination is what is wrong.
pub const ENCRYPTION_CONFIGURATION_PATH: &str = "EncryptionConfiguration";

/// The `ValidationException` message for an encryption configuration that names the
/// KMS encryption type with no key for it: ONE fixed sentence naming the two members
/// of the rule, carrying no value from the request. This is the sentence the real
/// service answered in the recorded response; the status (400) and the error code
/// (`ValidationException`) are the requirements either way.
pub const KMS_KEY_ARN_REQUIRED: &str =
    "kmsKeyArn must be specified when sseType is set to aws:kms";

/// See [`KMS_KEY_ARN_REQUIRED`]: the other direction of the same biconditional — a key
/// named beside an encryption type that takes none — stated the same way, so a client
/// reads which half of the rule it broke from the sentence.
pub const KMS_KEY_ARN_NOT_ALLOWED: &str =
    "kmsKeyArn is not allowed when sseType is not set to aws:kms";

/// The `IndexMode` a vector bucket's default carries from the moment it is created,
/// and so the mode every vector index created in that bucket inherits.
///
/// `VectorBucket$defaultIndexMode` is "the mode that is automatically assigned to new
/// vector indexes in the vector bucket": the assignment is the SERVICE's, not the
/// caller's — neither CreateVectorBucket nor CreateIndex takes a mode — so a bucket
/// that has never been handed one still has this default, and reports it. The model's
/// `IndexMode` enum holds `CLASSIC` and `ENHANCED`; this is the one the real service
/// answered for a freshly created vector bucket in the recorded response.
pub const DEFAULT_INDEX_MODE: &str = "ENHANCED";

/// `DistanceMetric` values, from the model's enum (`cosine`, `euclidean`).
pub const DISTANCE_METRIC_COSINE: &str = "cosine";
/// See [`DISTANCE_METRIC_COSINE`].
pub const DISTANCE_METRIC_EUCLIDEAN: &str = "euclidean";

/// Page size a list operation uses when the caller names no `maxResults`: the
/// top of the member's own modelled range (`ListVectorBuckets$maxResults` and
/// `ListIndexes$maxResults` are `range 1..500`), so a caller that asks for no
/// page size gets as much as one page can hold.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 500;

/// How many results ONE `QueryVectors` response carries, whatever `topK` asks for.
///
/// `QueryVectors` is `@paginated` (`inputToken`/`outputToken` `nextToken`, `items`
/// `vectors`) but takes no page-size member, so the per-response limit is the service's:
/// a query for more than this many nearest neighbours is drained over pages, each
/// continuing the distance order the previous one reported in. This is the size the real
/// service answered with for a `topK` above it in the recorded response (100 results plus
/// a `nextToken` for a `topK` of 130).
pub const QUERY_RESULTS_PER_PAGE: usize = 100;

/// Ceilings on how much this emulator stores.
///
/// These are the EMULATOR's bounds, not published service quotas: the store is
/// this process's memory, so a create or put loop with no ceiling grows the
/// resident set until the host runs out. A request that would cross one is
/// refused with the `ServiceQuotaExceededException` (HTTP 402) the model already
/// declares on these operations for exactly this ("Your request exceeds a
/// service quota"), which is the channel the real service refuses a quota on
/// too. Every ceiling is far above what any test or demo needs.
pub const MAX_VECTOR_BUCKETS: usize = 500;
/// See [`MAX_VECTOR_BUCKETS`].
pub const MAX_INDEXES_PER_BUCKET: usize = 200;
/// See [`MAX_VECTOR_BUCKETS`].
pub const MAX_VECTORS_PER_INDEX: usize = 10_000;
/// See [`MAX_VECTOR_BUCKETS`]: 2,000,000 `f32` is 8 MiB of vector data per index,
/// so the stored data is bounded by bytes as well as by vector count.
pub const MAX_VECTOR_VALUES_PER_INDEX: usize = 2_000_000;
/// See [`MAX_VECTOR_BUCKETS`]: the standard AWS limit of 50 tags per resource,
/// which TagResource states as "you can add up to 50 tags for each resource".
pub const MAX_TAGS_PER_RESOURCE: usize = 50;
/// See [`MAX_VECTOR_BUCKETS`]: how many elements one vector's `metadata`
/// document may hold, counted over the whole document.
pub const MAX_METADATA_NODES: usize = 1024;
/// See [`MAX_VECTOR_BUCKETS`]: how deeply one vector's `metadata` document may
/// nest. Bounding it at the write is also what gives the equality walk a later
/// filter runs over the STORED document a known recursion bound.
pub const MAX_METADATA_DEPTH: u32 = 32;
/// See [`MAX_VECTOR_BUCKETS`]: how large a bucket policy document may be. The
/// Amazon S3 User Guide's bucket policy limit — "bucket policies are limited to
/// 20 KB in size" — is the bound a policy document is held to here.
pub const MAX_POLICY_BYTES: usize = 20 * 1024;

/// The tag-key namespace AWS reserves for its own tags.
///
/// The AWS tagging reference states that "the `aws:` prefix is reserved for AWS
/// use" and that a user cannot create, edit or delete a tag whose key carries
/// it, so a caller-supplied key in that namespace is refused rather than stored.
/// The model's `TagKey` pattern allows `:`, so this is a rule the model cannot
/// express and the handlers enforce (see [`validate_tag_keys`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The `ValidationException` message for a caller-supplied tag key in the reserved
/// namespace, as the service words it: ONE fixed sentence about the kind of tag the
/// request tried to write, carrying no value from the request. This is the sentence the
/// real service answered in the recorded response; the status (400) and the error code
/// (`ValidationException`) are the requirements either way.
pub const SYSTEM_TAG_REFUSED: &str = "System tags cannot be added/updated by requester";

/// See [`MAX_VECTOR_BUCKETS`]: how many deleted vectors one index keeps as tombstones
/// (see [`VectorIndexRecord::tombstones`]). A tombstone holds the vector it replaced, so
/// the pool a query ranks — and the memory an index costs — stays bounded however many
/// deletes a caller issues; at the ceiling the oldest tombstone is given up, which is the
/// one point this emulator models the graph compaction that frees a candidate slot.
pub const MAX_TOMBSTONES_PER_INDEX: usize = 1_000;

/// How long a tag write on ONE resource takes to commit.
///
/// Tag state on a single resource is updated under optimistic concurrency control, so the
/// span a write is in flight is what decides a racing caller's fate: a mutation that
/// arrives while another mutation of the SAME resource is still committing is refused with
/// `ConflictException` (HTTP 409), the signal to retry its read-modify-write. The write is
/// committed before its own caller is answered, so a caller that waits for its response
/// can never collide with itself — only genuinely overlapping callers do, whatever this
/// span is. The value is this emulator's own; AWS publishes no commit latency.
pub const TAG_WRITE_COMMIT: Duration = Duration::from_millis(250);

/// The two `Effect` values the IAM policy grammar allows in a statement.
pub const POLICY_EFFECT_ALLOW: &str = "Allow";
/// See [`POLICY_EFFECT_ALLOW`].
pub const POLICY_EFFECT_DENY: &str = "Deny";

/// Per-service state for com-amazonaws-s3vectors-s3vectors. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsS3vectorsS3vectorsState {
    /// Vector buckets by `vectorBucketName`. A `BTreeMap` so ListVectorBuckets
    /// order and its `nextToken` cursor are deterministic.
    pub buckets: BTreeMap<String, VectorBucketRecord>,
    /// Resource tags by ARN, as written by a create request's `tags` and read and
    /// pruned by the `/tags/{resourceArn}` operations. A resource carrying no tags
    /// holds NO entry, so an absent entry and an empty map mean the same thing.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The tag writes still committing, by resource ARN, each holding the instant its
    /// commit completes ([`TAG_WRITE_COMMIT`]). This is the optimistic lock the tagging
    /// operations take: see [`Self::begin_tag_write`]. An entry whose instant has passed
    /// is one whose caller never came back to release it, and holds nothing up.
    pub tag_writes: BTreeMap<String, Instant>,
}

/// A stored vector bucket, with the vector indexes it contains.
#[derive(Debug, Clone)]
pub struct VectorBucketRecord {
    pub name: String,
    /// Minted once, at create, and reported unchanged by every later read.
    pub arn: String,
    pub creation_time: SystemTime,
    pub encryption: EncryptionRecord,
    /// The mode new vector indexes in this bucket are assigned, which every read of
    /// the bucket reports. [`DEFAULT_INDEX_MODE`] from the moment the bucket is
    /// created, until PutVectorBucketDefaultIndexMode replaces it.
    pub default_index_mode: String,
    /// The bucket policy document as PutVectorBucketPolicy was given it, in the form
    /// the service renders a policy it reports back
    /// ([`::service_common::iam_policy_rendering::rendered`]). `None` until a policy is
    /// put, and `None` again once
    /// DeleteVectorBucketPolicy removes it; GetVectorBucketPolicy answers
    /// `NotFoundException` while it is `None`.
    pub policy: Option<String>,
    pub indexes: BTreeMap<String, VectorIndexRecord>,
}

/// A stored vector index, with the vectors it holds.
#[derive(Debug, Clone)]
pub struct VectorIndexRecord {
    pub bucket_name: String,
    pub name: String,
    /// Minted once, at create, and reported unchanged by every later read.
    pub arn: String,
    pub creation_time: SystemTime,
    pub data_type: String,
    pub dimension: i32,
    pub distance_metric: String,
    /// Metadata keys that may be read back but not filtered on.
    pub non_filterable_metadata_keys: Option<Vec<String>>,
    pub encryption: EncryptionRecord,
    /// The mode this index processes queries in, which every read of the index
    /// reports. Inherited from the bucket's [`VectorBucketRecord::default_index_mode`]
    /// when the index is created, since CreateIndex takes no mode of its own.
    pub index_mode: String,
    /// Vectors by key. A `BTreeMap` so ListVectors order, its `nextToken`
    /// cursor and its segment partition are deterministic.
    pub vectors: BTreeMap<String, StoredVector>,
    /// The vectors DeleteVectors removed, newest last, for as long as the index keeps
    /// them (see [`MAX_TOMBSTONES_PER_INDEX`] and [`Self::candidate_pool`]).
    ///
    /// A deleted vector is gone from every read of the index — it is in no `vectors`
    /// entry — but the slot it occupies in the approximate-nearest-neighbour graph is
    /// not freed by the delete, so it still costs a query one of the candidates that
    /// query asked for. A queue, so the oldest tombstone is the one the ceiling gives up
    /// and giving it up costs nothing.
    pub tombstones: VecDeque<StoredVector>,
}

/// One stored vector: its key, its `float32` data and its metadata document.
#[derive(Debug, Clone)]
pub struct StoredVector {
    pub key: String,
    pub data: Vec<f32>,
    /// The vector's metadata. `PutInputVector$metadata` defaults to `{}`, so a
    /// put that names none stores an empty document rather than nothing.
    pub metadata: Document,
}

/// A stored encryption configuration.
#[derive(Debug, Clone)]
pub struct EncryptionRecord {
    pub sse_type: String,
    pub kms_key_arn: Option<String>,
}

impl Default for EncryptionRecord {
    fn default() -> Self {
        Self {
            sse_type: DEFAULT_SSE_TYPE.to_string(),
            kms_key_arn: None,
        }
    }
}

/// Why a lookup or an unmodelled input rule refused the request.
///
/// Returned by the helpers in this module so one implementation serves every
/// operation; each handler lifts it into its own modelled error channel (the
/// error enums are per operation, so the helpers cannot name them).
#[derive(Debug)]
pub enum LookupError {
    /// The named bucket, index or resource does not exist -> `NotFoundException` (404).
    NotFound(String),
    /// The request is well-formed per the model but breaks a rule the model
    /// cannot express -> `ValidationException` (400).
    Invalid {
        message: String,
        /// The input the failure is about, for `fieldList`: the path that entry
        /// names and the sentence it carries. That sentence is the top-level
        /// `message` for a refusal about the member alone, and the member's own
        /// half of it where the top-level sentence says more about the request
        /// (see [`LookupError::invalid_record`]).
        field: Option<(String, String)>,
    },
}

impl LookupError {
    /// A refusal about `path`, an input member path.
    pub fn invalid(path: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::Invalid {
            field: Some((path.to_string(), message.clone())),
            message,
        }
    }

    /// A refusal that is not about one member.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
            field: None,
        }
    }

    /// A refusal about ONE RECORD of a batch: the element of the input list that
    /// record was submitted in, and `defect`, the sentence saying what is wrong
    /// with it.
    ///
    /// The two halves of the body say different things here, which is what this
    /// constructor exists for: `fieldList` names the list ELEMENT and carries the
    /// defect alone, while the top-level `message` states which record of the batch
    /// was rejected — by the `key` the caller submitted it under, the only thing
    /// that identifies a record to the client that sent the batch.
    pub fn invalid_record(path: &str, key: &str, defect: impl Into<String>) -> Self {
        let defect = defect.into();
        Self::Invalid {
            message: format!("Invalid record for key '{key}': {defect}"),
            field: Some((path.to_string(), defect)),
        }
    }

    /// An absent resource.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    /// Whether this is the 404 case rather than the 400 one.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }

    /// The modelled 404 payload. Empty for the `Invalid` case, which a handler
    /// renders through [`validation_exception`](Self::validation_exception).
    pub fn not_found_exception(&self) -> NotFoundException {
        NotFoundException {
            message: match self {
                Self::NotFound(message) => message.clone(),
                Self::Invalid { message, .. } => message.clone(),
            },
        }
    }

    /// The modelled 400 payload, naming the offending input in `fieldList`.
    ///
    /// HOW that input is spelled is the refusing LAYER's, not this service's choice to
    /// make once: a `ValidationExceptionField$path` is "a path about the validation
    /// exception" and the recorded responses show S3 Vectors spelling it three ways, one
    /// per layer that refuses —
    ///
    /// * a value outside a constraint the MODEL declares is refused ahead of every
    ///   handler and names its member by a JSON POINTER into the request (`/dimension`),
    ///   which is `crate::service`'s half of the refusal;
    /// * a rule a handler enforces about ONE input member names that member as the model
    ///   spells it (`vectorBucketName`), or the input STRUCTURE whose members cannot hold
    ///   together as the model spells that shape ([`ENCRYPTION_CONFIGURATION_PATH`]);
    /// * a rule about one RECORD of a batch names the element of the input list it was
    ///   submitted in, in the subscript form ([`LookupError::invalid_record`]).
    ///
    /// Each of the three is the spelling the real service answered in a recorded
    /// response, so they are reproduced rather than unified.
    pub fn validation_exception(&self) -> ValidationException {
        match self {
            Self::NotFound(message) => ValidationException {
                message: message.clone(),
                field_list: None,
            },
            Self::Invalid { message, field } => ValidationException {
                message: message.clone(),
                field_list: field.as_ref().map(|(path, message)| {
                    vec![ValidationExceptionField {
                        path: path.clone(),
                        message: message.clone(),
                    }]
                }),
            },
        }
    }
}

/// The `NotFoundException` message for a vector bucket this account does not hold,
/// as the service words it: ONE fixed sentence per resource kind, carrying no value
/// from the request. A client branching on the resource that is missing reads the
/// kind from the sentence, so the two kinds say different things, and the kind a
/// refusal names is the one the OPERATION resolves — not whichever lookup missed on
/// the way to it (see [`ComAmazonawsS3vectorsS3vectorsState::index`]). This one is
/// the sentence the real service answered in the recorded response; the status (404)
/// and the error code (`NotFoundException`) are the requirements either way.
pub const BUCKET_NOT_FOUND: &str = "The specified vector bucket could not be found";

/// See [`BUCKET_NOT_FOUND`]: the sentence for the other resource kind, as the real
/// service answered it in the recorded response. The service names this kind "index"
/// where it names the other one "vector bucket", so the index wording is NOT the
/// bucket sentence with the kind swapped — it is this sentence, verbatim.
pub const INDEX_NOT_FOUND: &str = "The specified index could not be found";

/// See [`BUCKET_NOT_FOUND`]: the same sentence for the third resource kind a
/// `NotFoundException` can be about — the policy of a vector bucket that exists
/// but carries none — so a client reads the missing kind from the sentence and a
/// bucket that is missing cannot be confused with a policy that is.
pub const POLICY_NOT_FOUND: &str = "The specified vector bucket policy could not be found";

/// The `ConflictException` message for a create whose name a resource of that kind
/// already carries, as the service words it: ONE fixed sentence per resource kind,
/// saying only that a resource of that kind with "the specified name" exists. The
/// name the request asked for is NOT interpolated — the caller already holds it, and
/// the real service's sentence is the same for every name — so a client that
/// tolerates duplicate-create by inspecting the text matches one constant string.
/// These are the sentences the real service answered in the recorded responses; the
/// status (409) and the error code (`ConflictException`) are the requirements either
/// way.
pub const BUCKET_ALREADY_EXISTS: &str = "A vector bucket with the specified name already exists";

/// See [`BUCKET_ALREADY_EXISTS`]: the sentence for the other resource kind. As with
/// the not-found sentences, the service names this kind "index" where it names the
/// other one "vector bucket".
pub const INDEX_ALREADY_EXISTS: &str = "An index with the specified name already exists";

/// The `ConflictException` message for deleting a vector bucket that still holds
/// vector indexes. The same shape as the sentences above: ONE fixed sentence about
/// the state the bucket is in, interpolating neither the bucket name the request
/// carried nor how many indexes the store holds for it, so a teardown routine that
/// tells "delete the children first" from "already gone" matches one constant string.
pub const BUCKET_NOT_EMPTY: &str = "The specified vector bucket is not empty";

/// The `ConflictException` message for a tag mutation that raced another mutation of the
/// same resource: ONE fixed sentence, carrying no value from the request, saying what the
/// caller has to do about it — the conflict is between two callers rather than about
/// anything either of them named. The sentence is about the TAGS, which is what the
/// optimistic lock guards, so a client retrying a tag read-modify-write tells this 409
/// from every other condition the operation answers 409 for.
pub const TAG_WRITE_CONFLICT: &str =
    "The resource tags were modified by another request. Please retry the operation.";

/// The `ValidationException` message for a parallel ListVectors whose `segmentIndex` is
/// not inside the partition its `segmentCount` describes.
///
/// `ListVectorsInput$segmentIndex` documents the rule — "Segment IDs are zero-based, so
/// the first segment is always 0 [...] The value of segmentIndex must be less than the
/// value provided for segmentCount" — and this is ONE fixed sentence restating that
/// rule, interpolating neither submitted number: the caller already holds both, and a
/// client that branches on the text matches one constant string.
pub const SEGMENT_INDEX_OUT_OF_RANGE: &str =
    "Segment index is zero-indexed and must be smaller than segment count";

/// Wording for the one failure that is not about the request: the shared engine
/// did not hand this service's store back. Every operation models
/// `ServiceUnavailableException`, which is what that answers with.
pub const STATE_UNAVAILABLE: &str = "the S3 Vectors emulator state store is unavailable";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3vectorsS3vectorsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3vectorsS3vectorsState>)
}

/// The region this request is signed for, which every ARN this request mints
/// carries. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:s3vectors:<region>:<account>:bucket/<bucket>`, the form
/// `VectorBucketArn`'s pattern declares.
pub fn bucket_arn(region: &str, bucket: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("bucket/{bucket}"))
}

/// `arn:aws:s3vectors:<region>:<account>:bucket/<bucket>/index/<index>`, the
/// form `IndexArn`'s pattern declares.
pub fn index_arn(region: &str, bucket: &str, index: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("bucket/{bucket}/index/{index}"),
    )
}

/// The resource portion of an ARN: everything after the sixth `:`.
fn arn_resource(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(ARN_SERVICE), Some(_region), Some(_account)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    fields.next()
}

/// The bucket name a `vectorBucketArn` names.
pub fn bucket_from_arn(member: &str, arn: &str) -> Result<String, LookupError> {
    let resource = arn_resource(arn)
        .and_then(|resource| resource.strip_prefix("bucket/"))
        .filter(|bucket| !bucket.is_empty() && !bucket.contains('/'));
    match resource {
        Some(bucket) => Ok(bucket.to_string()),
        None => Err(LookupError::invalid(
            member,
            format!("`{member}` is not a vector bucket ARN: {arn}"),
        )),
    }
}

/// The (bucket, index) pair an `indexArn` names.
pub fn index_from_arn(member: &str, arn: &str) -> Result<(String, String), LookupError> {
    let parsed = arn_resource(arn)
        .and_then(|resource| resource.strip_prefix("bucket/"))
        .and_then(|rest| rest.split_once("/index/"))
        .filter(|(bucket, index)| {
            !bucket.is_empty()
                && !bucket.contains('/')
                && !index.is_empty()
                && !index.contains('/')
        });
    match parsed {
        Some((bucket, index)) => Ok((bucket.to_string(), index.to_string())),
        None => Err(LookupError::invalid(
            member,
            format!("`{member}` is not a vector index ARN: {arn}"),
        )),
    }
}

/// Which resource a request's name was for, for the one refusal a name outside the
/// form that kind of resource is named by gets.
///
/// The kinds are worded differently — the service says "vector bucket" where it says
/// "index", exactly as its not-found and already-exists sentences do (see
/// [`BUCKET_NOT_FOUND`]) — so a client branching on the text tells them apart, and the
/// member a request carries the name in is the path the refusal names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    /// A vector bucket's name, carried in `vectorBucketName`.
    VectorBucket,
    /// A vector index's name, carried in `indexName`.
    Index,
}

impl NameKind {
    /// The input member a request carries this kind of name in, which is the path the
    /// refusal's `fieldList` entry names.
    pub const fn member(self) -> &'static str {
        match self {
            Self::VectorBucket => "vectorBucketName",
            Self::Index => "indexName",
        }
    }

    /// The `ValidationException` message for a name outside this kind's form: ONE
    /// fixed sentence, which neither echoes the rejected name nor restates the rule,
    /// so a client matching the text matches one constant string. This is the sentence
    /// the real service answered in the recorded response for a vector bucket; the
    /// status (400) and the error code (`ValidationException`) are the requirements
    /// either way.
    pub const fn invalid_name(self) -> &'static str {
        match self {
            Self::VectorBucket => "Invalid vector bucket name",
            Self::Index => "Invalid index name",
        }
    }
}

/// The bucket name form the model's `VectorBucketArn` / `IndexArn` patterns
/// encode for both a bucket and an index segment:
/// `[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]` — 3 to 63 characters, lowercase
/// alphanumerics, dots and hyphens, starting and ending alphanumeric.
pub fn validate_resource_name(kind: NameKind, value: &str) -> Result<(), LookupError> {
    let bytes = value.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let well_formed = (3..=63).contains(&bytes.len())
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|b| alnum(*b) || *b == b'-' || *b == b'.');
    if well_formed {
        return Ok(());
    }
    Err(LookupError::invalid(kind.member(), kind.invalid_name()))
}

/// The `ValidationException` message for a `policy` document the service will not
/// accept, as the service words it: ONE fixed sentence that says the document is not a
/// policy and nothing about WHICH rule it broke — no parser diagnostic, no line or
/// column, no member name. This is the sentence the real service answered in the
/// recorded response; the status (400) and the error code (`ValidationException`) are
/// the requirements either way.
pub const POLICY_MALFORMED: &str = "The provided policy is malformed or invalid";

/// Check a bucket policy document against the IAM resource-policy grammar.
///
/// `PutVectorBucketPolicy$policy` is "the JSON that defines the policy", and the
/// AWS IAM reference gives that JSON's grammar: a policy is a JSON object whose
/// optional `Version` is a string, whose `Id` is a string, and whose required
/// `Statement` is one statement object or an array of them; every statement's
/// `Effect` is exactly `Allow` or `Deny` and names an action through `Action` or
/// `NotAction`. Anything that is not a policy document by that grammar —
/// starting with text that is not JSON at all — is refused as invalid input,
/// never stored: a stored non-policy would be handed back by
/// GetVectorBucketPolicy as if the service had accepted it.
///
/// Which rule the document broke decides only WHETHER it is refused, never what the
/// refusal says: every one of them is [`POLICY_MALFORMED`], the one sentence the
/// service answers for a document that is not a policy.
///
/// Only the grammar is checked here. Whether the principals, actions and
/// resources a well-formed statement names exist is not a grammar question, and
/// a policy may legitimately name a principal or a resource in another account.
pub fn validate_bucket_policy(policy: &str) -> Result<(), LookupError> {
    if is_policy_document(policy) {
        Ok(())
    } else {
        Err(LookupError::invalid_request(POLICY_MALFORMED))
    }
}

/// Whether `policy` is a policy document by the grammar [`validate_bucket_policy`]
/// states, and within the size [`MAX_POLICY_BYTES`] bounds.
fn is_policy_document(policy: &str) -> bool {
    if policy.len() > MAX_POLICY_BYTES {
        return false;
    }
    let Ok(document) = serde_json::from_str::<serde_json::Value>(policy) else {
        return false;
    };
    let Some(object) = document.as_object() else {
        return false;
    };
    for key in ["Version", "Id"] {
        if object.get(key).is_some_and(|value| !value.is_string()) {
            return false;
        }
    }
    let statements: Vec<&serde_json::Value> = match object.get("Statement") {
        Some(serde_json::Value::Array(statements)) => statements.iter().collect(),
        Some(other) => vec![other],
        None => return false,
    };
    if statements.is_empty() {
        return false;
    }
    statements.iter().all(|statement| {
        let Some(statement) = statement.as_object() else {
            return false;
        };
        let effect = statement.get("Effect").and_then(serde_json::Value::as_str);
        matches!(effect, Some(POLICY_EFFECT_ALLOW) | Some(POLICY_EFFECT_DENY))
            && (statement.contains_key("Action") || statement.contains_key("NotAction"))
    })
}

/// The `ValidationException` message for a bucket selector that is not EXACTLY one
/// of the two accepted forms, as the service words it: ONE fixed sentence covering
/// the whole selector rule — naming neither form, or naming both — carrying no value
/// from the request, so a client that branches on the text matches one constant
/// string. The "but not both" half is operative: the two forms are mutually
/// exclusive, so a request carrying both is refused even when they agree. This is the
/// sentence the real service answered in the recorded response; the status (400) and
/// the error code (`ValidationException`) are the requirements either way.
pub const BUCKET_SELECTOR_REFUSED: &str =
    "Must specify either vectorBucketName or vectorBucketArn but not both";

/// See [`BUCKET_SELECTOR_REFUSED`]: the one fixed sentence for the other selector —
/// the index pair `vectorBucketName` + `indexName` against `indexArn` — which the
/// service words in terms of the ARN first. Every selector that is not exactly one
/// complete form takes it: neither form, half of the name pair, or the ARN alongside
/// either name.
pub const INDEX_SELECTOR_REFUSED: &str =
    "Must specify either indexArn or both vectorBucketName and indexName";

/// Resolve the bucket a request names by `vectorBucketName` or `vectorBucketArn`.
///
/// The operations that take both document "you must use either the vector bucket
/// name or the vector bucket ARN": the two forms are MUTUALLY EXCLUSIVE, so a
/// request names exactly one of them. Naming neither, and naming both — even two
/// that agree — are the same refusal ([`BUCKET_SELECTOR_REFUSED`]); only the form
/// that was named is then parsed.
pub fn resolve_bucket_name(
    name: Option<&str>,
    arn: Option<&str>,
) -> Result<String, LookupError> {
    match (name, arn) {
        (Some(name), None) => Ok(name.to_string()),
        (None, Some(arn)) => bucket_from_arn("vectorBucketArn", arn),
        (Some(_), Some(_)) | (None, None) => {
            Err(LookupError::invalid_request(BUCKET_SELECTOR_REFUSED))
        }
    }
}

/// Resolve the index a request names by `vectorBucketName` + `indexName`, or by
/// `indexArn`.
///
/// The operations that take both document "you can either use both the vector
/// bucket name and the vector index name, or use the vector index ARN": as with the
/// bucket selector the two forms are MUTUALLY EXCLUSIVE, so a request carries
/// exactly one COMPLETE form. Neither form, half of the name pair, and the ARN
/// alongside either name are the same refusal ([`INDEX_SELECTOR_REFUSED`]).
pub fn resolve_index_name(
    bucket_name: Option<&str>,
    index_name: Option<&str>,
    index_arn: Option<&str>,
) -> Result<(String, String), LookupError> {
    match (bucket_name, index_name, index_arn) {
        (Some(bucket), Some(index), None) => Ok((bucket.to_string(), index.to_string())),
        (None, None, Some(arn)) => index_from_arn("indexArn", arn),
        _ => Err(LookupError::invalid_request(INDEX_SELECTOR_REFUSED)),
    }
}

/// Who is naming the tag keys one request carries, which is what decides whether a key in
/// the reserved `aws:` namespace may be written ([`validate_tag_keys`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKeyOrigin {
    /// A request a CLIENT made. A key in the reserved namespace is AWS's own, so such a
    /// request may neither write nor remove one.
    Caller,
    /// A request another emulated service made in-process on behalf of a resource it
    /// provisions — the path the tags AWS sets from inside itself arrive on, which no
    /// client's request can be. A key in the reserved namespace travels on it.
    Service,
}

/// Where the tag keys this request names came from.
///
/// A tag in the reserved namespace is AWS's to set: the refusal
/// ([`SYSTEM_TAG_REFUSED`]) is the answer to an API CALLER, while the services that own
/// such a tag set it from inside AWS, through no public API. In this emulator one service
/// reaches another ONLY through the in-process callback
/// ([`::smithy_server_runtime::OperationContext::callback`]), so that callback is the
/// inside path, and the signal is the
/// [`::smithy_server_runtime::CallbackRecursionDepth`] request EXTENSION the framework's
/// callback transport stamps on every in-process call. An extension never travels over the
/// wire: nothing a client can send — on any socket, signed with any credential — sets it,
/// so a client's reserved key is refused exactly as before. It is the same framework signal
/// other service crates read to tell an in-process call from a client's request (Amazon
/// EventBridge's `PutEvents` reads it to tell a service's own event from a published one),
/// so nothing about the transport is reconstructed here.
pub fn tag_key_origin(ctx: &::smithy_server_runtime::OperationContext<'_>) -> TagKeyOrigin {
    match ctx
        .parts()
        .extensions
        .get::<::smithy_server_runtime::CallbackRecursionDepth>()
    {
        Some(_) => TagKeyOrigin::Service,
        None => TagKeyOrigin::Caller,
    }
}

/// Refuse a tag key that lies in the reserved `aws:` namespace, where the request naming it
/// is a caller's ([`TagKeyOrigin`]).
///
/// One implementation serves every operation that takes tag keys — a create request's
/// `tags`, TagResource's `tags`, UntagResource's `tagKeys` — because the namespace belongs
/// to AWS whichever member names it. The prefix is matched case-insensitively, as AWS
/// documents it.
pub fn validate_tag_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    origin: TagKeyOrigin,
) -> Result<(), LookupError> {
    if origin == TagKeyOrigin::Service {
        return Ok(());
    }
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    for key in keys {
        let bytes = key.as_bytes();
        if bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return Err(LookupError::invalid_request(SYSTEM_TAG_REFUSED));
        }
    }
    Ok(())
}

impl VectorIndexRecord {
    /// The candidates a nearest-neighbour query over this index ranks, each paired with
    /// whether it is a LIVE vector (`true`) or a tombstone of a deleted one (`false`).
    ///
    /// A query asks the graph for a bounded number of candidates and reports the ones
    /// that are still there: a tombstone is never reported, but it is in the pool that
    /// bound applies to, so a query for as many candidates as the index holds vectors is
    /// short by however many tombstones outrank them.
    pub fn candidate_pool(&self) -> Vec<(&StoredVector, bool)> {
        self.vectors
            .values()
            .map(|stored| (stored, true))
            .chain(self.tombstones.iter().map(|stored| (stored, false)))
            .collect()
    }

    /// Record that `key` was deleted: the vector leaves the index and its candidate slot
    /// stays occupied by a tombstone. A no-op for a key the index does not hold, so a
    /// repeated delete leaves exactly one tombstone.
    pub fn tombstone(&mut self, key: &str) {
        let Some(deleted) = self.vectors.remove(key) else {
            return;
        };
        self.tombstones.push_back(deleted);
        // At the ceiling the oldest tombstone is given up, which frees its slot: this is
        // what bounds what one index keeps however many deletes a caller issues.
        while self.tombstones.len() > MAX_TOMBSTONES_PER_INDEX {
            self.tombstones.pop_front();
        }
    }

    /// Give up the tombstone of `key`, which a write of that key replaces: the vector is
    /// in the index again, so it occupies its slot as a live candidate rather than twice.
    pub fn clear_tombstone(&mut self, key: &str) {
        self.tombstones.retain(|stored| stored.key != key);
    }
}

impl ComAmazonawsS3vectorsS3vectorsState {
    /// The bucket `name`, or `NotFoundException`.
    pub fn bucket(&self, name: &str) -> Result<&VectorBucketRecord, LookupError> {
        self.buckets
            .get(name)
            .ok_or_else(|| LookupError::not_found(BUCKET_NOT_FOUND))
    }

    /// The bucket `name` for mutation, or `NotFoundException`.
    pub fn bucket_mut(&mut self, name: &str) -> Result<&mut VectorBucketRecord, LookupError> {
        self.buckets
            .get_mut(name)
            .ok_or_else(|| LookupError::not_found(BUCKET_NOT_FOUND))
    }

    /// The index `index` of bucket `bucket`, or `NotFoundException`.
    ///
    /// The refusal names the resource the CALLER asked for, which is the index: an
    /// index of a vector bucket that does not exist is as absent as one of a bucket
    /// that does, so both answer [`INDEX_NOT_FOUND`] rather than reporting the
    /// intermediate bucket lookup. Only an operation whose own target IS the bucket
    /// resolves it through [`Self::bucket`] and so reports [`BUCKET_NOT_FOUND`].
    pub fn index(&self, bucket: &str, index: &str) -> Result<&VectorIndexRecord, LookupError> {
        self.buckets
            .get(bucket)
            .and_then(|bucket| bucket.indexes.get(index))
            .ok_or_else(|| LookupError::not_found(INDEX_NOT_FOUND))
    }

    /// The index `index` of bucket `bucket` for mutation, or `NotFoundException`.
    /// Refused exactly as [`Self::index`] is.
    pub fn index_mut(
        &mut self,
        bucket: &str,
        index: &str,
    ) -> Result<&mut VectorIndexRecord, LookupError> {
        self.buckets
            .get_mut(bucket)
            .and_then(|bucket| bucket.indexes.get_mut(index))
            .ok_or_else(|| LookupError::not_found(INDEX_NOT_FOUND))
    }

    /// The STORED ARN of the vector bucket or vector index a tagging request's
    /// `resourceArn` names, which is the key the tag map is held under.
    ///
    /// The tagging operations document that "the tagged resource can be a vector
    /// bucket or a vector index", and `ResourceARN` itself carries no pattern —
    /// `smithy_query(query="shape",shape="ResourceARN")` is a 1011-character
    /// `@arnReference` string — so the two halves of the rule are enforced here: a
    /// value that is neither a vector bucket ARN nor a vector index ARN is refused
    /// as invalid (400), and a well-formed ARN whose resource this store does not
    /// hold is the modelled `NotFoundException` (404), "the request was rejected
    /// because the specified resource can't be found".
    ///
    /// The ARN grammar itself is not re-implemented: the two parsers every other
    /// operation resolves an ARN through — [`index_from_arn`] and
    /// [`bucket_from_arn`] — are what accept or refuse it, chosen by whether the
    /// resource segment names an index.
    pub fn tagged_resource_arn(&self, member: &str, arn: &str) -> Result<String, LookupError> {
        let names_index = arn_resource(arn).is_some_and(|resource| resource.contains("/index/"));
        if names_index {
            let (bucket, index) = index_from_arn(member, arn)?;
            Ok(self.index(&bucket, &index)?.arn.clone())
        } else {
            let bucket = bucket_from_arn(member, arn)?;
            Ok(self.bucket(&bucket)?.arn.clone())
        }
    }

    /// Take the optimistic lock on the tag state of the resource `arn` names, so the
    /// write that follows commits against the state it read.
    ///
    /// `Err` is the `ConflictException` message for a caller whose write raced another
    /// mutation of the SAME resource: the tag state it read was invalidated while its own
    /// write was still being made, so there is nothing for it to do but retry. Release the
    /// lock with [`Self::end_tag_write`] once the write is committed.
    pub fn begin_tag_write(&mut self, arn: &str, now: Instant) -> Result<(), String> {
        // A lock whose commit instant has passed was left behind by a caller that never
        // came back for it, so it holds nothing up.
        self.tag_writes.retain(|_, commits_at| *commits_at > now);
        if self.tag_writes.contains_key(arn) {
            return Err(TAG_WRITE_CONFLICT.to_string());
        }
        self.tag_writes
            .insert(arn.to_string(), now + TAG_WRITE_COMMIT);
        Ok(())
    }

    /// Release the optimistic lock [`Self::begin_tag_write`] took.
    pub fn end_tag_write(&mut self, arn: &str) {
        self.tag_writes.remove(arn);
    }
}

/// One tag write's optimistic lock, held for as long as that write is in flight and
/// released however the request holding it ends.
///
/// The lock is taken before the commit span and released after it
/// ([`ComAmazonawsS3vectorsS3vectorsState::begin_tag_write`] /
/// [`ComAmazonawsS3vectorsS3vectorsState::end_tag_write`]), and the handler AWAITS that
/// span in between — which is a point the request can be dropped at: a client that
/// disconnected, a wait that ran out, a cancelled task. So the release hangs off this
/// value's `Drop` rather than off the code after that await: a request that goes away gives
/// the lock up as it goes, instead of leaving the next caller to be told about a conflict
/// with a write nobody is making any more.
///
/// [`Self::release`] is the committed path, taken under a store guard the caller already
/// holds so the release and the write it belongs to take the lock once between them.
pub struct TagWriteLock {
    /// The store the lock lives in, as the shared handle rather than a borrow: a value
    /// whose `Drop` releases the lock cannot borrow the guard the release needs.
    state: ::engine::ServiceState<ComAmazonawsS3vectorsS3vectorsState>,
    /// The resource whose tag state is locked.
    arn: String,
    /// Whether the lock is still this value's to release: [`Self::release`] clears it, so
    /// the committed path does not reach for the store a second time.
    held: bool,
}

/// Prints the resource the lock is on — an ARN, which is a public identifier.
impl ::std::fmt::Debug for TagWriteLock {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("TagWriteLock")
            .field("arn", &self.arn)
            .field("held", &self.held)
            .finish()
    }
}

impl TagWriteLock {
    /// Take the lock on the tag state of the resource `arn` names, under the store guard
    /// `store`. `Err` is the `ConflictException` message a racing mutation of the same
    /// resource is answered with ([`ComAmazonawsS3vectorsS3vectorsState::begin_tag_write`]).
    pub fn take(
        state: &::engine::ServiceState<ComAmazonawsS3vectorsS3vectorsState>,
        store: &mut ComAmazonawsS3vectorsS3vectorsState,
        arn: &str,
        now: Instant,
    ) -> Result<Self, String> {
        store.begin_tag_write(arn, now)?;
        Ok(Self {
            state: state.clone(),
            arn: arn.to_string(),
            held: true,
        })
    }

    /// Release the lock under a store guard the caller holds, which is the committed path:
    /// the write the caller is about to make and this release happen under the one guard.
    pub fn release(mut self, store: &mut ComAmazonawsS3vectorsS3vectorsState) {
        store.end_tag_write(&self.arn);
        self.held = false;
    }
}

impl Drop for TagWriteLock {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        // Every other path out of the write — a refusal after the commit span, and the
        // request being dropped at it — releases here, so no lock outlives the write it
        // was taken for.
        self.state.lock().end_tag_write(&self.arn);
    }
}

/// Read an `encryptionConfiguration` input member into a stored record,
/// defaulting as the model does and enforcing the `kmsKeyArn` rule.
///
/// `inherited` is the configuration the resource inherits when the request names
/// none: the bucket's, for an index ("by default, if you don't specify, all new
/// vectors in the vector index will use the encryption configuration of the
/// vector bucket"), and the service default for a bucket.
pub fn encryption_record(
    requested: Option<&EncryptionConfiguration>,
    inherited: EncryptionRecord,
) -> Result<EncryptionRecord, LookupError> {
    let Some(requested) = requested else {
        return Ok(inherited);
    };
    // `sseType` carries the model default `AES256`, so a configuration that
    // names only a key still reads as a declared type.
    let sse_type = if requested.sse_type.is_empty() {
        DEFAULT_SSE_TYPE
    } else {
        requested.sse_type.as_str()
    };
    // `kmsKeyArn` "is allowed if and only if `sseType` is set to `aws:kms`" — a
    // BICONDITIONAL, so both directions are refused: a key named beside another
    // encryption type, and the KMS type named with no key for it. Either way the
    // refusal is about the configuration as a whole rather than about one of the two
    // members, which is the path the service's `fieldList` entry names.
    let names_key = requested.kms_key_arn.is_some();
    let names_kms = sse_type == KMS_SSE_TYPE;
    if names_kms && !names_key {
        return Err(LookupError::invalid(
            ENCRYPTION_CONFIGURATION_PATH,
            KMS_KEY_ARN_REQUIRED,
        ));
    }
    if names_key && !names_kms {
        return Err(LookupError::invalid(
            ENCRYPTION_CONFIGURATION_PATH,
            KMS_KEY_ARN_NOT_ALLOWED,
        ));
    }
    Ok(EncryptionRecord {
        sse_type: sse_type.to_string(),
        kms_key_arn: requested.kms_key_arn.clone(),
    })
}

/// The stored encryption configuration as the modelled output shape.
pub fn encryption_view(record: &EncryptionRecord) -> EncryptionConfiguration {
    EncryptionConfiguration {
        sse_type: record.sse_type.clone(),
        kms_key_arn: record.kms_key_arn.clone(),
    }
}

/// The stored bucket as the modelled `VectorBucket` shape GetVectorBucket returns.
///
/// `defaultIndexMode` is reported by every read: the service assigns it when the
/// bucket is created, so there is no state of a bucket in which it is absent.
pub fn bucket_view(record: &VectorBucketRecord) -> VectorBucket {
    VectorBucket {
        vector_bucket_name: record.name.clone(),
        vector_bucket_arn: record.arn.clone(),
        creation_time: ::aws_smithy_types::DateTime::from(record.creation_time),
        encryption_configuration: Some(encryption_view(&record.encryption)),
        default_index_mode: Some(record.default_index_mode.clone()),
    }
}

/// The stored bucket as the modelled `VectorBucketSummary` ListVectorBuckets returns.
pub fn bucket_summary_view(record: &VectorBucketRecord) -> VectorBucketSummary {
    VectorBucketSummary {
        vector_bucket_name: record.name.clone(),
        vector_bucket_arn: record.arn.clone(),
        creation_time: ::aws_smithy_types::DateTime::from(record.creation_time),
    }
}

/// The stored index as the modelled `Index` shape GetIndex returns.
///
/// `indexMode` is reported by every read, for the same reason `defaultIndexMode` is
/// (see [`bucket_view`]): the index is assigned one when it is created.
pub fn index_view(record: &VectorIndexRecord) -> Index {
    Index {
        vector_bucket_name: record.bucket_name.clone(),
        index_name: record.name.clone(),
        index_arn: record.arn.clone(),
        creation_time: ::aws_smithy_types::DateTime::from(record.creation_time),
        data_type: record.data_type.clone(),
        dimension: record.dimension,
        distance_metric: record.distance_metric.clone(),
        metadata_configuration: record.non_filterable_metadata_keys.as_ref().map(|keys| {
            MetadataConfiguration {
                non_filterable_metadata_keys: keys.clone(),
            }
        }),
        encryption_configuration: Some(encryption_view(&record.encryption)),
        index_mode: Some(record.index_mode.clone()),
    }
}

/// The stored index as the modelled `IndexSummary` ListIndexes returns.
pub fn index_summary_view(record: &VectorIndexRecord) -> IndexSummary {
    IndexSummary {
        vector_bucket_name: record.bucket_name.clone(),
        index_name: record.name.clone(),
        index_arn: record.arn.clone(),
        creation_time: ::aws_smithy_types::DateTime::from(record.creation_time),
    }
}

/// The distance between two vectors under `metric`.
///
/// `cosine` is the cosine DISTANCE, `1 - cosine similarity`, so that a smaller
/// value is a nearer vector for both metrics and the nearest-neighbour ordering
/// is ascending. `euclidean` is the L2 distance. The model's `DistanceMetric`
/// enum has exactly these two values and the boundary validator refuses any
/// other, so `cosine` is the remaining case.
pub fn vector_distance(metric: &str, left: &[f32], right: &[f32]) -> f32 {
    if metric == DISTANCE_METRIC_EUCLIDEAN {
        let sum: f32 = left
            .iter()
            .zip(right.iter())
            .map(|(l, r)| (l - r) * (l - r))
            .sum();
        sum.sqrt()
    } else {
        let dot: f32 = left.iter().zip(right.iter()).map(|(l, r)| l * r).sum();
        let left_norm: f32 = left.iter().map(|l| l * l).sum::<f32>().sqrt();
        let right_norm: f32 = right.iter().map(|r| r * r).sum::<f32>().sqrt();
        if left_norm == 0.0 || right_norm == 0.0 {
            // Unreachable for stored and query vectors, which
            // `vector_data_defect` refuses as zero vectors under `cosine`.
            return 1.0;
        }
        1.0 - dot / (left_norm * right_norm)
    }
}

/// Why one vector's `float32` data cannot be written to (or queried against) the
/// index, or `None` when it can.
///
/// ONE sentence about the VECTOR, which names no member and no index: WHICH vector
/// the request got wrong is the path the refusal carries (the element of the input
/// list, or the query member), so the sentence is the same wherever a vector is
/// checked. The dimension and zero-norm sentences are the ones the real service
/// answered in the recorded responses.
///
/// `PutInputVector$data` documents all three rules: "vector dimensions must
/// match the dimension count that's configured for the vector index"; "for the
/// cosine distance metric, zero vectors (vectors containing all zeros) aren't
/// allowed"; and "for both cosine and euclidean distance metrics, vector data
/// must contain only valid floating-point values. Invalid values such as NaN
/// (Not a Number) or Infinity aren't allowed".
pub fn vector_data_defect(index: &VectorIndexRecord, data: &[f32]) -> Option<String> {
    let dimension = usize::try_from(index.dimension.max(0)).unwrap_or(0);
    if data.len() != dimension {
        return Some(format!(
            "vector must have length {dimension}, but has length {}",
            data.len()
        ));
    }
    if data.iter().any(|value| !value.is_finite()) {
        return Some(
            "vector data must contain only valid floating-point values, so NaN and \
             Infinity aren't allowed"
                .to_string(),
        );
    }
    if index.distance_metric == DISTANCE_METRIC_COSINE && data.iter().all(|value| *value == 0.0) {
        // Named after the PROPERTY that makes the vector unusable — its norm is zero,
        // so the metric is undefined for it — rather than after the member it arrived
        // in, so a client tells this refusal from the dimension and the data-type ones
        // that share its error code.
        return Some(format!(
            "{DISTANCE_METRIC_COSINE} distance does not support vectors with zero norm"
        ));
    }
    None
}

/// Normalize an input metadata document: `PutInputVector$metadata` declares the
/// default `{}`, so a put that names no metadata stores an empty document (an
/// absent document deserializes to `Document::Null`).
pub fn normalized_metadata(metadata: Document) -> Document {
    match metadata {
        Document::Null => Document::Object(std::collections::HashMap::new()),
        other => other,
    }
}

/// Why a vector's `metadata` document cannot be stored — it holds more elements than
/// [`MAX_METADATA_NODES`] or nests deeper than [`MAX_METADATA_DEPTH`] — or `None` when
/// it can, so one request cannot store an unbounded document and the equality walk a
/// later filter runs over the stored document has a known bound.
///
/// As with [`vector_data_defect`], ONE sentence about the document: which record of a
/// batch it belongs to is the path the refusal carries, not something the sentence
/// says.
///
/// Walks the document with an explicit stack rather than recursively, so the
/// walk itself costs no stack whatever the caller nests.
pub fn metadata_defect(metadata: &Document) -> Option<String> {
    let mut pending: Vec<(&Document, u32)> = vec![(metadata, 0)];
    let mut nodes: usize = 0;
    while let Some((node, depth)) = pending.pop() {
        nodes += 1;
        if nodes > MAX_METADATA_NODES {
            return Some(format!(
                "metadata holds more than the {MAX_METADATA_NODES} elements this \
                 service stores for one vector"
            ));
        }
        if depth > MAX_METADATA_DEPTH {
            return Some(format!(
                "metadata nests deeper than the {MAX_METADATA_DEPTH} levels this \
                 service stores for one vector"
            ));
        }
        match node {
            Document::Object(members) => {
                pending.extend(members.values().map(|value| (value, depth + 1)));
            }
            Document::Array(elements) => {
                pending.extend(elements.iter().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    None
}

/// The quota message for one more vector bucket, or `None` when there is room.
pub fn bucket_quota_exceeded(state: &ComAmazonawsS3vectorsS3vectorsState) -> Option<String> {
    (state.buckets.len() >= MAX_VECTOR_BUCKETS).then(|| {
        format!(
            "this emulator stores at most {MAX_VECTOR_BUCKETS} vector buckets, and {} already exist",
            state.buckets.len()
        )
    })
}

/// The quota message for one more index in `bucket`, or `None` when there is room.
pub fn index_quota_exceeded(bucket: &VectorBucketRecord) -> Option<String> {
    (bucket.indexes.len() >= MAX_INDEXES_PER_BUCKET).then(|| {
        format!(
            "this emulator stores at most {MAX_INDEXES_PER_BUCKET} vector indexes per vector \
             bucket, and {} already exists in {}",
            bucket.indexes.len(),
            bucket.name
        )
    })
}

/// The quota message for a resource that would end up carrying `count` tags, or
/// `None` when that is within the ceiling. TagResource documents the same bound —
/// "you can add up to 50 tags for each resource" — so a write that would cross it is
/// refused however many tags the resource already carried.
pub fn tag_count_exceeded(count: usize) -> Option<String> {
    (count > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "a resource carries at most {MAX_TAGS_PER_RESOURCE} tags, and the request \
             would leave it with {count}"
        )
    })
}

/// The quota message for a `tags` map on a resource that carries none yet, or `None`
/// when it is within the ceiling.
pub fn tag_quota_exceeded(tags: &std::collections::HashMap<String, String>) -> Option<String> {
    tag_count_exceeded(tags.len())
}

/// The quota message for applying `batch` to `index`, or `None` when the index
/// has room for it. Counts the batch as the upsert will apply it: a key the index
/// already holds replaces that vector rather than adding one.
pub fn vector_quota_exceeded(index: &VectorIndexRecord, batch: &[StoredVector]) -> Option<String> {
    let mut count = index.vectors.len();
    let mut values: usize = index.vectors.values().map(|stored| stored.data.len()).sum();
    for vector in batch {
        match index.vectors.get(&vector.key) {
            Some(existing) => {
                values = values.saturating_sub(existing.data.len()) + vector.data.len();
            }
            None => {
                count += 1;
                values += vector.data.len();
            }
        }
    }
    if count > MAX_VECTORS_PER_INDEX {
        return Some(format!(
            "this emulator stores at most {MAX_VECTORS_PER_INDEX} vectors per vector index, \
             and the request would leave {count} in {}",
            index.name
        ));
    }
    if values > MAX_VECTOR_VALUES_PER_INDEX {
        return Some(format!(
            "this emulator stores at most {MAX_VECTOR_VALUES_PER_INDEX} vector values per \
             vector index, and the request would leave {values} in {}",
            index.name
        ));
    }
    None
}

/// How deeply a `filter` may nest, and how many elements it may hold.
///
/// The document comes straight off the request and the evaluation walks it
/// RECURSIVELY, so an unbounded walk would let one caller exhaust the thread
/// stack — which aborts the process rather than failing that one request, taking
/// every other caller of this emulator down with it. [`validate_filter`] is the
/// ONE place these bounds are enforced, before any vector is scanned, so the same
/// filter is answered the same way whatever the index holds. The real service
/// bounds filter size too, so a refusal is the shape of answer it gives; these
/// particular numbers are this emulator's own and are far above any filter a
/// client writes by hand.
pub const MAX_FILTER_DEPTH: u32 = 32;
/// See [`MAX_FILTER_DEPTH`]: EVERY element of the filter document counts, the
/// elements of an operand array included.
pub const MAX_FILTER_NODES: u32 = 4096;

/// A comparison operator a metadata key's clause may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Nin,
    Exists,
}

impl FilterOperator {
    /// Classify an operator name, refusing one this service does not implement,
    /// and check that its operand is of the kind that operator takes.
    ///
    /// The SINGLE place the operator set and its operand rules are written:
    /// [`validate_filter`] and the evaluation both resolve an operator here, so
    /// the up-front check and the match cannot drift apart.
    fn parse(name: &str, key: &str, operand: &Document) -> Result<Self, LookupError> {
        let operator = match name {
            "$eq" => Self::Eq,
            "$ne" => Self::Ne,
            "$gt" => Self::Gt,
            "$gte" => Self::Gte,
            "$lt" => Self::Lt,
            "$lte" => Self::Lte,
            "$in" => Self::In,
            "$nin" => Self::Nin,
            "$exists" => Self::Exists,
            other => {
                return Err(LookupError::invalid(
                    "filter",
                    format!("unsupported metadata filter operator `{other}`"),
                ))
            }
        };
        match operator {
            Self::In | Self::Nin if !matches!(operand, Document::Array(_)) => Err(
                LookupError::invalid("filter", format!("`{name}` takes an array of values")),
            ),
            Self::Exists if !matches!(operand, Document::Bool(_)) => Err(LookupError::invalid(
                "filter",
                format!("`$exists` on metadata key `{key}` takes a boolean"),
            )),
            _ => Ok(operator),
        }
    }
}

/// What one `key: clause` pair of a filter level means.
enum FilterClause<'a> {
    /// `$and` (`all`): every branch must match. `$or`: any branch must match.
    Branches { branches: &'a [Document], all: bool },
    /// A metadata key's clause, as an object of comparison operators.
    Operators(&'a std::collections::HashMap<String, Document>),
    /// A metadata key's clause, as a literal to compare for equality.
    Literal(&'a Document),
}

/// Classify one `key: clause` pair of a filter level, refusing an unsupported
/// operator, an operand of the wrong kind, and a metadata key the index declares
/// non-filterable. Both the up-front check and the evaluation go through here.
fn classify_clause<'a>(
    key: &str,
    clause: &'a Document,
    non_filterable: &[String],
) -> Result<FilterClause<'a>, LookupError> {
    if let operator @ ("$and" | "$or") = key {
        let Document::Array(branches) = clause else {
            return Err(LookupError::invalid(
                "filter",
                format!("`{operator}` takes an array of metadata filters"),
            ));
        };
        return Ok(FilterClause::Branches {
            branches,
            all: operator == "$and",
        });
    }
    if key.starts_with('$') {
        return Err(LookupError::invalid(
            "filter",
            format!("unsupported metadata filter operator `{key}`"),
        ));
    }
    if non_filterable.iter().any(|non| non == key) {
        return Err(LookupError::invalid(
            "filter",
            format!(
                "metadata key `{key}` is a non-filterable metadata key of this vector \
                 index and can't be used in a filter"
            ),
        ));
    }
    match clause {
        Document::Object(operators) if operators.keys().any(|name| name.starts_with('$')) => {
            for name in sorted_keys(operators) {
                FilterOperator::parse(name, key, &operators[name])?;
            }
            Ok(FilterClause::Operators(operators))
        }
        literal => Ok(FilterClause::Literal(literal)),
    }
}

/// A document map's keys in sorted order, so the same document is always refused
/// for the same reason whatever order the map happens to hold.
fn sorted_keys(members: &std::collections::HashMap<String, Document>) -> Vec<&String> {
    let mut keys: Vec<&String> = members.keys().collect();
    keys.sort();
    keys
}

/// A node [`validate_filter`] has still to visit: a filter LEVEL, whose keys are
/// clauses, or a value inside one clause.
enum FilterNode<'a> {
    Level(&'a Document),
    Value(&'a Document),
}

/// Check a `filter` document ONCE, before it is evaluated against any vector.
///
/// Evaluation short-circuits — a clause that does not match ends it — and runs
/// per stored vector, so a filter checked only while matching would be refused or
/// not depending on which vectors happen to be stored, and not refused at all on
/// an empty index. This walks the WHOLE document up front instead, counting every
/// element, so one request is answered the same way whatever the index holds: an
/// unsupported operator, an operand of the wrong kind, a non-filterable metadata
/// key, a filter that is not an object, or a filter past [`MAX_FILTER_DEPTH`] /
/// [`MAX_FILTER_NODES`] is refused before the scan starts.
///
/// Walks with an explicit stack rather than recursively, so the check itself
/// costs no stack whatever the caller nests.
pub fn validate_filter(filter: &Document, non_filterable: &[String]) -> Result<(), LookupError> {
    let mut pending: Vec<(FilterNode<'_>, u32)> = vec![(FilterNode::Level(filter), 0)];
    let mut nodes: u32 = 0;
    while let Some((node, depth)) = pending.pop() {
        nodes += 1;
        if nodes > MAX_FILTER_NODES {
            return Err(too_wide());
        }
        if depth > MAX_FILTER_DEPTH {
            return Err(too_deep());
        }
        match node {
            FilterNode::Level(Document::Null) => {}
            FilterNode::Level(Document::Object(clauses)) => {
                for key in sorted_keys(clauses) {
                    match classify_clause(key, &clauses[key], non_filterable)? {
                        FilterClause::Branches { branches, .. } => pending.extend(
                            branches
                                .iter()
                                .map(|branch| (FilterNode::Level(branch), depth + 1)),
                        ),
                        FilterClause::Operators(operators) => pending.extend(
                            operators
                                .values()
                                .map(|operand| (FilterNode::Value(operand), depth + 1)),
                        ),
                        FilterClause::Literal(literal) => {
                            pending.push((FilterNode::Value(literal), depth + 1));
                        }
                    }
                }
            }
            FilterNode::Level(_) => {
                return Err(LookupError::invalid(
                    "filter",
                    "`filter` must be a metadata filter object",
                ))
            }
            FilterNode::Value(Document::Object(members)) => pending.extend(
                members
                    .values()
                    .map(|value| (FilterNode::Value(value), depth + 1)),
            ),
            FilterNode::Value(Document::Array(elements)) => pending.extend(
                elements
                    .iter()
                    .map(|value| (FilterNode::Value(value), depth + 1)),
            ),
            FilterNode::Value(_) => {}
        }
    }
    Ok(())
}

/// The refusal for a filter past [`MAX_FILTER_DEPTH`].
fn too_deep() -> LookupError {
    LookupError::invalid(
        "filter",
        format!("`filter` nests deeper than the {MAX_FILTER_DEPTH} levels this service evaluates"),
    )
}

/// The refusal for a filter past [`MAX_FILTER_NODES`].
fn too_wide() -> LookupError {
    LookupError::invalid(
        "filter",
        format!("`filter` has more than the {MAX_FILTER_NODES} elements this service evaluates"),
    )
}

/// Evaluate a QueryVectors metadata filter against one vector's metadata.
///
/// The filter is the document language S3 Vectors metadata filtering defines: an
/// object whose keys are either metadata keys or the logical operators `$and`
/// and `$or` (each taking an array of filters); several keys at one level are
/// ANDed. A metadata key maps either to a literal, which is an equality test, or
/// to an object of the comparison operators `$eq`, `$ne`, `$gt`, `$gte`, `$lt`,
/// `$lte`, `$in`, `$nin` and `$exists`. Any other `$`-prefixed key is refused
/// rather than ignored, so a filter this emulator cannot evaluate never silently
/// returns unfiltered matches.
///
/// A metadata key the vector does not carry is compared as `null`: it is not
/// equal to any literal, is in no `$in` list, orders against nothing, and
/// `$exists` reports it absent.
///
/// Call [`validate_filter`] ONCE before evaluating, which is where the size
/// bounds live. The `depth_left` counters below are the explicit recursion bound
/// of the code that recurses; a filter `validate_filter` admitted, over metadata
/// [`metadata_defect`] admitted, cannot reach either of them.
pub fn metadata_matches(
    filter: &Document,
    metadata: &Document,
    non_filterable: &[String],
) -> Result<bool, LookupError> {
    filter_matches(filter, metadata, non_filterable, MAX_FILTER_DEPTH)
}

/// One filter level. `depth_left` is how much deeper this walk may still go.
fn filter_matches(
    filter: &Document,
    metadata: &Document,
    non_filterable: &[String],
    depth_left: u32,
) -> Result<bool, LookupError> {
    if depth_left == 0 {
        return Err(too_deep());
    }
    let clauses = match filter {
        Document::Null => return Ok(true),
        Document::Object(clauses) => clauses,
        _ => {
            return Err(LookupError::invalid(
                "filter",
                "`filter` must be a metadata filter object",
            ))
        }
    };
    for key in sorted_keys(clauses) {
        let matched = match classify_clause(key, &clauses[key], non_filterable)? {
            FilterClause::Branches { branches, all } => {
                let mut matched = all;
                for branch in branches {
                    let branch_matched =
                        filter_matches(branch, metadata, non_filterable, depth_left - 1)?;
                    if all {
                        matched &= branch_matched;
                    } else {
                        matched |= branch_matched;
                    }
                }
                matched
            }
            FilterClause::Operators(operators) => {
                operator_matches(key, operators, metadata_value(metadata, key))?
            }
            FilterClause::Literal(literal) => {
                document_equals(metadata_value(metadata, key), literal, MAX_METADATA_DEPTH)
            }
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The value a metadata document holds for `key`, or `None` when it holds none.
fn metadata_value<'a>(metadata: &'a Document, key: &str) -> Option<&'a Document> {
    match metadata {
        Document::Object(members) => members.get(key),
        _ => None,
    }
}

/// One metadata key's operator object, against the value the vector carries.
/// Every operator in it must hold, so an operator object is itself an AND.
fn operator_matches(
    key: &str,
    operators: &std::collections::HashMap<String, Document>,
    actual: Option<&Document>,
) -> Result<bool, LookupError> {
    for name in sorted_keys(operators) {
        let operand = &operators[name];
        let matched = match FilterOperator::parse(name, key, operand)? {
            FilterOperator::Eq => document_equals(actual, operand, MAX_METADATA_DEPTH),
            FilterOperator::Ne => !document_equals(actual, operand, MAX_METADATA_DEPTH),
            FilterOperator::Gt => ordering_matches(actual, operand, |ord| ord.is_gt()),
            FilterOperator::Gte => ordering_matches(actual, operand, |ord| ord.is_ge()),
            FilterOperator::Lt => ordering_matches(actual, operand, |ord| ord.is_lt()),
            FilterOperator::Lte => ordering_matches(actual, operand, |ord| ord.is_le()),
            FilterOperator::In => in_list(actual, operand),
            FilterOperator::Nin => !in_list(actual, operand),
            FilterOperator::Exists => {
                let present = actual.is_some_and(|value| !matches!(value, Document::Null));
                // `parse` has already refused a non-boolean operand.
                present == matches!(operand, Document::Bool(true))
            }
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `$in` / `$nin`: whether the vector's value equals any element of the list.
/// The operand is an array — [`FilterOperator::parse`] refused anything else.
fn in_list(actual: Option<&Document>, operand: &Document) -> bool {
    let Document::Array(candidates) = operand else {
        return false;
    };
    candidates
        .iter()
        .any(|candidate| document_equals(actual, candidate, MAX_METADATA_DEPTH))
}

/// Equality between a vector's metadata value and a filter literal. A list-valued
/// metadata entry matches when any of its elements equals the literal, and two
/// numbers of different wire kinds (`1` and `1.0`) are equal when they are equal
/// by value.
///
/// `depth_left` bounds the recursion into a list-valued entry.
/// [`metadata_defect`] stores no document that deep, so the bound is the
/// explicit recursion limit of this function rather than a behaviour a caller
/// can observe.
fn document_equals(actual: Option<&Document>, expected: &Document, depth_left: u32) -> bool {
    let Some(actual) = actual else {
        return matches!(expected, Document::Null);
    };
    if depth_left == 0 {
        return false;
    }
    match (actual, expected) {
        // Same-kind numbers are compared exactly; only a cross-kind pair falls
        // back to the lossy `f64` view, so a large integer never loses precision
        // against another integer.
        (Document::Number(left), Document::Number(right)) => {
            left == right || number_as_f64(left) == number_as_f64(right)
        }
        (Document::Array(elements), expected) => elements
            .iter()
            .any(|element| document_equals(Some(element), expected, depth_left - 1)),
        (actual, expected) => actual == expected,
    }
}

/// `$gt` / `$gte` / `$lt` / `$lte`: compare numbers by value and strings
/// lexicographically. Any other pairing, including an absent metadata key,
/// orders against nothing and does not match.
fn ordering_matches(
    actual: Option<&Document>,
    operand: &Document,
    accept: impl Fn(std::cmp::Ordering) -> bool,
) -> bool {
    match (actual, operand) {
        (Some(Document::Number(left)), Document::Number(right)) => {
            match number_as_f64(left).partial_cmp(&number_as_f64(right)) {
                Some(ordering) => accept(ordering),
                None => false,
            }
        }
        (Some(Document::String(left)), Document::String(right)) => accept(left.as_str().cmp(right)),
        _ => false,
    }
}

/// A document number as `f64`, so the three wire number kinds compare by value.
fn number_as_f64(number: &Number) -> f64 {
    number.to_f64_lossy()
}

/// One page of a cursor-paged list over keys in their stored (sorted) order.
///
/// `after` is the `nextToken` the previous page returned, which is the last key
/// that page reported, so the next page starts at the first key after it.
/// Returns the page's keys and the `nextToken` to report, which is `None` when
/// the page reached the end of the list.
pub fn page_of<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    prefix: Option<&str>,
    after: Option<&str>,
    max_results: usize,
) -> (Vec<&'a str>, Option<String>) {
    let mut page: Vec<&'a str> = Vec::new();
    let mut more = false;
    for key in keys {
        if prefix.is_some_and(|prefix| !key.starts_with(prefix)) {
            continue;
        }
        if after.is_some_and(|after| key <= after) {
            continue;
        }
        if page.len() == max_results {
            more = true;
            break;
        }
        page.push(key);
    }
    let next_token = match (more, page.last()) {
        (true, Some(last)) => Some((*last).to_string()),
        _ => None,
    };
    (page, next_token)
}

/// The page size a list request asks for, bounded by [`DEFAULT_LIST_PAGE_SIZE`]
/// when it names none. A declared `maxResults` is already inside its modelled
/// range by the time a handler runs.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|max| usize::try_from(max).ok())
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_LIST_PAGE_SIZE)
}
