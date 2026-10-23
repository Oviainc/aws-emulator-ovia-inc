// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3-amazons3 service.
//!
//! The bucket map is the only place a bucket or an object exists: CreateBucket /
//! HeadBucket answer from a bucket's own record, and the object-storage loop
//! (PutObject / GetObject / ListObjectsV2 / DeleteObject / DeleteBucket) computes
//! every response from the objects stored inside it.
//!
//! A bucket whose versioning status is `Enabled` holds more than one version of a key:
//! the CURRENT one, which every reader that names no `versionId` answers with, and the
//! noncurrent ones a later write or a delete displaced, each addressable by the version id
//! the store minted for it ([`mint_version_id`]). Every stored version counts against the
//! same ceilings a current object does.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use md5::{Digest, Md5};

/// The Region a general purpose bucket is created in when the request names no
/// `CreateBucketConfiguration.LocationConstraint`.
///
/// From that member's own documentation: "If you don't specify a Region, the bucket is
/// created in the US East (N. Virginia) Region (us-east-1) by default."
pub const DEFAULT_BUCKET_REGION: &str = "us-east-1";

/// The Region the legacy `LocationConstraint` value `EU` names.
///
/// Same member documentation: "Configurations using the value `EU` will create a bucket
/// in `eu-west-1`."
pub const EU_LOCATION_CONSTRAINT_REGION: &str = "eu-west-1";

/// The legacy `LocationConstraint` value that names [`EU_LOCATION_CONSTRAINT_REGION`]
/// rather than a Region code (it is one of `BucketLocationConstraint`'s enum values).
const EU_LOCATION_CONSTRAINT: &str = "EU";

/// How many general purpose buckets one account may hold, and so how many entries this
/// service's bucket map can ever carry.
///
/// AWS publishes this as a service quota — 10,000 general purpose buckets per account by
/// default — and the model corroborates the same ceiling: `ListBucketsRequest$MaxBuckets`
/// is bounded `range {min 1, max 10000}`
/// (`smithy_query(query="operation",operation="ListBuckets")`), i.e. one page may name
/// every bucket an account is allowed to hold. A create past it is refused rather than
/// recorded, so the store this emulator keeps is bounded whatever a caller does.
pub const MAX_BUCKETS_PER_ACCOUNT: usize = 10_000;

/// What [`ComAmazonawsS3Amazons3State::create_bucket`] did, so the caller answers each
/// outcome without having to re-derive it — and cannot reach the stored-bucket cases
/// without having gone through the quota check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketCreation {
    /// The bucket was recorded.
    Created,
    /// A bucket of that name already exists; NOTHING was written, so the existing bucket
    /// keeps the Region it was created in. What to answer differs by that Region, so this
    /// reports the outcome rather than deciding it.
    NameTaken,
    /// The account already holds [`MAX_BUCKETS_PER_ACCOUNT`] buckets; nothing was written.
    QuotaReached,
}

/// The `StorageClass` an object gets when the `PutObject` request names none.
///
/// From `PutObjectRequest$StorageClass`'s own documentation: "By default, Amazon S3 uses
/// the STANDARD Storage Class to store newly created objects."
pub const DEFAULT_STORAGE_CLASS: &str = "STANDARD";

/// The longest object key this emulator records, in UTF-8 bytes.
///
/// The published S3 limit: "The name for a key is a sequence of Unicode characters whose
/// UTF-8 encoding is at most 1,024 bytes long" (Amazon S3 User Guide, "Creating object key
/// names"). The model constrains only the other end — `ObjectKey` is `length {min 1}`
/// (`smithy_query(query="shape",shape="ObjectKey")`) — so nothing refuses a longer key
/// before this check, and a key is allocated per stored object.
pub const MAX_OBJECT_KEY_BYTES: usize = 1_024;

/// How many objects one bucket in this emulator may hold.
///
/// NOT a fidelity claim: S3 publishes no such limit ("you can store any number of objects
/// in a bucket"). It is the emulator's own ceiling, because every object it records lives
/// in this process's memory — set generously so no realistic workflow meets it, and
/// enforced so a caller looping on `PutObject` with fresh keys cannot grow the store
/// without bound. A write past it is refused rather than recorded, the way a create past
/// [`MAX_BUCKETS_PER_ACCOUNT`] is.
pub const MAX_OBJECTS_PER_BUCKET: usize = 1_000;

/// How many bytes of object payload one bucket in this emulator may hold in total.
///
/// Same reason as [`MAX_OBJECTS_PER_BUCKET`], for the other dimension: an object's bytes
/// are kept verbatim, so a few large writes exhaust memory as surely as many small ones.
/// Also not a fidelity claim — AWS caps one `PutObject` payload at 5 GiB and a bucket not
/// at all; this is the emulator's own budget, and a write that would take a bucket past it
/// is refused rather than recorded.
pub const MAX_OBJECT_BYTES_PER_BUCKET: usize = 16 * 1024 * 1024;

/// How many objects this emulator will hold ACROSS every bucket.
///
/// The per-bucket ceilings alone would leave total memory bounded only by their product
/// with [`MAX_BUCKETS_PER_ACCOUNT`], so the service-wide ones below are what actually
/// bound this process: whatever a caller does — one bucket or ten thousand — the object
/// store holds at most this many objects and [`MAX_OBJECT_BYTES_TOTAL`] bytes of payload.
pub const MAX_OBJECTS_TOTAL: usize = 10_000;

/// How many bytes of object payload this emulator will hold across every bucket.
///
/// Same reason as [`MAX_OBJECTS_TOTAL`], and the same order as the service-wide retained
/// budgets of this workspace's peer services (appconfig's
/// `MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`, codeguruprofiler's `MAX_RETAINED_PROFILE_BYTES`).
/// A write that would take the whole store past it is refused rather than recorded.
pub const MAX_OBJECT_BYTES_TOTAL: usize = 64 * 1024 * 1024;

/// The version id an object written while the bucket was NOT versioning-enabled is
/// addressed by.
///
/// "Objects stored in your bucket before you set the versioning state have a version ID
/// of null", and `PutBucketVersioning`'s own documentation says the same of a suspended
/// bucket: "Suspended—Disables versioning for the objects in the bucket. All objects
/// added to the bucket receive the version ID null." Such a version is stored with no
/// minted id at all ([`Object::version_id`] is `None`), so this is the string a request
/// names it by rather than a value this store keeps.
pub const NULL_VERSION_ID: &str = "null";

/// The `BucketVersioningStatus` value that makes a bucket version its objects.
///
/// "Enabled—Enables versioning for the objects in the bucket. All objects added to the
/// bucket receive a unique version ID" (`PutBucketVersioning`'s documentation).
pub const VERSIONING_ENABLED_STATUS: &str = "Enabled";

/// The most user-defined metadata one stored object may carry, in UTF-8 bytes summed
/// over every `x-amz-meta-` key and value.
///
/// The published S3 limit: "Within the PUT request header, the user-defined metadata is
/// limited to 2 KB in size" (Amazon S3 User Guide, "Working with object metadata"). The
/// model constrains `Metadata` not at all, so nothing refuses more before this check, and
/// every byte of it is retained with the object.
pub const MAX_OBJECT_METADATA_BYTES: usize = 2 * 1024;

/// What [`ComAmazonawsS3Amazons3State::put_object`] did, so the caller answers each
/// outcome without re-deriving it — and cannot record an object without having gone
/// through the key, count and byte-budget checks, per bucket AND service-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectWrite {
    /// The object was recorded at a key the bucket did not hold.
    Created,
    /// The object replaced the one the bucket held at that key.
    Replaced,
    /// The key is longer than [`MAX_OBJECT_KEY_BYTES`]; nothing was written.
    KeyTooLong { bytes: usize },
    /// The request's user-defined metadata is larger than
    /// [`MAX_OBJECT_METADATA_BYTES`]; nothing was written.
    MetadataTooLarge { bytes: usize },
    /// The bucket already holds [`MAX_OBJECTS_PER_BUCKET`] objects at other keys; nothing
    /// was written.
    ObjectCountReached,
    /// Recording the object would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// This emulator already holds [`MAX_OBJECTS_TOTAL`] objects across every bucket;
    /// nothing was written.
    TotalObjectCountReached,
    /// Recording the object would take the whole store past
    /// [`MAX_OBJECT_BYTES_TOTAL`] retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_tags`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record a tag set without having gone
/// through the byte budgets a stored object's tags count against, per bucket AND
/// service-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectTagWrite {
    /// The object's tag set is now the one that was handed in.
    Applied,
    /// The bucket exists but holds no object at that key; nothing was written.
    NoSuchKey,
    /// Keeping the tag set would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the tag set would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// How many tags one bucket's tag set may carry.
///
/// The Amazon S3 User Guide's bucket tagging restrictions ("Using cost allocation S3
/// bucket tags", the page `PutBucketTagging`'s own `InvalidTag` documentation points at)
/// publish three limits on a bucket tag set: at most 50 tags per bucket, a key of at most
/// 128 Unicode characters and a value of at most 256. A tag set past any of them is
/// refused rather than stored, which is what bounds what one bucket entry can hold.
pub const MAX_TAGS_PER_BUCKET: usize = 50;

/// How many tags one OBJECT's tag set may carry.
///
/// `PutObjectTagging`'s own documentation publishes this one: "Note that Amazon S3 limits
/// the maximum number of tags to 10 tags per object." The character bounds below are the
/// same for both tag sets (the "Tag Restrictions" page that operation points at), so the
/// count is the only published limit that differs between a bucket's tag set and an
/// object's.
pub const MAX_TAGS_PER_OBJECT: usize = 10;

/// The longest tag key a tag set may carry, in Unicode characters.
///
/// Same source as [`MAX_TAGS_PER_BUCKET`], and the same bound for an object's tag set. The
/// model constrains the other end of the key — `Tag$Key` targets `ObjectKey`,
/// `length {min 1}` (`smithy_query(query="shape",shape="Tag")`) — so an empty key is
/// already refused at the boundary, before any handler runs.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value a tag set may carry, in Unicode characters.
///
/// Same source as [`MAX_TAG_KEY_CHARS`]. `Tag$Value` targets the unconstrained `Value`
/// string, so nothing refuses an over-long value before this check.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// One tag on a resource this emulator holds — a bucket's tag set or an object's: the
/// key/value pair `Tag` carries on the wire.
///
/// ONE type for both, because a tag is the same thing in both places (`Tagging$TagSet` is
/// the payload of `PutBucketTagging` and of `PutObjectTagging` alike). Held separately
/// from the modelled `Tag` shape so this store owns no wire type, the way [`Bucket`]
/// holds the Region as a plain `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceTag {
    /// The tag's key — unique within one resource's tag set.
    pub key: String,
    /// The tag's value.
    pub value: String,
}

/// How many bytes a tag set retains: every key and value, in UTF-8 bytes.
///
/// What the object byte budgets count a tag set as — see [`Object::retained_bytes`].
pub fn tag_set_bytes(tags: &[ResourceTag]) -> usize {
    tags.iter()
        .map(|tag| tag.key.len() + tag.value.len())
        .sum()
}

/// The published limits one resource's tag set is judged against.
///
/// The character bounds are the same for every tag set, so what a scope carries is the tag
/// COUNT its resource publishes plus the noun a refusal names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagSetLimits {
    /// The most tags this resource's tag set may carry.
    pub max_tags: usize,
    /// What the resource is called in a refusal ("bucket", "object").
    pub resource: &'static str,
}

/// The limits a BUCKET's tag set is judged against ([`MAX_TAGS_PER_BUCKET`]).
pub const BUCKET_TAG_SET_LIMITS: TagSetLimits = TagSetLimits {
    max_tags: MAX_TAGS_PER_BUCKET,
    resource: "bucket",
};

/// The limits an OBJECT's tag set is judged against ([`MAX_TAGS_PER_OBJECT`]).
pub const OBJECT_TAG_SET_LIMITS: TagSetLimits = TagSetLimits {
    max_tags: MAX_TAGS_PER_OBJECT,
    resource: "object",
};

/// Which published restriction a tag set breaks.
///
/// Each variant is one of the published restrictions on a tag set, or the key-uniqueness
/// the set itself has. AWS answers every one of them with `InvalidTag`, which this
/// service's model does not declare as a shape
/// (`smithy_query(query="operation",operation="PutBucketTagging")` and the same query for
/// `PutObjectTagging` both report `errors: []`), so the caller renders it through the
/// framework's refusal channel — never as a success and never as a 501.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagSetRejectionCause {
    /// The tag set carries more tags than its resource's [`TagSetLimits::max_tags`].
    TooManyTags { count: usize },
    /// A tag carries an EMPTY key. `Tag$Key` targets `ObjectKey`, `length {min 1}`
    /// (`smithy_query(query="shape",shape="Tag")`), so the modelled `Tagging` container
    /// can never carry one — but a tag set that arrives as the URL-encoded
    /// `x-amz-tagging` header is not a modelled `Tag` list, so this is where such a pair
    /// is caught (see [`tag_set_from_header`]).
    EmptyKey,
    /// A key is longer than [`MAX_TAG_KEY_CHARS`] Unicode characters.
    KeyTooLong { chars: usize },
    /// A value is longer than [`MAX_TAG_VALUE_CHARS`] Unicode characters.
    ValueTooLong { key: String, chars: usize },
    /// Two tags in one set name the same key, so the set is not a key/value mapping.
    DuplicateKey { key: String },
}

/// Why a tag set cannot be stored on a resource: the restriction it breaks, and the
/// limits — so the refusal names the resource and the bound it was judged against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSetRejection {
    /// The limits the set was judged against.
    pub limits: TagSetLimits,
    /// The restriction it breaks.
    pub cause: TagSetRejectionCause,
}

impl std::fmt::Display for TagSetRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let TagSetLimits { max_tags, resource } = self.limits;
        match &self.cause {
            TagSetRejectionCause::TooManyTags { count } => write!(
                f,
                "Tagging.TagSet carries {count} tags, more than the {max_tags} \
                 tags one {resource} may hold"
            ),
            TagSetRejectionCause::EmptyKey => write!(
                f,
                "Tagging.TagSet has a Tag with an empty Tag.Key; a {resource} tag key \
                 holds at least one character"
            ),
            TagSetRejectionCause::KeyTooLong { chars } => write!(
                f,
                "Tagging.TagSet has a Tag.Key of {chars} characters, more than the \
                 {MAX_TAG_KEY_CHARS} characters a tag key may hold"
            ),
            TagSetRejectionCause::ValueTooLong { key, chars } => write!(
                f,
                "Tagging.TagSet has a Tag.Value of {chars} characters for key {key:?}, \
                 more than the {MAX_TAG_VALUE_CHARS} characters a tag value may hold"
            ),
            TagSetRejectionCause::DuplicateKey { key } => write!(
                f,
                "Tagging.TagSet names the key {key:?} twice; a {resource} tag set holds \
                 one value per key"
            ),
        }
    }
}

/// Whether `tags` may be stored as the tag set of a resource with these `limits`,
/// reporting the first restriction it breaks.
///
/// ONE rule for every tag set S3 holds: the count against the resource's own published
/// maximum, then each key and value against the character bounds every tag set shares, and
/// the key-uniqueness the set itself has. The count is checked before the per-tag rules so
/// an enormous set is refused for what it is rather than for whichever of its tags happens
/// to come first.
pub fn validate_tag_set(
    tags: &[ResourceTag],
    limits: TagSetLimits,
) -> Result<(), TagSetRejection> {
    let refuse = |cause| Err(TagSetRejection { limits, cause });
    if tags.len() > limits.max_tags {
        return refuse(TagSetRejectionCause::TooManyTags { count: tags.len() });
    }
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for tag in tags {
        if tag.key.is_empty() {
            return refuse(TagSetRejectionCause::EmptyKey);
        }
        let key_chars = tag.key.chars().count();
        if key_chars > MAX_TAG_KEY_CHARS {
            return refuse(TagSetRejectionCause::KeyTooLong { chars: key_chars });
        }
        let value_chars = tag.value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return refuse(TagSetRejectionCause::ValueTooLong {
                key: tag.key.clone(),
                chars: value_chars,
            });
        }
        if !seen.insert(tag.key.as_str()) {
            return refuse(TagSetRejectionCause::DuplicateKey {
                key: tag.key.clone(),
            });
        }
    }
    Ok(())
}

/// The tag set a write's `x-amz-tagging` header names, or which published restriction it
/// breaks — the ONE seam a handler uses, so no handler can store a header-delivered tag
/// set the limits would have refused.
///
/// "The tag-set for the object. The tag-set must be encoded as URL Query parameters. (For
/// example, "Key1=Value1")" — `PutObjectRequest$Tagging`, and the same sentence on
/// `CopyObjectRequest$Tagging` and `CreateMultipartUploadRequest$Tagging`. So the header
/// is an `application/x-www-form-urlencoded` string, read with the runtime's own parser
/// for that grammar (`aws_smithy_schema_server::query::parse_form_urlencoded`, which the
/// query-protocol services already read their bodies with): each `&`-separated pair is
/// one tag, percent escapes and `+` are decoded, and a pair with no `=` is a key with an
/// empty value. An EMPTY header carries no pairs and so is an empty tag set — the same
/// state as no tag set at all.
///
/// The PAIR COUNT is judged BEFORE the set is materialised, by counting the `&`-separated
/// pairs the same grammar defines — which allocates nothing — so a header naming more
/// pairs than the resource may hold is refused for exactly that without this process ever
/// holding a tag set it would never store. What is then materialised is at most
/// `limits.max_tags` tags, each judged by [`validate_tag_set`] like any other tag set.
pub fn tag_set_from_header(
    header: &str,
    limits: TagSetLimits,
) -> Result<Vec<ResourceTag>, TagSetRejection> {
    let pairs = header.split('&').filter(|pair| !pair.is_empty()).count();
    if pairs > limits.max_tags {
        return Err(TagSetRejection {
            limits,
            cause: TagSetRejectionCause::TooManyTags { count: pairs },
        });
    }
    let tags: Vec<ResourceTag> =
        ::aws_smithy_schema_server::query::parse_form_urlencoded(header.as_bytes())
            .into_iter()
            .map(|(key, value)| ResourceTag { key, value })
            .collect();
    validate_tag_set(&tags, limits)?;
    Ok(tags)
}

/// The versioning state a bucket carries once `PutBucketVersioning` has set one.
///
/// A bucket that has never had the subresource set carries NO value of this at all —
/// "If the versioning state has never been set on a bucket, it has no versioning state;
/// a GetBucketVersioning request does not return a versioning state value"
/// (`PutBucketVersioning`'s own documentation) — which is why [`Bucket::versioning`] is
/// an `Option` of it rather than a status defaulting to anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketVersioning {
    /// A `BucketVersioningStatus` value: `Enabled` or `Suspended`
    /// (`BUCKET_VERSIONING_STATUS_VALUES`), exactly as the configuration carried it.
    pub status: String,
    /// An `MFADeleteStatus` value, or `None` while the bucket has never been configured
    /// with MFA delete: "This element is only returned if the bucket has been configured
    /// with MFA delete. If the bucket has never been so configured, this element is not
    /// returned" (`GetBucketVersioningOutput$MFADelete`).
    pub mfa_delete: Option<String>,
}

/// The Block Public Access settings a bucket carries once `PutPublicAccessBlock` has set
/// a `PublicAccessBlockConfiguration` on it.
///
/// Four INDEPENDENT switches, not an `Option` each: every member of
/// `PublicAccessBlockConfiguration` is a `Setting` (a boolean) documented as "Specifies
/// whether Amazon S3 should block …  Setting this element to TRUE causes …"
/// (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`), so a switch the
/// configuration does not turn on is off — there is no third state for a member to be in.
/// A bucket that has never had the subresource set carries NO value of this at all, which
/// is why [`Bucket::public_access_block`] is an `Option` of it: the same distinction
/// [`BucketVersioning`] keeps between "never configured" and "configured off".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BucketPublicAccessBlock {
    /// Whether S3 blocks public ACLs for the bucket and its objects.
    pub block_public_acls: bool,
    /// Whether S3 ignores public ACLs on the bucket and its objects.
    pub ignore_public_acls: bool,
    /// Whether S3 blocks public bucket policies for the bucket.
    pub block_public_policy: bool,
    /// Whether S3 restricts access to the bucket when it has a public policy.
    pub restrict_public_buckets: bool,
}

/// Every value the `Event` enum publishes: the bucket events a notification configuration
/// may ask to be notified about, and so the only values one may name.
///
/// Re-exported from the model-generated value list for the shape rather than restated, so
/// there is ONE source of truth and a regenerated model cannot leave this behind.
pub use crate::ops::types::EVENT_VALUES;

/// Every value the `FilterRuleName` enum publishes: a key-name filter rule filters on the
/// prefix or on the suffix of an object key and on nothing else. Re-exported from the
/// model-generated value list, like [`EVENT_VALUES`].
pub use crate::ops::types::FILTER_RULE_NAME_VALUES;

/// How many destination configurations one bucket's notification configuration may carry.
///
/// `PutBucketNotificationConfiguration`'s own documentation points at the published quota
/// for this ("For more information about the number of event notification configurations
/// that you can create per bucket, see Amazon S3 service quotas"), which is 100 event
/// notification configurations per bucket; nothing in the model bounds the three lists
/// (`TopicConfigurationList` and its siblings carry no `@length`), so this is also what
/// bounds the records one bucket can park in this process's memory. A put past it is
/// refused rather than recorded, the way a create past [`MAX_BUCKETS_PER_ACCOUNT`] is.
pub const MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET: usize = 100;

/// Which destination a notification configuration publishes to, which is what decides the
/// member its ARN arrives on and the service that ARN has to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDestination {
    /// An Amazon SNS topic (`TopicConfiguration`).
    Topic,
    /// An Amazon SQS queue (`QueueConfiguration`).
    Queue,
    /// A Lambda function (`LambdaFunctionConfiguration`).
    LambdaFunction,
}

impl NotificationDestination {
    /// The member the destination ARN arrives on, as the wire spells it — the `xmlName`
    /// each required ARN member carries (`Topic`, `Queue` and `CloudFunction`
    /// respectively, from `smithy_query(query="shape",shape="TopicConfiguration")` and its
    /// siblings). Used to name the offending member in a refusal.
    pub fn arn_member(self) -> &'static str {
        match self {
            Self::Topic => "Topic",
            Self::Queue => "Queue",
            Self::LambdaFunction => "CloudFunction",
        }
    }

    /// The service field the destination's ARN names: an SNS topic ARN for a
    /// `TopicConfiguration`, an SQS queue ARN for a `QueueConfiguration` and a Lambda
    /// function ARN for a `LambdaFunctionConfiguration` — the three destinations
    /// `PutBucketNotificationConfiguration`'s documentation describes.
    pub fn arn_service(self) -> &'static str {
        match self {
            Self::Topic => "sns",
            Self::Queue => "sqs",
            Self::LambdaFunction => "lambda",
        }
    }
}

/// One object key name filtering rule of a configuration's `Filter`.
///
/// Both members are optional in the model (`smithy_query(query="shape",shape="FilterRule")`),
/// so each is kept exactly as the request carried it rather than defaulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationFilterRule {
    /// A `FilterRuleName` value — one of [`FILTER_RULE_NAME_VALUES`] — or `None` when the
    /// rule named none.
    pub name: Option<String>,
    /// "The value that the filter searches for in object key names" (`FilterRule$Value`),
    /// or `None` when the rule named none.
    pub value: Option<String>,
}

/// One destination configuration a bucket's notification configuration carries: a topic,
/// a queue or a Lambda function, with the events it is notified about.
///
/// The three lists have identical members (an optional `Id`, a required destination ARN, a
/// required `Events` list and an optional `Filter`), so one record serves all three and
/// the destination kind travels beside it as a [`NotificationDestination`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotificationTarget {
    /// The configuration's id. `None` only before the store has seen it: "An optional
    /// unique identifier for configurations in a notification configuration. If you don't
    /// provide one, Amazon S3 will assign an ID" (`NotificationId`), so
    /// [`ComAmazonawsS3Amazons3State::put_notification_configuration`] mints one for every
    /// record that arrives without, and every STORED record carries `Some`.
    pub id: Option<String>,
    /// The destination's ARN, verbatim.
    pub arn: String,
    /// The bucket events this destination is notified about, in the order the request
    /// carried them. Each is one of [`EVENT_VALUES`].
    pub events: Vec<String>,
    /// The key-name filter rules of the configuration's `Filter`, or `None` when it
    /// carries no `Filter` (and so matches every key).
    pub filter_rules: Option<Vec<NotificationFilterRule>>,
}

/// The whole notification configuration a bucket carries.
///
/// A bucket is created with the EMPTY value of this: "By default, your bucket has no event
/// notifications configured. That is, the notification configuration will be an empty
/// NotificationConfiguration" (`PutBucketNotificationConfiguration`'s documentation),
/// which is why this is a plain field rather than an `Option` — "no notifications" and "an
/// empty NotificationConfiguration" are the same state, and a put carrying an empty
/// container is how notifications are turned back off.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BucketNotificationConfiguration {
    /// The SNS topic configurations, in the order the request carried them.
    pub topic_configurations: Vec<NotificationTarget>,
    /// The SQS queue configurations, in the order the request carried them.
    pub queue_configurations: Vec<NotificationTarget>,
    /// The Lambda function configurations, in the order the request carried them.
    pub lambda_function_configurations: Vec<NotificationTarget>,
    /// Whether the configuration carries an `EventBridgeConfiguration`, which "enables
    /// delivery of events to Amazon EventBridge"
    /// (`NotificationConfiguration$EventBridgeConfiguration`). The shape has no members,
    /// so its PRESENCE is the whole state and a `bool` records it exactly.
    pub event_bridge_enabled: bool,
}

impl BucketNotificationConfiguration {
    /// Every destination configuration, with its destination kind, in list order:
    /// topics, then queues, then Lambda functions.
    pub fn targets(&self) -> impl Iterator<Item = (NotificationDestination, &NotificationTarget)> {
        self.topic_configurations
            .iter()
            .map(|target| (NotificationDestination::Topic, target))
            .chain(
                self.queue_configurations
                    .iter()
                    .map(|target| (NotificationDestination::Queue, target)),
            )
            .chain(
                self.lambda_function_configurations
                    .iter()
                    .map(|target| (NotificationDestination::LambdaFunction, target)),
            )
    }

    /// The same records, for writing — what lets the store mint an id into each record
    /// that arrived without one.
    fn targets_mut(&mut self) -> impl Iterator<Item = &mut NotificationTarget> {
        self.topic_configurations
            .iter_mut()
            .chain(self.queue_configurations.iter_mut())
            .chain(self.lambda_function_configurations.iter_mut())
    }

    /// How many destination configurations the container carries, across all three lists —
    /// what [`MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`] bounds.
    pub fn configuration_count(&self) -> usize {
        self.topic_configurations.len()
            + self.queue_configurations.len()
            + self.lambda_function_configurations.len()
    }

    /// Whether the bucket has no event notifications configured at all — the state a
    /// bucket is created in, and the one an empty `NotificationConfiguration` puts it back
    /// into.
    pub fn is_empty(&self) -> bool {
        self.configuration_count() == 0 && !self.event_bridge_enabled
    }
}

/// Why a `NotificationConfiguration` cannot be applied to a bucket.
///
/// Every one of these is a request S3 refuses with `InvalidArgument` (HTTP 400) rather
/// than recording, so a refused put leaves the bucket's current configuration exactly as
/// it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationConfigurationRejection {
    /// An event outside [`EVENT_VALUES`].
    UnsupportedEvent {
        /// The member the configuration arrived on, for the message.
        member: &'static str,
        /// The value that is not a published event.
        event: String,
    },
    /// A configuration naming no event at all: `Events` is a REQUIRED member of each
    /// configuration shape, so a configuration with an empty list asks for nothing to be
    /// published and cannot be applied.
    NoEvents {
        /// The member the configuration arrived on.
        member: &'static str,
    },
    /// A filter rule whose `Name` is not one of [`FILTER_RULE_NAME_VALUES`].
    InvalidFilterRuleName {
        /// The rule's `Name`, or the empty string when it named none.
        name: String,
    },
    /// Two configurations carrying the same `Id`: "An optional UNIQUE identifier for
    /// configurations in a notification configuration" (`NotificationId`).
    DuplicateConfigurationId {
        /// The id that is used twice.
        id: String,
    },
    /// A destination ARN that is not an ARN of the service that destination lives in.
    InvalidDestinationArn {
        /// The member the ARN arrived on (`Topic`, `Queue` or `CloudFunction`).
        member: &'static str,
        /// The service field such an ARN has to name.
        service: &'static str,
        /// The value that is not such an ARN.
        arn: String,
    },
    /// More configurations than [`MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`].
    TooManyConfigurations {
        /// How many the request carried.
        count: usize,
    },
}

impl std::fmt::Display for NotificationConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedEvent { member, event } => write!(
                f,
                "The event {event} is not supported for notifications (member {member})"
            ),
            Self::NoEvents { member } => write!(
                f,
                "A {member} configuration must specify at least one event"
            ),
            Self::InvalidFilterRuleName { name } => write!(
                f,
                "filter rule name must be either prefix or suffix, got {name}"
            ),
            Self::DuplicateConfigurationId { id } => {
                write!(f, "Configuration ID {id} is not unique")
            }
            Self::InvalidDestinationArn {
                member,
                service,
                arn,
            } => write!(
                f,
                "The {member} ARN {arn} is not a valid {service} ARN"
            ),
            Self::TooManyConfigurations { count } => write!(
                f,
                "A notification configuration may carry at most \
                 {MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET} configurations, got {count}"
            ),
        }
    }
}

impl std::error::Error for NotificationConfigurationRejection {}

/// Whether `arn` is an ARN of `service`.
///
/// The ARN grammar AWS publishes: `arn:<partition>:<service>:<region>:<account-id>:<resource>`,
/// so the first field is the literal `arn`, the third IS the service, and the partition,
/// Region, account and resource fields are non-empty. A Lambda function ARN carries extra
/// `:`-separated fields in its resource part, which is why only the first six are examined.
fn is_arn_of_service(arn: &str, service: &str) -> bool {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && fields[2] == service
        && !fields[3].is_empty()
        && !fields[4].is_empty()
        && !fields[5].is_empty()
}

/// Whether `configuration` is one S3 applies to a bucket, and what is wrong with it when
/// it is not.
///
/// Checked BEFORE anything is written, because "the PUT notification is an atomic
/// operation … If the message fails, the entire PUT action will fail, and Amazon S3 will
/// not add the configuration to your bucket" (`PutBucketNotificationConfiguration`'s own
/// documentation).
///
/// Three of the rules here are ALSO modelled constraints, and the boundary validator
/// refuses those before any handler runs (`Event` and `FilterRuleName` are enums and
/// `Events` is required, so a request breaking one never reaches the store). They are
/// still checked here because this function, not the wire, is what guards the STORE: a
/// configuration no reader could report back is never recorded, whoever writes it.
pub fn validate_notification_configuration(
    configuration: &BucketNotificationConfiguration,
) -> Result<(), NotificationConfigurationRejection> {
    let count = configuration.configuration_count();
    if count > MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET {
        return Err(NotificationConfigurationRejection::TooManyConfigurations { count });
    }

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    for (destination, target) in configuration.targets() {
        let member = destination.arn_member();

        if !is_arn_of_service(&target.arn, destination.arn_service()) {
            return Err(NotificationConfigurationRejection::InvalidDestinationArn {
                member,
                service: destination.arn_service(),
                arn: target.arn.clone(),
            });
        }

        if target.events.is_empty() {
            return Err(NotificationConfigurationRejection::NoEvents { member });
        }
        for event in &target.events {
            if !EVENT_VALUES.contains(&event.as_str()) {
                return Err(NotificationConfigurationRejection::UnsupportedEvent {
                    member,
                    event: event.clone(),
                });
            }
        }

        for rule in target.filter_rules.iter().flatten() {
            let name = rule.name.as_deref().unwrap_or_default();
            if !FILTER_RULE_NAME_VALUES.contains(&name) {
                return Err(NotificationConfigurationRejection::InvalidFilterRuleName {
                    name: name.to_string(),
                });
            }
        }

        if let Some(id) = target.id.as_deref() {
            if !seen_ids.insert(id) {
                return Err(
                    NotificationConfigurationRejection::DuplicateConfigurationId {
                        id: id.to_string(),
                    },
                );
            }
        }
    }

    Ok(())
}

/// One rule of a bucket's default server-side-encryption configuration: one
/// `ServerSideEncryptionRule` of the `ServerSideEncryptionConfiguration` a
/// `PutBucketEncryption` carried.
///
/// Every member is optional exactly where the model makes it so
/// (`smithy_query(query="shape",shape="ServerSideEncryptionRule")`): a rule may name a
/// default algorithm, a Bucket Key setting, a set of blocked encryption types, or any
/// combination of them. `sse_algorithm` and `kms_master_key_id` are the two members of
/// the rule's `ApplyServerSideEncryptionByDefault` container, flattened here because
/// `SSEAlgorithm` is REQUIRED inside it — so the container is present in the stored rule
/// exactly when the algorithm is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketEncryptionRule {
    /// `ApplyServerSideEncryptionByDefault$SSEAlgorithm`: a `ServerSideEncryption` value
    /// (`SERVER_SIDE_ENCRYPTION_VALUES`), or `None` when the rule names no default
    /// encryption at all.
    pub sse_algorithm: Option<String>,
    /// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID`, verbatim as the request
    /// carried it — "You can specify the key ID, key alias, or the Amazon Resource Name
    /// (ARN) of the KMS key" (that member's own documentation), and S3 "doesn't validate
    /// the KMS key ID provided in PutBucketEncryption requests" (`PutBucketEncryption`'s
    /// documentation), so whichever form arrives is what comes back.
    pub kms_master_key_id: Option<String>,
    /// `ServerSideEncryptionRule$BucketKeyEnabled`, or `None` when the rule names no
    /// Bucket Key setting.
    pub bucket_key_enabled: Option<bool>,
    /// `BlockedEncryptionTypes$EncryptionType`: the `EncryptionType` values the rule
    /// blocks, in the order the request carried them, or `None` when the rule carries no
    /// `BlockedEncryptionTypes` container.
    pub blocked_encryption_types: Option<Vec<String>>,
}

/// The default encryption rule a general purpose bucket carries while no
/// `PutBucketEncryption` has configured one.
///
/// "By default, all buckets have a default encryption configuration that uses server-side
/// encryption with Amazon S3 managed keys (SSE-S3)" (`GetBucketEncryption`'s own
/// documentation), and "General purpose buckets - By default, S3 Bucket Key is not
/// enabled" (`ServerSideEncryptionRule$BucketKeyEnabled`) — so the rule a never-configured
/// bucket reports is [`DEFAULT_SSE_ALGORITHM`] with the Bucket Key off and nothing blocked.
///
/// COMPUTED rather than stored, so a read of a bucket that was never configured reports
/// this without writing anything: [`Bucket::encryption`] stays `None` and a later reader
/// sees exactly what this one saw.
pub fn default_encryption_rule() -> BucketEncryptionRule {
    BucketEncryptionRule {
        sse_algorithm: Some(DEFAULT_SSE_ALGORITHM.to_string()),
        kms_master_key_id: None,
        bucket_key_enabled: Some(DEFAULT_BUCKET_KEY_ENABLED),
        blocked_encryption_types: None,
    }
}

/// One general purpose bucket this emulator holds.
#[derive(Debug, Clone, Default)]
pub struct Bucket {
    /// The Region the bucket lives in — what `HeadBucket` reports as
    /// `x-amz-bucket-region` and what decides the form of `CreateBucket`'s `Location`.
    pub region: String,
    /// When the bucket was created: the instant
    /// [`ComAmazonawsS3Amazons3State::create_bucket`] recorded it, which is what
    /// `Bucket$CreationDate` reports for a bucket of this account — whether the reader is
    /// S3's own `ListBuckets` or another service that reports the account's buckets.
    ///
    /// Private and never reassigned, so the value a reader sees does not move while the
    /// bucket exists. `None` only in a `Default`-built value, which is never a bucket this
    /// store holds: `create_bucket` is the one place a bucket comes into being and it
    /// always records the instant. A reader reports no creation date rather than inventing
    /// one.
    created_at: Option<SystemTime>,
    /// The bucket policy `PutBucketPolicy` attached, verbatim, or `None` while the
    /// bucket has none. A bucket is created WITHOUT a policy, which is what makes
    /// `GetBucketPolicy` answer `NoSuchBucketPolicy` until one is put.
    pub policy: Option<String>,
    /// The versioning state `PutBucketVersioning` last set on the bucket, or `None`
    /// while the bucket has no versioning state — the state a bucket is created in, and
    /// what makes `GetBucketVersioning` report no `Status` until a put sets one.
    pub versioning: Option<BucketVersioning>,
    /// The Block Public Access settings `PutPublicAccessBlock` last set on the bucket, or
    /// `None` while no `PublicAccessBlockConfiguration` is associated with it — the state a
    /// bucket is created in, and what makes `GetPublicAccessBlock` answer
    /// [`NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE`] until a put sets one.
    ///
    /// Private, like `tags`, so the accessor trio below is the only way in or out.
    public_access_block: Option<BucketPublicAccessBlock>,
    /// The bucket's tag set, in the order the tagging request carried it.
    ///
    /// Private, and empty when no tag set is associated with the bucket: "no tags" and
    /// "an empty tag set" are the same state on AWS, which is why `PutBucketTagging` with
    /// an empty `TagSet` and `DeleteBucketTagging` leave a bucket in the same place.
    tags: Vec<ResourceTag>,
    /// The bucket's ownership controls: one entry per `OwnershipControlsRule`, holding that
    /// rule's `ObjectOwnership` value in the order the request carried it, or `None` while
    /// no ownership controls are associated with the bucket.
    ///
    /// `None` and an empty list are NOT the same state here, which is why this is an
    /// `Option` where [`Self::tags`] is a plain `Vec`: `OwnershipControls$Rules` is a
    /// REQUIRED member (`smithy_query(query="shape",shape="OwnershipControls")`), so a
    /// bucket either has ownership controls that name at least one rule or has none at
    /// all — and "none at all" is what makes `GetBucketOwnershipControls` answer
    /// [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`] rather than report an empty container.
    ///
    /// Private, like `tags`, so the pair of accessors below is the only way in or out.
    ownership_controls: Option<Vec<String>>,
    /// The bucket's event notification configuration, as
    /// `PutBucketNotificationConfiguration` last set it.
    ///
    /// NOT an `Option`, unlike `ownership_controls`: a bucket is created with the EMPTY
    /// configuration ("By default, your bucket has no event notifications configured. That
    /// is, the notification configuration will be an empty NotificationConfiguration"), and
    /// a put carrying an empty container is how notifications are turned off again — the
    /// same state, so there is nothing for a `None` to mean.
    ///
    /// Private, like `tags`: the accessor pair below and
    /// [`ComAmazonawsS3Amazons3State::put_notification_configuration`] (which mints the
    /// ids) are the only way in or out.
    notification: BucketNotificationConfiguration,
    /// The bucket's own default server-side-encryption configuration: one entry per
    /// `ServerSideEncryptionRule` the last `PutBucketEncryption` carried, in that order,
    /// or `None` while no configuration of the bucket's own is associated with it.
    ///
    /// `None` is NOT "no encryption": a bucket without a configuration of its own is
    /// encrypted with SSE-S3 by default (see [`default_encryption_rule`]), which is what a
    /// read of a never-configured bucket reports. Keeping that case as `None` rather than
    /// as a stored copy of the default is what lets a reader report it without writing.
    ///
    /// Private, like `tags` and `ownership_controls`, so the accessor pair below is the
    /// only way in or out.
    encryption: Option<Vec<BucketEncryptionRule>>,
    /// The objects the bucket holds, by key, as `PutObject` last recorded each.
    ///
    /// The CURRENT version of each key, and so the whole of a bucket that was never
    /// versioning-enabled. Private for the same reason as `tags`, and sorted by key
    /// because that is the order S3 lists objects in ("returns some or all of the objects
    /// in a bucket … in lexicographical order").
    objects: BTreeMap<String, Object>,
    /// The NONCURRENT versions of each key, newest first.
    ///
    /// A version lands here only when it stops being the current one on a bucket whose
    /// versioning status is `Enabled`: "when you enable versioning for a bucket, if Amazon
    /// S3 receives multiple write requests for the same object simultaneously, it stores
    /// all of the objects" (`PutObject`'s documentation), and a delete on such a bucket
    /// "inserts a delete marker" rather than removing what was there
    /// (`DeleteObject`'s). Each entry keeps its own [`Object::version_id`], which is what
    /// makes it addressable after it is no longer current.
    ///
    /// Private, and written only through [`Self::put_object`] / [`Self::remove_object`],
    /// so a version cannot be recorded without passing the ceilings every stored version
    /// counts against.
    noncurrent_versions: BTreeMap<String, Vec<Object>>,
}

impl Bucket {
    /// When the bucket was created, as `create_bucket` recorded it. Stable across reads:
    /// nothing updates it. `None` only for a `Default`-built value, which this store
    /// never holds — a reader reports no creation date rather than inventing one.
    pub fn created_at(&self) -> Option<SystemTime> {
        self.created_at
    }

    /// The bucket's tag set, empty when none is associated with it.
    pub fn tags(&self) -> &[ResourceTag] {
        &self.tags
    }

    /// Replace the whole tag set with `tags`.
    ///
    /// A replacement, not a merge: "When this operation sets the tags for a bucket, it
    /// will overwrite any current tags the bucket already has. You cannot use this
    /// operation to add tags to an existing list of tags" (`PutBucketTagging`'s own
    /// documentation).
    pub fn replace_tags(&mut self, tags: Vec<ResourceTag>) {
        self.tags = tags;
    }

    /// Set the bucket's versioning state from the `VersioningConfiguration` a
    /// `PutBucketVersioning` request carried.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Bucket::replace_tags`] replaces
    /// the tag set: `VersioningConfiguration` is "Container for setting the versioning
    /// state" (`PutBucketVersioningRequest$VersioningConfiguration`), so what the bucket
    /// carries afterwards is exactly what this request carried and nothing of the
    /// configuration before it.
    pub fn set_versioning(&mut self, status: &str, mfa_delete: Option<&str>) {
        self.versioning = Some(BucketVersioning {
            status: status.to_string(),
            mfa_delete: mfa_delete.map(str::to_string),
        });
    }

    /// Whether the bucket's versioning status is `Enabled`, which is what makes a write
    /// mint a version id and keep the version it supersedes.
    pub fn versioning_enabled(&self) -> bool {
        self.versioning
            .as_ref()
            .is_some_and(|versioning| versioning.status == VERSIONING_ENABLED_STATUS)
    }

    /// The bucket's Block Public Access settings, or `None` when no
    /// `PublicAccessBlockConfiguration` is associated with the bucket.
    pub fn public_access_block(&self) -> Option<BucketPublicAccessBlock> {
        self.public_access_block
    }

    /// Associate `configuration` with the bucket as its whole Block Public Access
    /// configuration.
    ///
    /// A replacement, not a merge: `PutPublicAccessBlock` "creates or modifies the
    /// PublicAccessBlock configuration for an Amazon S3 bucket" (that operation's own
    /// documentation) from the complete `PublicAccessBlockConfiguration` it carries as its
    /// payload, so what the request names becomes the bucket's configuration in full — the
    /// way [`Bucket::set_ownership_controls`] replaces the ownership controls.
    pub fn set_public_access_block(&mut self, configuration: BucketPublicAccessBlock) {
        self.public_access_block = Some(configuration);
    }

    /// Remove the bucket's Block Public Access configuration, leaving none associated with
    /// it, and report whether the bucket carried one.
    ///
    /// What `DeletePublicAccessBlock` does — "Removes the PublicAccessBlock configuration
    /// for an Amazon S3 bucket" — which puts the bucket back in the state it was created
    /// in, so a later `GetPublicAccessBlock` reports no configuration again.
    pub fn clear_public_access_block(&mut self) -> bool {
        self.public_access_block.take().is_some()
    }

    /// Remove the bucket's tag set, leaving no tags associated with it.
    pub fn clear_tags(&mut self) {
        self.tags.clear();
    }

    /// The bucket's ownership control rules — one `ObjectOwnership` value per rule, in the
    /// order they were applied — or `None` when no ownership controls are associated with
    /// the bucket.
    pub fn ownership_controls(&self) -> Option<&[String]> {
        self.ownership_controls.as_deref()
    }

    /// Associate `rules` with the bucket as its whole ownership controls.
    ///
    /// A replacement, not a merge: `PutBucketOwnershipControls` "creates or modifies
    /// OwnershipControls for an Amazon S3 bucket" (that operation's own documentation) and
    /// carries the complete `OwnershipControls` container as its payload, so what the
    /// request names becomes the bucket's controls in full.
    pub fn set_ownership_controls(&mut self, rules: Vec<String>) {
        self.ownership_controls = Some(rules);
    }

    /// The bucket's event notification configuration — empty when the bucket has no event
    /// notifications configured, which is the state it is created in.
    ///
    /// Every destination configuration it holds carries an `Id`, minted by
    /// [`ComAmazonawsS3Amazons3State::put_notification_configuration`] when the request
    /// named none.
    pub fn notification_configuration(&self) -> &BucketNotificationConfiguration {
        &self.notification
    }

    /// The bucket's own default server-side-encryption rules, in the order they were
    /// applied, or `None` when no configuration of the bucket's own is associated with it
    /// — in which case [`default_encryption_rule`] is what applies to the bucket.
    pub fn encryption(&self) -> Option<&[BucketEncryptionRule]> {
        self.encryption.as_deref()
    }

    /// Associate `rules` with the bucket as its whole default encryption configuration.
    ///
    /// A replacement, not a merge: `PutBucketEncryption` "configures default encryption
    /// and Amazon S3 Bucket Keys for an existing bucket" from the complete
    /// `ServerSideEncryptionConfiguration` it carries as its payload, so what the request
    /// names becomes the bucket's configuration in full.
    pub fn set_encryption(&mut self, rules: Vec<BucketEncryptionRule>) {
        self.encryption = Some(rules);
    }

    /// The object stored at `key`, or `None` when the bucket holds no such key.
    ///
    /// The CURRENT version: what a read that names no `versionId` answers with, and the
    /// only version a bucket that was never versioning-enabled has.
    pub fn object(&self, key: &str) -> Option<&Object> {
        self.objects.get(key)
    }

    /// The version of `key` that `version_id` names — current or noncurrent — or `None`
    /// when the bucket holds no such version.
    ///
    /// [`NULL_VERSION_ID`] names the version a write made while the bucket was not
    /// versioning-enabled, which is stored with no minted id of its own.
    pub fn object_version(&self, key: &str, version_id: &str) -> Option<&Object> {
        let named = |held: &&Object| match held.version_id.as_deref() {
            Some(id) => id == version_id,
            None => version_id == NULL_VERSION_ID,
        };
        self.objects
            .get(key)
            .filter(named)
            .or_else(|| self.noncurrent_versions.get(key)?.iter().find(named))
    }

    /// Every version of `key` the bucket holds, newest first: the current one (when the
    /// key has one) followed by its noncurrent versions.
    ///
    /// The read seam for anything that reports a key's versions rather than its current
    /// object, so no caller reaches into either map.
    pub fn object_versions(&self, key: &str) -> Vec<&Object> {
        self.objects
            .get(key)
            .into_iter()
            .chain(
                self.noncurrent_versions
                    .get(key)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            )
            .collect()
    }

    /// Every object the bucket holds, keyed by object key.
    ///
    /// A `BTreeMap`, so iterating it IS the ascending UTF-8 byte order S3 lists keys in
    /// (`String`'s `Ord` compares bytes) and a listing needs no sort step. Read-only:
    /// an object is recorded only through
    /// [`ComAmazonawsS3Amazons3State::put_object`], which is the only place that sees
    /// every ceiling.
    pub fn objects(&self) -> &BTreeMap<String, Object> {
        &self.objects
    }

    /// Remove the object at `key`, reporting whether the bucket held one.
    ///
    /// A current version that is [`Self::keeps_superseded`] is kept as a noncurrent version
    /// instead of being destroyed: "If bucket versioning is enabled, the operation inserts
    /// a delete marker, which becomes the current version of the object. To permanently
    /// delete an object in a versioned bucket, you must include the object's versionId in
    /// the request" (`DeleteObject`'s documentation) — so the version a delete displaces
    /// is still addressable by its id afterwards, and its bytes are still retained.
    /// Removing the `null` version of a bucket that is not versioning-enabled releases
    /// everything that object retained.
    ///
    /// PRIVATE, so a delete goes through
    /// [`ComAmazonawsS3Amazons3State::remove_object`] and cannot name a bucket that does
    /// not exist.
    fn remove_object(&mut self, key: &str) -> bool {
        let keeps = self.keeps_superseded(key);
        let Some(removed) = self.objects.remove(key) else {
            return false;
        };
        if keeps {
            self.noncurrent_versions
                .entry(key.to_string())
                .or_default()
                .insert(0, removed);
        }
        true
    }

    /// The object stored at `key` for writing one of its subresources, or `None` when the
    /// bucket holds no such key.
    ///
    /// PRIVATE, and reached only through
    /// [`ComAmazonawsS3Amazons3State::replace_object_tags`]: that is the one place that
    /// sees the byte budgets a subresource write also has to stay inside, so a handler
    /// reaching an object this way directly would bypass them.
    fn object_mut(&mut self, key: &str) -> Option<&mut Object> {
        self.objects.get_mut(key)
    }

    /// Store `object` at `key` as the key's current version, reporting what that did.
    ///
    /// A replacement, not a merge: "If it receives multiple write requests for the same
    /// object simultaneously, it overwrites all but the last object written", and "You
    /// cannot use PutObject to only update a single piece of metadata for an existing
    /// object. You must put the entire object with updated metadata" (`PutObject`'s own
    /// documentation).
    ///
    /// `minted` is the version id this write mints, which the caller passes exactly when
    /// the bucket's versioning status is `Enabled` — "All objects added to the bucket
    /// receive a unique version ID" (`PutBucketVersioning`).
    ///
    /// What the write does to the version it supersedes is [`Self::keeps_superseded`]: on
    /// a versioning-enabled bucket it is KEPT as a noncurrent version, so both stay
    /// addressable, and so is a version that carries an id of its own whatever the
    /// bucket's status is now; the `null` version of a bucket that is not
    /// versioning-enabled is replaced in place — "Suspended … All objects added to the
    /// bucket receive the version ID null".
    ///
    /// PRIVATE, and reached only through
    /// [`ComAmazonawsS3Amazons3State::put_object`]: the per-bucket ceilings checked here
    /// cannot see the service-wide ones, so a handler calling this directly would bypass
    /// them. NOTHING is written unless the answer is [`ObjectWrite::Created`] or
    /// [`ObjectWrite::Replaced`]: an over-long key, metadata past
    /// [`MAX_OBJECT_METADATA_BYTES`], a bucket at [`MAX_OBJECTS_PER_BUCKET`] stored
    /// versions and a write past [`MAX_OBJECT_BYTES_PER_BUCKET`] retained bytes are each
    /// refused here. Replacing an object releases everything it retained, so the budget is
    /// over what the bucket WOULD hold, not over everything ever written — while a
    /// versioned write releases nothing, because the version it supersedes is still there.
    fn put_object(&mut self, key: &str, mut object: Object, minted: Option<String>) -> ObjectWrite {
        if key.len() > MAX_OBJECT_KEY_BYTES {
            return ObjectWrite::KeyTooLong { bytes: key.len() };
        }
        let metadata_bytes = object.metadata_bytes();
        if metadata_bytes > MAX_OBJECT_METADATA_BYTES {
            return ObjectWrite::MetadataTooLarge {
                bytes: metadata_bytes,
            };
        }
        // The id is on the object before anything is measured, so the bytes it retains are
        // inside the budgets like every other string kept beside the payload.
        object.version_id = minted;
        let retaining = self.keeps_superseded(key);
        let held = self.objects.contains_key(key);
        if (retaining || !held) && self.stored_versions() >= MAX_OBJECTS_PER_BUCKET {
            return ObjectWrite::ObjectCountReached;
        }
        let would_hold = self.retained_bytes_after_write(key, retaining) + object.retained_bytes();
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return ObjectWrite::ByteBudgetReached { would_hold };
        }
        let superseded = self.objects.insert(key.to_string(), object);
        if let (true, Some(superseded)) = (retaining, superseded) {
            self.noncurrent_versions
                .entry(key.to_string())
                .or_default()
                .insert(0, superseded);
        }
        if held {
            ObjectWrite::Replaced
        } else {
            ObjectWrite::Created
        }
    }

    /// Every key the bucket holds at least one version of, in ascending UTF-8 byte order
    /// — the keys with a current object, plus the keys whose only versions left are
    /// noncurrent ones.
    ///
    /// The read seam for a listing over VERSIONS (`ListObjectVersions`), the way
    /// [`Self::objects`] is the seam for a listing over current objects. Ascending order,
    /// because that is the order S3 lists keys in.
    pub fn version_keys(&self) -> Vec<&str> {
        let mut keys: ::std::collections::BTreeSet<&str> = self
            .objects
            .keys()
            .map(String::as_str)
            .collect();
        keys.extend(self.noncurrent_versions.keys().map(String::as_str));
        keys.into_iter().collect()
    }

    /// Remove the version of `key` that `version_id` names, reporting whether the bucket
    /// held such a version.
    ///
    /// "To permanently delete an object in a versioned bucket, you must include the
    /// object's versionId in the request" (`DeleteObject`'s documentation). Removing the
    /// key's CURRENT version promotes the newest version left to current, which is what
    /// makes a versioned bucket's delete reversible: the version before it becomes what a
    /// read without a `versionId` answers with. Everything the removed version retained is
    /// released with it.
    ///
    /// PRIVATE, so a delete goes through
    /// [`ComAmazonawsS3Amazons3State::remove_object_version`] and cannot name a bucket
    /// that does not exist.
    fn remove_version(&mut self, key: &str, version_id: &str) -> bool {
        let names = |held: &Object| match held.version_id.as_deref() {
            Some(id) => id == version_id,
            None => version_id == NULL_VERSION_ID,
        };
        let removed_current = self.objects.get(key).is_some_and(names);
        if removed_current {
            self.objects.remove(key);
            if let Some(remaining) = self.noncurrent_versions.get_mut(key) {
                if !remaining.is_empty() {
                    self.objects.insert(key.to_string(), remaining.remove(0));
                }
            }
        } else {
            let Some(remaining) = self.noncurrent_versions.get_mut(key) else {
                return false;
            };
            let Some(position) = remaining.iter().position(names) else {
                return false;
            };
            remaining.remove(position);
        }
        if self
            .noncurrent_versions
            .get(key)
            .is_some_and(Vec::is_empty)
        {
            self.noncurrent_versions.remove(key);
        }
        true
    }

    /// Whether a write or a delete at `key` KEEPS the version it displaces rather than
    /// releasing it.
    ///
    /// Two things keep it. A bucket whose versioning status is `Enabled` keeps every
    /// version it already holds, including one stored before versioning was enabled ("All
    /// objects added to the bucket receive a unique version ID", and the objects that were
    /// there keep "a version ID of null"). And a version that carries an id of its own is
    /// kept whatever the bucket's status is now, because nothing but a delete naming that
    /// id takes it away ("To permanently delete an object in a versioned bucket, you must
    /// include the object's versionId in the request") — which is what makes suspension a
    /// state of the bucket rather than of the versions it holds.
    ///
    /// What is left is the `null` version of a bucket that is not versioning-enabled:
    /// "Suspended … All objects added to the bucket receive the version ID null", so a
    /// write replaces it and a delete removes it, in place.
    fn keeps_superseded(&self, key: &str) -> bool {
        self.versioning_enabled()
            || self
                .objects
                .get(key)
                .is_some_and(|held| held.version_id.is_some())
    }

    /// How many object versions this bucket stores: the current object at each key plus
    /// every noncurrent version it keeps.
    ///
    /// What [`MAX_OBJECTS_PER_BUCKET`] counts, because a retained noncurrent version holds
    /// this process's memory exactly as a current one does.
    fn stored_versions(&self) -> usize {
        self.objects.len()
            + self
                .noncurrent_versions
                .values()
                .map(Vec::len)
                .sum::<usize>()
    }

    /// How many bytes this bucket's objects retain — payloads AND every string kept
    /// beside them, over every version it stores, so no part of a stored object escapes
    /// the budgets.
    fn retained_bytes(&self) -> usize {
        self.objects
            .values()
            .chain(self.noncurrent_versions.values().flatten())
            .map(Object::retained_bytes)
            .sum()
    }

    /// How many bytes this bucket would still retain with a write at `key` applied, not
    /// counting the incoming object itself.
    ///
    /// Everything it holds now, less the current object at `key` when that object is
    /// replaced in place — which is every write except one that mints a version id, since
    /// such a write keeps what it supersedes.
    fn retained_bytes_after_write(&self, key: &str, retaining_current: bool) -> usize {
        let released = if retaining_current {
            0
        } else {
            self.objects.get(key).map_or(0, Object::retained_bytes)
        };
        self.retained_bytes().saturating_sub(released)
    }
}

/// One object this emulator holds, as a single `PutObject` recorded it.
///
/// Only the members this emulator can report back faithfully are kept: everything a
/// stored object's reader (`GetObject`) answers comes from here, so a member that is not
/// stored is not reported either.
#[derive(Debug, Clone)]
pub struct Object {
    /// The object's bytes, exactly as the request carried them.
    pub body: Vec<u8>,
    /// The version id this stored version is addressed by, or `None` for the `null`
    /// version — a version written while the bucket's versioning status was not `Enabled`.
    ///
    /// Minted by [`ComAmazonawsS3Amazons3State::put_object`], the one place that sees both
    /// the bucket's versioning state and the store's version sequence, so no handler
    /// constructs one: an `Object` handed to that call carries `None` and comes back
    /// stored with whatever the bucket's state called for.
    pub version_id: Option<String>,
    /// The object's entity tag, including the quotes S3 reports it with — see
    /// [`object_etag`].
    pub etag: String,
    /// When the object was last written, which is what `Last-Modified` reports.
    pub last_modified: SystemTime,
    /// A `StorageClass` value; [`DEFAULT_STORAGE_CLASS`] when the request named none.
    pub storage_class: String,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
    pub content_disposition: Option<String>,
    pub content_encoding: Option<String>,
    pub content_language: Option<String>,
    pub expires: Option<String>,
    pub website_redirect_location: Option<String>,
    /// The user-defined metadata the request carried in its `x-amz-meta-` headers.
    pub metadata: BTreeMap<String, String>,
    /// The object's tag set, in the order the tagging request carried it.
    ///
    /// Empty when no tag set is associated with the object: "no tags" and "an empty tag
    /// set" are the same state, which is why `PutObjectTagging` with an empty `TagSet`
    /// leaves an object where `DeleteObjectTagging` would. A `PutObject` REPLACES the
    /// whole object ("You cannot use PutObject to only update a single piece of metadata
    /// for an existing object"), so the tag set of the object it replaces goes with it;
    /// only [`ComAmazonawsS3Amazons3State::replace_object_tags`] sets one afterwards.
    pub tags: Vec<ResourceTag>,
}

impl Object {
    /// How many bytes this stored object retains: its payload AND every string kept
    /// beside it (its entity tag, storage class, the response headers it carries, its
    /// user-defined metadata and its tag set).
    ///
    /// This, not the payload alone, is what the per-bucket and service-wide budgets are
    /// computed from: a one-byte object with large metadata or a large
    /// `Content-Disposition` retains just as much of this process's memory as a large
    /// payload does, so none of it may escape the ceilings.
    pub fn retained_bytes(&self) -> usize {
        self.body.len()
            + self.etag.len()
            + self.storage_class.len()
            + self.version_id.as_ref().map_or(0, String::len)
            + self.metadata_bytes()
            + tag_set_bytes(&self.tags)
            + [
                &self.content_type,
                &self.cache_control,
                &self.content_disposition,
                &self.content_encoding,
                &self.content_language,
                &self.expires,
                &self.website_redirect_location,
            ]
            .iter()
            .filter_map(|header| header.as_ref())
            .map(String::len)
            .sum::<usize>()
    }

    /// How many tags this object's tag set carries, or `None` when it carries none —
    /// what `GetObject`'s and `HeadObject`'s `TagCount` reports: "The number of tags, if
    /// any, on the object", so an object with no tags reports no count rather than a zero.
    ///
    /// ONE place, so the two readers of this member cannot drift. The count fits an `i32`
    /// for any tag set this store accepts ([`validate_tag_set`] bounds it at
    /// [`MAX_TAGS_PER_OBJECT`]), and a set that somehow grew past that reports the largest
    /// count the member can carry rather than a wrapped one.
    pub fn tag_count(&self) -> Option<i32> {
        (!self.tags.is_empty()).then(|| i32::try_from(self.tags.len()).unwrap_or(i32::MAX))
    }

    /// How large this object's user-defined metadata is, in UTF-8 bytes summed over every
    /// key and value — what [`MAX_OBJECT_METADATA_BYTES`] bounds.
    pub fn metadata_bytes(&self) -> usize {
        self.metadata
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum()
    }
}

/// The entity tag S3 reports for an object written by a single `PutObject`.
///
/// The rule is `PutObjectOutput$ETag`'s own documentation: "General purpose buckets - To
/// ensure that data is not corrupted traversing the network, for objects where the ETag
/// is the MD5 digest of the object, you can calculate the MD5 while putting an object to
/// Amazon S3 and compare the returned ETag to the calculated MD5 value." A one-part,
/// unencrypted PUT into a general purpose bucket — the only kind this emulator records —
/// is exactly that case, so the tag is the MD5 of the stored bytes, lowercase hex, inside
/// the double quotes S3 wraps it in.
pub fn object_etag(body: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Md5::digest(body)))
}

/// The same digest as [`object_etag`], Base64 encoded the way the `Content-MD5` header
/// carries it: "The Base64 encoded 128-bit MD5 digest of the data"
/// (`PutObjectRequest$ContentMD5`), which is what makes that header checkable.
pub fn content_md5(body: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(Md5::digest(body))
}

/// The canonical user id of the one account this emulator serves — what an S3 response
/// reports as the `Owner$ID` of the account's buckets and objects.
///
/// S3 names an owner by its CANONICAL USER ID, not by its account id: "a 64-digit
/// hexadecimal encoded, 256-bit value" (AWS General Reference, *AWS account identifiers*),
/// so [`::engine::emulator_account_id`] itself is the wrong shape for the member. This
/// derives one from that account id by SHA-256, which makes it deterministic (every read
/// reports the same owner, whatever the clock or the request does), of the documented
/// shape, and a disclosure of nothing: it is a hash of the canonical placeholder account
/// `000000000000`.
///
/// Service-wide rather than per-operation, because every reader that reports an owner —
/// `ListBuckets`, and the ACL and object-listing operations when they are implemented —
/// owes the SAME id for this account.
pub fn canonical_user_id() -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(::engine::emulator_account_id().as_bytes());
    // 32 digest bytes render as the 64 lowercase hex digits a canonical user id carries.
    ::hex::encode(digest.finalize())
}

/// The version id a write into a versioning-enabled bucket is stored under.
///
/// S3's version ids are OPAQUE: `ObjectVersionId` is a plain string in the model
/// (`smithy_query(query="shape",shape="ObjectVersionId")` reports no pattern and no
/// length), and the only thing documented about them is that each is unique ("All objects
/// added to the bucket receive a unique version ID" — `PutBucketVersioning`). So this is
/// DERIVED rather than drawn from a generator: the SHA-256 of the bucket, the key and the
/// store's monotonic mint sequence, Base64 encoded to the 32 characters an S3 version id
/// is shaped like. Two versions of a key — and two versions anywhere in the store —
/// therefore never collide, and the value stays what it was once stored, because nothing
/// re-derives it.
///
/// The URL-safe Base64 alphabet (RFC 4648 §5), so the id survives a round trip through the
/// `versionId` query parameter a client sends it back in: `+` would arrive decoded as a
/// space and `/` would end the path segment.
pub fn mint_version_id(bucket: &str, key: &str, sequence: u64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(bucket.as_bytes());
    // A separator, so ("a", "bc") and ("ab", "c") cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(key.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    // 24 bytes encode to exactly 32 unpadded Base64 characters.
    ::aws_smithy_types::base64::encode(&digest.finalize()[..24])
        .replace('+', "-")
        .replace('/', "_")
}

/// Per-service state for com-amazonaws-s3-amazons3. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsS3Amazons3State {
    /// Buckets by name.
    ///
    /// Keyed by name ALONE because that is the scope of S3's bucket namespace: "General
    /// purpose buckets exist in a global namespace, which means that each bucket name
    /// must be unique across all Amazon Web Services accounts in all the Amazon Web
    /// Services Regions within a partition" (`CreateBucket`'s documentation). Sorted,
    /// because S3 lists buckets in name order.
    buckets: BTreeMap<String, Bucket>,
    /// How many object versions this store has minted an id for, ever.
    ///
    /// The sequence [`mint_version_id`] derives each id from, so two versions of one key
    /// — and two versions anywhere in the store — can never carry the same id, whatever
    /// the clock does. Only [`Self::put_object`] advances it.
    minted_versions: u64,
    /// How many notification configuration ids this store has minted.
    ///
    /// Only ever increases, so [`Self::mint_notification_configuration_id`] cannot hand
    /// out the same id twice however many configurations are put.
    minted_notification_configuration_ids: u64,
}

impl ComAmazonawsS3Amazons3State {
    /// Every bucket this account holds, as `(name, bucket)` in bucket-name order — the
    /// order S3 lists buckets in.
    ///
    /// The read seam for any operation that reports the whole account's buckets, S3's own
    /// `ListBuckets` included, so none of them keeps a second copy of this map.
    pub fn buckets(&self) -> impl Iterator<Item = (&str, &Bucket)> + '_ {
        self.buckets
            .iter()
            .map(|(name, bucket)| (name.as_str(), bucket))
    }

    /// The bucket named `name`, or `None` when this emulator holds no such bucket.
    pub fn bucket(&self, name: &str) -> Option<&Bucket> {
        self.buckets.get(name)
    }

    /// The bucket named `name` for writing — a subresource (a policy or a tag set) or its
    /// objects — or `None` when this emulator holds no such bucket, so such a write
    /// refuses a name that is not a bucket instead of creating one.
    pub fn bucket_mut(&mut self, name: &str) -> Option<&mut Bucket> {
        self.buckets.get_mut(name)
    }

    /// Store `object` at `key` in the bucket named `bucket`, reporting what that did, or
    /// `None` when this emulator holds no such bucket (so an object write refuses a name
    /// that is not a bucket instead of creating one).
    ///
    /// This is also where an object's VERSION ID is minted, because this is the only place
    /// that sees both the bucket's versioning state and the store's version sequence: a
    /// write into a bucket whose status is `Enabled` mints a fresh id and keeps the version
    /// it supersedes ("All objects added to the bucket receive a unique version ID" —
    /// `PutBucketVersioning`), and a write anywhere else mints none and replaces what the
    /// key held, which is the `null` version. The id the write stored is read back off the
    /// stored object, so nothing reports an id the store did not record.
    ///
    /// This is the ONLY way an object is recorded, because it is the only place that can
    /// see BOTH the per-bucket ceilings and the service-wide ones: the per-bucket pair
    /// alone would leave the store bounded only by their product with
    /// [`MAX_BUCKETS_PER_ACCOUNT`], so the whole-store checks here ([`MAX_OBJECTS_TOTAL`]
    /// and [`MAX_OBJECT_BYTES_TOTAL`]) are what bound this process's object memory
    /// whatever a caller does. Nothing is written unless the answer is
    /// [`ObjectWrite::Created`] or [`ObjectWrite::Replaced`]; a refusal leaves every
    /// stored object exactly as it was, and mints nothing.
    pub fn put_object(&mut self, bucket: &str, key: &str, object: Object) -> Option<ObjectWrite> {
        let held_bucket = self.buckets.get(bucket)?;
        let versioned = held_bucket.versioning_enabled();
        // Whether this write keeps the version it supersedes, which is what decides
        // whether it adds a stored version to the store's counts and budgets or replaces
        // one in place. It turns on the superseded version, not on the bucket's status.
        let keeps = held_bucket.keeps_superseded(key);
        let held = held_bucket.object(key).is_some();

        // What the whole store would hold after this write: every other bucket's bytes,
        // plus what this bucket would hold with the object at `key` written. The counts
        // work the same way — a key the bucket already holds adds no entry, unless the
        // version there is kept rather than replaced.
        let held_versions: usize = self
            .buckets
            .values()
            .map(Bucket::stored_versions)
            .sum::<usize>();
        if (keeps || !held) && held_versions >= MAX_OBJECTS_TOTAL {
            return Some(ObjectWrite::TotalObjectCountReached);
        }
        // The id is minted before the budgets are measured, so the bytes it retains are
        // counted; a refusal below leaves it unused, which costs nothing but the sequence
        // number it consumed (ids are opaque, and nothing numbers them).
        let minted = versioned.then(|| {
            self.minted_versions = self.minted_versions.saturating_add(1);
            mint_version_id(bucket, key, self.minted_versions)
        });
        let incoming = object.retained_bytes() + minted.as_ref().map_or(0, String::len);
        let would_hold: usize = self
            .buckets
            .iter()
            .map(|(name, held)| {
                if name == bucket {
                    held.retained_bytes_after_write(key, keeps)
                } else {
                    held.retained_bytes()
                }
            })
            .sum::<usize>()
            + incoming;
        if would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectWrite::TotalByteBudgetReached { would_hold });
        }

        // Then the per-bucket ceilings, which also perform the write.
        self.buckets
            .get_mut(bucket)
            .map(|held| held.put_object(key, object, minted))
    }

    /// Record `configuration` as the whole notification configuration of the bucket named
    /// `bucket`, reporting `None` when this emulator holds no such bucket (so a put
    /// refuses a name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT, not a merge: "This action replaces the existing notification
    /// configuration with the configuration you include in the request body"
    /// (`PutBucketNotificationConfiguration`'s own documentation), so what the bucket
    /// carries afterwards is exactly what this request carried — an empty container
    /// included, which turns notifications off.
    ///
    /// Every record that arrives without an `Id` is given one here, because "if you don't
    /// provide one, Amazon S3 will assign an ID" (`NotificationId`). A minted id is
    /// recorded with the configuration, so every later read reports the same one, and it
    /// never collides with an id the request named itself.
    pub fn put_notification_configuration(
        &mut self,
        bucket: &str,
        mut configuration: BucketNotificationConfiguration,
    ) -> Option<()> {
        if !self.buckets.contains_key(bucket) {
            return None;
        }

        let mut taken: BTreeSet<String> = configuration
            .targets()
            .filter_map(|(_, target)| target.id.clone())
            .collect();
        for target in configuration.targets_mut() {
            if target.id.is_none() {
                let id = self.mint_notification_configuration_id(&taken);
                taken.insert(id.clone());
                target.id = Some(id);
            }
        }

        self.buckets.get_mut(bucket)?.notification = configuration;
        Some(())
    }

    /// An opaque configuration id no configuration in this store carries.
    ///
    /// S3's own assigned ids are opaque strings, so this is the MD5 digest of the counter
    /// above rendered as hex — the digest helper this module already uses for an object's
    /// ETag. Uniqueness comes from the counter, not from the digest, and an id a request
    /// named itself (`taken`) is stepped over rather than reused.
    fn mint_notification_configuration_id(&mut self, taken: &BTreeSet<String>) -> String {
        loop {
            self.minted_notification_configuration_ids += 1;
            let mut digest = Md5::new();
            digest.update(b"s3-notification-configuration-id:");
            digest.update(self.minted_notification_configuration_ids.to_be_bytes());
            let id = hex::encode(digest.finalize());
            if !taken.contains(&id) {
                return id;
            }
        }
    }

    /// Replace the tag set of the object at `key` in the bucket named `bucket`, reporting
    /// what that did, or `None` when this emulator holds no such bucket (so a tagging
    /// write refuses a name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole tag set, the way [`Bucket::replace_tags`] replaces a
    /// bucket's: `Tagging` is the complete container the request carries, so what the
    /// object holds afterwards is exactly what this call carried and nothing of the set
    /// before it — an empty `TagSet` therefore leaves the object with no tags.
    ///
    /// This is the ONLY way an object's tag set is recorded, because it is the only place
    /// that can see BOTH the per-bucket byte budget and the service-wide one: a tag set is
    /// retained with the object ([`Object::retained_bytes`]) and is not bounded by
    /// anything [`Self::put_object`] checked, so a tag set that would take the store past
    /// either budget is refused rather than recorded. Nothing is written unless the answer
    /// is [`ObjectTagWrite::Applied`].
    pub fn replace_object_tags(
        &mut self,
        bucket: &str,
        key: &str,
        tags: Vec<ResourceTag>,
    ) -> Option<ObjectTagWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.object(key) else {
            return Some(ObjectTagWrite::NoSuchKey);
        };

        // What the bucket and the whole store would retain with this tag set in place of
        // the object's current one: everything they retain now, less the bytes the tag set
        // being replaced holds, plus the bytes of the replacement.
        let released = tag_set_bytes(&object.tags);
        let taken = tag_set_bytes(&tags);
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectTagWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .buckets
            .values()
            .map(Bucket::retained_bytes)
            .sum::<usize>()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectTagWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.object_mut(key))
            .map(|object| {
                object.tags = tags;
                ObjectTagWrite::Applied
            })
    }

    /// Record a bucket named `name` in `region`, reporting what happened.
    ///
    /// Nothing is written unless the answer is [`BucketCreation::Created`]: a taken name
    /// leaves the existing bucket alone, and a create past
    /// [`MAX_BUCKETS_PER_ACCOUNT`] is refused, which is what bounds this map.
    pub fn create_bucket(&mut self, name: &str, region: &str) -> BucketCreation {
        if self.buckets.contains_key(name) {
            return BucketCreation::NameTaken;
        }
        if self.buckets.len() >= MAX_BUCKETS_PER_ACCOUNT {
            return BucketCreation::QuotaReached;
        }
        self.buckets.insert(
            name.to_string(),
            Bucket {
                region: region.to_string(),
                // The creation instant is read here, where the bucket comes into being,
                // so `CreationDate` is the moment the store accepted it.
                created_at: Some(SystemTime::now()),
                // A fresh bucket carries no policy, no tags, no ownership controls and no
                // Block Public Access configuration, and holds no objects;
                // `PutBucketPolicy`, `PutBucketTagging`, `PutBucketOwnershipControls` and
                // `PutPublicAccessBlock` are what associate those subresources with it and
                // `PutObject` what puts an object in it.
                ..Bucket::default()
            },
        );
        BucketCreation::Created
    }

    /// Remove the object at `key` from the bucket named `bucket`, reporting whether an
    /// object was there — or `None` when this emulator holds no such bucket, so a delete
    /// can tell "no such bucket" from "no such key".
    ///
    /// Everything the removed object retained is released with it, which is what lets a
    /// later write back into the byte budgets [`Self::put_object`] checks.
    pub fn remove_object(&mut self, bucket: &str, key: &str) -> Option<bool> {
        self.buckets
            .get_mut(bucket)
            .map(|held| held.remove_object(key))
    }

    /// Remove the version of `key` that `version_id` names from the bucket named
    /// `bucket`, reporting whether the bucket held such a version — or `None` when this
    /// emulator holds no such bucket, so a delete can tell "no such bucket" from "no such
    /// version".
    ///
    /// This is the permanent delete a versioned bucket needs ("To permanently delete an
    /// object in a versioned bucket, you must include the object's versionId in the
    /// request"), and the only way a stored version's bytes are released, which is what
    /// lets a later write back into the byte budgets [`Self::put_object`] checks.
    pub fn remove_object_version(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Option<bool> {
        self.buckets
            .get_mut(bucket)
            .map(|held| held.remove_version(key, version_id))
    }

    /// Remove the bucket named `bucket_name`, releasing everything it still held.
    /// `false` when this emulator holds no such bucket.
    pub fn remove_bucket(&mut self, bucket_name: &str) -> bool {
        self.buckets.remove(bucket_name).is_some()
    }
}

// ---------------------------------------------------------------------------
// The error codes S3's bucket subresource operations answer with.
//
// `PutBucketPolicy`, `GetBucketPolicy`, `PutBucketTagging` and `GetBucketTagging`
// declare NO errors in the model, yet S3 answers each of these on them, so they are
// returned through `scaffolded::Scaffolded::unmodeled`.
// ---------------------------------------------------------------------------

/// "The specified bucket does not exist."
pub const NO_SUCH_BUCKET_CODE: &str = "NoSuchBucket";
/// See [`NO_SUCH_BUCKET_CODE`].
pub const NO_SUCH_BUCKET_MESSAGE: &str = "The specified bucket does not exist";
/// "The specified key does not exist." — what a read of an object no bucket holds
/// answers with.
pub const NO_SUCH_KEY_CODE: &str = "NoSuchKey";
/// See [`NO_SUCH_KEY_CODE`].
pub const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";
/// The code S3 answers a read or a write that names a `versionId` the bucket holds no
/// version for: "Indicates that the version ID specified in the request does not match an
/// existing version", with HTTP 404. Not a shape in this model, so it is answered through
/// the unmodelled channel.
pub const NO_SUCH_VERSION_CODE: &str = "NoSuchVersion";
/// See [`NO_SUCH_VERSION_CODE`].
pub const NO_SUCH_VERSION_MESSAGE: &str =
    "The specified version does not exist.";
/// The code S3 answers a write whose `Content-MD5` is not the digest of the bytes it
/// carried: "The Content-MD5 or checksum value that you specified did not match what the
/// server received."
pub const BAD_DIGEST_CODE: &str = "BadDigest";
/// The code S3 answers a write whose body is larger than one request may carry
/// ([`MAX_OBJECT_BYTES`]).
pub const ENTITY_TOO_LARGE_CODE: &str = "EntityTooLarge";
/// The code S3 answers a conditional write whose precondition does not hold, with HTTP
/// 412 (`PutObjectRequest$IfMatch`: "If the ETag values do not match, the operation
/// returns a 412 Precondition Failed error").
pub const PRECONDITION_FAILED_CODE: &str = "PreconditionFailed";
/// The code S3 answers a read of a bucket that has no ownership controls associated with
/// it, with HTTP 404 — the case `GetBucketOwnershipControls`'s own documentation describes
/// ("A bucket doesn't have OwnershipControls settings in the following cases: … You've
/// manually deleted the bucket ownership control value using the
/// DeleteBucketOwnershipControls API operation").
pub const OWNERSHIP_CONTROLS_NOT_FOUND_CODE: &str = "OwnershipControlsNotFoundError";
/// See [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`].
pub const OWNERSHIP_CONTROLS_NOT_FOUND_MESSAGE: &str =
    "The bucket ownership controls were not found";
/// The code S3 answers a read of a bucket that has no `PublicAccessBlockConfiguration`
/// associated with it, with HTTP 404 — the subresource-not-there case, answered the way
/// [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`] answers it for ownership controls, because
/// `GetPublicAccessBlock` reports the configuration as its WHOLE @httpPayload and so has
/// no way to report "there is none" in a 200.
pub const NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE: &str =
    "NoSuchPublicAccessBlockConfiguration";
/// See [`NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE`].
pub const NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_MESSAGE: &str =
    "The public access block configuration was not found";
/// The code S3 answers a request whose XML payload does not validate against the schema
/// the API publishes for it: "MalformedXML - The XML provided does not match the schema"
/// (`PutBucketTagging`'s own list of special errors), with HTTP 400.
pub const MALFORMED_XML_CODE: &str = "MalformedXML";

/// How many ownership control rules this emulator records for one bucket.
///
/// NOT a fidelity claim: `OwnershipControlsRules` carries no `@length` in the model
/// (`smithy_query(query="shape",shape="OwnershipControlsRules")`), so nothing bounds the
/// list a request may send, and a bucket's rules are kept in this process's memory — the
/// same reason [`MAX_BUCKET_POLICY_BYTES`] bounds a policy. Each rule names one value of
/// the three-value `ObjectOwnership` enum, so this ceiling is far above any list that says
/// something new; a put past it is refused rather than recorded.
pub const MAX_OWNERSHIP_CONTROLS_RULES: usize = 10;

/// The `SSEAlgorithm` a general purpose bucket's default encryption uses until a
/// `PutBucketEncryption` names another one.
///
/// "By default, all buckets have a default encryption configuration that uses server-side
/// encryption with Amazon S3 managed keys (SSE-S3)" (`GetBucketEncryption`'s own
/// documentation); SSE-S3 is the `ServerSideEncryption` value `AES256`
/// (`smithy_query(query="shape",shape="ServerSideEncryption")`).
pub const DEFAULT_SSE_ALGORITHM: &str = "AES256";

/// Whether a bucket's default encryption uses an S3 Bucket Key until a configuration
/// enables one: "General purpose buckets - By default, S3 Bucket Key is not enabled"
/// (`ServerSideEncryptionRule$BucketKeyEnabled`).
pub const DEFAULT_BUCKET_KEY_ENABLED: bool = false;

/// The `ServerSideEncryption` values a `KMSMasterKeyID` may accompany in a default
/// encryption rule.
///
/// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID`'s own documentation: "General
/// purpose buckets - This parameter is allowed if and only if `SSEAlgorithm` is set to
/// `aws:kms` or `aws:kms:dsse`." Both are values of the `ServerSideEncryption` enum, so a
/// rule naming a key beside any other algorithm is one S3 refuses rather than records.
pub const KMS_SSE_ALGORITHMS: [&str; 2] = ["aws:kms", "aws:kms:dsse"];

/// The code S3 answers a request whose value is not valid for the parameter carrying it:
/// "InvalidArgument - Invalid Argument", HTTP 400 (Amazon S3 "Error responses", list of
/// error codes).
pub const INVALID_ARGUMENT_CODE: &str = "InvalidArgument";

/// The HTTP status [`INVALID_ARGUMENT_CODE`] is rendered with: 400, from the same list.
pub const INVALID_ARGUMENT_STATUS: u16 = 400;

/// The code S3 answers a request it could not carry out at all: "InternalError - We
/// encountered an internal error. Please try again" (same list) — what a handler answers
/// when the shared store is unreachable, since every other code would be a false statement
/// about the bucket.
pub const INTERNAL_ERROR_CODE: &str = "InternalError";

/// The HTTP status [`INTERNAL_ERROR_CODE`] is rendered with: 500, from the same list.
pub const INTERNAL_ERROR_STATUS: u16 = 500;

/// How many default-encryption rules this emulator records for one bucket.
///
/// NOT a fidelity claim, for the same reason as [`MAX_OWNERSHIP_CONTROLS_RULES`]:
/// `ServerSideEncryptionRules` carries no `@length` in the model
/// (`smithy_query(query="shape",shape="ServerSideEncryptionRules")`), so nothing bounds
/// the list a request may send, and a bucket's rules live in this process's memory. A put
/// past this ceiling is refused rather than recorded.
pub const MAX_SERVER_SIDE_ENCRYPTION_RULES: usize = 10;

/// "The bucket policy does not exist."
pub const NO_SUCH_BUCKET_POLICY_CODE: &str = "NoSuchBucketPolicy";
/// See [`NO_SUCH_BUCKET_POLICY_CODE`].
pub const NO_SUCH_BUCKET_POLICY_MESSAGE: &str = "The bucket policy does not exist";
/// The code S3 answers a `PutBucketVersioning` whose `VersioningConfiguration` is not
/// one it can apply: "Indicates that the versioning configuration specified in the
/// request is invalid", HTTP 400 (Amazon S3 "Error responses", list of error codes).
pub const ILLEGAL_VERSIONING_CONFIGURATION_CODE: &str = "IllegalVersioningConfigurationException";

/// "The policy must be a valid JSON policy document."
pub const MALFORMED_POLICY_CODE: &str = "MalformedPolicy";
/// See [`MALFORMED_POLICY_CODE`].
pub const MALFORMED_POLICY_MESSAGE: &str =
    "Policy must be a JSON policy document with a non-empty Statement list, each statement \
     carrying an Effect of Allow or Deny and an Action";

/// The most bytes a bucket policy may carry, and so the most one bucket's policy record
/// can hold.
///
/// `PutBucketPolicy$Policy` is the `@httpPayload`, a plain string with no `@length` in
/// the model, so without a ceiling one request could park an arbitrarily large document
/// in the store — once per bucket, up to [`MAX_BUCKETS_PER_ACCOUNT`]. AWS bounds it with
/// the bucket-policy limit it publishes, 20 KB, and refuses a longer document.
pub const MAX_BUCKET_POLICY_BYTES: usize = 20 * 1024;

/// Whether `policy` is a policy document S3 accepts on `PutBucketPolicy`.
///
/// The grammar an IAM resource policy has to satisfy: a JSON object carrying a
/// `Statement` that is a non-empty list of objects (or a single statement object), each
/// with an `Effect` of exactly `Allow` or `Deny` and an `Action`. A `Policy` that is not
/// even JSON fails here first, which is what `MalformedPolicy` reports.
pub fn is_valid_policy_document(policy: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(object) = parsed.as_object() else {
        return false;
    };
    let Some(statement) = object.get("Statement") else {
        return false;
    };
    let statements: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Array(list) => list.iter().collect(),
        single @ ::serde_json::Value::Object(_) => vec![single],
        _ => return false,
    };
    !statements.is_empty()
        && statements.iter().all(|statement| {
            let Some(statement) = statement.as_object() else {
                return false;
            };
            let effect = statement.get("Effect").and_then(::serde_json::Value::as_str);
            matches!(effect, Some("Allow") | Some("Deny")) && statement.contains_key("Action")
        })
}

/// The Region a `CreateBucket` request places its bucket in, from the
/// `CreateBucketConfiguration.LocationConstraint` it carries (or does not).
///
/// The rule is the member's own documentation: an absent constraint means
/// [`DEFAULT_BUCKET_REGION`], the legacy value `EU` means
/// [`EU_LOCATION_CONSTRAINT_REGION`], and every other value of
/// `BucketLocationConstraint` IS the Region code.
pub fn bucket_region(location_constraint: Option<&str>) -> String {
    match location_constraint {
        None => DEFAULT_BUCKET_REGION.to_string(),
        Some(EU_LOCATION_CONSTRAINT) => EU_LOCATION_CONSTRAINT_REGION.to_string(),
        Some(region) => region.to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3Amazons3State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3Amazons3State>)
}

#[cfg(test)]
mod tests {
    //! The store side of the object loop: what a listing iterates, what a delete
    //! releases, and that a removal is reported precisely enough for `DeleteObject` to
    //! tell "no such bucket" from "no such key".

    use super::*;

    /// An object of `size` bytes, with everything else at its empty value.
    fn object(size: usize) -> Object {
        let body = vec![b'x'; size];
        Object {
            etag: object_etag(&body),
            body,
            last_modified: SystemTime::now(),
            storage_class: DEFAULT_STORAGE_CLASS.to_string(),
            content_type: None,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            expires: None,
            website_redirect_location: None,
            metadata: BTreeMap::new(),
            tags: Vec::new(),
            // The store mints the id; an object handed to `put_object` carries none.
            version_id: None,
        }
    }

    /// A notification configuration carrying exactly `targets` as its queue
    /// configurations, so a validation row names only what it is about.
    fn queue_configuration(targets: Vec<NotificationTarget>) -> BucketNotificationConfiguration {
        BucketNotificationConfiguration {
            queue_configurations: targets,
            ..BucketNotificationConfiguration::default()
        }
    }

    /// One queue configuration publishing `events` to the emulator's account.
    fn queue_target(id: Option<&str>, events: &[&str]) -> NotificationTarget {
        NotificationTarget {
            id: id.map(str::to_string),
            arn: "arn:aws:sqs:us-east-1:123456789012:object-events".to_string(),
            events: events.iter().map(|event| (*event).to_string()).collect(),
            filter_rules: None,
        }
    }

    #[test]
    fn a_notification_configuration_is_validated_against_the_published_sets() {
        // (case, the configuration, what must be wrong with it — `None` for one that is
        // applied as given). The rows cover every rule the store enforces, the three the
        // boundary validator also refuses included, since this function is what guards the
        // store whoever writes to it.
        let rows: Vec<(&str, BucketNotificationConfiguration, Option<&str>)> = vec![
            (
                "a published event with no id",
                queue_configuration(vec![queue_target(None, &["s3:ObjectCreated:*"])]),
                None,
            ),
            (
                "the last published event of the catalog",
                queue_configuration(vec![queue_target(
                    Some("last"),
                    &[EVENT_VALUES[EVENT_VALUES.len() - 1]],
                )]),
                None,
            ),
            (
                "an event outside the published catalog",
                queue_configuration(vec![queue_target(None, &["s3:ObjectCreated:Rename"])]),
                Some("not supported"),
            ),
            (
                "a configuration naming no event",
                queue_configuration(vec![queue_target(None, &[])]),
                Some("at least one event"),
            ),
            (
                "a filter rule name outside the published set",
                queue_configuration(vec![NotificationTarget {
                    filter_rules: Some(vec![NotificationFilterRule {
                        name: Some("Prefix".to_string()),
                        value: Some("logs/".to_string()),
                    }]),
                    ..queue_target(None, &["s3:ObjectCreated:Put"])
                }]),
                Some("prefix or suffix"),
            ),
            (
                "both published filter rule names",
                queue_configuration(vec![NotificationTarget {
                    filter_rules: Some(
                        FILTER_RULE_NAME_VALUES
                            .iter()
                            .map(|name| NotificationFilterRule {
                                name: Some((*name).to_string()),
                                value: Some("logs/".to_string()),
                            })
                            .collect(),
                    ),
                    ..queue_target(None, &["s3:ObjectCreated:Put"])
                }]),
                None,
            ),
            (
                "the same id twice",
                queue_configuration(vec![
                    queue_target(Some("twice"), &["s3:ObjectCreated:Put"]),
                    queue_target(Some("twice"), &["s3:ObjectCreated:Post"]),
                ]),
                Some("not unique"),
            ),
            (
                "an SQS ARN where a topic ARN belongs",
                BucketNotificationConfiguration {
                    topic_configurations: vec![queue_target(None, &["s3:ObjectCreated:Put"])],
                    ..BucketNotificationConfiguration::default()
                },
                Some("not a valid sns ARN"),
            ),
            (
                "exactly as many configurations as a bucket may carry",
                queue_configuration(
                    (0..MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET)
                        .map(|_| queue_target(None, &["s3:ObjectCreated:Put"]))
                        .collect(),
                ),
                None,
            ),
            (
                "one configuration past the bound",
                queue_configuration(
                    (0..=MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET)
                        .map(|_| queue_target(None, &["s3:ObjectCreated:Put"]))
                        .collect(),
                ),
                Some("at most"),
            ),
        ];

        for (case, configuration, rejected_for) in rows {
            let outcome = validate_notification_configuration(&configuration);
            match rejected_for {
                None => assert!(
                    outcome.is_ok(),
                    "case {case:?}: expected the configuration to be applied as given, got \
                     {outcome:?}"
                ),
                Some(expected) => {
                    let rejection = outcome
                        .expect_err(&format!("case {case:?}: the configuration must be refused"));
                    assert!(
                        rejection.to_string().contains(expected),
                        "case {case:?}: expected a refusal mentioning {expected:?}, got \
                         {rejection}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_stored_configuration_carries_an_id_the_request_named_or_one_the_store_minted() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        assert_eq!(
            store.create_bucket("notified", DEFAULT_BUCKET_REGION),
            BucketCreation::Created
        );

        // A bucket is created with no event notifications configured at all.
        assert!(
            store
                .bucket("notified")
                .expect("the bucket exists")
                .notification_configuration()
                .is_empty(),
            "a fresh bucket carries the empty notification configuration"
        );

        assert_eq!(
            store.put_notification_configuration(
                "notified",
                queue_configuration(vec![
                    queue_target(Some("named"), &["s3:ObjectCreated:Put"]),
                    queue_target(None, &["s3:ObjectCreated:Post"]),
                    queue_target(None, &["s3:ObjectCreated:Copy"]),
                ]),
            ),
            Some(()),
            "a bucket this store holds takes a notification configuration"
        );

        let ids: Vec<String> = store
            .bucket("notified")
            .expect("the bucket exists")
            .notification_configuration()
            .targets()
            .map(|(_, target)| {
                target
                    .id
                    .clone()
                    .expect("every stored configuration carries an id")
            })
            .collect();
        assert_eq!(
            ids[0], "named",
            "an id the request named is kept verbatim, got {:?}",
            ids[0]
        );
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            ids.len(),
            "every configuration carries a distinct id, got {ids:?}"
        );
        assert!(
            ids[1..].iter().all(|id| !id.is_empty()),
            "a minted id is not empty, got {ids:?}"
        );

        // A put naming no bucket this store holds records nothing.
        assert_eq!(
            store.put_notification_configuration(
                "not-a-bucket",
                queue_configuration(vec![queue_target(None, &["s3:ObjectCreated:Put"])]),
            ),
            None,
            "a put refuses a name that is not a bucket instead of creating one"
        );
        assert!(
            store.bucket("not-a-bucket").is_none(),
            "nothing was created by the refused put"
        );
    }

    #[test]
    fn a_bucket_lists_its_objects_in_ascending_key_order() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        assert_eq!(
            store.create_bucket("one", DEFAULT_BUCKET_REGION),
            BucketCreation::Created
        );

        // Written out of order, and with keys whose order differs between byte-wise and
        // case-insensitive comparison, so the assertion pins the byte order S3 lists in.
        for key in ["b/second", "a/first", "B/upper", "a/first/deeper"] {
            assert_eq!(
                store.put_object("one", key, object(3)),
                Some(ObjectWrite::Created),
                "case {key:?}: a fresh key in an existing bucket is recorded"
            );
        }

        let listed: Vec<&str> = store
            .bucket("one")
            .expect("bucket one exists")
            .objects()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            listed,
            vec!["B/upper", "a/first", "a/first/deeper", "b/second"],
            "objects iterate in ascending UTF-8 byte order, got {listed:?}"
        );
    }

    #[test]
    fn a_removal_reports_what_it_released_and_frees_the_byte_budget() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        assert_eq!(
            store.create_bucket("one", DEFAULT_BUCKET_REGION),
            BucketCreation::Created
        );
        assert_eq!(
            store.put_object("one", "kept", object(4)),
            Some(ObjectWrite::Created),
            "a fresh key is recorded"
        );
        assert_eq!(
            store.put_object("one", "doomed", object(4)),
            Some(ObjectWrite::Created),
            "a second fresh key is recorded"
        );

        assert_eq!(
            store.remove_object("one", "doomed"),
            Some(true),
            "removing a stored key reports that an object was there"
        );
        assert_eq!(
            store.remove_object("one", "doomed"),
            Some(false),
            "removing it again reports that nothing was there"
        );
        assert_eq!(
            store.remove_object("absent", "doomed"),
            None,
            "naming a bucket this store does not hold reports no bucket at all"
        );
        assert!(
            store
                .bucket("one")
                .expect("bucket one exists")
                .object("kept")
                .is_some(),
            "a delete releases only the key it named"
        );

        // The bytes the removed object retained are released with it: the per-bucket
        // budget is over what the bucket WOULD hold, so a write that fills it succeeds
        // after a delete that freed room for it. An object retains its payload AND the
        // strings kept beside it, so the room left is the budget less what "kept" holds
        // and less the empty object's own overhead.
        let overhead = object(0).retained_bytes();
        let held = object(4).retained_bytes();
        let fills_the_bucket = MAX_OBJECT_BYTES_PER_BUCKET - held - overhead;
        assert_eq!(
            store.put_object("one", "large", object(fills_the_bucket)),
            Some(ObjectWrite::Created),
            "a write that exactly fits the per-bucket budget is recorded"
        );
        assert!(
            matches!(
                store.put_object("one", "one-too-many", object(8)),
                Some(ObjectWrite::ByteBudgetReached { .. })
            ),
            "a write past the per-bucket budget is refused"
        );
        assert_eq!(
            store.remove_object("one", "large"),
            Some(true),
            "the large object was stored"
        );
        assert_eq!(
            store.put_object("one", "one-too-many", object(8)),
            Some(ObjectWrite::Created),
            "the delete released the bytes the refused write needed"
        );

        assert!(store.remove_bucket("one"), "bucket one exists");
        assert!(
            store.bucket("one").is_none(),
            "a removed bucket is no longer held"
        );
        assert!(!store.remove_bucket("one"), "bucket one is already gone");
        assert_eq!(
            store.remove_object("one", "kept"),
            None,
            "a removed bucket holds no objects to delete"
        );
    }

    #[test]
    fn a_versioned_write_keeps_what_it_supersedes_and_counts_it_against_the_budgets() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        assert_eq!(
            store.create_bucket("one", DEFAULT_BUCKET_REGION),
            BucketCreation::Created
        );

        // Unversioned first: a second write to the same key REPLACES what was there, so
        // the store holds one version of it and the first one's bytes are released.
        for round in 0..2 {
            assert!(
                store.put_object("one", "key", object(4)).is_some(),
                "round {round}: the write is recorded"
            );
        }
        let bucket = store.bucket("one").expect("bucket one exists");
        assert_eq!(
            bucket.object_versions("key").len(),
            1,
            "an unversioned key holds exactly its current object, got {}",
            bucket.object_versions("key").len()
        );
        assert_eq!(
            bucket.retained_bytes(),
            object(4).retained_bytes(),
            "the replaced version's bytes are released"
        );

        // With versioning enabled, each write keeps the version before it, and every
        // stored version counts toward what the bucket retains.
        store
            .bucket_mut("one")
            .expect("bucket one exists")
            .set_versioning(VERSIONING_ENABLED_STATUS, None);
        for round in 0..2 {
            assert!(
                store.put_object("one", "key", object(4)).is_some(),
                "versioned round {round}: the write is recorded"
            );
        }
        let bucket = store.bucket("one").expect("bucket one exists");
        let versions = bucket.object_versions("key");
        assert_eq!(
            versions.len(),
            3,
            "two versioned writes over a null version leave three stored versions, got {}",
            versions.len()
        );
        let ids: Vec<Option<&str>> = versions
            .iter()
            .map(|held| held.version_id.as_deref())
            .collect();
        assert_eq!(
            ids.iter().filter(|id| id.is_some()).count(),
            2,
            "only the two versioned writes carry an id, got {ids:?}"
        );
        assert!(
            ids[0] != ids[1],
            "every minted id differs, got {ids:?}"
        );
        assert!(
            bucket.retained_bytes() > object(4).retained_bytes() * 2,
            "every stored version is counted, got {} retained bytes",
            bucket.retained_bytes()
        );

        // Each stored version is reachable by its id, and the null version by `null`.
        for id in ids.iter().flatten() {
            assert!(
                bucket.object_version("key", id).is_some(),
                "the version {id:?} is addressable by its id"
            );
        }
        assert!(
            bucket.object_version("key", NULL_VERSION_ID).is_some(),
            "the version written before versioning was enabled is the null version"
        );
        assert!(
            bucket.object_version("key", "no-such-version").is_none(),
            "an id no version carries addresses nothing"
        );

        // Only a delete that NAMES a version releases it; the current one it displaces
        // stays addressable.
        let displaced = ids[0].expect("the newest version carries an id").to_string();
        assert_eq!(
            store.remove_object("one", "key"),
            Some(true),
            "the key held a current object"
        );
        let bucket = store.bucket("one").expect("bucket one exists");
        assert!(
            bucket.object("key").is_none(),
            "the delete took the key out of the current view"
        );
        assert!(
            bucket.object_version("key", &displaced).is_some(),
            "the displaced version is still addressable by its id"
        );
        let held_before = bucket.retained_bytes();
        assert_eq!(
            store.remove_object_version("one", "key", &displaced),
            Some(true),
            "naming the version removes it"
        );
        assert_eq!(
            store.remove_object_version("one", "key", &displaced),
            Some(false),
            "a version removed once is no longer there"
        );
        assert_eq!(
            store.remove_object_version("absent", "key", &displaced),
            None,
            "naming a bucket this store does not hold reports no bucket at all"
        );
        let bucket = store.bucket("one").expect("bucket one exists");
        assert!(
            bucket.retained_bytes() < held_before,
            "removing a version releases its bytes, still holding {} of {held_before}",
            bucket.retained_bytes()
        );
        assert_eq!(
            bucket.object_versions("key").len(),
            2,
            "the two versions the delete did not name are still stored, got {}",
            bucket.object_versions("key").len()
        );
    }
}
