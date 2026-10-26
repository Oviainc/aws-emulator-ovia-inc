// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3tables-s3tablebuckets service.
//!
//! The S3 Tables control plane is, at this level, a store of **table buckets**.
//! Everything the handlers in `src/ops/` answer with is reconstructed from the
//! records below: nothing is echoed straight back out of a request and nothing
//! is canned.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// The region a table bucket's ARN carries when the request carries no
/// parseable SigV4 credential scope to name one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most table buckets this emulator keeps resident for one account.
///
/// CHOSEN, not an AWS-published figure: the S3 Tables table-bucket limit is an
/// adjustable account quota, so the emulator does not claim a number AWS
/// guarantees. What matters is that the store is BOUNDED — a caller cannot grow
/// the process's memory without limit by creating table buckets it never
/// deletes — and that the refusal is the client-input error this operation
/// models, `BadRequestException`.
pub const TABLE_BUCKETS_PER_ACCOUNT: usize = 1_000;

/// `TableBucketType` — the `type` member of a table bucket. Values from
/// `com.amazonaws.s3tables#TableBucketType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableBucketType {
    /// A bucket the caller created.
    Customer,
    /// A bucket Amazon Web Services created on the account's behalf.
    Aws,
}

impl TableBucketType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Customer, Self::Aws];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Aws => "aws",
        }
    }
}

/// `SSEAlgorithm` — the server-side encryption algorithm an encryption
/// configuration names. Values from `com.amazonaws.s3tables#SSEAlgorithm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseAlgorithm {
    /// S3-managed encryption keys.
    Aes256,
    /// Amazon Web Services KMS-managed encryption keys.
    AwsKms,
}

impl SseAlgorithm {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Aes256, Self::AwsKms];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Aes256 => "AES256",
            Self::AwsKms => "aws:kms",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }

    /// Whether this algorithm takes a KMS key: the model documents `kmsKeyArn`
    /// as "required only when `sseAlgorithm` is set to `aws:kms`".
    pub fn takes_kms_key(self) -> bool {
        matches!(self, Self::AwsKms)
    }
}

/// `StorageClass` — the storage class a storage-class configuration names.
/// Values from `com.amazonaws.s3tables#StorageClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    Standard,
    IntelligentTiering,
}

impl StorageClass {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Standard, Self::IntelligentTiering];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Standard => "STANDARD",
            Self::IntelligentTiering => "INTELLIGENT_TIERING",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// The stored form of the `EncryptionConfiguration` structure: the algorithm a
/// create named, and the KMS key when it named one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketEncryption {
    pub sse_algorithm: SseAlgorithm,
    pub kms_key_arn: Option<String>,
}

/// A table bucket, as this emulator stores it.
///
/// `encryption` and `storage_class` are `None` when the create named neither:
/// the model documents a service default for each, and which default that is
/// belongs to the operations that report them (`GetTableBucketEncryption`,
/// `GetTableBucketStorageClass`), so what is persisted here is what the caller
/// asked for and nothing invented on its behalf.
#[derive(Debug, Clone)]
pub struct TableBucket {
    pub name: String,
    pub arn: String,
    pub owner_account_id: String,
    pub bucket_id: String,
    pub bucket_type: TableBucketType,
    pub created_at: SystemTime,
    pub encryption: Option<BucketEncryption>,
    pub storage_class: Option<StorageClass>,
    pub tags: BTreeMap<String, String>,
}

/// Per-service state for com-amazonaws-s3tables-s3tablebuckets. A handler reaches it through [`service_state`].
///
/// `table_buckets` is keyed by the bucket's ARN — which carries its region and
/// its name — so a name is unique per region, as the service's naming rules
/// require, and a list operation has one stable order to page over.
#[derive(Debug, Default)]
pub struct ComAmazonawsS3tablesS3tablebucketsState {
    /// Every live table bucket, keyed by its `tableBucketARN`.
    pub table_buckets: BTreeMap<String, TableBucket>,
    /// Counts the table buckets this store has minted, so two ids minted inside
    /// one clock tick still differ. Never exposed on the wire.
    pub minted_buckets: u64,
}

impl ComAmazonawsS3tablesS3tablebucketsState {
    /// A fresh `tableBucketId`: 32 lowercase hex characters.
    ///
    /// The id is a digest of the instant the bucket is being created and this
    /// store's mint counter, so it is server-generated and opaque: it carries
    /// nothing from the request, two creates never collide (the counter
    /// separates them inside one clock tick), and two emulator runs do not
    /// repeat each other's ids. Retried against the store so an id is never
    /// reused.
    pub fn mint_bucket_id(&mut self) -> String {
        use ::sha2::{Digest, Sha256};
        loop {
            self.minted_buckets = self.minted_buckets.wrapping_add(1);
            let since_epoch = SystemTime::now()
                .duration_since(::std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let digest = Sha256::digest(
                format!("s3tables-table-bucket:{since_epoch}:{}", self.minted_buckets).as_bytes(),
            );
            let candidate = ::hex::encode(&digest[..16]);
            if !self
                .table_buckets
                .values()
                .any(|bucket| bucket.bucket_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// Whether the store already holds [`TABLE_BUCKETS_PER_ACCOUNT`] table
    /// buckets, so a further create must be refused.
    pub fn is_at_bucket_quota(&self) -> bool {
        self.table_buckets.len() >= TABLE_BUCKETS_PER_ACCOUNT
    }
}

/// The region this request is for: the one its SigV4 credential scope names,
/// else [`DEFAULT_REGION`] when the request carries no parseable scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `target` every audit record this service emits carries, so a consumer
/// can select the access trail without reading the rest of the emulator's
/// traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the
/// operation, the caller's presented access key id, the resource it changed and
/// the outcome. Metadata only — never a request body, a tag value or a secret.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in
/// its SigV4 credential scope. `emulator_server::extract_akid` answers
/// `"default"` when the request carries no scope. An access key id NAMES a
/// caller and is not a secret — the signature and the secret key are never
/// read here.
pub fn request_caller(parts: &::http::request::Parts) -> String {
    ::emulator_server::extract_akid(&parts.headers)
}

/// The ARN of a table bucket: `arn:aws:s3tables:<region>:<account>:bucket/<name>`.
pub fn table_bucket_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("s3tables", region, &format!("bucket/{name}"))
}

/// Whether `name` satisfies the table-bucket naming rule the model's
/// `@pattern` cannot express: a name begins and ends with a letter or a digit,
/// so a hyphen may only appear between them.
///
/// The modeled pattern (`^[0-9a-z-]*$`) and length (3..63) are already enforced
/// at the boundary; this is the remaining rule, restated structurally rather
/// than as a second regex.
pub fn name_begins_and_ends_alphanumeric(name: &str) -> bool {
    let alphanumeric = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    name.chars().next().is_some_and(alphanumeric)
        && name.chars().next_back().is_some_and(alphanumeric)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3tablesS3tablebucketsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3tablesS3tablebucketsState>)
}
