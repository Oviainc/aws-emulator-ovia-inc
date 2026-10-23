// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3vectors-s3vectors service.
//!
//! Holds the vector bucket -> vector index -> vector hierarchy every handler in
//! `src/ops/` reads and writes, together with the lookup, validation, distance
//! and metadata-filter helpers they share. `src/lib.rs` is generated and
//! `src/ops/mod.rs` is an `include!` of generated code, so this module is the
//! one place a helper several operations need can live.

use std::collections::BTreeMap;
use std::time::SystemTime;

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

/// `DistanceMetric` values, from the model's enum (`cosine`, `euclidean`).
pub const DISTANCE_METRIC_COSINE: &str = "cosine";
/// See [`DISTANCE_METRIC_COSINE`].
pub const DISTANCE_METRIC_EUCLIDEAN: &str = "euclidean";

/// Page size a list operation uses when the caller names no `maxResults`: the
/// top of the member's own modelled range (`ListVectorBuckets$maxResults` and
/// `ListIndexes$maxResults` are `range 1..500`), so a caller that asks for no
/// page size gets as much as one page can hold.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 500;

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
/// See [`MAX_VECTOR_BUCKETS`]: the standard AWS limit of 50 tags per resource.
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
}

/// A stored vector bucket, with the vector indexes it contains.
#[derive(Debug, Clone)]
pub struct VectorBucketRecord {
    pub name: String,
    /// Minted once, at create, and reported unchanged by every later read.
    pub arn: String,
    pub creation_time: SystemTime,
    pub encryption: EncryptionRecord,
    /// `None` until PutVectorBucketDefaultIndexMode sets one; a new index
    /// inherits it.
    pub default_index_mode: Option<String>,
    /// The bucket policy document, exactly as PutVectorBucketPolicy was given
    /// it. `None` until a policy is put, and `None` again once
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
    pub index_mode: Option<String>,
    /// Vectors by key. A `BTreeMap` so ListVectors order, its `nextToken`
    /// cursor and its segment partition are deterministic.
    pub vectors: BTreeMap<String, StoredVector>,
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
        /// The input member the failure is about, for `fieldList`.
        path: Option<String>,
    },
}

impl LookupError {
    /// A refusal about `path`, an input member path.
    pub fn invalid(path: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
            path: Some(path.to_string()),
        }
    }

    /// A refusal that is not about one member.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
            path: None,
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

    /// The modelled 400 payload, naming the offending member in `fieldList`.
    pub fn validation_exception(&self) -> ValidationException {
        match self {
            Self::NotFound(message) => ValidationException {
                message: message.clone(),
                field_list: None,
            },
            Self::Invalid { message, path } => ValidationException {
                message: message.clone(),
                field_list: path.as_ref().map(|path| {
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
/// kind from the sentence, so the two kinds say different things. This one is the
/// sentence the real service answered in the recorded response; the status (404) and
/// the error code (`NotFoundException`) are the requirements either way.
pub const BUCKET_NOT_FOUND: &str = "The specified vector bucket could not be found";

/// See [`BUCKET_NOT_FOUND`]: the same sentence for the other resource kind, by
/// analogy with it — no recording of this corpus carries the index wording.
pub const INDEX_NOT_FOUND: &str = "The specified vector index could not be found";

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

/// The bucket name form the model's `VectorBucketArn` / `IndexArn` patterns
/// encode for both a bucket and an index segment:
/// `[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]` — 3 to 63 characters, lowercase
/// alphanumerics, dots and hyphens, starting and ending alphanumeric.
pub fn validate_resource_name(member: &str, value: &str) -> Result<(), LookupError> {
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
    Err(LookupError::invalid(
        member,
        format!(
            "`{member}` must be 3 to 63 characters of lowercase letters, numbers, \
             hyphens and dots, and must begin and end with a letter or a number: {value}"
        ),
    ))
}

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
/// Only the grammar is checked here. Whether the principals, actions and
/// resources a well-formed statement names exist is not a grammar question, and
/// a policy may legitimately name a principal or a resource in another account.
pub fn validate_bucket_policy(member: &str, policy: &str) -> Result<(), LookupError> {
    let refuse = |detail: String| {
        Err(LookupError::invalid(
            member,
            format!("`{member}` is not a valid policy document: {detail}"),
        ))
    };

    if policy.len() > MAX_POLICY_BYTES {
        return refuse(format!(
            "a policy document is at most {MAX_POLICY_BYTES} bytes, and this one is {} bytes",
            policy.len()
        ));
    }

    let document: serde_json::Value = match serde_json::from_str(policy) {
        Ok(document) => document,
        Err(error) => return refuse(format!("it is not well-formed JSON ({error})")),
    };
    let Some(object) = document.as_object() else {
        return refuse("a policy document is a JSON object".to_string());
    };
    for key in ["Version", "Id"] {
        if object.get(key).is_some_and(|value| !value.is_string()) {
            return refuse(format!("`{key}` is a string"));
        }
    }
    let Some(statement) = object.get("Statement") else {
        return refuse("a policy document has a `Statement`".to_string());
    };
    let statements: Vec<&serde_json::Value> = match statement {
        serde_json::Value::Array(statements) => statements.iter().collect(),
        other => vec![other],
    };
    if statements.is_empty() {
        return refuse("`Statement` names at least one statement".to_string());
    }
    for (position, statement) in statements.iter().enumerate() {
        let Some(statement) = statement.as_object() else {
            return refuse(format!("statement {position} is a JSON object"));
        };
        match statement.get("Effect").and_then(serde_json::Value::as_str) {
            Some(POLICY_EFFECT_ALLOW) | Some(POLICY_EFFECT_DENY) => {}
            other => {
                return refuse(format!(
                    "statement {position}'s `Effect` is `{POLICY_EFFECT_ALLOW}` or \
                     `{POLICY_EFFECT_DENY}`, not {}",
                    match other {
                        Some(effect) => format!("`{effect}`"),
                        None => "absent".to_string(),
                    }
                ));
            }
        }
        if !statement.contains_key("Action") && !statement.contains_key("NotAction") {
            return refuse(format!(
                "statement {position} names an `Action` or a `NotAction`"
            ));
        }
    }
    Ok(())
}

/// Resolve the bucket a request names by `vectorBucketName` or `vectorBucketArn`.
///
/// The operations that take both document "you must use either the vector bucket
/// name or the vector bucket ARN", so naming neither is refused, and naming both
/// is accepted only when they agree.
pub fn resolve_bucket_name(
    name: Option<&str>,
    arn: Option<&str>,
) -> Result<String, LookupError> {
    let from_arn = match arn {
        Some(arn) => Some(bucket_from_arn("vectorBucketArn", arn)?),
        None => None,
    };
    match (name, from_arn) {
        (Some(name), None) => Ok(name.to_string()),
        (None, Some(from_arn)) => Ok(from_arn),
        (Some(name), Some(from_arn)) if name == from_arn => Ok(from_arn),
        (Some(name), Some(from_arn)) => Err(LookupError::invalid_request(format!(
            "`vectorBucketName` ({name}) and `vectorBucketArn` (bucket {from_arn}) \
             name different vector buckets"
        ))),
        (None, None) => Err(LookupError::invalid_request(
            "either `vectorBucketName` or `vectorBucketArn` is required",
        )),
    }
}

/// Resolve the index a request names by `vectorBucketName` + `indexName`, or by
/// `indexArn`.
///
/// The operations that take both document "you can either use both the vector
/// bucket name and the vector index name, or use the vector index ARN", so a
/// request must carry one of the two forms, and a request carrying both is
/// accepted only when they agree.
pub fn resolve_index_name(
    bucket_name: Option<&str>,
    index_name: Option<&str>,
    index_arn: Option<&str>,
) -> Result<(String, String), LookupError> {
    let from_arn = match index_arn {
        Some(arn) => Some(index_from_arn("indexArn", arn)?),
        None => None,
    };
    match (bucket_name, index_name, from_arn) {
        (Some(bucket), Some(index), None) => Ok((bucket.to_string(), index.to_string())),
        (None, None, Some(from_arn)) => Ok(from_arn),
        (bucket, index, Some((arn_bucket, arn_index))) => {
            let bucket_ok = bucket.is_none_or(|bucket| bucket == arn_bucket);
            let index_ok = index.is_none_or(|index| index == arn_index);
            if bucket_ok && index_ok {
                Ok((arn_bucket, arn_index))
            } else {
                Err(LookupError::invalid_request(format!(
                    "`indexArn` names index {arn_index} in vector bucket {arn_bucket}, \
                     which is not the index `vectorBucketName`/`indexName` name"
                )))
            }
        }
        (_, _, None) => Err(LookupError::invalid_request(
            "either both `vectorBucketName` and `indexName`, or `indexArn`, is required",
        )),
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
    pub fn index(&self, bucket: &str, index: &str) -> Result<&VectorIndexRecord, LookupError> {
        self.bucket(bucket)?
            .indexes
            .get(index)
            .ok_or_else(|| LookupError::not_found(INDEX_NOT_FOUND))
    }

    /// The index `index` of bucket `bucket` for mutation, or `NotFoundException`.
    pub fn index_mut(
        &mut self,
        bucket: &str,
        index: &str,
    ) -> Result<&mut VectorIndexRecord, LookupError> {
        self.bucket_mut(bucket)?
            .indexes
            .get_mut(index)
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
    if requested.kms_key_arn.is_some() && sse_type != KMS_SSE_TYPE {
        return Err(LookupError::invalid(
            "encryptionConfiguration.kmsKeyArn",
            format!(
                "`kmsKeyArn` is allowed if and only if `sseType` is `{KMS_SSE_TYPE}`, not `{sse_type}`"
            ),
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
pub fn bucket_view(record: &VectorBucketRecord) -> VectorBucket {
    VectorBucket {
        vector_bucket_name: record.name.clone(),
        vector_bucket_arn: record.arn.clone(),
        creation_time: ::aws_smithy_types::DateTime::from(record.creation_time),
        encryption_configuration: Some(encryption_view(&record.encryption)),
        default_index_mode: record.default_index_mode.clone(),
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
        index_mode: record.index_mode.clone(),
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
            // `validate_vector_data` refuses as zero vectors under `cosine`.
            return 1.0;
        }
        1.0 - dot / (left_norm * right_norm)
    }
}

/// Check one vector's `float32` data against the index it is being written to or
/// queried with.
///
/// `PutInputVector$data` documents all three rules: "vector dimensions must
/// match the dimension count that's configured for the vector index"; "for the
/// cosine distance metric, zero vectors (vectors containing all zeros) aren't
/// allowed"; and "for both cosine and euclidean distance metrics, vector data
/// must contain only valid floating-point values. Invalid values such as NaN
/// (Not a Number) or Infinity aren't allowed".
pub fn validate_vector_data(
    member: &str,
    index: &VectorIndexRecord,
    data: &[f32],
) -> Result<(), LookupError> {
    if data.len() != usize::try_from(index.dimension.max(0)).unwrap_or(0) {
        return Err(LookupError::invalid(
            member,
            format!(
                "`{member}` has {} values, but vector index {} is configured for {} dimensions",
                data.len(),
                index.name,
                index.dimension
            ),
        ));
    }
    if data.iter().any(|value| !value.is_finite()) {
        return Err(LookupError::invalid(
            member,
            format!("`{member}` must contain only finite values: NaN and Infinity aren't allowed"),
        ));
    }
    if index.distance_metric == DISTANCE_METRIC_COSINE && data.iter().all(|value| *value == 0.0) {
        return Err(LookupError::invalid(
            member,
            format!(
                "`{member}` is a zero vector, which isn't allowed for the \
                 `{DISTANCE_METRIC_COSINE}` distance metric"
            ),
        ));
    }
    Ok(())
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

/// Refuse a vector's `metadata` document that holds more elements than
/// [`MAX_METADATA_NODES`] or nests deeper than [`MAX_METADATA_DEPTH`], so one
/// request cannot store an unbounded document — and so the equality walk a later
/// filter runs over the stored document has a known bound.
///
/// Walks the document with an explicit stack rather than recursively, so the
/// walk itself costs no stack whatever the caller nests.
pub fn validate_metadata(member: &str, metadata: &Document) -> Result<(), LookupError> {
    let mut pending: Vec<(&Document, u32)> = vec![(metadata, 0)];
    let mut nodes: usize = 0;
    while let Some((node, depth)) = pending.pop() {
        nodes += 1;
        if nodes > MAX_METADATA_NODES {
            return Err(LookupError::invalid(
                member,
                format!(
                    "`{member}` holds more than the {MAX_METADATA_NODES} elements this \
                     service stores for one vector"
                ),
            ));
        }
        if depth > MAX_METADATA_DEPTH {
            return Err(LookupError::invalid(
                member,
                format!(
                    "`{member}` nests deeper than the {MAX_METADATA_DEPTH} levels this \
                     service stores for one vector"
                ),
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
    Ok(())
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
/// [`validate_metadata`] admitted, cannot reach either of them.
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
/// [`validate_metadata`] stores no document that deep, so the bound is the
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
