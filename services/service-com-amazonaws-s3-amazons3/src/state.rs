// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3-amazons3 service.
//!
//! The bucket map is the only place a bucket or an object exists: CreateBucket /
//! HeadBucket answer from a bucket's own record, and the object-storage loop
//! (PutObject / GetObject / ListObjectsV2 / DeleteObject / DeleteBucket) computes
//! every response from the objects stored inside it.

use std::collections::BTreeMap;
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

/// How many tags one bucket's tag set may carry.
///
/// The Amazon S3 User Guide's bucket tagging restrictions ("Using cost allocation S3
/// bucket tags", the page `PutBucketTagging`'s own `InvalidTag` documentation points at)
/// publish three limits on a bucket tag set: at most 50 tags per bucket, a key of at most
/// 128 Unicode characters and a value of at most 256. A tag set past any of them is
/// refused rather than stored, which is what bounds what one bucket entry can hold.
pub const MAX_TAGS_PER_BUCKET: usize = 50;

/// The longest tag key a bucket tag set may carry, in Unicode characters.
///
/// Same source as [`MAX_TAGS_PER_BUCKET`]. The model constrains the other end of the key
/// — `Tag$Key` targets `ObjectKey`, `length {min 1}`
/// (`smithy_query(query="shape",shape="Tag")`) — so an empty key is already refused at
/// the boundary, before any handler runs.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value a bucket tag set may carry, in Unicode characters.
///
/// Same source as [`MAX_TAGS_PER_BUCKET`]. `Tag$Value` targets the unconstrained `Value`
/// string, so nothing refuses an over-long value before this check.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// One tag on a bucket: the key/value pair `Tag` carries on the wire.
///
/// Held separately from the modelled `Tag` shape so this store owns no wire type, the way
/// [`Bucket`] holds the Region as a plain `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketTag {
    /// The tag's key — unique within a bucket's tag set.
    pub key: String,
    /// The tag's value.
    pub value: String,
}

/// Why a tag set cannot be stored on a bucket.
///
/// Each variant is one of the published restrictions on a bucket tag set, or the
/// key-uniqueness the set itself has. AWS answers every one of them with `InvalidTag`,
/// which this service's model does not declare as a shape
/// (`smithy_query(query="operation",operation="PutBucketTagging")` reports `errors: []`),
/// so the caller renders it through the framework's refusal channel — never as a success
/// and never as a 501.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagSetRejection {
    /// The tag set carries more than [`MAX_TAGS_PER_BUCKET`] tags.
    TooManyTags { count: usize },
    /// A key is longer than [`MAX_TAG_KEY_CHARS`] Unicode characters.
    KeyTooLong { chars: usize },
    /// A value is longer than [`MAX_TAG_VALUE_CHARS`] Unicode characters.
    ValueTooLong { key: String, chars: usize },
    /// Two tags in one set name the same key, so the set is not a key/value mapping.
    DuplicateKey { key: String },
}

impl std::fmt::Display for TagSetRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyTags { count } => write!(
                f,
                "Tagging.TagSet carries {count} tags, more than the {MAX_TAGS_PER_BUCKET} \
                 tags a bucket may hold"
            ),
            Self::KeyTooLong { chars } => write!(
                f,
                "Tagging.TagSet has a Tag.Key of {chars} characters, more than the \
                 {MAX_TAG_KEY_CHARS} characters a tag key may hold"
            ),
            Self::ValueTooLong { key, chars } => write!(
                f,
                "Tagging.TagSet has a Tag.Value of {chars} characters for key {key:?}, \
                 more than the {MAX_TAG_VALUE_CHARS} characters a tag value may hold"
            ),
            Self::DuplicateKey { key } => write!(
                f,
                "Tagging.TagSet names the key {key:?} twice; a bucket tag set holds one \
                 value per key"
            ),
        }
    }
}

/// Whether `tags` may be stored as a bucket's tag set, reporting the first restriction it
/// breaks.
///
/// The count is checked before the per-tag rules so an enormous set is refused for what it
/// is rather than for whichever of its tags happens to come first.
pub fn validate_tag_set(tags: &[BucketTag]) -> Result<(), TagSetRejection> {
    if tags.len() > MAX_TAGS_PER_BUCKET {
        return Err(TagSetRejection::TooManyTags { count: tags.len() });
    }
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for tag in tags {
        let key_chars = tag.key.chars().count();
        if key_chars > MAX_TAG_KEY_CHARS {
            return Err(TagSetRejection::KeyTooLong { chars: key_chars });
        }
        let value_chars = tag.value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return Err(TagSetRejection::ValueTooLong {
                key: tag.key.clone(),
                chars: value_chars,
            });
        }
        if !seen.insert(tag.key.as_str()) {
            return Err(TagSetRejection::DuplicateKey {
                key: tag.key.clone(),
            });
        }
    }
    Ok(())
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
    /// The bucket's tag set, in the order the tagging request carried it.
    ///
    /// Private, and empty when no tag set is associated with the bucket: "no tags" and
    /// "an empty tag set" are the same state on AWS, which is why `PutBucketTagging` with
    /// an empty `TagSet` and `DeleteBucketTagging` leave a bucket in the same place.
    tags: Vec<BucketTag>,
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
    /// The objects the bucket holds, by key, as `PutObject` last recorded each.
    ///
    /// Private for the same reason as `tags`, and sorted by key because that is the order
    /// S3 lists objects in ("returns some or all of the objects in a bucket … in
    /// lexicographical order").
    objects: BTreeMap<String, Object>,
}

impl Bucket {
    /// When the bucket was created, as `create_bucket` recorded it. Stable across reads:
    /// nothing updates it. `None` only for a `Default`-built value, which this store
    /// never holds — a reader reports no creation date rather than inventing one.
    pub fn created_at(&self) -> Option<SystemTime> {
        self.created_at
    }

    /// The bucket's tag set, empty when none is associated with it.
    pub fn tags(&self) -> &[BucketTag] {
        &self.tags
    }

    /// Replace the whole tag set with `tags`.
    ///
    /// A replacement, not a merge: "When this operation sets the tags for a bucket, it
    /// will overwrite any current tags the bucket already has. You cannot use this
    /// operation to add tags to an existing list of tags" (`PutBucketTagging`'s own
    /// documentation).
    pub fn replace_tags(&mut self, tags: Vec<BucketTag>) {
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

    /// The object stored at `key`, or `None` when the bucket holds no such key.
    pub fn object(&self, key: &str) -> Option<&Object> {
        self.objects.get(key)
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
    /// PRIVATE, so a delete goes through
    /// [`ComAmazonawsS3Amazons3State::remove_object`] and cannot name a bucket that does
    /// not exist.
    fn remove_object(&mut self, key: &str) -> bool {
        self.objects.remove(key).is_some()
    }

    /// Store `object` at `key`, replacing any object already there, and report what that
    /// did.
    ///
    /// A replacement, not a merge: "If it receives multiple write requests for the same
    /// object simultaneously, it overwrites all but the last object written", and "You
    /// cannot use PutObject to only update a single piece of metadata for an existing
    /// object. You must put the entire object with updated metadata" (`PutObject`'s own
    /// documentation).
    ///
    /// PRIVATE, and reached only through
    /// [`ComAmazonawsS3Amazons3State::put_object`]: the per-bucket ceilings checked here
    /// cannot see the service-wide ones, so a handler calling this directly would bypass
    /// them. NOTHING is written unless the answer is [`ObjectWrite::Created`] or
    /// [`ObjectWrite::Replaced`]: an over-long key, metadata past
    /// [`MAX_OBJECT_METADATA_BYTES`], a bucket at [`MAX_OBJECTS_PER_BUCKET`] objects and a
    /// write past [`MAX_OBJECT_BYTES_PER_BUCKET`] retained bytes are each refused here.
    /// Replacing an object releases everything it retained, so the budget is over what the
    /// bucket WOULD hold, not over everything ever written.
    fn put_object(&mut self, key: &str, object: Object) -> ObjectWrite {
        if key.len() > MAX_OBJECT_KEY_BYTES {
            return ObjectWrite::KeyTooLong { bytes: key.len() };
        }
        let metadata_bytes = object.metadata_bytes();
        if metadata_bytes > MAX_OBJECT_METADATA_BYTES {
            return ObjectWrite::MetadataTooLarge {
                bytes: metadata_bytes,
            };
        }
        let replacing = self.objects.contains_key(key);
        if !replacing && self.objects.len() >= MAX_OBJECTS_PER_BUCKET {
            return ObjectWrite::ObjectCountReached;
        }
        let would_hold = self.retained_bytes_excluding(key) + object.retained_bytes();
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return ObjectWrite::ByteBudgetReached { would_hold };
        }
        self.objects.insert(key.to_string(), object);
        if replacing {
            ObjectWrite::Replaced
        } else {
            ObjectWrite::Created
        }
    }

    /// How many bytes this bucket's objects retain — payloads AND every string kept
    /// beside them, so no part of a stored object escapes the budgets.
    fn retained_bytes(&self) -> usize {
        self.objects
            .values()
            .map(Object::retained_bytes)
            .sum()
    }

    /// How many bytes this bucket's objects retain at keys OTHER than `key` — what it
    /// would still hold if the object at `key` were replaced.
    fn retained_bytes_excluding(&self, key: &str) -> usize {
        self.objects
            .iter()
            .filter(|(held_key, _)| held_key.as_str() != key)
            .map(|(_, held)| held.retained_bytes())
            .sum()
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
}

impl Object {
    /// How many bytes this stored object retains: its payload AND every string kept
    /// beside it (its entity tag, storage class, the response headers it carries and its
    /// user-defined metadata).
    ///
    /// This, not the payload alone, is what the per-bucket and service-wide budgets are
    /// computed from: a one-byte object with large metadata or a large
    /// `Content-Disposition` retains just as much of this process's memory as a large
    /// payload does, so none of it may escape the ceilings.
    pub fn retained_bytes(&self) -> usize {
        self.body.len()
            + self.etag.len()
            + self.storage_class.len()
            + self.metadata_bytes()
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
    /// This is the ONLY way an object is recorded, because it is the only place that can
    /// see BOTH the per-bucket ceilings and the service-wide ones: the per-bucket pair
    /// alone would leave the store bounded only by their product with
    /// [`MAX_BUCKETS_PER_ACCOUNT`], so the whole-store checks here ([`MAX_OBJECTS_TOTAL`]
    /// and [`MAX_OBJECT_BYTES_TOTAL`]) are what bound this process's object memory
    /// whatever a caller does. Nothing is written unless the answer is
    /// [`ObjectWrite::Created`] or [`ObjectWrite::Replaced`]; a refusal leaves every
    /// stored object exactly as it was.
    pub fn put_object(&mut self, bucket: &str, key: &str, object: Object) -> Option<ObjectWrite> {
        if !self.buckets.contains_key(bucket) {
            return None;
        }

        // What the whole store would hold after this write: every other bucket's bytes,
        // plus what this bucket would hold with the object at `key` replaced. The counts
        // work the same way — a key the bucket already holds adds no entry.
        let replacing = self
            .buckets
            .get(bucket)
            .is_some_and(|held| held.object(key).is_some());
        let held_objects: usize = self
            .buckets
            .values()
            .map(|held| held.objects.len())
            .sum::<usize>();
        if !replacing && held_objects >= MAX_OBJECTS_TOTAL {
            return Some(ObjectWrite::TotalObjectCountReached);
        }
        let would_hold: usize = self
            .buckets
            .iter()
            .map(|(name, held)| {
                if name == bucket {
                    held.retained_bytes_excluding(key)
                } else {
                    held.retained_bytes()
                }
            })
            .sum::<usize>()
            + object.retained_bytes();
        if would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectWrite::TotalByteBudgetReached { would_hold });
        }

        // Then the per-bucket ceilings, which also perform the write.
        self.buckets
            .get_mut(bucket)
            .map(|held| held.put_object(key, object))
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
                // A fresh bucket carries no policy, no tags and no ownership controls, and
                // holds no objects; `PutBucketPolicy`, `PutBucketTagging` and
                // `PutBucketOwnershipControls` are what associate those subresources with
                // it and `PutObject` what puts an object in it.
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
        }
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
}
