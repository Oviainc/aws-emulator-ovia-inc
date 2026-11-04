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
//! the store minted for it ([`mint_version_id`]). A delete that names no version adds a
//! version of its own — a [`DeleteMarker`], which "becomes the current version of the
//! object" and takes nothing away — so a key's versions are one chain of
//! [`StoredVersion`]s, newest first. Every stored version counts against the same ceilings
//! a current object does, and every one of them has to be deleted before the bucket can
//! be ([`Bucket::holds_no_versions`]).
//!
//! A bucket also holds the multipart uploads it has IN PROGRESS ([`MultipartUpload`]),
//! each addressable by the upload id the store minted for it ([`mint_upload_id`]) until a
//! completion or an abort takes it away. An in-progress upload is not an object: it is
//! where everything the initiating `CreateMultipartUpload` settled about the object it
//! will become is kept in the meantime, together with the parts uploaded into it so far
//! ([`UploadedPart`]), each addressable by its own part number.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use md5::{Digest, Md5};

// The bucket a request ADDRESSES, which is what a bucket-scoped refusal that did not name
// a bucket itself is named by — see `state/addressed_bucket.rs`.
pub(crate) mod addressed_bucket;
pub mod aws_published;
// What a bucket's stored notification configuration DOES when an object changes — which
// configured destinations are notified, what each notification carries, and the
// in-process delivery itself — lives in `state/notifications.rs`.
pub mod notifications;
// The request-correlation pair one response carries, which is also what a notification
// names the call it came of by — see `state/request_ids.rs`.
pub mod request_ids;

// What a bucket's stored replication configuration DOES when an object version is written
// or a delete marker is laid — which rules cover a key, where each one copies it, and what
// the source version then reports — lives in `state/replication.rs`.
pub mod replication;

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
/// recorded, so the buckets of the account this emulator keeps are bounded whatever a
/// caller does. It counts the ACCOUNT's buckets: a bucket another emulated service
/// provisioned as the object store of one of its own resources
/// ([`Bucket::provisioned_by`]) is bounded by that service's own limit on the resource it
/// belongs to.
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

/// The `ObjectAttributes` value naming an object's entity tag, as the
/// `x-amz-object-attributes` header spells it.
///
/// The five root-level fields `GetObjectAttributes` reports are each named by one value of
/// that enum (`smithy_query(query="shape",shape="ObjectAttributes")` publishes `[ETag,
/// Checksum, ObjectParts, StorageClass, ObjectSize]`); these constants are what the handler
/// matches the header's values against, so the spelling lives in one place.
pub const OBJECT_ATTRIBUTE_ETAG: &str = "ETag";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the object's size in bytes.
pub const OBJECT_ATTRIBUTE_OBJECT_SIZE: &str = "ObjectSize";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the object's storage class.
pub const OBJECT_ATTRIBUTE_STORAGE_CLASS: &str = "StorageClass";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the object's checksum container.
pub const OBJECT_ATTRIBUTE_CHECKSUM: &str = "Checksum";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the collection of parts a multipart
/// upload assembled the object from.
pub const OBJECT_ATTRIBUTE_OBJECT_PARTS: &str = "ObjectParts";

/// The `StorageClass` values whose objects are ARCHIVED: not readable, not copyable and
/// not usable until a temporary copy of them has been restored.
///
/// "Before using an object as a source object for the copy operation, you must restore a
/// copy of it if … the storage class of the source object is `GLACIER` or `DEEP_ARCHIVE`"
/// (`smithy_query(query="doc",shape="CopyObjectRequest",member="StorageClass")`), and the
/// same two are the storage classes `InvalidObjectState` names — "the S3 Glacier Flexible
/// Retrieval storage class, the S3 Glacier Deep Archive storage class, the S3
/// Intelligent-Tiering Archive Access tier, or the S3 Intelligent-Tiering Deep Archive
/// Access tier" (`smithy_query(query="shape",shape="InvalidObjectState")`), whose other two
/// entries are TIERS of `INTELLIGENT_TIERING` rather than storage classes and are not
/// recorded here. Both are values of the `StorageClass` enum
/// (`smithy_query(query="shape",shape="StorageClass")`).
///
/// ONE list, because the operations that judge an object by it — `CopyObject`'s source and
/// `RestoreObject`'s target — must not disagree about which objects are archived.
pub const ARCHIVED_STORAGE_CLASSES: [&str; 2] = ["GLACIER", "DEEP_ARCHIVE"];

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
/// NOT a fidelity claim, in either direction: AWS bounds what one bucket holds not at all,
/// and the only size constraints it enforces are on a SINGLE payload — 5 GiB per
/// `PutObject`, and 5 MiB to 5 GiB per part of a multipart upload. This is the emulator's
/// own budget, because an object's bytes are kept verbatim in this process's memory, and it
/// is therefore set ABOVE the traffic AWS accepts rather than below it: an SDK or the CLI
/// uploading a file of tens of MiB in 8 MiB parts, several such files into one bucket, and
/// the parts of an upload in flight beside the objects already stored. A write past it is
/// refused rather than recorded — a refusal that is the emulator's own and says so, never a
/// quota the API announces.
pub const MAX_OBJECT_BYTES_PER_BUCKET: usize = 64 * 1024 * 1024;

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
///
/// Sized above [`MAX_OBJECT_BYTES_PER_BUCKET`] for the same reason that one is sized above
/// the traffic AWS accepts: several buckets each holding the multipart uploads of an
/// ordinary session is traffic S3 serves, so it is traffic this store records.
pub const MAX_OBJECT_BYTES_TOTAL: usize = 128 * 1024 * 1024;

/// How many sessions `CreateSession` keeps vended at once.
///
/// Each session is a credential this process accepts for signing until it expires, so the
/// count is what bounds that memory whatever a caller does — a vend past this is refused
/// rather than recorded, and an expired one is released
/// ([`ComAmazonawsS3Amazons3State::release_expired_sessions`]) before the bound is read, so
/// a caller that keeps a reasonable number of live sessions never meets it. Not a fidelity
/// claim: AWS publishes no limit on concurrent sessions, and the same order as the peer
/// credential budget in this workspace (s3control's `DATA_ACCESS_CREDENTIAL_QUOTA`).
pub const MAX_VENDED_SESSIONS: usize = 256;

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

/// The `BucketVersioningStatus` value of a bucket whose versioning was turned off again.
///
/// "Suspended—Disables versioning for the objects in the bucket. All objects added to the
/// bucket receive the version ID null" (`PutBucketVersioning`'s documentation). A
/// suspended bucket KEEPS every version it already holds — suspension is a state of the
/// bucket, not of its versions — which is what makes it different from a bucket that was
/// never versioned at all.
pub const VERSIONING_SUSPENDED_STATUS: &str = "Suspended";

/// Who pays for downloads from a bucket whose request payment configuration has never
/// been set.
///
/// "By default, the bucket owner pays for downloads from the bucket"
/// (`PutBucketRequestPayment`'s own documentation), and `BucketOwner` is one of the two
/// values `Payer` takes (`PAYER_VALUES`) — so a never-configured bucket reports this
/// rather than reporting no payer at all.
pub const DEFAULT_PAYER: &str = "BucketOwner";

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

/// What a `DeleteObject` that named NO `versionId` did
/// ([`ComAmazonawsS3Amazons3State::remove_object`]), so the caller answers each outcome
/// without re-deriving it from the bucket's versioning state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectDeletion {
    /// The bucket holds no versioning state, and the `null` version at that key was
    /// removed; everything it retained is released.
    Removed,
    /// The bucket holds no versioning state and held nothing at that key, so nothing was
    /// written — "if there isn't a null version, Amazon S3 does not remove anything"
    /// (`DeleteObject`'s documentation).
    NothingThere,
    /// The bucket carries a versioning state, so a DELETE MARKER is now the key's current
    /// version, under `version_id` (`None` for the `null` marker of a suspended bucket).
    /// Every version behind it is still stored. `displaced_object` is whether the key had
    /// a current object for the marker to hide, which is what `DeleteObject`'s
    /// `x-amz-delete-marker` reports.
    MarkerLaid {
        version_id: Option<String>,
        displaced_object: bool,
    },
    /// The bucket is at [`MAX_OBJECTS_PER_BUCKET`] stored versions, so the delete marker
    /// this delete would lay was refused rather than recorded; nothing was written.
    VersionCountReached,
    /// This emulator is at [`MAX_OBJECTS_TOTAL`] stored versions across every bucket, so
    /// the delete marker was refused rather than recorded; nothing was written.
    TotalVersionCountReached,
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

/// What [`ComAmazonawsS3Amazons3State::replace_object_retention`] did, so the caller
/// answers each outcome without re-deriving it — and cannot record a retention without
/// having gone through the byte budgets a stored object's members count against, per
/// bucket AND service-wide. The shape [`ObjectTagWrite`] has, for the same reason: a
/// retention is one more string kept beside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRetentionWrite {
    /// The addressed object version's retention is now the one that was handed in (or
    /// none, when the call removed it).
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the retention would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the retention would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_encryption`] did, so the caller
/// answers each outcome without re-deriving it — and cannot record an encryption
/// configuration without having gone through the byte budgets a stored object's members
/// count against, per bucket AND service-wide. The shape [`ObjectRetentionWrite`] has, for
/// the same reason: an encryption configuration is a handful of caller-supplied strings
/// kept beside an object ([`ObjectEncryption::retained_bytes`] counts them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectEncryptionWrite {
    /// The addressed object version's encryption configuration is now the one that was
    /// handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the configuration would take the bucket past
    /// [`MAX_OBJECT_BYTES_PER_BUCKET`] retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the configuration would take the whole store past
    /// [`MAX_OBJECT_BYTES_TOTAL`] retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_legal_hold`] did, so the caller
/// answers each outcome without re-deriving it.
///
/// Shorter than [`ObjectRetentionWrite`] by the two byte-budget outcomes, and for a
/// reason: a legal hold is one `ObjectLockLegalHoldStatus` value — whether the hold is on
/// or off — so what the store keeps for it is a flag rather than a caller-supplied
/// string, and it cannot take the bucket or the whole store past a byte budget the way a
/// retention's mode can ([`Object::retained_bytes`] counts no bytes for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectLegalHoldWrite {
    /// The addressed object version's legal hold status is now the one that was handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_acl`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record an ACL without having gone
/// through the byte budgets a stored object's members count against, per bucket AND
/// service-wide. The shape [`ObjectRetentionWrite`] has, for the same reason: an ACL is a
/// list of caller-supplied strings kept beside an object ([`acl_grants_bytes`] measures
/// it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectAclWrite {
    /// The addressed object version's ACL is now the one that was handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the ACL would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`] retained
    /// bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the ACL would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`] retained
    /// bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::restore_object_version`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record a restore without having gone
/// through the byte budgets a stored object's members count against, per bucket AND
/// service-wide. The shape [`ObjectRetentionWrite`] has, for the same reason: a restore is
/// one more record kept beside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRestoreWrite {
    /// The addressed object version now carries the restore that was handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the restore would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the restore would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::put_object_annotation`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record an annotation without having
/// gone through the per-object annotation ceiling and the byte budgets a stored object's
/// members count against, per bucket AND service-wide. The shape [`ObjectRestoreWrite`]
/// has, for the same reason: an annotation is one more payload kept beside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationWrite {
    /// The addressed object version now carries the annotation that was handed in, under
    /// the name it was handed in with.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// The object already carries [`MAX_ANNOTATIONS_PER_OBJECT`] annotations and this call
    /// names a new one; nothing was written.
    LimitReached,
    /// Keeping the annotation would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the annotation would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::remove_object_annotation`] did, so the caller
/// answers each outcome without re-deriving it — the removal counterpart of
/// [`AnnotationWrite`], and without its ceilings: taking an annotation away only releases
/// bytes, so no budget can refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationRemoval {
    /// The addressed object version carried that name and no longer does; the payload and
    /// everything it retained are gone.
    Removed,
    /// The bucket exists and holds the addressed object version, but that version carries
    /// no annotation under the name; nothing was removed.
    NoSuchAnnotation,
    /// The bucket exists but holds no such object version; nothing was removed.
    NoSuchObject,
}

/// How many annotations one object may carry.
///
/// From `PutObjectAnnotation`'s own documentation: "Each object can have up to 1,000
/// annotations." A write that would take an object past it is refused rather than
/// recorded.
pub const MAX_ANNOTATIONS_PER_OBJECT: usize = 1_000;

/// The longest an annotation name may be, in BYTES.
///
/// "Length Constraints: Minimum length of 1. Maximum length of 512 bytes"
/// (`PutObjectAnnotationRequest$AnnotationName`), which is the bound the
/// `AnnotationNameTooLong` error is answered past (the sentence that error carries states
/// the rule rather than this number — see `crate::scaffolded::DOCUMENTED_ERROR_MESSAGES`).
/// `AnnotationName` carries no `length` trait in the model, so nothing refuses an
/// over-long name before a handler does.
pub const MAX_ANNOTATION_NAME_BYTES: usize = 512;

/// The smallest an annotation payload may be, in bytes, and the largest: "Must be between
/// 1 byte and 1 MiB in size" (`PutObjectAnnotationRequest$AnnotationPayload`).
pub const MIN_ANNOTATION_PAYLOAD_BYTES: usize = 1;
/// See [`MIN_ANNOTATION_PAYLOAD_BYTES`].
pub const MAX_ANNOTATION_PAYLOAD_BYTES: usize = 1024 * 1024;

/// The name prefixes S3 reserves for itself, which no caller's annotation name may start
/// with: "Annotation names are UTF-8 encoded and cannot start with `aws` or `s3`
/// (case-insensitive)" (`DeleteObjectAnnotationRequest$AnnotationName`).
pub const RESERVED_ANNOTATION_NAME_PREFIXES: [&str; 2] = ["aws", "s3"];

/// Why an annotation name is not one this service accepts — see
/// [`annotation_name_rejection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationNameRejection {
    /// The name is longer than [`MAX_ANNOTATION_NAME_BYTES`] bytes.
    TooLong { bytes: usize },
    /// The name carries no character at all, under the documented minimum length of 1.
    Empty,
    /// The name starts with one of the [`RESERVED_ANNOTATION_NAME_PREFIXES`].
    ReservedPrefix { prefix: &'static str },
}

/// Why `name` is not an annotation name this service accepts, or `None` when it is one.
///
/// ONE place, so a name is valid or invalid wherever it arrives: the two bounds its own
/// member documents (1 to [`MAX_ANNOTATION_NAME_BYTES`] bytes) and the prefixes S3 keeps
/// for itself ([`RESERVED_ANNOTATION_NAME_PREFIXES`]). The length is measured in BYTES
/// because that is the unit both the member and the `AnnotationNameTooLong` error state it
/// in, which differs from the character count for any name outside ASCII.
pub fn annotation_name_rejection(name: &str) -> Option<AnnotationNameRejection> {
    if name.len() > MAX_ANNOTATION_NAME_BYTES {
        return Some(AnnotationNameRejection::TooLong { bytes: name.len() });
    }
    if name.is_empty() {
        return Some(AnnotationNameRejection::Empty);
    }
    RESERVED_ANNOTATION_NAME_PREFIXES
        .into_iter()
        .find(|reserved| {
            name.as_bytes()
                .get(..reserved.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(reserved.as_bytes()))
        })
        .map(|prefix| AnnotationNameRejection::ReservedPrefix { prefix })
}

/// How many in-progress multipart uploads one bucket in this emulator keeps.
///
/// NOT a fidelity claim: S3 publishes no limit on concurrent multipart uploads of a
/// bucket, and the model bounds nothing either (`MultipartUploadId` is a plain string,
/// `smithy_query(query="shape",shape="MultipartUploadId")`). It is the emulator's own
/// ceiling, for the same reason as [`MAX_OBJECTS_PER_BUCKET`]: every upload this store
/// records lives in this process's memory, so a caller looping on
/// `CreateMultipartUpload` must not grow it without bound. An initiation past it is
/// refused rather than recorded.
pub const MAX_MULTIPART_UPLOADS_PER_BUCKET: usize = 1_000;

/// How many in-progress multipart uploads this emulator keeps ACROSS every bucket.
///
/// Same reason as [`MAX_OBJECTS_TOTAL`]: the per-bucket ceiling alone would leave the
/// total bounded only by its product with [`MAX_BUCKETS_PER_ACCOUNT`], so this is what
/// bounds the uploads this process holds whatever a caller does.
pub const MAX_MULTIPART_UPLOADS_TOTAL: usize = 10_000;

/// What [`ComAmazonawsS3Amazons3State::create_multipart_upload`] did, so the caller
/// answers each outcome without re-deriving it — and cannot record an upload without
/// having gone through the key, metadata and count checks, per bucket AND service-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultipartUploadCreation {
    /// The upload was recorded, and is addressed by this id from now on.
    Initiated { upload_id: String },
    /// The key is longer than [`MAX_OBJECT_KEY_BYTES`]; nothing was written.
    KeyTooLong { bytes: usize },
    /// The request's user-defined metadata is larger than
    /// [`MAX_OBJECT_METADATA_BYTES`]; nothing was written.
    MetadataTooLarge { bytes: usize },
    /// The bucket already holds [`MAX_MULTIPART_UPLOADS_PER_BUCKET`] in-progress
    /// uploads; nothing was written.
    UploadCountReached,
    /// This emulator already holds [`MAX_MULTIPART_UPLOADS_TOTAL`] in-progress uploads
    /// across every bucket; nothing was written.
    TotalUploadCountReached,
}

/// The lowest part number a part of a multipart upload may carry.
///
/// From `UploadPartRequest$PartNumber`'s own documentation: "Part number of part being
/// uploaded. This is a positive integer between 1 and 10,000." The model constrains the
/// member not at all (`PartNumber` is a plain integer,
/// `smithy_query(query="shape",shape="PartNumber")`), so nothing refuses a number outside
/// the published range before a handler does.
pub const MIN_PART_NUMBER: i32 = 1;

/// The highest part number a part of a multipart upload may carry — the other end of the
/// range [`MIN_PART_NUMBER`] opens, and so also the most parts one upload can hold, since
/// "a part number uniquely identifies a part" (`UploadPart`'s own documentation).
pub const MAX_PART_NUMBER: i32 = 10_000;

/// The fewest bytes a part of a multipart upload may carry, EXCEPT the last one.
///
/// The published multipart upload limit: "Part size: 5 MiB to 5 GiB. There is no minimum
/// size limit on the last part of your multipart upload" (Amazon S3 User Guide, *Amazon S3
/// multipart upload limits*). Nothing refuses a smaller part when it is UPLOADED — a part
/// is only known to be the last one once a completion names the parts to assemble — so this
/// is what a completion judges every part but the final one against.
pub const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// What [`ComAmazonawsS3Amazons3State::upload_part`] did, so the caller answers each
/// outcome without re-deriving it — and cannot record a part without having gone through
/// the byte budgets, per bucket AND service-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartUpload {
    /// The part was recorded against the upload. `replaced` is whether the upload already
    /// held a part of that number: "If you upload a new part using the same part number
    /// that was used with a previous part, the previously uploaded part is overwritten"
    /// (`UploadPart`'s own documentation).
    Stored { replaced: bool },
    /// The bucket has no such upload in progress under that id and key; nothing was
    /// written.
    NoSuchUpload,
    /// Recording the part would leave the bucket holding more than
    /// [`MAX_OBJECT_BYTES_PER_BUCKET`] bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Recording the part would leave this emulator holding more than
    /// [`MAX_OBJECT_BYTES_TOTAL`] bytes across every bucket; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::abort_multipart_upload`] did, so the caller answers
/// each outcome without re-deriving it — and cannot discard an upload without having
/// addressed it by both its id and its key, the way every other reach for an upload does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadAbort {
    /// The upload was discarded with every part it held: "After a multipart upload is
    /// aborted, no additional parts can be uploaded using that upload ID. The storage
    /// consumed by any previously uploaded parts will be freed"
    /// (`AbortMultipartUpload`'s own documentation).
    Discarded,
    /// The bucket has no such upload in progress under that id and key — never initiated,
    /// or already discarded — so nothing was removed.
    NoSuchUpload,
}

/// What [`ComAmazonawsS3Amazons3State::complete_multipart_upload`] did, so the caller
/// answers each outcome without re-deriving it — and cannot turn an upload into an object
/// without having gone through the object-write ceilings, per bucket AND service-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadCompletion {
    /// The parts became the object at the upload's key, and the upload is no longer in
    /// progress: "after you successfully complete … a multipart upload, you can no longer
    /// use the upload ID" (`ListParts`'s own documentation). The write is what the object
    /// write did — [`ObjectWrite::Created`] or [`ObjectWrite::Replaced`].
    Completed(ObjectWrite),
    /// The bucket has no such upload in progress under that id and key — never initiated,
    /// or already completed or aborted — so nothing was written.
    NoSuchUpload,
    /// The object write the completion performs was refused (an over-long key, or one of
    /// the ceilings), so nothing was written and the upload is left in progress exactly as
    /// it was, with every part it held.
    Refused(ObjectWrite),
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

/// The punctuation a tag key or value may carry, beside Unicode letters, digits and
/// whitespace.
///
/// The published tag restrictions the two tagging operations point at ("Tag restrictions",
/// Amazon S3 User Guide, and the same list in the AWS *Tagging Resources* guide): "the
/// allowed characters are letters, whitespace, and numbers representable in UTF-8, plus
/// the following special characters: + - = . _ : / @". A code point outside that set — an
/// emoji, a currency sign, a bracket — is refused rather than stored, which is what
/// [`tag_text_is_allowed`] decides.
pub const TAG_PUNCTUATION: [char; 8] = ['+', '-', '=', '.', '_', ':', '/', '@'];

/// Whether every character of `text` is one a tag key or value may carry.
///
/// ONE character class for both halves of a tag and for both tag sets S3 holds, from the
/// published restrictions quoted on [`TAG_PUNCTUATION`]: Unicode letters, Unicode digits,
/// whitespace, and that punctuation. "Letters" is the Unicode property, not ASCII, so a
/// tag written in any script is storable; a SYMBOL (an emoji is one) is not a letter and
/// is refused. Stated once, by the function that finds the offending character.
pub fn tag_text_is_allowed(text: &str) -> bool {
    first_disallowed_tag_char(text).is_none()
}

/// The first character of `text` that a tag key or value may NOT carry, or `None` when
/// every one of them is allowed.
///
/// The one place the class is spelled; [`tag_text_is_allowed`] is this answering "none".
pub fn first_disallowed_tag_char(text: &str) -> Option<char> {
    text.chars().find(|character| {
        !(character.is_alphabetic()
            || character.is_numeric()
            || character.is_whitespace()
            || TAG_PUNCTUATION.contains(character))
    })
}

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
    /// A key begins with the prefix AWS keeps for the tags it generates itself
    /// ([`RESERVED_TAG_KEY_PREFIX`]), which a caller may therefore not write.
    ReservedKeyPrefix { key: String },
    /// A key or a value carries a character outside the published class every tag set
    /// shares ([`tag_text_is_allowed`]).
    DisallowedCharacter {
        /// Which half of the tag carries it ([`TAG_KEY_FIELD`], [`TAG_VALUE_FIELD`]).
        field: &'static str,
        /// The character the published class does not allow.
        character: char,
    },
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
            // The resource's noun capitalised, and the published maximum the set was
            // judged against: both come from the limits, never from the request — the
            // count the set carried is not in the sentence.
            TagSetRejectionCause::TooManyTags { .. } => {
                let mut characters = resource.chars();
                let noun: String = match characters.next() {
                    Some(first) => first.to_uppercase().chain(characters).collect(),
                    None => String::new(),
                };
                write!(f, "{noun} tags cannot be greater than {max_tags}")
            }
            TagSetRejectionCause::EmptyKey | TagSetRejectionCause::KeyTooLong { .. } => {
                write!(f, "{INVALID_TAG_KEY_MESSAGE}")
            }
            TagSetRejectionCause::ValueTooLong { .. } => {
                write!(f, "{INVALID_TAG_VALUE_MESSAGE}")
            }
            // Which half of the tag carried the character is what decides which of the two
            // sentences S3 states; the character itself is not in either of them.
            TagSetRejectionCause::DisallowedCharacter { field, .. } => {
                let message = if *field == TAG_KEY_FIELD {
                    INVALID_TAG_KEY_MESSAGE
                } else {
                    INVALID_TAG_VALUE_MESSAGE
                };
                write!(f, "{message}")
            }
            TagSetRejectionCause::DuplicateKey { .. } => {
                write!(f, "Cannot provide multiple Tags with the same key")
            }
            // The reserved prefix is the fixed text of this sentence; the key that carried
            // it is a variable part of the ENVELOPE ([`TagSetRejection::tag_key`]) rather
            // than of the message.
            TagSetRejectionCause::ReservedKeyPrefix { .. } => {
                write!(f, "{RESERVED_TAG_KEY_PREFIX_MESSAGE}")
            }
        }
    }
}

/// Which half of a tag [`TagSetRejectionCause::DisallowedCharacter`] names, for the KEY.
pub const TAG_KEY_FIELD: &str = "Tag.Key";

/// Which half of a tag [`TagSetRejectionCause::DisallowedCharacter`] names, for the VALUE.
pub const TAG_VALUE_FIELD: &str = "Tag.Value";

/// The `<Message>` S3 sends for a tag whose KEY breaks one of the published restrictions —
/// its length or the character class — and for a tag that carries no key at all.
///
/// Fixed text: S3 states which HALF of the tag was not valid and nothing else, interpolating
/// neither the key, the bound it broke nor the character class.
const INVALID_TAG_KEY_MESSAGE: &str = "The TagKey you have provided is invalid";

/// The `<Message>` S3 sends for a tag whose VALUE breaks one of those restrictions — the
/// value half of [`INVALID_TAG_KEY_MESSAGE`], worded the same way.
const INVALID_TAG_VALUE_MESSAGE: &str = "The TagValue you have provided is invalid";

/// The prefix AWS keeps for the tags it generates itself, and which a caller's tag key may
/// therefore not begin with: "the `aws:` prefix is reserved for Amazon Web Services use",
/// and "you can't edit, delete, or add tags with the `aws:` prefix" (AWS *Tagging your
/// resources*, *Tag restrictions* — the same list both S3 tagging operations point at).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The `<Message>` S3 sends for a tag key beginning with [`RESERVED_TAG_KEY_PREFIX`].
///
/// Fixed text: S3 states the reservation and nothing else; the offending key is a member of
/// the error document rather than part of this sentence.
const RESERVED_TAG_KEY_PREFIX_MESSAGE: &str = "Your TagKey cannot be prefixed with aws:";

impl TagSetRejection {
    /// The wire error code S3 answers this rejection with.
    ///
    /// Two codes, one per class of restriction, both HTTP 400: a tag set carrying more
    /// tags than its resource may hold is refused `BadRequest` — the set is too big,
    /// whatever is in it — while a restriction one INDIVIDUAL tag breaks (its key, its
    /// value, or the set naming that key twice) is refused `InvalidTag`, "the tag provided
    /// was not a valid tag. This error can occur if the tag did not pass input validation"
    /// (`PutBucketTagging`'s own list of special errors). Neither code is a shape in this
    /// model, so a caller renders it through the unmodelled channel.
    pub fn code(&self) -> &'static str {
        match self.cause {
            TagSetRejectionCause::TooManyTags { .. } => BAD_REQUEST_CODE,
            TagSetRejectionCause::EmptyKey
            | TagSetRejectionCause::KeyTooLong { .. }
            | TagSetRejectionCause::ValueTooLong { .. }
            | TagSetRejectionCause::DuplicateKey { .. }
            | TagSetRejectionCause::ReservedKeyPrefix { .. }
            | TagSetRejectionCause::DisallowedCharacter { .. } => INVALID_TAG_CODE,
        }
    }

    /// The tag KEY this rejection's error document names, or `None` for a rejection whose
    /// document names none.
    ///
    /// S3's envelope for a key the reserved prefix refuses is
    /// `<Error><Code/><Message/><TagKey/><RequestId/><HostId/></Error>`: the code and the
    /// message are the fixed text of the class and `<TagKey>` is the variable part, copied
    /// from the request, because the message names no key and the envelope is the only
    /// place the refusal says WHICH key it is about. The caller renders it through the
    /// naming channel, so no handler decides it for itself.
    pub fn tag_key(&self) -> Option<&str> {
        match &self.cause {
            TagSetRejectionCause::ReservedKeyPrefix { key } => Some(key.as_str()),
            TagSetRejectionCause::TooManyTags { .. }
            | TagSetRejectionCause::EmptyKey
            | TagSetRejectionCause::KeyTooLong { .. }
            | TagSetRejectionCause::ValueTooLong { .. }
            | TagSetRejectionCause::DuplicateKey { .. }
            | TagSetRejectionCause::DisallowedCharacter { .. } => None,
        }
    }
}

/// The code S3 answers a tag set whose SIZE is past what the resource may hold:
/// "BadRequest", HTTP 400 (Amazon S3 "Error responses", list of error codes).
pub const BAD_REQUEST_CODE: &str = "BadRequest";

/// The code S3 answers a tag that does not pass input validation: "InvalidTag - The tag
/// provided was not a valid tag. This error can occur if the tag did not pass input
/// validation" (`PutBucketTagging`'s own list of special errors), HTTP 400.
pub const INVALID_TAG_CODE: &str = "InvalidTag";

/// The HTTP status both tag-set codes are rendered with: 400, from that same list.
pub const TAG_REJECTION_STATUS: u16 = 400;

/// Whether `tags` may be stored as the tag set of a resource with these `limits`,
/// reporting the first restriction it breaks.
///
/// ONE rule for every tag set S3 holds: the count against the resource's own published
/// maximum, then each key and value against the character bounds every tag set shares, then
/// each key against the prefix AWS keeps for the tags it generates itself
/// ([`RESERVED_TAG_KEY_PREFIX`]), and the key-uniqueness the set itself has. The count is
/// checked before the per-tag rules so an enormous set is refused for what it is rather than
/// for whichever of its tags happens to come first, and a key's own bounds are checked
/// before the reservation so the key the refusal NAMES is one within those bounds.
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
        if let Some(character) = first_disallowed_tag_char(&tag.key) {
            return refuse(TagSetRejectionCause::DisallowedCharacter {
                field: TAG_KEY_FIELD,
                character,
            });
        }
        // Judged after the key's own bounds, so the key this refusal names in the error
        // document is one those bounds admit.
        if tag.key.starts_with(RESERVED_TAG_KEY_PREFIX) {
            return refuse(TagSetRejectionCause::ReservedKeyPrefix {
                key: tag.key.clone(),
            });
        }
        let value_chars = tag.value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return refuse(TagSetRejectionCause::ValueTooLong {
                key: tag.key.clone(),
                chars: value_chars,
            });
        }
        if let Some(character) = first_disallowed_tag_char(&tag.value) {
            return refuse(TagSetRejectionCause::DisallowedCharacter {
                field: TAG_VALUE_FIELD,
                character,
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

// ---------------------------------------------------------------------------
// Access control lists.
//
// ONE type for every ACL S3 holds, the way `ResourceTag` is one tag for every tag set:
// an ACL is "a list of grants" (`AccessControlPolicy$Grants`) in a bucket's record and
// in an object's alike, so the grant, the grantee and the rules they are judged against
// live here once rather than in each handler.
// ---------------------------------------------------------------------------

/// Every value the `Permission` enum publishes: the access an ACL grant may give, and so
/// the only values a grant may name. Re-exported from the model-generated value list,
/// like [`EVENT_VALUES`].
pub use crate::ops::types::PERMISSION_VALUES;

/// Every value the `Type` enum publishes: how an ACL grantee may be named. Re-exported
/// from the model-generated value list, like [`PERMISSION_VALUES`].
pub use crate::ops::types::TYPE_VALUES;

/// Every value the `ObjectCannedACL` enum publishes: the canned ACLs an OBJECT may be
/// given, which [`object_canned_acl_grants`] expands. Re-exported from the
/// model-generated value list, like [`PERMISSION_VALUES`].
pub use crate::ops::types::OBJECT_CANNED_A_C_L_VALUES;

/// The `Grantee$Type` of a grantee named by its canonical user id — `TYPE_VALUES[1]`.
pub const GRANTEE_CANONICAL_USER: &str = "CanonicalUser";

/// The `Grantee$Type` of a grantee named by a predefined group's URI — `TYPE_VALUES[2]`.
pub const GRANTEE_GROUP: &str = "Group";

/// The `Grantee$Type` of a grantee named by the email address of an account —
/// `TYPE_VALUES[0]`.
pub const GRANTEE_BY_EMAIL: &str = "AmazonCustomerByEmail";

/// `Permission` `FULL_CONTROL`: "the read, write, read ACP, and write ACP permissions"
/// (`PutBucketAclRequest$GrantFullControl`).
pub const PERMISSION_FULL_CONTROL: &str = "FULL_CONTROL";
/// `Permission` `READ`: "Allows grantee to list the objects in the bucket"
/// (`PutBucketAclRequest$GrantRead`).
pub const PERMISSION_READ: &str = "READ";
/// `Permission` `READ_ACP`: "Allows grantee to read the bucket ACL"
/// (`PutBucketAclRequest$GrantReadACP`).
pub const PERMISSION_READ_ACP: &str = "READ_ACP";
/// `Permission` `WRITE`: "Allows grantee to create new objects in the bucket"
/// (`PutBucketAclRequest$GrantWrite`).
pub const PERMISSION_WRITE: &str = "WRITE";
/// `Permission` `WRITE_ACP`: "Allows grantee to write the ACL for the applicable bucket"
/// (`PutBucketAclRequest$GrantWriteACP`).
pub const PERMISSION_WRITE_ACP: &str = "WRITE_ACP";

/// The URI of the predefined group *All Users* — "anyone in the world", which is what
/// makes a grant to it a PUBLIC grant.
///
/// The three URIs below are the predefined groups the Amazon S3 User Guide publishes
/// ("Amazon S3 predefined groups"), and they are the only URIs an ACL grantee of type
/// `Group` may name.
pub const ALL_USERS_GROUP_URI: &str = "http://acs.amazonaws.com/groups/global/AllUsers";

/// The URI of the predefined group *Authenticated Users* — every AWS account.
pub const AUTHENTICATED_USERS_GROUP_URI: &str =
    "http://acs.amazonaws.com/groups/global/AuthenticatedUsers";

/// The URI of the predefined group *Log Delivery* — the group S3 server access logging
/// and other AWS log producers write through, so a grant to it is what lets logs land in
/// the bucket.
pub const LOG_DELIVERY_GROUP_URI: &str = "http://acs.amazonaws.com/groups/s3/LogDelivery";

/// Every predefined group URI an ACL grantee of type [`GRANTEE_GROUP`] may name. See
/// [`ALL_USERS_GROUP_URI`] for the source.
pub const PREDEFINED_GROUP_URIS: [&str; 3] = [
    ALL_USERS_GROUP_URI,
    AUTHENTICATED_USERS_GROUP_URI,
    LOG_DELIVERY_GROUP_URI,
];

/// The Regions in which S3 has discontinued Email Grantee ACLs.
///
/// From `PutBucketAcl`'s own documentation: "As of October 1, 2025, Amazon S3 has
/// discontinued support for Email Grantee Access Control Lists (ACLs). If you attempt to
/// use an Email Grantee ACL in a request after October 1, 2025, the request will receive
/// an HTTP 405 (Method Not Allowed) error. This change affects the following Amazon Web
/// Services Regions: US East (N. Virginia), US West (N. California), US West (Oregon),
/// Asia Pacific (Singapore), Asia Pacific (Sydney), Asia Pacific (Tokyo), Europe
/// (Ireland), and South America (São Paulo)" — those eight Regions by their codes.
pub const EMAIL_GRANTEE_DISCONTINUED_REGIONS: [&str; 8] = [
    "us-east-1",
    "us-west-1",
    "us-west-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-northeast-1",
    "eu-west-1",
    "sa-east-1",
];

/// How many grants one ACL may carry.
///
/// Published, unlike [`MAX_OWNERSHIP_CONTROLS_RULES`]: "An ACL can have up to 100 grants"
/// (Amazon S3 User Guide, *Access control list (ACL) overview*). Nothing in the model
/// bounds `Grants` (`smithy_query(query="shape",shape="Grants")` carries no `@length`), so
/// this is also what bounds the grants one bucket can park in this process's memory; an
/// ACL past it is refused rather than recorded.
pub const MAX_ACL_GRANTS: usize = 100;

/// Who an ACL grant gives its permission to: the `Grantee` of one `Grant`.
///
/// Which member carries the grantee depends on [`Self::grantee_type`], exactly as the
/// wire shape does (`smithy_query(query="shape",shape="Grantee")`): a `CanonicalUser` is
/// named by `ID`, a `Group` by `URI` and an `AmazonCustomerByEmail` by `EmailAddress`.
/// Held separately from the modelled `Grantee` shape so this store owns no wire type, the
/// way [`ResourceTag`] is held separately from `Tag`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclGrantee {
    /// A `Type` value ([`TYPE_VALUES`]): [`GRANTEE_CANONICAL_USER`], [`GRANTEE_GROUP`]
    /// or [`GRANTEE_BY_EMAIL`].
    pub grantee_type: String,
    /// `Grantee$ID`, the canonical user id of the grantee — present exactly when the
    /// type is [`GRANTEE_CANONICAL_USER`].
    pub id: Option<String>,
    /// `Grantee$URI`, the predefined group's URI — present exactly when the type is
    /// [`GRANTEE_GROUP`], and then one of [`PREDEFINED_GROUP_URIS`].
    pub uri: Option<String>,
    /// `Grantee$EmailAddress` — present exactly when the type is [`GRANTEE_BY_EMAIL`].
    pub email_address: Option<String>,
}

impl AclGrantee {
    /// The grantee that is the account whose canonical user id is `id`.
    pub fn canonical_user(id: impl Into<String>) -> Self {
        Self {
            grantee_type: GRANTEE_CANONICAL_USER.to_string(),
            id: Some(id.into()),
            uri: None,
            email_address: None,
        }
    }

    /// The grantee that is the predefined group named by `uri`.
    pub fn group(uri: impl Into<String>) -> Self {
        Self {
            grantee_type: GRANTEE_GROUP.to_string(),
            id: None,
            uri: Some(uri.into()),
            email_address: None,
        }
    }
}

/// One grant of an ACL: who, and what they may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclGrant {
    /// Who the permission is given to.
    pub grantee: AclGrantee,
    /// A `Permission` value ([`PERMISSION_VALUES`]).
    pub permission: String,
}

impl AclGrant {
    /// The grant giving `permission` to the account whose canonical user id is `id`.
    pub fn to_canonical_user(id: impl Into<String>, permission: &str) -> Self {
        Self {
            grantee: AclGrantee::canonical_user(id),
            permission: permission.to_string(),
        }
    }

    /// The grant giving `permission` to the predefined group named by `uri`.
    pub fn to_group(uri: &str, permission: &str) -> Self {
        Self {
            grantee: AclGrantee::group(uri),
            permission: permission.to_string(),
        }
    }
}

/// The ACL a bucket carries until a `PutBucketAcl` sets another one: the owner's
/// `FULL_CONTROL` and nothing else.
///
/// The canned ACL `private` — "Owner gets FULL_CONTROL. No one else has access rights
/// (default)" (Amazon S3 User Guide, *Canned ACL*) — which is the ACL `CreateBucket`
/// applies when its request names none (`CreateBucketRequest$ACL` defaults to nothing,
/// and S3 documents `private` as the default canned ACL). Derived on read rather than
/// written at create time, so a reader reports it without changing anything.
pub fn default_acl_grants() -> Vec<AclGrant> {
    vec![AclGrant::to_canonical_user(
        canonical_user_id(),
        PERMISSION_FULL_CONTROL,
    )]
}

/// The grants a `BucketCannedACL` value stands for, or `None` for a value outside
/// `BUCKET_CANNED_A_C_L_VALUES`.
///
/// "Amazon S3 supports a set of predefined ACLs, known as canned ACLs. Each canned ACL has
/// a predefined set of grantees and permissions" (`PutBucketAcl`'s own documentation); the
/// four expansions are the bucket rows of the published *Canned ACL* table:
///
/// * `private` — owner `FULL_CONTROL`, nobody else;
/// * `public-read` — owner `FULL_CONTROL`, `AllUsers` `READ`;
/// * `public-read-write` — owner `FULL_CONTROL`, `AllUsers` `READ` and `WRITE`;
/// * `authenticated-read` — owner `FULL_CONTROL`, `AuthenticatedUsers` `READ`;
/// * `log-delivery-write` — owner `FULL_CONTROL`, `LogDelivery` `WRITE` and `READ_ACP`.
pub fn canned_acl_grants(canned: &str) -> Option<Vec<AclGrant>> {
    let owner = AclGrant::to_canonical_user(canonical_user_id(), PERMISSION_FULL_CONTROL);
    let grants = match canned {
        "private" => vec![owner],
        "public-read" => vec![
            owner,
            AclGrant::to_group(ALL_USERS_GROUP_URI, PERMISSION_READ),
        ],
        "public-read-write" => vec![
            owner,
            AclGrant::to_group(ALL_USERS_GROUP_URI, PERMISSION_READ),
            AclGrant::to_group(ALL_USERS_GROUP_URI, PERMISSION_WRITE),
        ],
        "authenticated-read" => vec![
            owner,
            AclGrant::to_group(AUTHENTICATED_USERS_GROUP_URI, PERMISSION_READ),
        ],
        // The fifth bucket row of that same table: "log-delivery-write — the LogDelivery
        // group gets WRITE and READ_ACP permissions on the bucket", which is the ACL S3
        // server access logging requires on a target bucket.
        "log-delivery-write" => vec![
            owner,
            AclGrant::to_group(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE),
            AclGrant::to_group(LOG_DELIVERY_GROUP_URI, PERMISSION_READ_ACP),
        ],
        _ => return None,
    };
    Some(grants)
}

/// The grants an `ObjectCannedACL` value stands for, or `None` for a value outside
/// [`OBJECT_CANNED_A_C_L_VALUES`].
///
/// The OBJECT rows of the published *Canned ACL* table (Amazon S3 User Guide, *Canned
/// ACL*), as [`canned_acl_grants`] carries the bucket ones:
///
/// * `private`, `public-read`, `public-read-write` and `authenticated-read` stand for the
///   same grants on an object as on a bucket, so they come from that one table rather than
///   being restated here (`log-delivery-write` is a BUCKET-only value, which is why a
///   value the object enum does not publish is refused);
/// * `bucket-owner-read` — "Object owner gets FULL_CONTROL. Bucket owner gets READ";
/// * `bucket-owner-full-control` — "Both the object owner and the bucket owner get
///   FULL_CONTROL";
/// * `aws-exec-read` — "Owner gets FULL_CONTROL", the one grant of that row this emulator
///   can state: the READ it also gives goes to an Amazon EC2 grantee whose canonical user
///   id AWS publishes nowhere, and an id is not invented for it.
///
/// This emulator serves ONE account ([`canonical_user_id`]), so an object's owner and its
/// bucket's owner are the same grantee: `bucket-owner-full-control` names that one
/// grantee's `FULL_CONTROL` twice, and an ACL granting the same grantee the same
/// permission twice grants nothing a second time, so it stands for the single grant.
pub fn object_canned_acl_grants(canned: &str) -> Option<Vec<AclGrant>> {
    let owner = AclGrant::to_canonical_user(canonical_user_id(), PERMISSION_FULL_CONTROL);
    let grants = match canned {
        "bucket-owner-read" => vec![
            owner,
            AclGrant::to_canonical_user(canonical_user_id(), PERMISSION_READ),
        ],
        "bucket-owner-full-control" | "aws-exec-read" => vec![owner],
        shared if OBJECT_CANNED_A_C_L_VALUES.contains(&shared) => canned_acl_grants(shared)?,
        _ => return None,
    };
    Some(grants)
}

/// How many bytes an ACL retains: every grant's permission, its grantee's `Type` and
/// whichever member names that grantee, in UTF-8 bytes.
///
/// What the object byte budgets count an ACL kept beside an object as — see
/// [`Object::retained_bytes`], which counts a tag set the same way ([`tag_set_bytes`]).
pub fn acl_grants_bytes(grants: &[AclGrant]) -> usize {
    grants
        .iter()
        .map(|grant| {
            grant.permission.len()
                + grant.grantee.grantee_type.len()
                + [
                    &grant.grantee.id,
                    &grant.grantee.uri,
                    &grant.grantee.email_address,
                ]
                .iter()
                .filter_map(|member| member.as_ref())
                .map(String::len)
                .sum::<usize>()
        })
        .sum()
}

/// Why an ACL cannot be stored on a bucket or an object.
///
/// Each variant is one rule the API publishes for an ACL; the caller maps it to the wire
/// code and status S3 answers it with, the way a [`TagSetRejection`] is rendered by its
/// caller — this store owns no wire concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclRejection {
    /// The ACL carries more than [`MAX_ACL_GRANTS`] grants.
    TooManyGrants { count: usize },
    /// A grant names a `Permission` outside [`PERMISSION_VALUES`].
    UnknownPermission { permission: String },
    /// A grantee names a `Type` outside [`TYPE_VALUES`].
    UnknownGranteeType { grantee_type: String },
    /// A grantee's `Type` and its members disagree: the member that type is named by is
    /// missing or empty.
    GranteeMemberMissing {
        grantee_type: String,
        member: &'static str,
    },
    /// A `Group` grantee names a URI that is not one of [`PREDEFINED_GROUP_URIS`].
    UnknownGroupUri { uri: String },
    /// An `AmazonCustomerByEmail` grantee in a Region where S3 has discontinued Email
    /// Grantee ACLs ([`EMAIL_GRANTEE_DISCONTINUED_REGIONS`]).
    EmailGranteeDiscontinued { region: String },
    /// An `AmazonCustomerByEmail` grantee anywhere else: this emulator serves ONE account
    /// and holds no email address for it, so no email address it is given can match an
    /// account on record.
    EmailGranteeUnresolvable { email_address: String },
}

impl std::fmt::Display for AclRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyGrants { count } => write!(
                f,
                "the access control list carries {count} grants, more than the \
                 {MAX_ACL_GRANTS} grants one ACL may hold"
            ),
            Self::UnknownPermission { permission } => write!(
                f,
                "Grant.Permission must be one of {PERMISSION_VALUES:?}, got {permission:?}"
            ),
            Self::UnknownGranteeType { grantee_type } => write!(
                f,
                "Grant.Grantee.Type must be one of {TYPE_VALUES:?}, got {grantee_type:?}"
            ),
            Self::GranteeMemberMissing {
                grantee_type,
                member,
            } => write!(
                f,
                "a Grant.Grantee of type {grantee_type:?} is named by its {member}, which \
                 this grant does not carry"
            ),
            Self::UnknownGroupUri { uri } => write!(
                f,
                "Grant.Grantee.URI must be one of the predefined group URIs \
                 {PREDEFINED_GROUP_URIS:?}, got {uri:?}"
            ),
            Self::EmailGranteeDiscontinued { region } => write!(
                f,
                "Email Grantee ACLs are no longer supported in {region}; name the grantee \
                 by its canonical user ID instead"
            ),
            Self::EmailGranteeUnresolvable { email_address } => write!(
                f,
                "the email address {email_address:?} does not match any account on record"
            ),
        }
    }
}

impl AclRejection {
    /// The wire error code S3 answers this broken rule with, and the HTTP status its
    /// error-responses list carries for that code ([`Self::status`]).
    ///
    /// ONE mapping for every ACL S3 holds, so the code a client branches on is the same
    /// whether the ACL was named in a request body, in the `x-amz-grant-*` headers or as a
    /// canned ACL, and whether the resource is a bucket or an object.
    pub fn code(&self) -> &'static str {
        match self {
            // The grantee's own identity is the parameter that is not valid, which is what
            // `InvalidArgument` reports ("Invalid Argument", HTTP 400).
            Self::UnknownGroupUri { .. } => INVALID_ARGUMENT_CODE,
            // "As of October 1, 2025, Amazon S3 has discontinued support for Email Grantee
            // Access Control Lists … the request will receive an HTTP 405 (Method Not
            // Allowed) error", in the Regions that change affects.
            Self::EmailGranteeDiscontinued { .. } => METHOD_NOT_ALLOWED_CODE,
            // Everywhere else an email grantee is still taken by the API and then resolved
            // against the accounts on record, of which this emulator has exactly one and
            // no email address for it.
            Self::EmailGranteeUnresolvable { .. } => UNRESOLVABLE_GRANT_BY_EMAIL_CODE,
            // The rest are ACLs that do not validate against the schema the API publishes
            // for one: too many grants, a value outside a published set, or a grantee whose
            // `Type` and members disagree.
            Self::TooManyGrants { .. }
            | Self::UnknownPermission { .. }
            | Self::UnknownGranteeType { .. }
            | Self::GranteeMemberMissing { .. } => MALFORMED_ACL_CODE,
        }
    }

    /// The HTTP status [`Self::code`] is rendered with: 400 for every one of them except
    /// the discontinued-email refusal, whose documented status is 405.
    pub fn status(&self) -> u16 {
        match self {
            Self::EmailGranteeDiscontinued { .. } => METHOD_NOT_ALLOWED_STATUS,
            _ => ACL_REFUSAL_STATUS,
        }
    }
}

/// The HTTP status an ACL refusal is rendered with, except the discontinued-email one
/// ([`METHOD_NOT_ALLOWED_STATUS`]): 400, the status S3's error-responses list carries for
/// `MalformedACLError`, `MissingRequestBodyError`, `AccessControlListNotSupported` and
/// `UnresolvableGrantByEmailAddress` alike.
pub const ACL_REFUSAL_STATUS: u16 = 400;

/// Whether `grants` may be stored as the ACL of a resource in `region`, reporting the
/// first published rule it breaks.
///
/// ONE rule for every ACL S3 holds. The count is judged before the per-grant rules so an
/// enormous list is refused for what it is rather than for whichever grant happens to come
/// first, and each grant is then judged on its `Permission` and on whether its grantee's
/// `Type` and members agree — the agreement the wire shape leaves open, because `Grantee`
/// makes only `Type` required and every id-carrying member optional.
///
/// `region` decides only how an email grantee is refused: see
/// [`EMAIL_GRANTEE_DISCONTINUED_REGIONS`].
pub fn validate_acl_grants(grants: &[AclGrant], region: &str) -> Result<(), AclRejection> {
    if grants.len() > MAX_ACL_GRANTS {
        return Err(AclRejection::TooManyGrants {
            count: grants.len(),
        });
    }
    for grant in grants {
        if !PERMISSION_VALUES.contains(&grant.permission.as_str()) {
            return Err(AclRejection::UnknownPermission {
                permission: grant.permission.clone(),
            });
        }
        let grantee = &grant.grantee;
        let named = |member: Option<&String>| -> Option<String> {
            member
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        match grantee.grantee_type.as_str() {
            GRANTEE_CANONICAL_USER => {
                // The id is recorded as given: S3 resolves a canonical user id against
                // the account namespace, which an emulator serving one account cannot do,
                // so it refuses only an id that is not there at all.
                if named(grantee.id.as_ref()).is_none() {
                    return Err(AclRejection::GranteeMemberMissing {
                        grantee_type: grantee.grantee_type.clone(),
                        member: "ID",
                    });
                }
            }
            GRANTEE_GROUP => {
                let Some(uri) = named(grantee.uri.as_ref()) else {
                    return Err(AclRejection::GranteeMemberMissing {
                        grantee_type: grantee.grantee_type.clone(),
                        member: "URI",
                    });
                };
                if !PREDEFINED_GROUP_URIS.contains(&uri.as_str()) {
                    return Err(AclRejection::UnknownGroupUri { uri });
                }
            }
            GRANTEE_BY_EMAIL => {
                let Some(email_address) = named(grantee.email_address.as_ref()) else {
                    return Err(AclRejection::GranteeMemberMissing {
                        grantee_type: grantee.grantee_type.clone(),
                        member: "EmailAddress",
                    });
                };
                if EMAIL_GRANTEE_DISCONTINUED_REGIONS.contains(&region) {
                    return Err(AclRejection::EmailGranteeDiscontinued {
                        region: region.to_string(),
                    });
                }
                return Err(AclRejection::EmailGranteeUnresolvable { email_address });
            }
            unknown => {
                return Err(AclRejection::UnknownGranteeType {
                    grantee_type: unknown.to_string(),
                })
            }
        }
    }
    Ok(())
}

/// The grantee one `type=value` pair of an `x-amz-grant-*` header names, or `None` when
/// the pair is not one of the published forms.
///
/// "You specify each grantee as a type=value pair, where the type is one of the following:
/// id – if the value specified is the canonical user ID of an Amazon Web Services account[;]
/// uri – if you are granting permissions to a predefined group[;] emailAddress – if the
/// value specified is the email address of an [account]" (`PutBucketAcl`'s own
/// documentation). The value may be quoted — the documented examples spell it
/// `emailAddress="xyz@amazon.com"` — so one pair of surrounding double quotes is stripped,
/// and the type is matched without regard to case, the way an HTTP header's token is read.
pub fn acl_grantee_from_pair(pair: &str) -> Option<AclGrantee> {
    let (grantee_type, value) = pair.split_once('=')?;
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
        .trim();
    if value.is_empty() {
        return None;
    }
    match grantee_type.trim().to_ascii_lowercase().as_str() {
        "id" => Some(AclGrantee::canonical_user(value)),
        "uri" => Some(AclGrantee::group(value)),
        "emailaddress" => Some(AclGrantee {
            grantee_type: GRANTEE_BY_EMAIL.to_string(),
            id: None,
            uri: None,
            email_address: Some(value.to_string()),
        }),
        _ => None,
    }
}

/// The grants one `x-amz-grant-*` header names, or the pair that is not a grantee.
///
/// The header carries a comma-separated list of the pairs [`acl_grantee_from_pair`] reads
/// ("x-amz-grant-read: emailAddress="xyz@amazon.com", emailAddress="abc@amazon.com"" —
/// `PutBucketAcl`'s documented example), and every grantee in it is granted the one
/// `permission` that header stands for.
pub fn acl_grants_from_header(header: &str, permission: &str) -> Result<Vec<AclGrant>, String> {
    let mut grants: Vec<AclGrant> = Vec::new();
    for pair in header.split(',').map(str::trim).filter(|pair| !pair.is_empty()) {
        let Some(grantee) = acl_grantee_from_pair(pair) else {
            return Err(pair.to_string());
        };
        grants.push(AclGrant {
            grantee,
            permission: permission.to_string(),
        });
    }
    Ok(grants)
}

/// The code S3 answers an ACL request whose ACL does not validate against the schema the
/// API publishes for it: "MalformedACLError - The XML that you provided was not well
/// formed or did not validate against our published schema", HTTP 400 (Amazon S3 "Error
/// responses", list of error codes).
pub const MALFORMED_ACL_CODE: &str = "MalformedACLError";

/// The code S3 answers a request that carries no body where one is required: "This happens
/// when the user sends an empty XML document as a request. The error message is, 'Request
/// body is empty'", HTTP 400 (same list) — what an ACL request that names no ACL at all
/// gets, since the ACL travels either in the body or in the request headers.
pub const MISSING_REQUEST_BODY_CODE: &str = "MissingRequestBodyError";
/// See [`MISSING_REQUEST_BODY_CODE`].
pub const MISSING_REQUEST_BODY_MESSAGE: &str = "Request body is empty";

/// The code S3 answers an attempt to set an ACL on a bucket whose Object Ownership setting
/// is `BucketOwnerEnforced`: "AccessControlListNotSupported - The bucket does not allow
/// ACLs", HTTP 400 (same list).
///
/// The case `PutBucketAcl`'s own documentation describes: "If your bucket uses the bucket
/// owner enforced setting for S3 Object Ownership, ACLs are disabled and no longer affect
/// permissions. … Requests to set ACLs or update ACLs fail and return the
/// AccessControlListNotSupported error code. Requests to read ACLs are still supported."
pub const ACL_NOT_SUPPORTED_CODE: &str = "AccessControlListNotSupported";
/// See [`ACL_NOT_SUPPORTED_CODE`].
pub const ACL_NOT_SUPPORTED_MESSAGE: &str = "The bucket does not allow ACLs";

/// The code S3 answers an ACL naming a grantee by an email address that resolves to no
/// account: "UnresolvableGrantByEmailAddress - The email address that you provided does
/// not match any account on record", HTTP 400 (same list).
pub const UNRESOLVABLE_GRANT_BY_EMAIL_CODE: &str = "UnresolvableGrantByEmailAddress";

/// The code S3 answers a request the resource does not allow at all: "MethodNotAllowed -
/// The specified method is not allowed against this resource", HTTP 405 (same list) — what
/// an Email Grantee ACL gets in the Regions where S3 has discontinued it
/// ([`EMAIL_GRANTEE_DISCONTINUED_REGIONS`]), and what a read that names a version which is
/// a DELETE MARKER gets ("If the specified version is a delete marker, the response
/// returns a 405 Method Not Allowed error" — `GetObject`'s own documentation, whose
/// options `HeadObject` shares).
pub const METHOD_NOT_ALLOWED_CODE: &str = "MethodNotAllowed";
/// See [`METHOD_NOT_ALLOWED_CODE`] — the message of that same list entry.
pub const METHOD_NOT_ALLOWED_MESSAGE: &str =
    "The specified method is not allowed against this resource.";
/// The HTTP status [`METHOD_NOT_ALLOWED_CODE`] is rendered with: 405, from the same list
/// and from `PutBucketAcl`'s own documentation ("the request will receive an HTTP 405
/// (Method Not Allowed) error").
pub const METHOD_NOT_ALLOWED_STATUS: u16 = 405;

/// The `<ResourceType>` S3's `MethodNotAllowed` document names a request for an object's
/// BitTorrent file by: the `?torrent` subresource the method was refused against.
///
/// The code's message is fixed ("the specified method is not allowed against this
/// resource"), so the `<Method>` / `<ResourceType>` pair is the only part of the document
/// that says which method and which resource type it was \u2014 see
/// [`crate::scaffolded::Scaffolded::method_not_allowed`].
pub const TORRENT_RESOURCE_TYPE: &str = "TORRENT";

/// The `<ResourceType>` of the same document for a call addressed at a version that is a
/// DELETE MARKER: "if the specified version is a delete marker, the response returns a 405
/// Method Not Allowed error" (`GetObject`'s own documentation, whose options `HeadObject`
/// shares), so the resource the method was refused against is the marker.
pub const DELETE_MARKER_RESOURCE_TYPE: &str = "DeleteMarker";

/// The `ObjectOwnership` value that DISABLES ACLs on a bucket: "ACLs are disabled and no
/// longer affect permissions" (`PutBucketAcl`'s documentation), so a request to set one is
/// refused with [`ACL_NOT_SUPPORTED_CODE`] while a request to read one still answers.
pub const BUCKET_OWNER_ENFORCED: &str = "BucketOwnerEnforced";

/// The `ObjectOwnership` value a bucket is left in once its ownership controls are
/// REMOVED: ACLs affect permissions again, and "the uploading account will own the object
/// if the object is uploaded with the bucket-owner-full-control canned ACL"
/// (`smithy_query(query="shape",shape="ObjectOwnership")`). A bucket with no
/// `OwnershipControls` is one whose ACLs are enabled — the state
/// `GetBucketOwnershipControls` describes as "the bucket was created before the
/// BucketOwnerEnforced setting was introduced" or "you've manually deleted the bucket
/// ownership control value".
pub const OBJECT_WRITER: &str = "ObjectWriter";

/// The `ObjectOwnership` a general purpose bucket is CREATED with when the request names
/// none: "By default, `ObjectOwnership` is set to `BucketOwnerEnforced` and ACLs are
/// disabled" (`smithy_query(query="shape",shape="ObjectOwnership")`), so a bucket nothing
/// has configured refuses an ACL write with [`ACL_NOT_SUPPORTED_CODE`].
pub const DEFAULT_OBJECT_OWNERSHIP: &str = BUCKET_OWNER_ENFORCED;

/// The code S3 answers a request that would leave a bucket with BOTH an ACL that grants
/// more than the owner's `FULL_CONTROL` and the `BucketOwnerEnforced` Object Ownership
/// setting that disables ACLs: "InvalidBucketAclWithObjectOwnership - Bucket cannot have
/// ACLs set with ObjectOwnership's BucketOwnerEnforced setting", HTTP 400 (Amazon S3
/// "Error responses", list of error codes).
pub const INVALID_BUCKET_ACL_WITH_OBJECT_OWNERSHIP_CODE: &str =
    "InvalidBucketAclWithObjectOwnership";
/// See [`INVALID_BUCKET_ACL_WITH_OBJECT_OWNERSHIP_CODE`] — the text of that same entry.
pub const INVALID_BUCKET_ACL_WITH_OBJECT_OWNERSHIP_MESSAGE: &str =
    "Bucket cannot have ACLs set with ObjectOwnership's BucketOwnerEnforced setting";

/// Whether one ACL grant gives its permission to a PREDEFINED GROUP that stands for people
/// outside the bucket owner's account — `AllUsers` (anyone) or `AuthenticatedUsers` (any
/// AWS account).
///
/// "The Meaning of Public" (Amazon S3 User Guide, Block Public Access): an ACL is public
/// when it grants access to `AllUsers` or `AuthenticatedUsers`. A grant to the
/// `LogDelivery` group is to an S3 service principal, not to the public, and a grant to a
/// canonical user or an email address names one account.
pub fn acl_grant_is_public(grant: &AclGrant) -> bool {
    grant
        .grantee
        .uri
        .as_deref()
        .is_some_and(|uri| uri == ALL_USERS_GROUP_URI || uri == AUTHENTICATED_USERS_GROUP_URI)
}

/// Whether `grants` gives any access to a predefined group that stands for people outside
/// the bucket owner's account — see [`acl_grant_is_public`], the per-grant rule this is the
/// any-of over.
pub fn acl_grants_public_access(grants: &[AclGrant]) -> bool {
    grants.iter().any(acl_grant_is_public)
}

/// Whether `grants` gives `permission` to the caller that presents NO credential.
///
/// `AllUsers` is the predefined group that "includes anyone in the world" while
/// `AuthenticatedUsers` "includes all AWS accounts" (Amazon S3 User Guide, *Access control
/// list (ACL) overview*, Amazon S3 predefined groups), so a request speaking for nobody is
/// reached by the first and NOT by the second. `FULL_CONTROL` "allows grantee the READ,
/// WRITE, READ_ACP, and WRITE_ACP permissions" (same overview), so it answers for any one
/// permission asked about.
///
/// The grants passed in are what an ACL READ reports ([`Bucket::object_acl_grants`]), which
/// is where `IgnorePublicAcls` has already dropped every public grant S3 ignores: a grant
/// S3 ignores gives nobody anything, so it cannot admit this caller either.
pub fn acl_grants_to_all_users(grants: &[AclGrant], permission: &str) -> bool {
    grants.iter().any(|grant| {
        grant.grantee.uri.as_deref() == Some(ALL_USERS_GROUP_URI)
            && (grant.permission == permission || grant.permission == PERMISSION_FULL_CONTROL)
    })
}

/// The ARN of the S3 bucket `bucket`: `arn:aws:s3:::<bucket>`.
///
/// An S3 bucket's ARN leaves the Region and the account-id fields EMPTY, "because a bucket
/// name is globally unique" (see [`ArnField`]), so it is spelled here rather than through
/// `engine::emulator_arn`, which always carries the account. One place, so every refusal
/// and every policy decision that names a bucket names it the same way.
pub fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// The ARN of the object `key` in `bucket`: `arn:aws:s3:::<bucket>/<key>` — the bucket's
/// ARN ([`bucket_arn`]) with the key as the rest of the resource part, which is how S3
/// names an object in a policy and in a refusal.
pub fn object_arn(bucket: &str, key: &str) -> String {
    format!("{}/{key}", bucket_arn(bucket))
}

/// The principal a request speaks for, as `sts:GetCallerIdentity` reports it: the account's
/// own identity, `arn:aws:iam::<account>:user/emulator`, which is what a refusal names.
///
/// This emulator serves one account, and a request presenting its credential speaks for
/// that account; the ARN is built through `engine::emulator_arn` so it carries the one
/// account id this emulator presents.
pub fn caller_principal_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The message S3 words an `AccessDenied` the Block Public Access `BlockPublicAcls` setting
/// caused: who was refused, which IAM `action`, on which `resource`, and the setting that
/// prevented it.
///
/// Every value in it comes from the request and from this emulator's own identity: the
/// principal is the caller ([`caller_principal_arn`]), the action is the operation the
/// request called and the resource is the bucket or object it addressed. One place, so the
/// bucket half and the object half of the same guardrail word it identically.
pub fn block_public_acls_denial(action: &str, resource: &str) -> String {
    block_public_access_denial(
        action,
        resource,
        "public ACLs are prevented by the BlockPublicAcls",
    )
}

/// The message S3 words an `AccessDenied` the Block Public Access `BlockPublicPolicy`
/// setting caused — the policy half of the guardrail [`block_public_acls_denial`] states
/// for an ACL, in the same sentence with the setting that prevented the call.
///
/// Every value in it comes from the request and from this emulator's own identity, exactly
/// as in the ACL half.
pub fn block_public_policy_denial(action: &str, resource: &str) -> String {
    block_public_access_denial(
        action,
        resource,
        "public policies are prevented by the BlockPublicPolicy",
    )
}

/// The one sentence a Block Public Access refusal is worded in: who was refused, which IAM
/// `action`, on which `resource`, and the `prevented_by` clause naming what the setting
/// prevents and which setting it is.
///
/// One place, so every switch's refusal reads alike and only the clause that names the
/// setting differs — which is the only thing that differs about them.
fn block_public_access_denial(action: &str, resource: &str, prevented_by: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: \"{resource}\" because \
         {prevented_by} setting in S3 Block Public Access.",
        caller_principal_arn()
    )
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

/// The Block Public Access settings a bucket carries.
///
/// Four INDEPENDENT switches, not an `Option` each: every member of
/// `PublicAccessBlockConfiguration` is a `Setting` (a boolean) documented as "Specifies
/// whether Amazon S3 should block …  Setting this element to TRUE causes …"
/// (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`), so a switch the
/// configuration does not turn on is off — there is no third state for a member to be in.
///
/// [`Bucket::public_access_block`] is an `Option` of it because a bucket can carry NO
/// configuration at all — the state `DeletePublicAccessBlock` leaves it in, and the only
/// state in which a read of the subresource is not-found. A bucket this store records comes
/// into being with [`Self::all_enabled`] rather than with nothing, because S3 creates every
/// new bucket with all four settings on.
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

impl BucketPublicAccessBlock {
    /// Every one of the four switches ON — the configuration a bucket comes into being
    /// carrying.
    ///
    /// "Amazon S3 automatically enables S3 Block Public Access … for new buckets" (Amazon
    /// S3 User Guide, Blocking public access to your Amazon S3 storage): a bucket created
    /// today starts out fully blocked, so a read of the subresource on a bucket nothing has
    /// configured reports four `true`s rather than "there is none".
    pub const fn all_enabled() -> Self {
        Self {
            block_public_acls: true,
            ignore_public_acls: true,
            block_public_policy: true,
            restrict_public_buckets: true,
        }
    }
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

/// Each `FilterRuleName` wire value beside the CANONICAL spelling S3 keeps the rule under.
///
/// The enum is the REQUEST vocabulary — `FilterRuleName`'s values are `prefix` and `suffix`
/// (`smithy_query(query="shape",shape="FilterRuleName")`), which is what a caller sends —
/// while the rule S3 stores, and reports to every reader of the bucket's notification
/// document, is named with the capitalised spelling. The two differ only in the case of the
/// first character, so the pair is kept here once and [`canonical_filter_rule_name`] is the
/// one way either spelling is resolved.
const FILTER_RULE_NAME_SPELLINGS: &[(&str, &str)] = &[
    (FILTER_RULE_PREFIX, "Prefix"),
    (FILTER_RULE_SUFFIX, "Suffix"),
];

/// The `FilterRuleName` value that filters on the START of an object key, and the one that
/// filters on its END — the request vocabulary's two values, named so a rule of one kind
/// can be asked for by kind.
pub const FILTER_RULE_PREFIX: &str = "prefix";
/// See [`FILTER_RULE_PREFIX`].
pub const FILTER_RULE_SUFFIX: &str = "suffix";

/// The canonical spelling of one key-name filter rule's `Name`, or `None` for a value that
/// is no `FilterRuleName` at all.
///
/// Both spellings resolve — the wire value a request carries and the canonical one a stored
/// rule holds — so a configuration read out of the store is as valid as the one that was
/// put, and the store never holds two spellings of one rule kind.
pub fn canonical_filter_rule_name(name: &str) -> Option<&'static str> {
    FILTER_RULE_NAME_SPELLINGS
        .iter()
        .find(|(wire, _)| wire.eq_ignore_ascii_case(name))
        .map(|(_, canonical)| *canonical)
}

/// The WIRE value of a filter rule kind, as the sentence refusing a second rule of that
/// kind spells it, or `None` for a value that is no `FilterRuleName`.
fn filter_rule_kind(name: &str) -> Option<&'static str> {
    FILTER_RULE_NAME_SPELLINGS
        .iter()
        .find(|(wire, _)| wire.eq_ignore_ascii_case(name))
        .map(|(wire, _)| *wire)
}

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

    /// What the destination IS, as the per-destination detail of a validation refusal names
    /// it — the noun S3's own sentence for that destination kind uses (see
    /// [`destination_permissions_detail`]).
    pub fn noun(self) -> &'static str {
        match self {
            Self::Topic => "topic",
            Self::Queue => "queue",
            Self::LambdaFunction => "Lambda function",
        }
    }
}

/// The per-destination detail S3 carries for a destination that EXISTS and whose resource
/// policy does not let Amazon S3 publish this bucket's notifications to it.
///
/// The `<ArgumentValue{n}>` beside the fixed
/// [`UNABLE_TO_VALIDATE_DESTINATIONS_MESSAGE`] sentence: the refusal's message is the same
/// for every destination that failed, so the detail is where the reason for ONE of them is
/// stated. Only the destination's kind varies in it ([`NotificationDestination::noun`]);
/// nothing of the request is interpolated.
pub fn destination_permissions_detail(destination: NotificationDestination) -> String {
    format!(
        "Permissions on the destination {} do not allow S3 to publish notifications from \
         this bucket",
        destination.noun()
    )
}

/// The same detail for a destination the service that owns it does not hold at all, which
/// is a different reason and so reads differently.
pub fn destination_absent_detail(destination: NotificationDestination) -> String {
    format!("The destination {} does not exist", destination.noun())
}

/// One object key name filtering rule of a configuration's `Filter`.
///
/// Both members are optional in the model (`smithy_query(query="shape",shape="FilterRule")`),
/// so each is kept exactly as the request carried it rather than defaulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationFilterRule {
    /// A `FilterRuleName` value in its CANONICAL spelling
    /// ([`canonical_filter_rule_name`]) — the name S3 keeps the rule under and reports to
    /// every reader — or `None` when the rule named none.
    pub name: Option<String>,
    /// "The value that the filter searches for in object key names" (`FilterRule$Value`),
    /// or `None` when the rule named none.
    pub value: Option<String>,
}

impl NotificationFilterRule {
    /// Which KIND of rule this is, as the `FilterRuleName` wire vocabulary spells it, or
    /// `None` for a rule naming no published name at all. Whichever spelling the rule is
    /// held in answers the same kind, so a filter cannot carry one kind twice under two
    /// spellings.
    pub fn kind(&self) -> Option<&'static str> {
        filter_rule_kind(self.name.as_deref()?)
    }

    /// The value this rule filters by, empty when it named none — which filters on
    /// nothing, the way a filter with no rule at all does.
    pub fn filtered_value(&self) -> &str {
        self.value.as_deref().unwrap_or_default()
    }
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

impl NotificationTarget {
    /// The value this configuration filters keys by for one rule KIND (`prefix` /
    /// `suffix`), empty when it carries no rule of that kind — which restricts nothing, so
    /// it is the value a configuration with no `Filter` filters by too.
    fn filtered_by(&self, kind: &str) -> &str {
        self.filter_rules
            .iter()
            .flatten()
            .find(|rule| rule.kind() == Some(kind))
            .map_or("", NotificationFilterRule::filtered_value)
    }
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
    /// An event outside [`EVENT_VALUES`]. The sentence S3 sends for this is FIXED text,
    /// so neither member is interpolated into it; they are what the refusal is logged with.
    UnsupportedEvent {
        /// The member the configuration arrived on.
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
    /// A second rule of one KIND inside a single `Filter`: "overlapping prefixes and
    /// suffixes are not supported" (`FilterRule$Name`), so one filter carries at most one
    /// prefix rule and at most one suffix rule.
    DuplicateFilterRuleKind {
        /// The kind named twice, as the request vocabulary spells it (`prefix`/`suffix`).
        kind: &'static str,
    },
    /// Two configurations that would both match the same key for the same event: their
    /// prefixes overlap AND their suffixes overlap, which leaves the document unable to say
    /// which rule an object matches.
    AmbiguousConfiguration,
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
            Self::UnsupportedEvent { .. } => f.write_str(UNSUPPORTED_EVENT_MESSAGE),
            Self::NoEvents { member } => write!(
                f,
                "A {member} configuration must specify at least one event"
            ),
            Self::InvalidFilterRuleName { name } => write!(
                f,
                "filter rule name must be either prefix or suffix, got {name}"
            ),
            Self::DuplicateFilterRuleKind { kind } => {
                write!(f, "Cannot specify more than one {kind} rule in a filter.")
            }
            Self::AmbiguousConfiguration => f.write_str(
                "Configuration is ambiguously defined. Cannot have overlapping suffixes in \
                 two rules if the prefixes are overlapping for the same event type.",
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

/// What one field of an ARN carries for the service that owns the resource.
///
/// The Region and the account-id fields are the two that differ between services: a
/// regional, account-owned resource names both, a GLOBAL service's ARN leaves the Region
/// out (`arn:aws:iam::<account>:role/<name>`) and an Amazon S3 bucket's ARN leaves both out
/// (`arn:aws:s3:::<bucket>`), because a bucket name is globally unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArnField {
    /// The field carries a value.
    Present,
    /// The field is EMPTY — the `::` an ARN of such a service spells.
    Empty,
}

impl ArnField {
    /// Whether one ARN field as written satisfies this expectation.
    fn holds(self, field: &str) -> bool {
        match self {
            Self::Present => !field.is_empty(),
            Self::Empty => field.is_empty(),
        }
    }
}

/// Whether `arn` is an ARN of `service` whose Region and account-id fields are as that
/// service writes them, and whose resource part starts with `resource_prefix` and carries
/// something past it.
///
/// The value is read through the ONE shared ARN parse (`service_common::arn`), so this
/// crate does not split an ARN its own way. `resource_prefix` is the leading segment the
/// resource KIND is named by where one exists (`role/` for an IAM role) and empty for a
/// service whose resource part is the bare name (an S3 bucket, an SQS queue).
///
/// This is the one place an ARN MEMBER is judged in this crate: every member that takes an
/// ARN — a notification destination, a replication role, a replication destination bucket —
/// is judged through it rather than through a rule of its own.
pub fn is_arn_of(
    arn: &str,
    service: &str,
    region: ArnField,
    account: ArnField,
    resource_prefix: &str,
) -> bool {
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return false;
    };
    let resource = parsed.resource;
    parsed.vendor == service
        && region.holds(parsed.region)
        && account.holds(parsed.account)
        && resource.len() > resource_prefix.len()
        && resource.starts_with(resource_prefix)
}

/// Whether `arn` is an ARN of `service` for a regional, account-owned resource named by its
/// bare resource part — the form every notification destination takes.
fn is_arn_of_service(arn: &str, service: &str) -> bool {
    is_arn_of(arn, service, ArnField::Present, ArnField::Present, "")
}

/// Whether `value` is the ARN of an S3 bucket: `arn:<partition>:s3:::<name>`.
///
/// A bucket ARN carries neither a Region nor an account id, because a bucket name is
/// globally unique, and its resource part is the bare bucket name. Read through
/// [`is_arn_of`], the crate's one ARN grammar.
///
/// Shared, because MORE than one member takes a bucket ARN: a replication rule's
/// `Destination$Bucket` and the analytics export's `AnalyticsS3BucketDestination$Bucket`
/// are both "the Amazon Resource Name (ARN) of the bucket", so neither may judge one its
/// own way.
pub fn is_s3_bucket_arn(value: &str) -> bool {
    is_arn_of(value, "s3", ArnField::Empty, ArnField::Empty, "")
}

/// Why `value` is not an ARN, as AWS states it where a refusal quotes the reason.
///
/// The ARN grammar is the published one [`is_arn_of`] reads —
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>` — and the diagnostics are
/// the ones AWS's own ARN parser publishes for each way a value breaks it, in the order it
/// checks them: the `arn:` prefix, the six sections, then the three sections that may not
/// be empty. The last arm is for a value that IS an ARN and still does not name the
/// resource the member requires.
///
/// Nothing here is derived from the recorded request: the reason comes from `value`'s own
/// shape, and `value` itself is not in any of the sentences.
pub fn malformed_arn_reason(value: &str) -> &'static str {
    const PREFIX: &str = "arn:";
    if !value.starts_with(PREFIX) {
        return "Malformed ARN - doesn't start with 'arn:'";
    }
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    if fields.len() < 6 {
        return "Malformed ARN - doesn't contain enough sections";
    }
    if fields[1].is_empty() {
        return "Malformed ARN - no AWS partition";
    }
    if fields[2].is_empty() {
        return "Malformed ARN - no AWS service";
    }
    if fields[5].is_empty() {
        return "Malformed ARN - no AWS resource";
    }
    "Invalid ARN - the ARN does not name the resource this member requires"
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
///
/// The last two rules are about the document as a whole rather than one value in it, and
/// no model can state either: a `Filter` carries at most one rule of each kind, and no two
/// configurations may both match the same key for the same event
/// ([`configurations_are_ambiguous`]).
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
            if canonical_filter_rule_name(name).is_none() {
                return Err(NotificationConfigurationRejection::InvalidFilterRuleName {
                    name: name.to_string(),
                });
            }
        }

        // "Overlapping prefixes and suffixes are not supported" (`FilterRule$Name`): one
        // `Filter` carries at most ONE rule of each kind, because two of a kind leave the
        // filter with two answers for one key.
        let mut seen_kinds: BTreeSet<&str> = BTreeSet::new();
        for rule in target.filter_rules.iter().flatten() {
            if let Some(kind) = rule.kind() {
                if !seen_kinds.insert(kind) {
                    return Err(NotificationConfigurationRejection::DuplicateFilterRuleKind {
                        kind,
                    });
                }
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

    // And the document as a WHOLE: no two of its configurations may both match the same
    // key for the same event — "overlapping prefixes and suffixes are not supported"
    // ([`filters_overlap`] is the key half of the rule) — because S3 could not then say
    // which rule an object matches. The check is over every pair across all three lists,
    // since which destination a configuration publishes to does not make two rules
    // distinguish an object.
    //
    // Each configuration's events are read ONCE, into the bit set [`event_mask`] encodes
    // them as, so the pair walk below costs one AND per pair: the walk is bounded by the
    // configuration count alone (at most
    // [`MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`], checked above, so at most 4950
    // pairs) and reading the `Events` lists is ONE pass over them, whatever a request
    // carries in them. Comparing the lists pair by pair instead would multiply the pair
    // count by the two lists' lengths, which no published limit bounds.
    let targets: Vec<(&NotificationTarget, u64)> = configuration
        .targets()
        .map(|(_, target)| (target, event_mask(target)))
        .collect();
    for (position, (one, one_events)) in targets.iter().enumerate() {
        for (other, other_events) in &targets[position + 1..] {
            if one_events & other_events != 0 && filters_overlap(one, other) {
                return Err(NotificationConfigurationRejection::AmbiguousConfiguration);
            }
        }
    }

    Ok(())
}

/// The events one configuration names, as one bit per published `Event` value — the set two
/// configurations are asked to share an element of.
///
/// The `Event` vocabulary is CLOSED and small ([`EVENT_VALUES`], which
/// [`EVENT_VALUES_FIT_ONE_MASK`] holds to the width of this mask), and every event of the
/// container has already been checked to be one of its values by the time this is built, so
/// the mask loses nothing: a value outside the set has been refused, and a value repeated in
/// one list is the same bit.
///
/// The event sets are compared AS THE DOCUMENT NAMES THEM: two configurations share an
/// event when the same `Event` value appears in both, so a configuration naming a value and
/// another naming the `s3:<Type>:*` form of its type are two distinct rules and are both
/// kept — the pair a bucket carries to route one event class and one event of it to
/// different destinations.
fn event_mask(target: &NotificationTarget) -> u64 {
    target
        .events
        .iter()
        .filter_map(|event| {
            EVENT_VALUES
                .iter()
                .position(|published| *published == event.as_str())
        })
        .fold(0, |mask, index| mask | (1 << index))
}

/// Whether the published `Event` catalog still fits in the one mask [`event_mask`] encodes
/// a configuration's events as. Asserted by this crate's own test beside the rule it serves.
pub const EVENT_VALUES_FIT_ONE_MASK: usize = u64::BITS as usize;

/// Whether two destination configurations of one document would both match the same object
/// KEY: their prefixes overlap and their suffixes overlap, so some key passes both filters.
///
/// A rule's prefix and suffix each restrict the keys it matches, and two restrictions of
/// the same kind overlap when one is a prefix (respectively a suffix) of the other — a
/// configuration carrying no rule of that kind restricts nothing, which overlaps every
/// value. Two configurations whose prefixes are disjoint, or whose suffixes are, can
/// therefore never both match one key and are both kept, however their events compare.
fn filters_overlap(one: &NotificationTarget, other: &NotificationTarget) -> bool {
    prefixes_overlap(
        one.filtered_by(FILTER_RULE_PREFIX),
        other.filtered_by(FILTER_RULE_PREFIX),
    ) && suffixes_overlap(
        one.filtered_by(FILTER_RULE_SUFFIX),
        other.filtered_by(FILTER_RULE_SUFFIX),
    )
}

/// Whether two key-name PREFIXES restrict to key sets that intersect: one is a prefix of
/// the other, which includes either of them being empty.
fn prefixes_overlap(one: &str, other: &str) -> bool {
    one.starts_with(other) || other.starts_with(one)
}

/// Whether two key-name SUFFIXES restrict to key sets that intersect: one is a suffix of
/// the other, which includes either of them being empty.
fn suffixes_overlap(one: &str, other: &str) -> bool {
    one.ends_with(other) || other.ends_with(one)
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
/// bucket reports is [`DEFAULT_SSE_ALGORITHM`] with the Bucket Key off and
/// [`DEFAULT_BLOCKED_ENCRYPTION_TYPES`] blocked.
///
/// COMPUTED rather than stored, so a read of a bucket that was never configured reports
/// this without writing anything: [`Bucket::encryption`] stays `None` and a later reader
/// sees exactly what this one saw.
pub fn default_encryption_rule() -> BucketEncryptionRule {
    BucketEncryptionRule {
        sse_algorithm: Some(DEFAULT_SSE_ALGORITHM.to_string()),
        kms_master_key_id: None,
        bucket_key_enabled: Some(DEFAULT_BUCKET_KEY_ENABLED),
        blocked_encryption_types: Some(default_blocked_encryption_types()),
    }
}

/// The `EncryptionType` values a bucket blocks while no `PutBucketEncryption` has named a
/// set of its own — [`DEFAULT_BLOCKED_ENCRYPTION_TYPES`], as a stored rule holds them.
pub fn default_blocked_encryption_types() -> Vec<String> {
    DEFAULT_BLOCKED_ENCRYPTION_TYPES
        .iter()
        .map(|blocked| (*blocked).to_string())
        .collect()
}

/// Every value the `Protocol` enum publishes: the two schemes a website redirect may be
/// made over. Re-exported from the model-generated value list, like [`EVENT_VALUES`], so
/// there is ONE source of truth for the set.
pub use crate::ops::types::PROTOCOL_VALUES;

/// How many routing rules one bucket's website configuration may carry.
///
/// The published limitation, from `PutBucketWebsite`'s own documentation: "Amazon S3 has a
/// limitation of 50 routing rules per website configuration. If you require more than 50
/// routing rules, you can use object redirect." The model bounds `RoutingRules` at neither
/// end (`smithy_query(query="shape",shape="RoutingRules")`), so this check is what enforces
/// the limitation, and it also bounds what one bucket's record parks in this process's
/// memory — a put past it is refused rather than recorded.
pub const MAX_WEBSITE_ROUTING_RULES: usize = 50;

/// The lowest and highest HTTP status a `Redirect$HttpRedirectCode` may name.
///
/// "The HTTP redirect code to use on the response" (that member's own documentation): a
/// response code that redirects is one of the 3xx class other than 300, which asks the
/// client to choose among representations rather than naming one to follow — the bounds S3's
/// own refusal of a code outside them states
/// ([`WebsiteConfigurationRejection::InvalidHttpRedirectCode`]). The
/// model types the member as a plain string
/// (`smithy_query(query="shape",shape="HttpRedirectCode")`), so nothing refuses a
/// non-redirect code before the store does.
pub const HTTP_REDIRECT_CODES: std::ops::RangeInclusive<u16> = 301..=399;

/// The lowest and highest HTTP status a `Condition$HttpErrorCodeReturnedEquals` may name.
///
/// "The HTTP error code when the redirect is applied. In the event of an error, if the error
/// code equals this value, then the specified redirect is applied" (that member's own
/// documentation): only a response that IS an error can equal it, which is the 4xx and 5xx
/// classes — the bounds S3's own refusal of a code outside them states
/// ([`WebsiteConfigurationRejection::InvalidHttpErrorCode`]). The model types the member as
/// a plain string
/// (`smithy_query(query="shape",shape="HttpErrorCodeReturnedEquals")`), so a rule
/// conditioned on a status that is not an error would never be applied by anything and is
/// refused rather than recorded.
pub const HTTP_ERROR_CODES: std::ops::RangeInclusive<u16> = 400..=599;

/// Where every request to a bucket's website endpoint is redirected, as the
/// `RedirectAllRequestsTo` container of the configuration named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebsiteRedirectAllRequestsTo {
    /// `RedirectAllRequestsTo$HostName`, a REQUIRED member: "Name of the host where
    /// requests are redirected."
    pub host_name: String,
    /// `RedirectAllRequestsTo$Protocol`: one of [`PROTOCOL_VALUES`], or `None` when the
    /// container named none. Nothing is stored for an absent protocol because the default
    /// is a property of the REQUEST, not of the configuration — "The default is the
    /// protocol that is used in the original request" (that member's documentation) — so a
    /// reader reports no protocol rather than inventing one.
    pub protocol: Option<String>,
}

/// The condition a stored routing rule applies under: one `Condition` container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebsiteRoutingCondition {
    /// `Condition$HttpErrorCodeReturnedEquals`: "The HTTP error code when the redirect is
    /// applied."
    pub http_error_code_returned_equals: Option<String>,
    /// `Condition$KeyPrefixEquals`: "The object key name prefix when the redirect is
    /// applied."
    pub key_prefix_equals: Option<String>,
}

impl WebsiteRoutingCondition {
    /// Whether the condition names NEITHER of its two members, in which case there is no
    /// condition to apply the rule under.
    ///
    /// Each member is required when the other is absent — "Required when parent element
    /// `Condition` is specified and sibling `KeyPrefixEquals` is not specified" and the
    /// mirror sentence on the other member — so a container naming neither is one S3
    /// refuses rather than records.
    pub fn is_empty(&self) -> bool {
        self.http_error_code_returned_equals.is_none() && self.key_prefix_equals.is_none()
    }
}

/// Where a stored routing rule redirects to: one `Redirect` container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebsiteRedirect {
    /// `Redirect$HostName`: "The host name to use in the redirect request."
    pub host_name: Option<String>,
    /// `Redirect$HttpRedirectCode`, within [`HTTP_REDIRECT_CODES`].
    pub http_redirect_code: Option<String>,
    /// `Redirect$Protocol`: one of [`PROTOCOL_VALUES`].
    pub protocol: Option<String>,
    /// `Redirect$ReplaceKeyPrefixWith`: "The object key prefix to use in the redirect
    /// request." Mutually exclusive with [`Self::replace_key_with`].
    pub replace_key_prefix_with: Option<String>,
    /// `Redirect$ReplaceKeyWith`: "The specific object key to use in the redirect request."
    pub replace_key_with: Option<String>,
}

/// One `RoutingRule` of a bucket's website configuration: when a redirect is applied, and
/// where to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebsiteRoutingRule {
    /// `RoutingRule$Condition`, or `None` when the rule names none — in which case the
    /// redirect applies to every request.
    pub condition: Option<WebsiteRoutingCondition>,
    /// `RoutingRule$Redirect`, a REQUIRED member.
    pub redirect: WebsiteRedirect,
}

/// One bucket's website configuration, as `PutBucketWebsite` last set it.
///
/// Two shapes of configuration, which the container's own documentation keeps apart: one
/// that redirects EVERY request ([`Self::redirect_all_requests_to`] alone — "If you specify
/// this property, you can't specify any other property"), and one that serves the bucket as
/// a website, which "must provide an index document for the bucket" and may add an error
/// document and routing rules. [`validate_website_configuration`] is what keeps a stored
/// configuration to one of those two shapes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketWebsite {
    /// `IndexDocument$Suffix`: "A suffix that is appended to a request that is for a
    /// directory on the website endpoint", or `None` for a redirect-everything
    /// configuration.
    pub index_document_suffix: Option<String>,
    /// `ErrorDocument$Key`: "The object key name to use when a 4XX class error occurs", or
    /// `None` when the configuration names no error document.
    pub error_document_key: Option<String>,
    /// `WebsiteConfiguration$RedirectAllRequestsTo`, or `None` for a configuration that
    /// serves the bucket as a website rather than redirecting every request.
    pub redirect_all_requests_to: Option<WebsiteRedirectAllRequestsTo>,
    /// `WebsiteConfiguration$RoutingRules`, in the order the request carried them, and
    /// EMPTY when the configuration carried no `RoutingRules` container.
    ///
    /// A plain `Vec` rather than an `Option`, because "no routing rules" is ONE state here:
    /// the XML boundary parses a `RoutingRules` container that names no `RoutingRule` to the
    /// same absent list as no container at all, so there is nothing for a `None` to mean
    /// that an empty list does not — and a reader reports the container exactly when there
    /// is a rule in it.
    pub routing_rules: Vec<WebsiteRoutingRule>,
}

/// The `<ArgumentValue>` S3's refusal of an ABSENT member carries: the member held no value,
/// which its error document spells as the JSON null literal rather than leaving the element
/// empty.
pub const ABSENT_ARGUMENT_VALUE: &str = "null";

/// Why a website configuration is not one S3 would apply to a bucket, and how S3 refuses it.
///
/// Each variant names the member that is wrong and carries what that member held, and
/// answers the three things S3's own refusal of that member states: the wire code
/// ([`Self::code`]), the sentence (its [`std::fmt::Display`]) and — for a refusal about one
/// member of the document — the `<ArgumentName>` / `<ArgumentValue>` pair naming it
/// ([`Self::argument`]).
///
/// The sentence of each is FIXED text for that class of fault; the only thing interpolated
/// into one is a value the REQUEST carried, where the sentence S3 publishes names it (a
/// rejected status code). Nothing about one request's wording reaches another's refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebsiteConfigurationRejection {
    /// A document that does not validate against the schema S3 publishes for the container
    /// — including one that is not well-formed XML at all. See
    /// [`website_document_rejection`].
    MalformedDocument,
    /// `RedirectAllRequestsTo` beside another member of the container: "If you specify this
    /// property, you can't specify any other property"
    /// (`WebsiteConfiguration$RedirectAllRequestsTo`).
    RedirectAllNotExclusive {
        /// `RedirectAllRequestsTo$HostName`, the "name of the host where requests are
        /// redirected" — the value the container the refusal names carries, as sent.
        host_name: String,
    },
    /// A configuration that neither redirects every request nor names an index document:
    /// "the website configuration must provide an index document for the bucket, because
    /// some requests might not be redirected" (`PutBucketWebsite`'s own documentation).
    NoIndexDocument,
    /// An `IndexDocument$Suffix` that is empty or carries a slash: "The suffix must not be
    /// empty and must not include a slash character" (that member's documentation).
    MalformedIndexDocumentSuffix {
        /// The suffix the request carried, as it spelled it.
        suffix: String,
    },
    /// An `ErrorDocument$Key` that is not an object key: the empty string, which
    /// `ObjectKey`'s `length {min 1}` forbids.
    MalformedErrorDocumentKey {
        /// The key the request carried, as it spelled it.
        key: String,
    },
    /// More routing rules than [`MAX_WEBSITE_ROUTING_RULES`].
    TooManyRoutingRules {
        /// How many the request carried.
        count: usize,
    },
    /// A `Condition` naming neither of its members. See
    /// [`WebsiteRoutingCondition::is_empty`].
    EmptyCondition,
    /// A `Redirect` naming both `ReplaceKeyPrefixWith` and `ReplaceKeyWith`: each "can be
    /// present only if" the other "is not provided" (both members' documentation).
    ConflictingKeyReplacement,
    /// A `Protocol` outside [`PROTOCOL_VALUES`], on either member that carries one. The
    /// sentence S3 refuses it with names the two published values itself and echoes nothing
    /// from the request, so the value is not carried.
    UnsupportedProtocol,
    /// An `HttpRedirectCode` outside [`HTTP_REDIRECT_CODES`].
    InvalidHttpRedirectCode {
        /// The value that is not an HTTP redirect code, as the request spelled it.
        code: String,
    },
    /// An `HttpErrorCodeReturnedEquals` outside [`HTTP_ERROR_CODES`].
    InvalidHttpErrorCode {
        /// The value that is not an HTTP error code, as the request spelled it.
        code: String,
    },
}

impl WebsiteConfigurationRejection {
    /// The wire error code S3 answers this refusal with.
    ///
    /// Three codes from S3's published "Error responses" list, all HTTP 400. A document that
    /// does not satisfy the published schema is [`MALFORMED_XML_CODE`]. A member of the
    /// document whose VALUE — or whose absence — is not one the member may carry is
    /// [`INVALID_ARGUMENT_CODE`], and the refusal names that member in its
    /// `<ArgumentName>` / `<ArgumentValue>` pair ([`Self::argument`]). A document whose
    /// members are each acceptable on their own but which asks for behaviour the service
    /// cannot carry out — a redirect conditioned on a status that is not an error, a rule
    /// naming two replacements for one key, a redirect over a scheme the service does not
    /// make, more rules than a configuration may hold — is [`INVALID_REQUEST_CODE`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::MalformedDocument => MALFORMED_XML_CODE,
            Self::RedirectAllNotExclusive { .. }
            | Self::NoIndexDocument
            | Self::MalformedIndexDocumentSuffix { .. }
            | Self::MalformedErrorDocumentKey { .. } => INVALID_ARGUMENT_CODE,
            Self::TooManyRoutingRules { .. }
            | Self::EmptyCondition
            | Self::ConflictingKeyReplacement
            | Self::UnsupportedProtocol
            | Self::InvalidHttpRedirectCode { .. }
            | Self::InvalidHttpErrorCode { .. } => INVALID_REQUEST_CODE,
        }
    }

    /// The HTTP status this refusal is rendered with: 400 for all three codes
    /// ([`Self::code`]), read from that code's own constant rather than repeated here.
    pub fn status(&self) -> u16 {
        match self.code() {
            MALFORMED_XML_CODE => MALFORMED_XML_STATUS,
            INVALID_REQUEST_CODE => INVALID_REQUEST_STATUS,
            _ => INVALID_ARGUMENT_STATUS,
        }
    }

    /// The `<ArgumentName>` / `<ArgumentValue>` pair S3's error document names the refused
    /// member by, or `None` for a refusal that is about the document as a whole, or about a
    /// rule inside it, rather than about one member of the container.
    ///
    /// The name is the CONTAINER element the value arrived in, as the API publishes it, and
    /// the value is what that container held: the scalar the member carries, the host a
    /// redirect-everything container names, or [`ABSENT_ARGUMENT_VALUE`] where the refusal is
    /// that the member held nothing at all.
    ///
    /// Only a refusal [`Self::code`] answers [`INVALID_ARGUMENT_CODE`] names an argument: the
    /// pair is that code's own document (Amazon S3 API Reference, *Error responses*).
    pub fn argument(&self) -> Option<(&'static str, Option<&str>)> {
        match self {
            Self::NoIndexDocument => Some(("IndexDocument", Some(ABSENT_ARGUMENT_VALUE))),
            Self::MalformedIndexDocumentSuffix { suffix } => {
                Some(("IndexDocument", Some(suffix.as_str())))
            }
            Self::MalformedErrorDocumentKey { key } => {
                Some(("ErrorDocument", Some(key.as_str())))
            }
            Self::RedirectAllNotExclusive { host_name } => {
                Some(("RedirectAllRequestsTo", Some(host_name.as_str())))
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for WebsiteConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The code's own FIXED published text, which states the fault class rather than
            // anything about this document.
            Self::MalformedDocument => write!(f, "{MALFORMED_XML_MESSAGE}"),
            Self::RedirectAllNotExclusive { .. } => write!(
                f,
                "RedirectAllRequestsTo cannot be provided in conjunction with other Routing \
                 Rules."
            ),
            Self::NoIndexDocument => write!(
                f,
                "A value for IndexDocument Suffix must be provided if RedirectAllRequestsTo \
                 is empty"
            ),
            Self::MalformedIndexDocumentSuffix { .. } => {
                write!(f, "The IndexDocument Suffix is not well formed")
            }
            Self::MalformedErrorDocumentKey { .. } => {
                write!(f, "The ErrorDocument Key is not well formed")
            }
            Self::TooManyRoutingRules { .. } => write!(
                f,
                "The number of routing rules must not exceed {MAX_WEBSITE_ROUTING_RULES}."
            ),
            Self::EmptyCondition => write!(
                f,
                "Condition cannot be empty. To redirect all requests without a condition, \
                 the condition element shouldn't be present."
            ),
            Self::ConflictingKeyReplacement => write!(
                f,
                "You can only define ReplaceKeyPrefix or ReplaceKey but not both."
            ),
            Self::UnsupportedProtocol => write!(
                f,
                "Invalid protocol, protocol can be http or https. If not defined the \
                 protocol will be selected automatically."
            ),
            // The two status-code sentences are the one place a REQUEST value reaches a
            // website refusal's text: S3 states the class the code had to be in and, in
            // parentheses, the code the request sent.
            Self::InvalidHttpRedirectCode { code } => write!(
                f,
                "The provided HTTP redirect code ({code}) is not valid. Valid codes are 3XX \
                 except 300."
            ),
            Self::InvalidHttpErrorCode { code } => write!(
                f,
                "The provided HTTP error code ({code}) is not valid. Valid codes are 4XX or \
                 5XX."
            ),
        }
    }
}

impl std::error::Error for WebsiteConfigurationRejection {}

/// Whether `configuration` is one S3 applies to a bucket, and what is wrong with it when it
/// is not.
///
/// Checked BEFORE anything is written, so a refused put leaves the bucket's current website
/// configuration exactly as it was — the same order every other bucket subresource put
/// keeps.
///
/// Two of the rules here are ALSO modelled constraints, which the request boundary would
/// refuse before any handler ran — `ErrorDocument$Key`'s `length {min 1}` and `Protocol`'s
/// value set — and S3 publishes a refusal of its own for each, which a boundary refusal
/// cannot state (it carries a code and a sentence, and the key's names the member and echoes
/// what it held). So a document carrying either is withheld from the codec
/// (`crate::service::wire`) and the refusal is read from the request's own document
/// ([`website_document_rejection`]), which is where those two values still exist. They are
/// still checked here because this function, not the wire, is what guards the STORE: a
/// configuration no reader could report back is never recorded, whoever writes it.
pub fn validate_website_configuration(
    configuration: &BucketWebsite,
) -> Result<(), WebsiteConfigurationRejection> {
    if let Some(redirect_all) = &configuration.redirect_all_requests_to {
        // "If you specify this property, you can't specify any other property"
        // (`WebsiteConfiguration$RedirectAllRequestsTo`).
        if configuration.index_document_suffix.is_some()
            || configuration.error_document_key.is_some()
            || !configuration.routing_rules.is_empty()
        {
            return Err(WebsiteConfigurationRejection::RedirectAllNotExclusive {
                host_name: redirect_all.host_name.clone(),
            });
        }
        check_protocol(&redirect_all.protocol)?;
        return Ok(());
    }

    // No redirect-everything container, so the configuration serves the bucket as a website
    // and "must provide an index document for the bucket" (`PutBucketWebsite`).
    let Some(suffix) = configuration.index_document_suffix.as_deref() else {
        return Err(WebsiteConfigurationRejection::NoIndexDocument);
    };
    if suffix.is_empty() || suffix.contains('/') {
        return Err(WebsiteConfigurationRejection::MalformedIndexDocumentSuffix {
            suffix: suffix.to_string(),
        });
    }
    if let Some(key) = configuration
        .error_document_key
        .as_deref()
        .filter(|key| key.is_empty())
    {
        return Err(WebsiteConfigurationRejection::MalformedErrorDocumentKey {
            key: key.to_string(),
        });
    }
    if configuration.routing_rules.len() > MAX_WEBSITE_ROUTING_RULES {
        return Err(WebsiteConfigurationRejection::TooManyRoutingRules {
            count: configuration.routing_rules.len(),
        });
    }

    for rule in &configuration.routing_rules {
        if let Some(condition) = rule.condition.as_ref() {
            if condition.is_empty() {
                return Err(WebsiteConfigurationRejection::EmptyCondition);
            }
            if let Some(code) = condition.http_error_code_returned_equals.as_deref() {
                let status = code.parse::<u16>().ok();
                if !status.is_some_and(|status| HTTP_ERROR_CODES.contains(&status)) {
                    return Err(WebsiteConfigurationRejection::InvalidHttpErrorCode {
                        code: code.to_string(),
                    });
                }
            }
        }
        if rule.redirect.replace_key_prefix_with.is_some()
            && rule.redirect.replace_key_with.is_some()
        {
            return Err(WebsiteConfigurationRejection::ConflictingKeyReplacement);
        }
        check_protocol(&rule.redirect.protocol)?;
        if let Some(code) = rule.redirect.http_redirect_code.as_deref() {
            let status = code.parse::<u16>().ok();
            if !status.is_some_and(|status| HTTP_REDIRECT_CODES.contains(&status)) {
                return Err(WebsiteConfigurationRejection::InvalidHttpRedirectCode {
                    code: code.to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Refuse a `Protocol` value outside [`PROTOCOL_VALUES`].
fn check_protocol(protocol: &Option<String>) -> Result<(), WebsiteConfigurationRejection> {
    match protocol.as_deref() {
        Some(value) if !PROTOCOL_VALUES.contains(&value) => {
            Err(WebsiteConfigurationRejection::UnsupportedProtocol)
        }
        _ => Ok(()),
    }
}

/// Every element the published `WebsiteConfiguration` schema declares, with the elements
/// each one may contain — the ROOT element first.
///
/// Source: the `WebsiteConfiguration` shape closure
/// (`smithy_query(query="shape",shape="WebsiteConfiguration,ErrorDocument,IndexDocument,
/// RedirectAllRequestsTo,RoutingRules,RoutingRule,Condition,Redirect",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`).
/// Each structure's children are its members under the element name restXml spells them with
/// — their own declared names, none of the closure's members being renamed — and the one
/// list in the closure, `RoutingRules`, under its member's `xmlName` (`RoutingRule`). A
/// member whose target is a plain string is a LEAF, so it declares no child element.
///
/// What makes this a *schema*, rather than a list of words a document may contain: an
/// element is judged against the children of the element it arrived IN, so a declared name
/// in a position the schema does not declare it in fails the same way an undeclared one
/// does.
///
/// `pub`, with [`WEBSITE_DOCUMENT_REQUIRED_ELEMENTS`]: `tests/all/bucket_website_refusals.rs`
/// re-derives both from the model itself, so a model update that renames, adds or removes an
/// element of this closure cannot leave the table behind.
pub const WEBSITE_DOCUMENT_SCHEMA: &[(&str, &[&str])] = &[
    (
        "WebsiteConfiguration",
        &[
            "ErrorDocument",
            "IndexDocument",
            "RedirectAllRequestsTo",
            "RoutingRules",
        ],
    ),
    ("ErrorDocument", &["Key"]),
    ("IndexDocument", &["Suffix"]),
    ("RedirectAllRequestsTo", &["HostName", "Protocol"]),
    ("RoutingRules", &["RoutingRule"]),
    ("RoutingRule", &["Condition", "Redirect"]),
    (
        "Condition",
        &["HttpErrorCodeReturnedEquals", "KeyPrefixEquals"],
    ),
    (
        "Redirect",
        &[
            "HostName",
            "HttpRedirectCode",
            "Protocol",
            "ReplaceKeyPrefixWith",
            "ReplaceKeyWith",
        ],
    ),
];

/// The child element each of [`WEBSITE_DOCUMENT_SCHEMA`]'s containers REQUIRES.
///
/// Source: the same shape closure, whose `required` members are `IndexDocument$Suffix`,
/// `ErrorDocument$Key`, `RedirectAllRequestsTo$HostName` and `RoutingRule$Redirect`. A
/// container that names none of them does not validate against the published schema, however
/// well spelled the children it does name are.
///
/// `pub` for the reason [`WEBSITE_DOCUMENT_SCHEMA`] is: the same test re-derives it from the
/// model.
pub const WEBSITE_DOCUMENT_REQUIRED_ELEMENTS: &[(&str, &str)] = &[
    ("IndexDocument", "Suffix"),
    ("ErrorDocument", "Key"),
    ("RedirectAllRequestsTo", "HostName"),
    ("RoutingRule", "Redirect"),
];

/// The child element the schema requires inside `element`, or `None` for one that requires
/// none.
fn required_element(element: &str) -> Option<&'static str> {
    WEBSITE_DOCUMENT_REQUIRED_ELEMENTS
        .iter()
        .find(|(container, _)| *container == element)
        .map(|(_, required)| *required)
}

/// What S3 refuses the website document `document` for, read from the DOCUMENT itself rather
/// than from the configuration parsed out of it, or `None` for a document it refuses nothing
/// about.
///
/// Three refusals can only be read here, and each is one the REQUEST BOUNDARY would otherwise
/// answer before `PutBucketWebsite` ran — in a document that carries a code and a sentence and
/// no room for the `<ArgumentName>` / `<ArgumentValue>` pair S3 names a refused member by. So
/// a document this reports a rejection for is withheld from the codec in full
/// (`crate::service::wire`), leaving the operation the ONE place a website document's refusal
/// is worded:
///
/// * a document that does not validate against the schema S3 publishes for it
///   ([`website_document_validates`]), which is a DIFFERENT answer from a refusal of the
///   configuration the document describes: a codec reads an element it does not know by
///   ignoring it, so a document that misspells, misnests or invents an element parses to a
///   configuration MISSING the member that element was meant to carry, and refusing that
///   configuration would state the absence of a member the caller did spell, under a code
///   saying the value was wrong rather than the document;
/// * an `ErrorDocument` whose `Key` is EMPTY, which `ObjectKey`'s `length {min 1}` makes a
///   modelled violation; and
/// * a `Protocol` outside the two published schemes, which the modelled `Protocol` enum makes
///   one.
///
/// Read from the document and not from the input because that is the only place these values
/// survive: a parsed input carries no element the codec did not know, and carries nothing at
/// all for a request the boundary refused.
pub fn website_document_rejection(document: &[u8]) -> Option<WebsiteConfigurationRejection> {
    if !website_document_validates(document) {
        return Some(WebsiteConfigurationRejection::MalformedDocument);
    }
    if website_document_names_empty_error_document_key(document) {
        return Some(WebsiteConfigurationRejection::MalformedErrorDocumentKey {
            key: String::new(),
        });
    }
    website_document_names_unsupported_protocol(document)
        .then_some(WebsiteConfigurationRejection::UnsupportedProtocol)
}

/// Whether `document` names a `Protocol` — on either member that carries one — outside
/// [`PROTOCOL_VALUES`].
///
/// ANY one such element is enough, so the walk answers on the first: a document naming a
/// scheme the service does not make is refused for that element whatever the others carry.
///
/// The text is compared with its surrounding whitespace trimmed, which is how the codec reads
/// an element's text, so this and the parsed input cannot disagree about which values are the
/// two published schemes.
fn website_document_names_unsupported_protocol(document: &[u8]) -> bool {
    let mut reader = ::quick_xml::Reader::from_reader(document);
    // Whether the reader is inside a `Protocol` element, and whether that ONE element has
    // carried a published scheme yet: an element that opens and closes with no text between
    // carries the empty string, which is no scheme either, so each element starts unsatisfied
    // and is settled by the text inside it.
    let mut inside = false;
    let mut published = false;
    loop {
        match reader.read_event() {
            Ok(::quick_xml::events::Event::Start(start)) => {
                inside = start.name().as_ref() == PROTOCOL_ELEMENT;
                published = false;
            }
            // A self-closing `<Protocol/>` carries no text at all, so it names no scheme.
            Ok(::quick_xml::events::Event::Empty(empty)) => {
                if empty.name().as_ref() == PROTOCOL_ELEMENT {
                    return true;
                }
            }
            Ok(::quick_xml::events::Event::Text(text)) if inside => {
                let spelled = text.xml10_content().unwrap_or_default();
                published = PROTOCOL_VALUES.contains(&spelled.trim());
            }
            Ok(::quick_xml::events::Event::End(_)) => {
                if inside && !published {
                    return true;
                }
                inside = false;
            }
            Ok(::quick_xml::events::Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

/// The element both members that carry a `Protocol` are spelled with: the member's own
/// declared name, which restXml writes it under (`smithy_query(query="shape",
/// shape="RedirectAllRequestsTo,Redirect")` reports no `xmlName` on either).
const PROTOCOL_ELEMENT: &[u8] = b"Protocol";

/// Whether `document` names an `ErrorDocument` whose `Key` element carries no text — the
/// empty object key [`website_document_rejection`] reads from the document.
fn website_document_names_empty_error_document_key(document: &[u8]) -> bool {
    let mut reader = ::quick_xml::Reader::from_reader(document);
    // Where in the `ErrorDocument` container the reader is, and whether the `Key` inside it
    // has carried any text.
    let mut inside_container = false;
    let mut inside_key = false;
    let mut empty = false;
    loop {
        match reader.read_event() {
            Ok(::quick_xml::events::Event::Start(start)) => {
                match start.name().as_ref() {
                    b"ErrorDocument" => inside_container = true,
                    b"Key" if inside_container => {
                        inside_key = true;
                        // Nothing read yet, so the key is empty until a text event says
                        // otherwise.
                        empty = true;
                    }
                    _ => {}
                }
            }
            // A self-closing `<Key/>` carries no text at all.
            Ok(::quick_xml::events::Event::Empty(start)) => {
                if inside_container && start.name().as_ref() == b"Key" {
                    empty = true;
                }
            }
            Ok(::quick_xml::events::Event::Text(text)) if inside_key => {
                empty = text.is_empty();
            }
            Ok(::quick_xml::events::Event::End(end)) => match end.name().as_ref() {
                b"Key" => inside_key = false,
                b"ErrorDocument" => return empty,
                _ => {}
            },
            Ok(::quick_xml::events::Event::Eof) | Err(_) => return empty,
            _ => {}
        }
    }
}

/// Whether `document` — the XML a `PutBucketWebsite` request carried as its payload —
/// validates against the schema S3 publishes for it ([`WEBSITE_DOCUMENT_SCHEMA`]).
///
/// `false` for a document that is not a well-formed XML document at all — bytes no XML
/// reader can walk, an element left unclosed, an end tag naming another element, or no
/// element whatever — which is the same refusal: the code's own sentence states both ("not
/// well-formed or did not validate against our published schema").
fn website_document_validates(document: &[u8]) -> bool {
    let mut reader = ::quick_xml::Reader::from_reader(document);
    // The trail of element names from the root down to the element being read, each with the
    // child the schema requires inside it until that child is seen: an element is judged
    // against the children its PARENT declares, and a container against the child it owes.
    let mut open: Vec<(&'static str, Option<&'static str>)> = Vec::new();
    // Whether the document has a root element at all, which an XML document has by
    // definition and bytes that are not markup do not.
    let mut rooted = false;
    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            // Not well formed.
            Err(_) => return false,
        };
        let parent = open.last().map(|(element, _)| *element);
        match event {
            ::quick_xml::events::Event::Start(start) => {
                let Some(element) = declared_element(parent, start.name().as_ref()) else {
                    return false;
                };
                rooted = true;
                satisfy_required(&mut open, element);
                open.push((element, required_element(element)));
            }
            // A self-closing element (`<Condition/>`) is an element with no children at
            // all: the name is judged the same way, and a container that owes one is not
            // satisfied by it.
            ::quick_xml::events::Event::Empty(start) => {
                let Some(element) = declared_element(parent, start.name().as_ref()) else {
                    return false;
                };
                if required_element(element).is_some() {
                    return false;
                }
                rooted = true;
                satisfy_required(&mut open, element);
            }
            ::quick_xml::events::Event::End(_) => match open.pop() {
                // A container that never named the child the schema requires inside it.
                Some((_, Some(_))) => return false,
                Some((_, None)) => {}
                None => return false,
            },
            ::quick_xml::events::Event::Eof => return rooted && open.is_empty(),
            _ => {}
        }
    }
}

/// Note `element` as named inside the container it arrived in, which is what satisfies that
/// container's required child.
fn satisfy_required(open: &mut [(&'static str, Option<&'static str>)], element: &'static str) {
    if let Some((_, required)) = open.last_mut() {
        if *required == Some(element) {
            *required = None;
        }
    }
}

/// The element [`WEBSITE_DOCUMENT_SCHEMA`] declares under `parent` (or as the ROOT, for a
/// `parent` of `None`) whose name is `element`, or `None` for a name the schema does not
/// declare there.
fn declared_element(parent: Option<&str>, element: &[u8]) -> Option<&'static str> {
    let declared: &[&'static str] = match parent {
        // The root element of the document: the container the payload IS.
        None => &[WEBSITE_DOCUMENT_SCHEMA[0].0],
        Some(parent) => WEBSITE_DOCUMENT_SCHEMA
            .iter()
            .find(|(name, _)| *name == parent)
            // A LEAF carries text, not elements, so nothing is declared inside it.
            .map_or(&[], |(_, children)| *children),
    };
    declared
        .iter()
        .copied()
        .find(|name| name.as_bytes() == element)
}

/// How many `CORSRule`s one bucket's cors configuration may carry.
///
/// The published limit, from `CORSConfiguration$CORSRules`'s own documentation: "A set of
/// origins and methods (cross-origin access that you want to allow). You can add up to 100
/// rules to the configuration." The model bounds the list at neither end
/// (`smithy_query(query="shape",shape="CORSRules")`), so this check is what enforces it —
/// and it also bounds what one bucket's record parks in this process's memory. A put past
/// it is refused rather than recorded.
pub const MAX_CORS_RULES: usize = 100;

/// The longest `CORSRule$ID` a stored rule may carry, in characters.
///
/// That member's own documentation: "Unique identifier for the rule. The value cannot be
/// longer than 255 characters." `ID` is a plain string with no `@length` in the model
/// (`smithy_query(query="shape",shape="ID")`), so nothing refuses a longer id before this
/// check does.
pub const MAX_CORS_RULE_ID_CHARS: usize = 255;

/// The HTTP methods a `CORSRule$AllowedMethods` entry may name.
///
/// That member's own documentation: "An HTTP method that you allow the origin to execute.
/// Valid values are `GET`, `PUT`, `HEAD`, `POST`, and `DELETE`." `AllowedMethod` is a plain
/// string in the model (`smithy_query(query="shape",shape="AllowedMethods")`), not an enum,
/// so this catalog is what refuses a method S3 does not support — including one that names
/// a supported method in another case, since the published values are upper case.
pub const CORS_ALLOWED_METHODS: [&str; 5] = ["GET", "PUT", "HEAD", "POST", "DELETE"];

/// The most bytes a bucket's cors configuration document may carry.
///
/// `PutBucketCors`'s own documentation: "The cors subresource is an XML document in which
/// you configure rules that identify origins and the HTTP methods that can be executed on
/// your bucket. The document is limited to 64 KB in size." Measured over the stored rules by
/// [`cors_configuration_bytes`], which is also what keeps one bucket's record from parking
/// an arbitrarily large configuration in this process's memory.
pub const MAX_CORS_CONFIGURATION_BYTES: usize = 64 * 1024;

/// One `CORSRule` of a bucket's cors configuration: "Specifies a cross-origin access rule
/// for an Amazon S3 bucket."
///
/// Every member is optional exactly where the model makes it so
/// (`smithy_query(query="shape",shape="CORSRule")`): `AllowedMethods` and `AllowedOrigins`
/// are REQUIRED, the other three are not. The two optional lists are plain `Vec`s rather
/// than `Option`s for the reason [`BucketWebsite::routing_rules`] is: both are
/// `xmlFlattened`, so the XML boundary parses a document naming no `AllowedHeader` to the
/// same absent list as one naming none at all, and there is nothing for a `None` to mean
/// that an empty list does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketCorsRule {
    /// `CORSRule$ID`, or `None` when the rule names none.
    pub id: Option<String>,
    /// `CORSRule$AllowedHeaders`: "Headers that are specified in the
    /// `Access-Control-Request-Headers` header", in the order the document named them.
    pub allowed_headers: Vec<String>,
    /// `CORSRule$AllowedMethods`, a REQUIRED member: each one of
    /// [`CORS_ALLOWED_METHODS`], in the order the document named them.
    pub allowed_methods: Vec<String>,
    /// `CORSRule$AllowedOrigins`, a REQUIRED member: "One or more origins you want
    /// customers to be able to access the bucket from", in the order the document named
    /// them.
    pub allowed_origins: Vec<String>,
    /// `CORSRule$ExposeHeaders`: "One or more headers in the response that you want
    /// customers to be able to access from their applications", in document order.
    pub expose_headers: Vec<String>,
    /// `CORSRule$MaxAgeSeconds`: "The time in seconds that your browser is to cache the
    /// preflight response for the specified resource", or `None` when the rule names none.
    pub max_age_seconds: Option<i32>,
}

/// The size of the smallest `CORSConfiguration` document that carries `rules`, in bytes.
///
/// A LOWER bound on the document the request carried — the markup of each element plus the
/// text inside it, with no attributes, namespace or whitespace — so a configuration this
/// reports more than [`MAX_CORS_CONFIGURATION_BYTES`] for could not have arrived in a
/// document within the published 64 KB. Element names are the `xmlName`s the model puts on
/// the members (`smithy_query(query="shape",shape="CORSRule")`), which is also how
/// `GetBucketCors` spells them on the way out.
pub fn cors_configuration_bytes(rules: &[BucketCorsRule]) -> usize {
    /// The bytes `<name>text</name>` takes.
    fn element(name: &str, text: &str) -> usize {
        name.len() * 2 + "<></>".len() + text.len()
    }
    /// The bytes every entry of a flattened list named `name` takes.
    fn elements(name: &str, texts: &[String]) -> usize {
        texts.iter().map(|text| element(name, text)).sum()
    }

    let mut bytes = element("CORSConfiguration", "");
    for rule in rules {
        bytes += element("CORSRule", "");
        if let Some(id) = &rule.id {
            bytes += element("ID", id);
        }
        bytes += elements("AllowedHeader", &rule.allowed_headers);
        bytes += elements("AllowedMethod", &rule.allowed_methods);
        bytes += elements("AllowedOrigin", &rule.allowed_origins);
        bytes += elements("ExposeHeader", &rule.expose_headers);
        if let Some(max_age_seconds) = rule.max_age_seconds {
            bytes += element("MaxAgeSeconds", &max_age_seconds.to_string());
        }
    }
    bytes
}

/// Why a cors configuration is not one S3 would apply to a bucket.
///
/// Each variant names the member that is wrong and the published constraint it breaks, so
/// the refusal's message states the rule the caller has to satisfy rather than one request's
/// wording. [`Self::code`] is the wire error code each one goes out with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsConfigurationRejection {
    /// A `CORSConfiguration` naming no `CORSRule` at all: `CORSConfiguration$CORSRules` is
    /// a REQUIRED member, so a document naming none does not validate against the schema
    /// S3 publishes.
    NoRules,
    /// More rules than [`MAX_CORS_RULES`].
    TooManyRules {
        /// How many the document carried.
        count: usize,
    },
    /// A rule naming no `AllowedMethod`: `CORSRule$AllowedMethods` is a REQUIRED member.
    NoAllowedMethods {
        /// Which rule carries it, counted from 1 as the document orders them.
        rule: usize,
    },
    /// A rule naming no `AllowedOrigin`: `CORSRule$AllowedOrigins` is a REQUIRED member.
    NoAllowedOrigins {
        /// Which rule carries it, counted from 1.
        rule: usize,
    },
    /// An `AllowedMethod` outside [`CORS_ALLOWED_METHODS`].
    UnsupportedMethod {
        /// Which rule carries it, counted from 1.
        rule: usize,
        /// The value that is not a published method.
        method: String,
    },
    /// An `ID` longer than [`MAX_CORS_RULE_ID_CHARS`] characters.
    IdTooLong {
        /// Which rule carries it, counted from 1.
        rule: usize,
        /// How many characters it carried.
        length: usize,
    },
    /// A `MaxAgeSeconds` below zero, which is no cache time: the member is "the time in
    /// seconds that your browser is to cache the preflight response".
    NegativeMaxAgeSeconds {
        /// Which rule carries it, counted from 1.
        rule: usize,
        /// The value that is not a time in seconds.
        seconds: i32,
    },
    /// A configuration whose document could not fit in
    /// [`MAX_CORS_CONFIGURATION_BYTES`]. See [`cors_configuration_bytes`].
    DocumentTooLarge {
        /// The size of the smallest document carrying these rules.
        bytes: usize,
    },
}

impl CorsConfigurationRejection {
    /// The wire error code S3 answers this refusal with.
    ///
    /// Two codes from S3's published "Error responses" list, both HTTP 400: a document that
    /// does not satisfy the schema the API publishes is [`MALFORMED_XML_CODE`] ("The XML
    /// provided does not match the schema"), and a document that satisfies the schema but
    /// carries a value or a count the API does not accept — an HTTP method outside the
    /// published set above all — is [`INVALID_REQUEST_CODE`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoRules | Self::NoAllowedMethods { .. } | Self::NoAllowedOrigins { .. } => {
                MALFORMED_XML_CODE
            }
            Self::TooManyRules { .. }
            | Self::UnsupportedMethod { .. }
            | Self::IdTooLong { .. }
            | Self::NegativeMaxAgeSeconds { .. }
            | Self::DocumentTooLarge { .. } => INVALID_REQUEST_CODE,
        }
    }
}

impl std::fmt::Display for CorsConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRules => write!(
                f,
                "CORSConfiguration.CORSRules names no CORSRule; a cors configuration \
                 carries at least one rule"
            ),
            Self::TooManyRules { count } => write!(
                f,
                "CORSConfiguration.CORSRules carries {count} rules, more than the \
                 {MAX_CORS_RULES} rules one cors configuration may hold"
            ),
            Self::NoAllowedMethods { rule } => write!(
                f,
                "CORSRule {rule}: AllowedMethods names no AllowedMethod; a cors rule \
                 allows at least one method"
            ),
            Self::NoAllowedOrigins { rule } => write!(
                f,
                "CORSRule {rule}: AllowedOrigins names no AllowedOrigin; a cors rule \
                 allows at least one origin"
            ),
            Self::UnsupportedMethod { rule, method } => write!(
                f,
                "CORSRule {rule}: AllowedMethod must be one of {}, got {method:?}",
                CORS_ALLOWED_METHODS.join(", ")
            ),
            Self::IdTooLong { rule, length } => write!(
                f,
                "CORSRule {rule}: ID is {length} characters long, more than the \
                 {MAX_CORS_RULE_ID_CHARS} characters a rule identifier may carry"
            ),
            Self::NegativeMaxAgeSeconds { rule, seconds } => write!(
                f,
                "CORSRule {rule}: MaxAgeSeconds is a time in seconds to cache the \
                 preflight response and cannot be negative, got {seconds}"
            ),
            Self::DocumentTooLarge { bytes } => write!(
                f,
                "the cors configuration takes {bytes} bytes, more than the \
                 {MAX_CORS_CONFIGURATION_BYTES} bytes a cors document may carry"
            ),
        }
    }
}

impl std::error::Error for CorsConfigurationRejection {}

/// Whether `rules` may be stored as a bucket's cors configuration, reporting the first
/// published rule they break.
///
/// Checked BEFORE anything is written, so a refused put leaves the bucket's current cors
/// configuration exactly as it was — the order every other bucket subresource put keeps.
/// The counts are judged before the per-rule values so an oversized document is refused for
/// what it is rather than for whichever rule happens to come first.
///
/// Two of the rules here (both required lists being non-empty) are ALSO what the modelled
/// `@required` traits express, and the boundary validator refuses a payload missing either
/// member before any handler runs. They are still checked here because this function, not
/// the wire, is what guards the STORE: a configuration no reader could report back is never
/// recorded, whoever writes it.
pub fn validate_cors_configuration(
    rules: &[BucketCorsRule],
) -> Result<(), CorsConfigurationRejection> {
    if rules.is_empty() {
        return Err(CorsConfigurationRejection::NoRules);
    }
    if rules.len() > MAX_CORS_RULES {
        return Err(CorsConfigurationRejection::TooManyRules { count: rules.len() });
    }
    let bytes = cors_configuration_bytes(rules);
    if bytes > MAX_CORS_CONFIGURATION_BYTES {
        return Err(CorsConfigurationRejection::DocumentTooLarge { bytes });
    }

    for (index, rule) in rules.iter().enumerate() {
        // Counted the way the document orders the rules, so the message names the rule a
        // caller can find.
        let position = index + 1;
        if rule.allowed_methods.is_empty() {
            return Err(CorsConfigurationRejection::NoAllowedMethods { rule: position });
        }
        if rule.allowed_origins.is_empty() {
            return Err(CorsConfigurationRejection::NoAllowedOrigins { rule: position });
        }
        for method in &rule.allowed_methods {
            if !CORS_ALLOWED_METHODS.contains(&method.as_str()) {
                return Err(CorsConfigurationRejection::UnsupportedMethod {
                    rule: position,
                    method: method.clone(),
                });
            }
        }
        if let Some(id) = rule.id.as_deref() {
            let length = id.chars().count();
            if length > MAX_CORS_RULE_ID_CHARS {
                return Err(CorsConfigurationRejection::IdTooLong {
                    rule: position,
                    length,
                });
            }
        }
        if let Some(seconds) = rule.max_age_seconds {
            if seconds < 0 {
                return Err(CorsConfigurationRejection::NegativeMaxAgeSeconds {
                    rule: position,
                    seconds,
                });
            }
        }
    }

    Ok(())
}

/// How many rules one lifecycle configuration may carry: "An Amazon S3 Lifecycle
/// configuration can have up to 1,000 rules. This limit is not adjustable"
/// (`PutBucketLifecycleConfiguration`'s own documentation).
pub const MAX_LIFECYCLE_RULES: usize = 1_000;

/// The longest rule identifier a lifecycle rule may carry: "Unique identifier for the
/// rule. The value cannot be longer than 255 characters"
/// (`smithy_query(query="shape",shape="LifecycleRule")`, member `ID`). The `ID` shape
/// itself carries no `@length`, so nothing refuses a longer one before this check.
pub const MAX_LIFECYCLE_RULE_ID_CHARS: usize = 255;

/// How many noncurrent versions a lifecycle rule may ask to retain: "You can specify up to
/// 100 noncurrent versions to retain"
/// (`NoncurrentVersionExpiration$NewerNoncurrentVersions`'s own documentation).
pub const MAX_NEWER_NONCURRENT_VERSIONS: i32 = 100;

/// Days an object must spend in its current class before a rule may transition it to one of
/// the infrequent-access classes.
///
/// From the Amazon S3 User Guide's transition constraints: objects must be stored at least
/// 30 days in the current storage class before Amazon S3 transitions them to
/// `STANDARD_IA` or `ONEZONE_IA`.
pub const MINIMUM_DAYS_BEFORE_INFREQUENT_ACCESS: i32 = 30;

/// The default minimum-object-size behaviour a lifecycle configuration carries when the
/// request names no `TransitionDefaultMinimumObjectSize`.
///
/// One of that member's two values
/// (`smithy_query(query="shape",shape="TransitionDefaultMinimumObjectSize")`), and the one
/// whose documentation states the DEFAULT: "`all_storage_classes_128K` - Objects smaller
/// than 128 KB will not transition to any storage class by default."
pub const DEFAULT_TRANSITION_MINIMUM_OBJECT_SIZE: &str = "all_storage_classes_128K";

/// The storage classes a lifecycle transition may name, WARMEST FIRST, each with its
/// published minimum storage duration in days.
///
/// The ORDER is the one the Amazon S3 User Guide's transition chart allows a lifecycle
/// rule to follow — Standard-IA, then Intelligent-Tiering, then One Zone-IA, then Glacier
/// Instant Retrieval, then Glacier Flexible Retrieval, then Glacier Deep Archive — and
/// transitions in the other direction, from a colder class to a warmer one, are not
/// supported at all. The DAYS are each class's minimum storage duration from the same
/// guide's storage-class comparison (30 days for the two infrequent-access classes, 90 for
/// the two Glacier retrieval classes, 180 for Deep Archive, none for Intelligent-Tiering),
/// which is how long an object stays in a class before a later transition may move it on.
///
/// Every value is one of `TransitionStorageClass`'s enum values
/// (`smithy_query(query="shape",shape="TransitionStorageClass")`).
pub const TRANSITION_STORAGE_CLASS_CHAIN: [(&str, i32); 6] = [
    ("STANDARD_IA", 30),
    ("INTELLIGENT_TIERING", 0),
    ("ONEZONE_IA", 30),
    ("GLACIER_IR", 90),
    ("GLACIER", 90),
    ("DEEP_ARCHIVE", 180),
];

/// Where `storage_class` sits in [`TRANSITION_STORAGE_CLASS_CHAIN`] — the lower the rank,
/// the warmer the class — or `None` for a value that is not a transition storage class.
pub fn transition_storage_class_rank(storage_class: &str) -> Option<usize> {
    TRANSITION_STORAGE_CLASS_CHAIN
        .iter()
        .position(|(class, _)| *class == storage_class)
}

/// The published minimum storage duration of `storage_class`, in days, or `None` for a
/// value that is not a transition storage class. See [`TRANSITION_STORAGE_CLASS_CHAIN`].
pub fn minimum_storage_duration_days(storage_class: &str) -> Option<i32> {
    TRANSITION_STORAGE_CLASS_CHAIN
        .iter()
        .find(|(class, _)| *class == storage_class)
        .map(|(_, days)| *days)
}

/// Seconds in one day, the unit a lifecycle `Date` is judged in: the member's time "is
/// always midnight UTC" (`LifecycleExpiration$Date`), so the value is a whole number of
/// these.
const SECONDS_PER_DAY: i64 = 86_400;

/// Whether `date` is midnight UTC — the only time of day a lifecycle `Date` may carry:
/// "The date value must conform to the ISO 8601 format. The time is always midnight UTC"
/// (`LifecycleExpiration$Date`, and `Transition$Date` says the same).
fn is_utc_midnight(date: &::aws_smithy_types::DateTime) -> bool {
    date.subsec_nanos() == 0 && date.secs().rem_euclid(SECONDS_PER_DAY) == 0
}

/// The lifecycle configuration a bucket carries: the rules it was put with, and the
/// minimum-object-size behaviour that put applied to them.
///
/// The MODELLED rules are stored whole, the way [`Bucket::replication`] stores its
/// container: `Rules` is exactly what the put carries and what
/// `GetBucketLifecycleConfiguration` reports back, so a read answers the configuration the
/// bucket actually carries rather than a re-derived copy of it. Each rule's `ID` is filled
/// in here if the request omitted it ([`ComAmazonawsS3Amazons3State::put_lifecycle_configuration`]),
/// so every later read reports the same identifier.
#[derive(Debug, Clone, Default)]
pub struct BucketLifecycle {
    /// The configuration's rules, in the order the request carried them.
    pub rules: Vec<crate::ops::types::LifecycleRule>,
    /// Which default minimum object size behaviour applies to the configuration — the
    /// value the put named, or [`DEFAULT_TRANSITION_MINIMUM_OBJECT_SIZE`] when it named
    /// none. Reported back by both the put and the read.
    pub transition_default_minimum_object_size: String,
}

/// Why a lifecycle configuration may not be stored on a bucket.
///
/// Each variant names the member that is wrong and the published constraint it breaks, so
/// the refusal's message states the rule the caller has to satisfy rather than one
/// request's wording. [`Self::code`] is the wire error code each one goes out with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleConfigurationRejection {
    /// A configuration naming no rule at all: `BucketLifecycleConfiguration$Rules` is a
    /// REQUIRED member, so a document naming none does not validate against the schema S3
    /// publishes.
    NoRules,
    /// More rules than [`MAX_LIFECYCLE_RULES`].
    TooManyRules {
        /// How many the document carried.
        count: usize,
    },
    /// An `ID` longer than [`MAX_LIFECYCLE_RULE_ID_CHARS`] characters.
    IdTooLong {
        /// Which rule carries it, counted from 1 as the document orders them.
        rule: usize,
        /// How many characters it carried.
        length: usize,
    },
    /// Two rules carrying the same `ID`, which is "unique identifier for the rule".
    DuplicateId {
        /// Which rule repeats an identifier an earlier rule already carries.
        rule: usize,
        /// The repeated identifier.
        id: String,
    },
    /// A rule naming BOTH the legacy `Prefix` and a `Filter`: the two are alternatives —
    /// "`Filter` is required if the `LifecycleRule` does not contain a `Prefix` element"
    /// (`LifecycleRule$Filter`).
    PrefixAndFilter {
        /// Which rule names both.
        rule: usize,
    },
    /// A rule naming NEITHER a `Prefix` nor a `Filter`, so nothing says which objects it
    /// applies to. Same sentence as [`Self::PrefixAndFilter`].
    NoPrefixOrFilter {
        /// Which rule names neither.
        rule: usize,
    },
    /// One configuration mixing a legacy `Prefix` rule with a `Filter` rule: a
    /// configuration is written against one version of the API or the other, and the
    /// `Prefix` form is the one "supported for backward compatibility"
    /// (`PutBucketLifecycleConfiguration`'s own documentation).
    MixedPrefixAndFilterRules {
        /// The first rule naming a legacy `Prefix`.
        prefix_rule: usize,
        /// The first rule naming a `Filter`.
        filter_rule: usize,
    },
    /// A `Filter` naming more than one predicate directly: "A `Filter` must have exactly
    /// one of `Prefix`, `Tag`, `ObjectSizeGreaterThan`, `ObjectSizeLessThan`, or `And`
    /// specified" (`LifecycleRule$Filter`) — several conditions travel inside `And`.
    FilterNamesSeveralPredicates {
        /// Which rule carries that filter.
        rule: usize,
        /// How many predicates it named.
        named: usize,
    },
    /// An object-size bound below zero, which is no object size: the members are a
    /// "minimum object size to which the rule applies" and a maximum.
    NegativeObjectSize {
        /// Which rule carries it.
        rule: usize,
        /// The member that carried it.
        member: &'static str,
        /// The value that is not an object size.
        bytes: i64,
    },
    /// An object-size range no object can be in, because the minimum is not below the
    /// maximum.
    ObjectSizeRangeEmpty {
        /// Which rule carries it.
        rule: usize,
        /// The `ObjectSizeGreaterThan` it named.
        greater_than: i64,
        /// The `ObjectSizeLessThan` it named.
        less_than: i64,
    },
    /// A rule naming no action at all: a lifecycle rule carries "one or more lifecycle
    /// transition and expiration actions that you want Amazon S3 to perform on the objects
    /// identified by the filter" (`PutBucketLifecycleConfiguration`'s own documentation).
    NoAction {
        /// Which rule names none.
        rule: usize,
    },
    /// An `Expiration` naming more than one of `Days`, `Date` and
    /// `ExpiredObjectDeleteMarker`, which are alternatives: the delete-marker form "cannot
    /// be specified with Days or Date" (`LifecycleExpiration$ExpiredObjectDeleteMarker`).
    SeveralExpirationPredicates {
        /// Which rule carries that expiration.
        rule: usize,
        /// How many of the three it named.
        named: usize,
    },
    /// A day count that is not a non-zero positive integer, where the member's own
    /// documentation requires one ("The value must be a non-zero positive integer").
    NonPositiveDays {
        /// The member that carried it.
        member: &'static str,
        /// The action that member belongs to.
        action: &'static str,
    },
    /// A `Transition$Days` below zero: that one "can be `0` or any positive integer".
    NegativeDays {
        /// The member that carried it.
        member: &'static str,
        /// The action that member belongs to.
        action: &'static str,
    },
    /// A `Date` whose time of day is not midnight UTC. See [`is_utc_midnight`].
    DateNotMidnight {
        /// Which rule carries it.
        rule: usize,
        /// The member that carried it.
        member: &'static str,
    },
    /// One rule mixing `Date`-based and `Days`-based actions: a rule is written against
    /// one clock or the other, since a day count is measured from each object's own
    /// creation while a date is the same instant for every object.
    MixedDateAndDays {
        /// Which rule mixes them.
        rule: usize,
    },
    /// A transition action naming no storage class, so nothing says where the object goes.
    TransitionWithoutStorageClass {
        /// Which rule carries it.
        rule: usize,
        /// The action that carried it.
        member: &'static str,
    },
    /// An action naming no day count and no date, so nothing says when it is due.
    ActionWithoutWhen {
        /// Which rule carries it.
        rule: usize,
        /// The action that carried it.
        member: &'static str,
    },
    /// Two transitions of one rule naming the same storage class, so one of them could
    /// never move the object anywhere.
    DuplicateTransitionStorageClass {
        /// Which rule carries them.
        rule: usize,
        /// The class named twice.
        storage_class: String,
    },
    /// A transition to a WARMER class than the one an earlier transition of the same rule
    /// already moved the object to. See [`TRANSITION_STORAGE_CLASS_CHAIN`].
    TransitionToWarmerClass {
        /// Which rule carries them.
        rule: usize,
        /// The class the earlier transition names.
        earlier: String,
        /// The warmer class the later transition names.
        later: String,
    },
    /// A transition due before the class the object is in has been held for its minimum
    /// storage duration. See [`TRANSITION_STORAGE_CLASS_CHAIN`].
    TransitionBeforeMinimumDuration {
        /// Which rule carries them.
        rule: usize,
        /// The class the object is in.
        earlier: String,
        /// The class the later transition names.
        later: String,
        /// How many days apart the two transitions are.
        gap: i64,
        /// How many days apart they have to be.
        minimum: i32,
    },
    /// A transition to an infrequent-access class due before
    /// [`MINIMUM_DAYS_BEFORE_INFREQUENT_ACCESS`] days after the object was created.
    TransitionBeforeInfrequentAccessMinimum {
        /// Which rule carries it.
        rule: usize,
        /// The infrequent-access class it names.
        storage_class: String,
        /// The day count it names.
        days: i32,
    },
    /// A rule asking to retain more noncurrent versions than
    /// [`MAX_NEWER_NONCURRENT_VERSIONS`].
    TooManyNewerNoncurrentVersions {
        /// Which rule carries it.
        rule: usize,
        /// The count it named.
        count: i32,
    },
}

impl LifecycleConfigurationRejection {
    /// The wire error code S3 answers this refusal with.
    ///
    /// Three codes from S3's published "Error responses" list, all HTTP 400, chosen the way
    /// [`CorsConfigurationRejection::code`] chooses between the first two: a document that
    /// does not satisfy the schema the API publishes — the rule COUNT included, since the
    /// published schema is what bounds a configuration at [`MAX_LIFECYCLE_RULES`] rules —
    /// is [`MALFORMED_XML_CODE`]; a single member whose VALUE is outside what that member
    /// accepts, or whose value cannot hold beside the value of another member of the same
    /// action, is [`INVALID_ARGUMENT_CODE`]; and a configuration whose members are each
    /// acceptable but which together ask for something S3 cannot carry out is
    /// [`INVALID_REQUEST_CODE`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoRules
            | Self::TooManyRules { .. }
            | Self::PrefixAndFilter { .. }
            | Self::NoPrefixOrFilter { .. }
            | Self::FilterNamesSeveralPredicates { .. }
            | Self::SeveralExpirationPredicates { .. }
            | Self::TransitionWithoutStorageClass { .. }
            | Self::ActionWithoutWhen { .. } => MALFORMED_XML_CODE,
            Self::IdTooLong { .. }
            | Self::DuplicateId { .. }
            | Self::NonPositiveDays { .. }
            | Self::NegativeDays { .. }
            | Self::DateNotMidnight { .. }
            | Self::NegativeObjectSize { .. }
            | Self::TransitionToWarmerClass { .. }
            | Self::TransitionBeforeMinimumDuration { .. }
            | Self::TransitionBeforeInfrequentAccessMinimum { .. }
            | Self::TooManyNewerNoncurrentVersions { .. } => INVALID_ARGUMENT_CODE,
            Self::MixedPrefixAndFilterRules { .. }
            | Self::ObjectSizeRangeEmpty { .. }
            | Self::NoAction { .. }
            | Self::MixedDateAndDays { .. }
            | Self::DuplicateTransitionStorageClass { .. } => INVALID_REQUEST_CODE,
        }
    }

    /// The `<Message>` this refusal goes out with.
    ///
    /// A refusal S3 answers [`MALFORMED_XML_CODE`] with carries that code's own FIXED
    /// published text ([`MALFORMED_XML_MESSAGE`]) rather than anything about this
    /// configuration: the code says the document did not validate against the published
    /// schema, and the schema is the same for every document. Every other refusal states
    /// the rule the caller has to satisfy, which is what [`Display`](std::fmt::Display)
    /// writes.
    pub fn message(&self) -> String {
        if self.code() == MALFORMED_XML_CODE {
            return MALFORMED_XML_MESSAGE.to_string();
        }
        self.to_string()
    }
}

impl std::fmt::Display for LifecycleConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRules => write!(
                f,
                "LifecycleConfiguration.Rules names no Rule; a lifecycle configuration \
                 carries at least one rule"
            ),
            Self::TooManyRules { count } => write!(
                f,
                "LifecycleConfiguration.Rules carries {count} rules, more than the \
                 {MAX_LIFECYCLE_RULES} rules one lifecycle configuration may hold"
            ),
            Self::IdTooLong { rule, length } => write!(
                f,
                "Rule {rule}: ID is {length} characters long, more than the \
                 {MAX_LIFECYCLE_RULE_ID_CHARS} characters a rule identifier may carry"
            ),
            Self::DuplicateId { rule, id } => write!(
                f,
                "Rule {rule}: ID {id:?} identifies an earlier rule as well; a rule \
                 identifier is unique within a lifecycle configuration"
            ),
            Self::PrefixAndFilter { rule } => write!(
                f,
                "Rule {rule}: names both Prefix and Filter; a rule identifies its objects \
                 with one or the other"
            ),
            Self::NoPrefixOrFilter { rule } => write!(
                f,
                "Rule {rule}: names neither Prefix nor Filter; Filter is required when a \
                 rule carries no Prefix"
            ),
            Self::MixedPrefixAndFilterRules { .. } => write!(
                f,
                "Filter element can only be used in Lifecycle V2."
            ),
            Self::FilterNamesSeveralPredicates { rule, named } => write!(
                f,
                "Rule {rule}: Filter names {named} of Prefix, Tag, ObjectSizeGreaterThan, \
                 ObjectSizeLessThan and And; a filter names exactly one of them, and \
                 several conditions travel inside And"
            ),
            Self::NegativeObjectSize {
                rule,
                member,
                bytes,
            } => write!(
                f,
                "Rule {rule}: {member} is an object size in bytes and cannot be negative, \
                 got {bytes}"
            ),
            Self::ObjectSizeRangeEmpty {
                rule,
                greater_than,
                less_than,
            } => write!(
                f,
                "Rule {rule}: ObjectSizeGreaterThan {greater_than} is not below \
                 ObjectSizeLessThan {less_than}, so no object size matches the filter"
            ),
            Self::NoAction { rule } => write!(
                f,
                "Rule {rule}: names no action; a lifecycle rule carries at least one \
                 expiration or transition action"
            ),
            Self::SeveralExpirationPredicates { rule, named } => write!(
                f,
                "Rule {rule}: Expiration names {named} of Days, Date and \
                 ExpiredObjectDeleteMarker; an expiration names exactly one of them"
            ),
            Self::NonPositiveDays { member, action } => write!(
                f,
                "'{member}' for {action} action must be a positive integer"
            ),
            Self::NegativeDays { member, action } => write!(
                f,
                "'{member}' for {action} action must be 0 or a positive integer"
            ),
            Self::DateNotMidnight { rule, member } => write!(
                f,
                "Rule {rule}: {member} must be at midnight UTC"
            ),
            Self::MixedDateAndDays { rule } => write!(
                f,
                "Rule {rule}: mixes Date-based and Days-based actions; a rule measures \
                 every action from one of the two"
            ),
            Self::TransitionWithoutStorageClass { rule, member } => write!(
                f,
                "Rule {rule}: {member} names no StorageClass, so nothing says which \
                 storage class the object transitions to"
            ),
            Self::ActionWithoutWhen { rule, member } => write!(
                f,
                "Rule {rule}: {member} says when it is due with neither a day count nor a \
                 date"
            ),
            Self::DuplicateTransitionStorageClass {
                rule,
                storage_class,
            } => write!(
                f,
                "Rule {rule}: two transitions name storage class {storage_class:?}"
            ),
            Self::TransitionToWarmerClass {
                rule,
                earlier,
                later,
            } => write!(
                f,
                "Rule {rule}: transitions to {earlier:?} and then to the warmer class \
                 {later:?}; transitions follow the order {}",
                TRANSITION_STORAGE_CLASS_CHAIN
                    .iter()
                    .map(|(class, _)| *class)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
            Self::TransitionBeforeMinimumDuration {
                rule,
                earlier,
                later,
                gap,
                minimum,
            } => write!(
                f,
                "Rule {rule}: transitions to {later:?} {gap} days after the transition to \
                 {earlier:?}, which has a minimum storage duration of {minimum} days"
            ),
            Self::TransitionBeforeInfrequentAccessMinimum {
                rule,
                storage_class,
                days,
            } => write!(
                f,
                "Rule {rule}: transitions to {storage_class:?} after {days} days, before \
                 the {MINIMUM_DAYS_BEFORE_INFREQUENT_ACCESS} days an object is stored in \
                 its current class first"
            ),
            Self::TooManyNewerNoncurrentVersions { rule, count } => write!(
                f,
                "Rule {rule}: NewerNoncurrentVersions retains {count} versions, more than \
                 the {MAX_NEWER_NONCURRENT_VERSIONS} noncurrent versions a rule may retain"
            ),
        }
    }
}

impl std::error::Error for LifecycleConfigurationRejection {}

/// Whether `rules` may be stored as a bucket's lifecycle configuration, reporting the first
/// published rule they break.
///
/// Checked BEFORE anything is written, so a refused put leaves the bucket's current
/// lifecycle configuration exactly as it was — the order every other bucket subresource put
/// keeps. The counts are judged before the per-rule values, so a configuration past the
/// published rule ceiling is refused for that rather than for whichever rule happens to
/// come first.
pub fn validate_lifecycle_configuration(
    rules: &[crate::ops::types::LifecycleRule],
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    if rules.is_empty() {
        return Err(Rejection::NoRules);
    }
    if rules.len() > MAX_LIFECYCLE_RULES {
        return Err(Rejection::TooManyRules { count: rules.len() });
    }

    let mut ids: BTreeSet<&str> = BTreeSet::new();
    let mut prefix_rule: Option<usize> = None;
    let mut filter_rule: Option<usize> = None;

    for (index, rule) in rules.iter().enumerate() {
        // Counted from 1, the way the document orders the rules, so a message names the
        // rule a caller can find in what it sent.
        let position = index + 1;

        if let Some(id) = rule.id.as_deref() {
            let length = id.chars().count();
            if length > MAX_LIFECYCLE_RULE_ID_CHARS {
                return Err(Rejection::IdTooLong {
                    rule: position,
                    length,
                });
            }
            if !ids.insert(id) {
                return Err(Rejection::DuplicateId {
                    rule: position,
                    id: id.to_string(),
                });
            }
        }

        match (rule.prefix.is_some(), rule.filter.is_some()) {
            (true, true) => return Err(Rejection::PrefixAndFilter { rule: position }),
            (false, false) => return Err(Rejection::NoPrefixOrFilter { rule: position }),
            (true, false) => prefix_rule = prefix_rule.or(Some(position)),
            (false, true) => filter_rule = filter_rule.or(Some(position)),
        }
        if let (Some(prefix_rule), Some(filter_rule)) = (prefix_rule, filter_rule) {
            return Err(Rejection::MixedPrefixAndFilterRules {
                prefix_rule,
                filter_rule,
            });
        }

        if let Some(filter) = &rule.filter {
            validate_lifecycle_filter(position, filter)?;
        }
        validate_lifecycle_actions(position, rule)?;
    }

    Ok(())
}

/// Whether one rule's `Filter` names a set of objects, reporting the first published rule it
/// breaks. See [`validate_lifecycle_configuration`].
fn validate_lifecycle_filter(
    position: usize,
    filter: &crate::ops::types::LifecycleRuleFilter,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    let named = [
        filter.prefix.is_some(),
        filter.tag.is_some(),
        filter.object_size_greater_than.is_some(),
        filter.object_size_less_than.is_some(),
        filter.and.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count();
    // An EMPTY filter is not a refusal: "If the Filter element is left empty, the Lifecycle
    // Rule applies to all objects in the bucket" (`LifecycleRuleFilter`'s own
    // documentation).
    if named > 1 {
        return Err(Rejection::FilterNamesSeveralPredicates {
            rule: position,
            named,
        });
    }

    validate_lifecycle_object_sizes(
        position,
        filter.object_size_greater_than,
        filter.object_size_less_than,
    )?;
    if let Some(and) = &filter.and {
        validate_lifecycle_object_sizes(
            position,
            and.object_size_greater_than,
            and.object_size_less_than,
        )?;
    }

    Ok(())
}

/// Whether a filter's object-size bounds describe a size some object can have.
fn validate_lifecycle_object_sizes(
    position: usize,
    greater_than: Option<i64>,
    less_than: Option<i64>,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    for (member, bytes) in [
        ("ObjectSizeGreaterThan", greater_than),
        ("ObjectSizeLessThan", less_than),
    ] {
        if let Some(bytes) = bytes {
            if bytes < 0 {
                return Err(Rejection::NegativeObjectSize {
                    rule: position,
                    member,
                    bytes,
                });
            }
        }
    }
    if let (Some(greater_than), Some(less_than)) = (greater_than, less_than) {
        if greater_than >= less_than {
            return Err(Rejection::ObjectSizeRangeEmpty {
                rule: position,
                greater_than,
                less_than,
            });
        }
    }

    Ok(())
}

/// Whether one rule's ACTIONS are a set of things S3 can do to the objects the rule
/// identifies, reporting the first published rule they break.
///
/// A rule's actions are judged together rather than one at a time, because three of the
/// published rules are about the whole set: a rule carries at least one action, measures
/// every one of them from a date or from a day count but not from both, and moves an object
/// through the storage classes in one direction only. See
/// [`validate_lifecycle_configuration`].
fn validate_lifecycle_actions(
    position: usize,
    rule: &crate::ops::types::LifecycleRule,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    let mut actions = 0usize;
    let mut days_based = false;
    let mut date_based = false;

    if let Some(expiration) = &rule.expiration {
        let named = [
            expiration.days.is_some(),
            expiration.date.is_some(),
            expiration.expired_object_delete_marker.is_some(),
        ]
        .into_iter()
        .filter(|named| *named)
        .count();
        if named > 1 {
            return Err(Rejection::SeveralExpirationPredicates {
                rule: position,
                named,
            });
        }
        actions += named;

        if let Some(days) = expiration.days {
            if days <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "Days",
                    action: "Expiration",
                });
            }
            days_based = true;
        }
        if let Some(date) = &expiration.date {
            if !is_utc_midnight(date) {
                return Err(Rejection::DateNotMidnight {
                    rule: position,
                    member: "Expiration.Date",
                });
            }
            date_based = true;
        }
    }

    // Each transition as (the day it is due, the class it moves the object to). A
    // days-based rule counts from each object's creation and a date-based one from the
    // epoch; a rule mixing the two is refused below, so the numbers in here are always on
    // one of the two scales and the gaps between them are days either way.
    let mut transitions: Vec<(i64, &str)> = Vec::new();
    for transition in rule.transitions.iter().flatten() {
        actions += 1;
        let Some(storage_class) = transition.storage_class.as_deref() else {
            return Err(Rejection::TransitionWithoutStorageClass {
                rule: position,
                member: "Transition",
            });
        };
        match (transition.days, &transition.date) {
            (Some(_), Some(_)) => return Err(Rejection::MixedDateAndDays { rule: position }),
            (None, None) => {
                return Err(Rejection::ActionWithoutWhen {
                    rule: position,
                    member: "Transition",
                })
            }
            (Some(days), None) => {
                if days < 0 {
                    return Err(Rejection::NegativeDays {
                        member: "Days",
                        action: "Transition",
                    });
                }
                if days < MINIMUM_DAYS_BEFORE_INFREQUENT_ACCESS
                    && matches!(storage_class, "STANDARD_IA" | "ONEZONE_IA")
                {
                    return Err(Rejection::TransitionBeforeInfrequentAccessMinimum {
                        rule: position,
                        storage_class: storage_class.to_string(),
                        days,
                    });
                }
                days_based = true;
                transitions.push((i64::from(days), storage_class));
            }
            (None, Some(date)) => {
                if !is_utc_midnight(date) {
                    return Err(Rejection::DateNotMidnight {
                        rule: position,
                        member: "Transition.Date",
                    });
                }
                date_based = true;
                transitions.push((date.secs() / SECONDS_PER_DAY, storage_class));
            }
        }
    }

    if let Some(expiration) = &rule.noncurrent_version_expiration {
        let mut named = 0usize;
        if let Some(days) = expiration.noncurrent_days {
            if days <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "NoncurrentDays",
                    action: "NoncurrentVersionExpiration",
                });
            }
            named += 1;
        }
        if let Some(count) = expiration.newer_noncurrent_versions {
            if count <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "NewerNoncurrentVersions",
                    action: "NoncurrentVersionExpiration",
                });
            }
            if count > MAX_NEWER_NONCURRENT_VERSIONS {
                return Err(Rejection::TooManyNewerNoncurrentVersions {
                    rule: position,
                    count,
                });
            }
            named += 1;
        }
        actions += named;
    }

    // The noncurrent transitions are measured in days a version has been noncurrent, which
    // is a scale of its own rather than the rule's date-or-days one, so they are ordered
    // against each other and nothing else.
    let mut noncurrent_transitions: Vec<(i64, &str)> = Vec::new();
    for transition in rule.noncurrent_version_transitions.iter().flatten() {
        actions += 1;
        let Some(storage_class) = transition.storage_class.as_deref() else {
            return Err(Rejection::TransitionWithoutStorageClass {
                rule: position,
                member: "NoncurrentVersionTransition",
            });
        };
        let Some(days) = transition.noncurrent_days else {
            return Err(Rejection::ActionWithoutWhen {
                rule: position,
                member: "NoncurrentVersionTransition",
            });
        };
        if days <= 0 {
            return Err(Rejection::NonPositiveDays {
                member: "NoncurrentDays",
                action: "NoncurrentVersionTransition",
            });
        }
        if let Some(count) = transition.newer_noncurrent_versions {
            if count <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "NewerNoncurrentVersions",
                    action: "NoncurrentVersionTransition",
                });
            }
            if count > MAX_NEWER_NONCURRENT_VERSIONS {
                return Err(Rejection::TooManyNewerNoncurrentVersions {
                    rule: position,
                    count,
                });
            }
        }
        noncurrent_transitions.push((i64::from(days), storage_class));
    }

    if let Some(abort) = &rule.abort_incomplete_multipart_upload {
        let Some(days) = abort.days_after_initiation else {
            return Err(Rejection::ActionWithoutWhen {
                rule: position,
                member: "AbortIncompleteMultipartUpload",
            });
        };
        if days <= 0 {
            return Err(Rejection::NonPositiveDays {
                member: "DaysAfterInitiation",
                action: "AbortIncompleteMultipartUpload",
            });
        }
        actions += 1;
    }

    if actions == 0 {
        return Err(Rejection::NoAction { rule: position });
    }
    if days_based && date_based {
        return Err(Rejection::MixedDateAndDays { rule: position });
    }

    validate_transition_order(position, transitions)?;
    validate_transition_order(position, noncurrent_transitions)?;

    Ok(())
}

/// Whether `transitions` — each as (the day it is due, the class it moves the object to) —
/// move an object through the storage classes in the one direction S3 supports, holding it
/// in each class for that class's minimum storage duration first.
///
/// See [`TRANSITION_STORAGE_CLASS_CHAIN`] for both published rules.
fn validate_transition_order(
    position: usize,
    mut transitions: Vec<(i64, &str)>,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    // Due order, not document order: the rule is about the sequence the object actually
    // moves through, and a configuration may list its transitions in any order.
    transitions.sort_by_key(|(day, _)| *day);
    for pair in transitions.windows(2) {
        // Destructured rather than indexed, so nothing here can be out of bounds.
        let [(earlier_day, earlier_class), (later_day, later_class)] = pair else {
            continue;
        };
        if earlier_class == later_class {
            return Err(Rejection::DuplicateTransitionStorageClass {
                rule: position,
                storage_class: later_class.to_string(),
            });
        }
        // An unranked class is one `TransitionStorageClass` does not name, which the
        // boundary validator refuses before any of this runs; nothing is claimed about its
        // place in the chain here.
        let (Some(earlier_rank), Some(later_rank)) = (
            transition_storage_class_rank(earlier_class),
            transition_storage_class_rank(later_class),
        ) else {
            continue;
        };
        if later_rank < earlier_rank {
            return Err(Rejection::TransitionToWarmerClass {
                rule: position,
                earlier: earlier_class.to_string(),
                later: later_class.to_string(),
            });
        }
        let minimum = minimum_storage_duration_days(earlier_class).unwrap_or_default();
        let gap = later_day.saturating_sub(*earlier_day);
        if gap < i64::from(minimum) {
            return Err(Rejection::TransitionBeforeMinimumDuration {
                rule: position,
                earlier: earlier_class.to_string(),
                later: later_class.to_string(),
                gap,
                minimum,
            });
        }
    }

    Ok(())
}

/// The lifecycle expiration in force on one stored object: when the rule that applies to
/// it expires it, and which rule that is.
///
/// Derived from the bucket's configuration and the object, never stored: an object moves
/// into and out of a rule as its tag set, its size and the configuration change, so the
/// answer is computed per read ([`Bucket::object_expiration`]) rather than stamped on the
/// object when it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectExpiration {
    /// When the object expires.
    pub expires_at: SystemTime,
    /// The `ID` of the rule that expires it.
    pub rule_id: String,
}

impl ObjectExpiration {
    /// This expiration as the `Expiration` response member carries it: "It includes the
    /// expiry-date and rule-id key-value pairs providing object expiration information. The
    /// value of the rule-id is URL-encoded" (`GetObjectOutput$Expiration`), the expiry date
    /// as an HTTP date.
    ///
    /// `None` only when the instant cannot be rendered as an HTTP date, which a stored
    /// object's never is.
    pub fn header_value(&self) -> Option<String> {
        let expiry_date = ::aws_smithy_types::DateTime::from(self.expires_at)
            .fmt(::aws_smithy_types::date_time::Format::HttpDate)
            .ok()?;
        Some(format!(
            "expiry-date=\"{expiry_date}\", rule-id=\"{}\"",
            url_encoded_key(&self.rule_id)
        ))
    }
}

/// When a lifecycle rule aborts one in-progress multipart upload, and which rule does it.
///
/// Derived from the bucket's configuration and the upload, the way [`ObjectExpiration`] is
/// derived for an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartUploadAbort {
    /// When the incomplete upload is aborted.
    pub aborts_at: SystemTime,
    /// The `ID` of the rule that aborts it — what `x-amz-abort-rule-id` reports.
    pub rule_id: String,
}

/// The `ExpirationStatus` value that puts a rule in effect: "If 'Enabled', the rule is
/// currently being applied. If 'Disabled', the rule is not currently being applied"
/// (`LifecycleRule$Status`).
pub const LIFECYCLE_RULE_ENABLED: &str = "Enabled";

/// The instant `days` days after `from`, rounded up to the next midnight UTC.
///
/// How S3 dates a lifecycle action: Amazon S3 adds the number of days to the time the
/// object was created (or the upload was initiated) and rounds the result up to midnight
/// UTC of the following day, which is why an action's date is always a midnight and never
/// the time of day the write happened to land on.
fn midnight_after_days(from: SystemTime, days: i32) -> Option<SystemTime> {
    let from = i64::try_from(
        from.duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    )
    .ok()?;
    let due = from.checked_add(i64::from(days).checked_mul(SECONDS_PER_DAY)?)?;
    let whole_days = due.div_euclid(SECONDS_PER_DAY);
    // Already a midnight: there is nothing to round up to.
    let midnight = if due.rem_euclid(SECONDS_PER_DAY) == 0 {
        whole_days
    } else {
        whole_days.checked_add(1)?
    }
    .checked_mul(SECONDS_PER_DAY)?;
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(midnight).ok()?))
}

/// The instant `date` names, or `None` when it is outside the range a [`SystemTime`] on
/// this platform can hold.
///
/// The ONE place a modelled timestamp becomes an instant the store keeps, whichever member
/// carried it: the midnight UTC a lifecycle `Date` names, and the end of the period an
/// object version's retention runs to ([`ObjectRetention::retain_until`]), so the value
/// `GetObjectRetention` reports back is the one the write recorded.
/// `::aws_smithy_types::DateTime::from(SystemTime)` is the other direction, and both are
/// the crate's own conversions — nothing here re-derives epoch arithmetic, which is also
/// what lets a date BEFORE the epoch through rather than refusing it.
pub fn instant_of(date: &::aws_smithy_types::DateTime) -> Option<SystemTime> {
    SystemTime::try_from(*date).ok()
}

/// Whether `tags` carries `tag`'s key with `tag`'s value — "this tag must exist in the
/// object's tag set in order for the rule to apply" (`LifecycleRuleFilter$Tag`).
fn carries_tag(tags: &[ResourceTag], tag: &crate::ops::types::Tag) -> bool {
    tags.iter()
        .any(|carried| carried.key == tag.key && carried.value == tag.value)
}

/// The key prefix `rule` applies to: the legacy `Prefix`, the filter's own `Prefix`, or the
/// one inside its `And` — and the empty prefix, which every key starts with, for a rule
/// that names none.
fn lifecycle_rule_prefix(rule: &crate::ops::types::LifecycleRule) -> &str {
    if let Some(prefix) = &rule.prefix {
        return prefix.as_str();
    }
    match &rule.filter {
        None => "",
        Some(filter) => filter
            .prefix
            .as_deref()
            .or_else(|| filter.and.as_ref().and_then(|and| and.prefix.as_deref()))
            .unwrap_or(""),
    }
}

/// Whether `rule` applies to an object at `key` of `size` bytes carrying `tags`.
///
/// The predicates are `LifecycleRuleFilter`'s own: a key prefix, a tag the object's tag set
/// must carry, a minimum object size and a maximum one, with `And` requiring all of the
/// ones it names at once. A filter that names none "applies to all objects in the bucket"
/// (that shape's own documentation), as does a rule whose only predicate is the legacy
/// `Prefix`.
fn lifecycle_rule_applies(
    rule: &crate::ops::types::LifecycleRule,
    key: &str,
    size: i64,
    tags: &[ResourceTag],
) -> bool {
    if !key.starts_with(lifecycle_rule_prefix(rule)) {
        return false;
    }
    let Some(filter) = &rule.filter else {
        // A legacy `Prefix` rule: the prefix above is the whole of what it selects on.
        return true;
    };
    if let Some(tag) = &filter.tag {
        if !carries_tag(tags, tag) {
            return false;
        }
    }
    if !object_size_matches(size, filter.object_size_greater_than, filter.object_size_less_than) {
        return false;
    }
    if let Some(and) = &filter.and {
        if and.tags.iter().flatten().any(|tag| !carries_tag(tags, tag)) {
            return false;
        }
        if !object_size_matches(size, and.object_size_greater_than, and.object_size_less_than) {
            return false;
        }
    }
    true
}

/// Whether an object of `size` bytes is inside the bounds a filter names: strictly above
/// the "minimum object size to which the rule applies" and strictly below the maximum
/// (`LifecycleRuleFilter$ObjectSizeGreaterThan` / `$ObjectSizeLessThan`).
fn object_size_matches(size: i64, greater_than: Option<i64>, less_than: Option<i64>) -> bool {
    greater_than.is_none_or(|minimum| size > minimum)
        && less_than.is_none_or(|maximum| size < maximum)
}

/// The `TableStatus` a metadata table carries while it is still being created:
/// "`CREATING` - The journal table is in the process of being created in the specified
/// table bucket" (`smithy_query(query="doc",shape="JournalTableConfigurationResult",
/// member="TableStatus")`).
pub const METADATA_TABLE_STATUS_CREATING: &str = "CREATING";

/// The `TableStatus` a metadata table carries once it is in service: "`ACTIVE` - The
/// journal table has been created successfully, and records are being delivered to the
/// table" (same member's documentation).
pub const METADATA_TABLE_STATUS_ACTIVE: &str = "ACTIVE";

/// The `TableStatus` a metadata table carries when it could not be created: "`FAILED` -
/// Amazon S3 is unable to create the journal table, or Amazon S3 is unable to deliver
/// records" (same member's documentation). A table in this status carries the `ErrorDetails`
/// saying why — see [`MetadataTableFailure`].
pub const METADATA_TABLE_STATUS_FAILED: &str = "FAILED";

/// The `ErrorDetails$ErrorCode` provisioning reports when the journal table it would create
/// is already in the managed table bucket's namespace: "`JournalTableAlreadyExists` - A
/// journal table already exists in the Amazon Web Services managed table bucket's namespace"
/// (`smithy_query(query="doc",shape="ErrorDetails",member="ErrorCode")`, the V2 list).
pub const METADATA_JOURNAL_TABLE_EXISTS_CODE: &str = "JournalTableAlreadyExists";

/// The `ErrorDetails$ErrorMessage` that code carries — the FIXED sentence S3 sends with it,
/// naming neither the table nor the bucket.
pub const METADATA_JOURNAL_TABLE_EXISTS_MESSAGE: &str =
    "A journal table already exists in the AWS managed table bucket's namespace. Delete the \
     journal table, and then try again. To create a new S3 Metadata table, you must delete \
     the S3 Metadata configuration for this bucket, and then create a new S3 Metadata \
     configuration.";

/// The same, for the INVENTORY table: "`InventoryTableAlreadyExists` - An inventory table
/// already exists in the Amazon Web Services managed table bucket's namespace" (same list).
pub const METADATA_INVENTORY_TABLE_EXISTS_CODE: &str = "InventoryTableAlreadyExists";

/// See [`METADATA_INVENTORY_TABLE_EXISTS_CODE`].
pub const METADATA_INVENTORY_TABLE_EXISTS_MESSAGE: &str =
    "An inventory table already exists in the AWS managed table bucket's namespace. Delete \
     the inventory table, and then try again. To create a new S3 Metadata table, you must \
     delete the S3 Metadata configuration for this bucket, and then create a new S3 \
     Metadata configuration.";

/// The code an inventory table carries when the journal table it depends on could not be
/// created: "`JournalTableNotAvailable` - The journal table that the inventory table relies
/// on has a `FAILED` status. An inventory table requires a journal table with an `ACTIVE`
/// status" (same published list).
pub const METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE: &str = "JournalTableNotAvailable";

/// See [`METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE`].
pub const METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE: &str =
    "The journal table that the inventory table relies on has a FAILED status. An inventory \
     table requires a journal table with an ACTIVE status. To create a new journal or \
     inventory table, you must delete the metadata configuration for this bucket, along with \
     any journal or inventory tables, and then create a new metadata configuration.";

/// How long after the call that asked for it a metadata table reaches
/// [`METADATA_TABLE_STATUS_ACTIVE`] in this emulator.
///
/// A CHOSEN span, not an AWS-documented duration: what IS faithful is that the table is
/// NOT in service when the create returns — the status member publishes a transient
/// `CREATING` value for exactly that window — so a caller reading the configuration
/// straight after the create still observes `CREATING`, as it would against AWS. The
/// instant it falls due is stored ([`MetadataTable::active_at`]) and nothing sleeps for
/// it, the way [`RESTORE_COMPLETION_DELAY`] is handled.
pub const METADATA_TABLE_ACTIVATION_DELAY: std::time::Duration =
    std::time::Duration::from_millis(500);

/// `DestinationResult$TableBucketType` of a bucket's metadata configuration: "V2 metadata
/// configurations are stored in Amazon Web Services managed table buckets"
/// (`smithy_query(query="doc",shape="DestinationResult",member="TableBucketType")`), and
/// `aws` is the value that "indicates an Amazon Web Services managed table bucket". One of
/// `S3TablesBucketType`'s own two values.
pub const METADATA_TABLE_BUCKET_TYPE: &str = "aws";

/// The name of the Amazon Web Services managed table bucket this emulator reports a
/// metadata configuration's tables as stored in.
///
/// ONE managed bucket per Region, shared by every configuration, which is what the `aws`
/// bucket type means: the caller names no table bucket on a V2 create and AWS does not
/// publish the managed bucket's name, so this is the name this emulator reports. It
/// satisfies the form every S3 Tables operation accepts for a table bucket
/// (`[a-z0-9_-]{3,63}`, from the `TableBucketARN` pattern those requests publish).
///
/// REPORTED-ONLY: no table bucket is created in the `s3tables` emulator behind this ARN,
/// so a caller cannot follow it into that service and find one. The bucket is that
/// service's resource and this service owns no part of its state, so the ARN names where
/// the metadata tables WOULD live — which is all `GetBucketMetadataConfiguration` reports
/// about them.
pub const METADATA_TABLE_BUCKET_NAME: &str = "aws-s3";

/// The namespace inside that table bucket a bucket's metadata tables live in — "the
/// namespace in the table bucket where the metadata tables for a metadata configuration
/// are stored" (`smithy_query(query="doc",shape="DestinationResult",
/// member="TableNamespace")`), which S3 Metadata names `aws_s3_metadata`.
pub const METADATA_TABLE_NAMESPACE: &str = "aws_s3_metadata";

/// The service namespace of a metadata table's (and its table bucket's) ARN, so one reads
/// `arn:aws:s3tables:<region>:<account>:bucket/<bucket>` and a table's
/// `…:bucket/<bucket>/table/<table>` — the two forms every S3 Tables request publishes a
/// pattern for.
const METADATA_TABLE_ARN_SERVICE: &str = "s3tables";

/// What a metadata table's name ends in, per table: the journal table, the inventory
/// table and the annotation table of one general purpose bucket are three tables in one
/// namespace, so the suffix is what tells them apart.
pub const METADATA_JOURNAL_TABLE_SUFFIX: &str = "_journal";
/// See [`METADATA_JOURNAL_TABLE_SUFFIX`].
pub const METADATA_INVENTORY_TABLE_SUFFIX: &str = "_inventory";
/// See [`METADATA_JOURNAL_TABLE_SUFFIX`].
pub const METADATA_ANNOTATION_TABLE_SUFFIX: &str = "_annotation";

/// The `ConfigurationState` value that asks for a table to be created, for both the
/// inventory table (`InventoryConfigurationState`) and the annotation table
/// (`AnnotationConfigurationState`) — each enum publishes `ENABLED` and `DISABLED`
/// (`smithy_query(query="shape",shape="InventoryConfigurationState,AnnotationConfigurationState")`).
pub const METADATA_CONFIGURATION_STATE_ENABLED: &str = "ENABLED";

/// The `TableSseAlgorithm` value that encrypts a metadata table with a Key Management
/// Service key, and so the one value that needs a key ARN beside it: the other value that
/// enum publishes, `AES256`, is S3 managed encryption and takes none
/// (`smithy_query(query="shape",shape="TableSseAlgorithm")`).
pub const METADATA_TABLE_SSE_KMS_ALGORITHM: &str = "aws:kms";

/// The fewest days journal table records may be retained for: "Journal table records must
/// be retained for a minimum of 7 days. To set this value, specify any whole number from
/// `7` to `2147483647`" (`smithy_query(query="doc",shape="RecordExpiration",
/// member="Days")`).
pub const MIN_METADATA_RECORD_EXPIRATION_DAYS: i32 = 7;

/// The most days journal table records may be retained for — the upper end of the same
/// published range.
pub const MAX_METADATA_RECORD_EXPIRATION_DAYS: i32 = 2_147_483_647;

/// The longest IAM role ARN this emulator records for an annotation table.
///
/// An emulator resource guard, NOT an AWS quota: `Role` is "The ARN of the IAM role used
/// to manage the annotation table" (`AnnotationTableConfiguration$Role`) and the model
/// bounds its length at neither end, so this is what keeps the one caller-supplied string
/// a metadata configuration retains from growing without limit. 2048 is the longest ARN
/// AWS accepts anywhere in an IAM request.
pub const MAX_METADATA_TABLE_ROLE_CHARS: usize = 2048;

/// The most metadata tables this emulator records in the managed table bucket's namespace
/// ([`ComAmazonawsS3Amazons3State::metadata_namespace_holds`]).
///
/// An emulator resource guard, NOT an AWS quota. The namespace is the one record here that
/// nothing releases — a table OUTLIVES the configuration that created it, and nothing in
/// this service's API deletes a table from an S3 Tables namespace — so without a ceiling a
/// caller creating and deleting buckets could grow it for the life of the process. Three
/// tables (journal, inventory, annotation) for as many general purpose buckets as this
/// account may hold ([`MAX_BUCKETS_PER_ACCOUNT`]) is the most any sequence of creates can
/// legitimately need; a create that would carry the namespace past it is refused with
/// nothing recorded.
pub const MAX_METADATA_NAMESPACE_TABLES: usize = 3 * MAX_BUCKETS_PER_ACCOUNT;

/// The `InvalidArgument` message the encryption configuration carried in the request member
/// `member` earns, or `None` when there is nothing wrong with it.
///
/// "If server-side encryption with Key Management Service (KMS) keys (SSE-KMS) is specified,
/// you must also specify the KMS key Amazon Resource Name (ARN)"
/// (`MetadataTableEncryptionConfiguration$KmsKeyArn`): ONE rule for every table that carries
/// that container, so the create that names all three tables and each update that names one
/// cannot drift apart on it. The other algorithm `TableSseAlgorithm` publishes, `AES256`, is
/// S3 managed encryption and takes no key.
pub fn metadata_table_encryption_refusal(
    member: &str,
    encryption: Option<&crate::ops::types::MetadataTableEncryptionConfiguration>,
) -> Option<String> {
    let encryption = encryption?;
    (encryption.sse_algorithm == METADATA_TABLE_SSE_KMS_ALGORITHM
        && encryption.kms_key_arn.is_none())
    .then(|| {
        format!(
            "{member}.EncryptionConfiguration names SseAlgorithm {:?} without a \
             KmsKeyArn; a KMS key ARN is required with that algorithm",
            encryption.sse_algorithm
        )
    })
}

/// The `InvalidArgument` message the journal table record expiration settings `expiration`
/// earn, or `None` when there is nothing wrong with them.
///
/// "Journal table records must be retained for a minimum of 7 days. To set this value,
/// specify any whole number from `7` to `2147483647`" (`RecordExpiration$Days`): the model
/// bounds that member at neither end, so a day count outside the published range reaches a
/// handler, and a value that is not valid for the parameter carrying it is what
/// `InvalidArgument` reports. ONE rule for every request that carries a `RecordExpiration`
/// — the create that settles a configuration's journal table and the update that changes
/// its record expiration — so the two cannot drift apart on it.
pub fn metadata_record_expiration_refusal(
    expiration: &crate::ops::types::RecordExpiration,
) -> Option<String> {
    let days = expiration.days?;
    (!(MIN_METADATA_RECORD_EXPIRATION_DAYS..=MAX_METADATA_RECORD_EXPIRATION_DAYS).contains(&days))
        .then(|| {
            format!(
                "RecordExpiration.Days must be a whole number from {MIN_METADATA_RECORD_EXPIRATION_DAYS} to {MAX_METADATA_RECORD_EXPIRATION_DAYS}, got {days}"
            )
        })
}

/// The `ErrorDetails` an inventory table being provisioned NOW carries, or `None` when
/// nothing stops it from being created: `name_taken` says the managed table bucket's
/// namespace already holds the name it would be created under, and `journal_failed` that the
/// journal table it reads from could not be created.
///
/// ONE rule for every request that provisions an inventory table — the create that names a
/// whole configuration and the update that enables the table on one — so the two cannot
/// drift apart on it. Its OWN name is judged first: that is what stops a second table being
/// created under a name a table already holds ([`METADATA_INVENTORY_TABLE_EXISTS_CODE`]),
/// and only a table whose name is free can then report the journal table it relies on —
/// "the journal table that the inventory table relies on has a `FAILED` status. An inventory
/// table requires a journal table with an `ACTIVE` status"
/// ([`METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE`], from
/// `smithy_query(query="doc",shape="ErrorDetails",member="ErrorCode")`'s V2 list).
pub fn metadata_inventory_table_failure(
    name_taken: bool,
    journal_failed: bool,
) -> Option<MetadataTableFailure> {
    if name_taken {
        Some(MetadataTableFailure {
            code: METADATA_INVENTORY_TABLE_EXISTS_CODE,
            message: METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
        })
    } else if journal_failed {
        Some(MetadataTableFailure {
            code: METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE,
            message: METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
        })
    } else {
        None
    }
}

/// What enabling or disabling the inventory table of a bucket's metadata configuration did —
/// the outcomes the caller of
/// [`ComAmazonawsS3Amazons3State::update_metadata_inventory_table`] answers differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataInventoryTableUpdate {
    /// Recorded: the state the request named is the configuration's and the table behind it
    /// followed. A table provisioned under a name the namespace already holds is recorded
    /// [`METADATA_TABLE_STATUS_FAILED`], which is a provisioning outcome a READER reports
    /// rather than a refusal of this call.
    Recorded,
    /// This store holds no such general purpose bucket.
    NoSuchBucket,
    /// The bucket carries no metadata configuration, so there is no inventory table of one to
    /// enable or disable and nothing is written.
    NoConfiguration,
    /// The enable would provision a table under a name the managed table bucket's namespace
    /// has no room left for ([`MAX_METADATA_NAMESPACE_TABLES`]) — this emulator's own
    /// ceiling, with nothing written.
    NamespaceFull,
}

/// One table of a bucket's S3 Metadata configuration: its name, and when it falls due to
/// be in service.
///
/// Nothing here is table CONTENT: the Iceberg table itself is an S3 Tables resource this
/// service owns no part of. What is recorded is what every reader of the configuration
/// reports about the table — the name S3 created it under and the status it is in
/// ([`Self::status_at`]).
#[derive(Debug, Clone)]
pub struct MetadataTable {
    /// The table's name in [`METADATA_TABLE_NAMESPACE`], as
    /// [`metadata_table_name`] derived it from the general purpose bucket's name.
    pub name: String,
    /// When the table falls due to be in service — the instant it stops being reported as
    /// `CREATING`, [`METADATA_TABLE_ACTIVATION_DELAY`] after the call that asked for it.
    pub active_at: SystemTime,
    /// Why the table could not be created, or `None` for one whose creation is under way.
    ///
    /// A table that cannot be created never reaches `ACTIVE`: it is reported `FAILED` with
    /// the `ErrorDetails` this record carries ([`MetadataTableFailure`]), from the create
    /// onwards.
    pub failure: Option<MetadataTableFailure>,
}

/// Why a metadata table could not be created: the `ErrorDetails` a reader reports beside its
/// `FAILED` status.
///
/// The members are `ErrorDetails`' own
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ErrorDetails")`): the error code and
/// the message, each a published pair from that shape's own documentation — so the code and
/// the sentence a caller reads are AWS's rather than anything built here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataTableFailure {
    /// An `ErrorDetails$ErrorCode` value, e.g. [`METADATA_JOURNAL_TABLE_EXISTS_CODE`].
    pub code: &'static str,
    /// The `ErrorDetails$ErrorMessage` that code publishes.
    pub message: &'static str,
}

impl MetadataTable {
    /// The table `name`, as the call that asked for it at `created_at` records it: being
    /// created now, in service a span later.
    pub fn creating(name: String, created_at: SystemTime) -> Self {
        Self {
            name,
            active_at: created_at + METADATA_TABLE_ACTIVATION_DELAY,
            failure: None,
        }
    }

    /// The table `name` that could not be created, with `failure` saying why: reported
    /// `FAILED` from the create onwards rather than ever falling due.
    pub fn failed(name: String, created_at: SystemTime, failure: MetadataTableFailure) -> Self {
        Self {
            failure: Some(failure),
            ..Self::creating(name, created_at)
        }
    }

    /// The `TableStatus` the table is in at `instant`: `FAILED` for a table that could not
    /// be created, else being created until it falls due and in service from then on.
    pub fn status_at(&self, instant: SystemTime) -> &'static str {
        if self.failure.is_some() {
            METADATA_TABLE_STATUS_FAILED
        } else if instant < self.active_at {
            METADATA_TABLE_STATUS_CREATING
        } else {
            METADATA_TABLE_STATUS_ACTIVE
        }
    }
}

/// One of the OPTIONAL tables of a metadata configuration — the inventory table or the
/// annotation table — as the configuration names it.
///
/// The state is what the request carried, and the TABLE is there exactly when that state
/// is [`METADATA_CONFIGURATION_STATE_ENABLED`]: a disabled table is a configuration with
/// no table behind it, which is why `InventoryTableConfigurationResult` requires its
/// `ConfigurationState` and leaves its `TableStatus`, `TableName` and `TableArn` optional
/// (`smithy_query(query="shape",shape="InventoryTableConfigurationResult")`).
///
/// ONE type for both, because both are the same record: a state, the table it has while
/// that state is enabled, and — for the annotation table alone — the IAM role that
/// manages it (`AnnotationTableConfiguration$Role`; the inventory table has no such
/// member, so its `role` is always `None`).
#[derive(Debug, Clone)]
pub struct MetadataTableConfiguration {
    /// The `ConfigurationState` the request named for this table.
    pub configuration_state: String,
    /// The table itself, or `None` while the state above is not enabled.
    pub table: Option<MetadataTable>,
    /// "The ARN of the IAM role used to manage the annotation table"
    /// (`AnnotationTableConfiguration$Role`), for the annotation table and nothing else.
    pub role: Option<String>,
}

/// Record `configuration_state` as the state of the optional table configuration `slot`,
/// with `table_name` the name a table this creates is created under, `role` the IAM role the
/// call names for the table (`None` when it names none), `failure` why a table this creates
/// could not be created (`None` when nothing stops it), and `now` the instant of the call
/// asking for it.
///
/// The shape of enabling or disabling one of a metadata configuration's optional tables: the
/// state the request names becomes the table's state, and the table behind it follows —
/// enabling a table the configuration has none of creates one, being created from `now`
/// ([`METADATA_TABLE_ACTIVATION_DELAY`] later in service) or reported `FAILED` with `failure`
/// when provisioning it could not succeed, and disabling one drops it, which is what leaves
/// the reported result carrying a state with no table name, status or ARN. Enabling a table
/// that is already there leaves that table exactly as it is, so a repeated enable neither
/// restarts its creation nor renames it — and `failure` is therefore about the table this
/// call would CREATE, never about one already there.
///
/// `role` is "the new IAM role ARN to apply" (`AnnotationTableConfigurationUpdates$Role`) —
/// for the annotation table, the only optional table managed by one: a call that names one
/// replaces the role in effect, and a call that names none leaves whatever role the
/// configuration carried in place (the inventory table has no such member, so its caller
/// names none and its `role` stays `None`).
fn set_optional_metadata_table_state(
    slot: &mut Option<MetadataTableConfiguration>,
    configuration_state: String,
    role: Option<String>,
    table_name: &str,
    failure: Option<MetadataTableFailure>,
    now: SystemTime,
) {
    let enabled = configuration_state == METADATA_CONFIGURATION_STATE_ENABLED;
    let carried = slot.take();
    let carried_table = carried.as_ref().and_then(|table| table.table.clone());
    *slot = Some(MetadataTableConfiguration {
        configuration_state,
        table: enabled.then(|| {
            carried_table.unwrap_or_else(|| match failure {
                Some(failure) => MetadataTable::failed(table_name.to_string(), now, failure),
                None => MetadataTable::creating(table_name.to_string(), now),
            })
        }),
        role: role.or_else(|| carried.and_then(|table| table.role)),
    });
}

/// The S3 Metadata configuration a general purpose bucket carries, as
/// `CreateBucketMetadataConfiguration` recorded it.
///
/// A bucket carries at most one: the create "creates an S3 Metadata V2 metadata
/// configuration for a general purpose bucket" (that operation's own documentation) and
/// every other operation on it names only the bucket, so this lives behind an `Option` on
/// [`Bucket`] and a second create while one is there is refused rather than recorded.
///
/// The JOURNAL table is not optional, because `MetadataConfiguration$JournalTableConfiguration`
/// is a required member: a configuration always has one, and the inventory and annotation
/// tables are there only when the create named them.
#[derive(Debug, Clone)]
pub struct BucketMetadataConfiguration {
    /// The journal table, which every configuration has.
    pub journal: MetadataTable,
    /// "The journal table record expiration settings for the journal table"
    /// (`JournalTableConfiguration$RecordExpiration`), stored as the modelled container
    /// the request carried — the way [`Bucket::replication`] keeps its own — so a reader
    /// reports the settings the bucket actually carries rather than a re-derived copy.
    pub journal_record_expiration: crate::ops::types::RecordExpiration,
    /// The inventory table configuration, or `None` when the create named none.
    pub inventory: Option<MetadataTableConfiguration>,
    /// The annotation table configuration, or `None` when the create named none.
    pub annotation: Option<MetadataTableConfiguration>,
}

/// The name of one metadata table of the general purpose bucket `bucket`: its name, folded
/// to the character set an S3 Tables table name is written in, and `suffix` naming which of
/// the configuration's tables it is.
///
/// A general purpose bucket's name may carry dots and hyphens, which a table name may not
/// (every S3 Tables request publishes `[a-zA-Z0-9-_]{1,255}` for the table portion of a
/// table ARN, and S3 Tables table names are written in lowercase letters, digits and
/// underscores), so each character outside that set becomes an underscore. Derived from the
/// bucket's own name rather than minted, so the table a reader reports for one bucket is
/// the same on every read — and two bucket names that differ only in the characters this
/// folds (a dot where another has a hyphen) name the same table, which is what the
/// `*AlreadyExists` provisioning failures report.
pub fn metadata_table_name(bucket: &str, suffix: &str) -> String {
    let stem: String = bucket
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{stem}{suffix}")
}

/// The ARN of the managed table bucket a metadata configuration's tables live in, in
/// `region` — see [`METADATA_TABLE_BUCKET_NAME`] for what it does and does not claim.
pub fn metadata_table_bucket_arn(region: &str) -> String {
    ::engine::emulator_arn(
        METADATA_TABLE_ARN_SERVICE,
        region,
        &format!("bucket/{METADATA_TABLE_BUCKET_NAME}"),
    )
}

/// The ARN of the metadata table `table` in `region`: the table bucket's own ARN above with
/// the table's name appended, which is the form every S3 Tables request publishes for a
/// table ARN.
pub fn metadata_table_arn(region: &str, table: &str) -> String {
    ::engine::emulator_arn(
        METADATA_TABLE_ARN_SERVICE,
        region,
        &format!("bucket/{METADATA_TABLE_BUCKET_NAME}/table/{table}"),
    )
}

/// Where a DIRECTORY bucket is placed, and how its data is kept there.
///
/// Both members come from the `CreateBucketConfiguration` the create carried: "the location
/// type is Availability Zone or Local Zone" and, for a directory bucket, "the name of the
/// location is the Zone ID of the Availability Zone (AZ) or Local Zone (LZ) where the
/// bucket will be created" (`LocationInfo`), with "the number of Zone … that's used for
/// redundancy for the bucket" beside it (`BucketInfo$DataRedundancy`). Recorded as the
/// create settled them, so every later answer about the bucket reports the placement it was
/// created with rather than one re-derived from its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryBucketPlacement {
    /// The `LocationType` value the create named: one of [`LOCATION_TYPE_VALUES`].
    pub location_type: String,
    /// The Zone ID the create named, which is also the zone segment of the bucket's own
    /// name ([`directory_bucket_zone`]).
    pub zone_id: String,
    /// The `DataRedundancy` value the create named, or the one its location type implies
    /// ([`implied_data_redundancy`]).
    pub data_redundancy: String,
}

/// The `BucketType` value that asks `CreateBucket` for a directory bucket — the single
/// value that enum publishes (`smithy_query(query="shape",shape="BucketType")`).
pub const DIRECTORY_BUCKET_TYPE: &str = "Directory";

/// The `LocationType` value for an Availability Zone, the first of that enum's two values.
pub const LOCATION_TYPE_AVAILABILITY_ZONE: &str = "AvailabilityZone";

/// The `LocationType` value for a Local Zone, the other one.
pub const LOCATION_TYPE_LOCAL_ZONE: &str = "LocalZone";

/// The `DataRedundancy` value a bucket in an Availability Zone is stored with.
pub const DATA_REDUNDANCY_SINGLE_AVAILABILITY_ZONE: &str = "SingleAvailabilityZone";

/// The `DataRedundancy` value a bucket in a Local Zone is stored with.
pub const DATA_REDUNDANCY_SINGLE_LOCAL_ZONE: &str = "SingleLocalZone";

/// The redundancy a directory bucket in `location_type` is kept with when the create named
/// none: one zone, the zone it was placed in — the `DataRedundancy` value that names that
/// zone's kind ("the number of Zone (Availability Zone or Local Zone) that's used for
/// redundancy for the bucket", `BucketInfo$DataRedundancy`, whose two values are exactly
/// the two location types).
pub fn implied_data_redundancy(location_type: &str) -> &'static str {
    if location_type == LOCATION_TYPE_LOCAL_ZONE {
        DATA_REDUNDANCY_SINGLE_LOCAL_ZONE
    } else {
        DATA_REDUNDANCY_SINGLE_AVAILABILITY_ZONE
    }
}

/// The suffix every directory bucket's name ends with.
///
/// "Directory bucket names must … end with the suffix `--x-s3`" and carry the zone before
/// it: the name is `<base>--<zone-id>--x-s3` (Amazon S3 User Guide, *Directory bucket
/// naming rules*), which is why the zone a request's bucket lives in can be read off its
/// name at all ([`directory_bucket_zone`]).
pub const DIRECTORY_BUCKET_SUFFIX: &str = "--x-s3";

/// The separator the base name, the zone id and the suffix of a directory bucket's name are
/// joined with — two hyphens, which is what makes a zone id (itself hyphenated, `usw2-az1`)
/// readable out of the name.
const DIRECTORY_BUCKET_SEPARATOR: &str = "--";

/// The Zone ID in the directory-bucket name `name`, or `None` for a name that is not a
/// directory bucket's.
///
/// The rule is the published naming rule above, read in one place so the create that places
/// a bucket in a zone and anything that later asks which zone a name claims agree.
pub fn directory_bucket_zone(name: &str) -> Option<&str> {
    let base = name.strip_suffix(DIRECTORY_BUCKET_SUFFIX)?;
    let (stem, zone_id) = base.rsplit_once(DIRECTORY_BUCKET_SEPARATOR)?;
    (!stem.is_empty() && !zone_id.is_empty()).then_some(zone_id)
}

/// The ARN of the directory bucket `bucket` in `region`: `CreateBucket` reports one for a
/// directory bucket and for no other kind ("this parameter is only supported for S3
/// directory buckets", `CreateBucketOutput$BucketArn`).
///
/// A directory bucket's ARN carries the Region and the account — unlike a general purpose
/// bucket's ([`bucket_arn`]), whose name is globally unique — and is scoped to the
/// `s3express` namespace, which is what the member's own pattern admits beside `s3`
/// (`^arn:[^:]+:(s3|s3express):`).
pub fn directory_bucket_arn(region: &str, bucket: &str) -> String {
    ::engine::emulator_arn(
        DIRECTORY_BUCKET_ARN_SERVICE,
        region,
        &format!("bucket/{bucket}"),
    )
}

/// The ARN namespace a directory bucket is named in — the S3 Express One Zone service,
/// whose own API operations are the `s3express:` actions a directory bucket's policy grants.
const DIRECTORY_BUCKET_ARN_SERVICE: &str = "s3express";

/// The storage class the objects of a directory bucket are stored in.
///
/// "Directory buckets … only support the S3 Express One Zone storage class"
/// (`PutObjectRequest$StorageClass`), so a write into one stores its object in that class
/// rather than in the [`DEFAULT_STORAGE_CLASS`] a general purpose bucket's objects default
/// to — and it is the class an appending write requires
/// (`PutObjectRequest$WriteOffsetBytes`).
pub const EXPRESS_ONEZONE_STORAGE_CLASS: &str = "EXPRESS_ONEZONE";

/// The code S3 answers a bucket name that does not follow the published naming rules with:
/// "InvalidBucketName — The specified bucket is not valid" (Amazon S3 *Error responses*,
/// list of error codes), HTTP 400.
pub const INVALID_BUCKET_NAME_CODE: &str = "InvalidBucketName";

/// See [`INVALID_BUCKET_NAME_CODE`] — the text that list publishes for it.
pub const INVALID_BUCKET_NAME_MESSAGE: &str = "The specified bucket is not valid.";

/// The `<Message>` the modelled `InvalidRequest` shape states for itself
/// (`smithy_query(query="shape",shape="InvalidRequest")`), for the places that answer that
/// code through the unmodelled channel because the operation does not declare the shape.
pub const INVALID_REQUEST_MESSAGE: &str =
    "A parameter or header in your request isn't valid. For details, see the description of \
     this API operation.";

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
    /// Whether S3 Object Lock is enabled on the bucket — "whether this bucket has an
    /// Object Lock configuration enabled" (`ObjectLockConfiguration$ObjectLockEnabled`,
    /// whose enum has the single value `Enabled`).
    ///
    /// A `bool` rather than an `Option<String>` because that enum names ONE state: a
    /// bucket either has Object Lock enabled or has no Object Lock configuration at all,
    /// and there is no second value for an `Option` to distinguish. `false` is the state
    /// every bucket is created in unless the create asks for it
    /// (`CreateBucketRequest$ObjectLockEnabledForBucket`, "specifies whether you want S3
    /// Object Lock to be enabled for the new bucket"), and it is what makes an object-level
    /// Object Lock call on the bucket a request S3 cannot carry out
    /// ([`OBJECT_LOCK_CONFIGURATION_MISSING_MESSAGE`]).
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    object_lock_enabled: bool,
    /// The default retention the bucket's Object Lock configuration names
    /// ([`BucketDefaultRetention`]), or `None` while that configuration carries no rule —
    /// the state a bucket Object Lock was enabled on by `CreateBucket` is in, and what makes
    /// a write place no retention of its own.
    ///
    /// Only ever set beside [`Self::object_lock_enabled`]: the rule is one member of the
    /// `ObjectLockConfiguration` that member is the other half of, and
    /// [`Self::set_object_lock_configuration`] is the one place either of them moves.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    object_lock_rule: Option<BucketDefaultRetention>,
    /// The Transfer Acceleration state the last `PutBucketAccelerateConfiguration` set on
    /// the bucket, or `None` while the bucket has none.
    ///
    /// `None` is NOT a third state and NOT a stored default: "A bucket has no Transfer
    /// Acceleration state if a state has never been set on the bucket", and a read of such
    /// a bucket "does not return a state value" for it
    /// (`GetBucketAccelerateConfiguration`'s own documentation) — the same distinction
    /// [`Self::versioning`] keeps between "never set" and "set to one of the published
    /// values". The API publishes no delete for the subresource, so a bucket leaves this
    /// state only by a put and never goes back to it.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    accelerate: Option<String>,
    /// The `Payer` the last `PutBucketRequestPayment` set on the bucket, or `None` while
    /// its request payment configuration has never been set.
    ///
    /// `None` is NOT "nobody pays": a bucket that has never been configured is one the
    /// bucket owner pays for ([`DEFAULT_PAYER`]), which is what a read of it reports.
    /// Keeping that case as `None` rather than as a stored copy of the default is what
    /// lets `GetBucketRequestPayment` report it without writing — the same distinction
    /// [`Self::encryption`] keeps for a never-configured bucket.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    request_payment: Option<String>,
    /// The attribute-based access control (ABAC) status `PutBucketAbac` last set on the
    /// bucket, or `None` while no put has set one.
    ///
    /// `None` is NOT a third state: ABAC is a PROPERTY every general purpose bucket has,
    /// and `BucketAbacStatus` names both of the states it can be in — `Enabled` or
    /// `Disabled` (`smithy_query(query="shape",shape="BucketAbacStatus")`). A bucket is
    /// created without ABAC (`CreateBucket` carries no member for it) and the API publishes
    /// neither a delete for the subresource nor a "there is none" error for a read of it,
    /// so a bucket no put has touched carries [`DEFAULT_BUCKET_ABAC_STATUS`]. Keeping that
    /// case as `None` rather than as a stored copy of the default is what lets a read of a
    /// never-configured bucket report it without writing — the same distinction
    /// [`Self::encryption`] keeps.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    abac: Option<String>,
    /// The Block Public Access settings in force on the bucket: the ones
    /// `PutPublicAccessBlock` last set, the all-on configuration the bucket was created
    /// with ([`BucketPublicAccessBlock::all_enabled`]) while no put has replaced it, or
    /// `None` once `DeletePublicAccessBlock` has withdrawn the subresource — the one state
    /// in which `GetPublicAccessBlock` answers
    /// [`NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE`].
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
    /// The bucket's EFFECTIVE `ObjectOwnership` setting, or `None` while nothing has
    /// changed it from the one a bucket is created with.
    ///
    /// Distinct from [`Self::ownership_controls`], which is the `OwnershipControls`
    /// SUBRESOURCE a reader reports: the two part company in exactly the case
    /// `GetBucketOwnershipControls` describes, a bucket with no controls associated with it
    /// at all. A freshly created bucket has none (so the read is not-found) and yet is
    /// `BucketOwnerEnforced` — "by default, `ObjectOwnership` is set to
    /// `BucketOwnerEnforced` and ACLs are disabled" — while a bucket whose controls were
    /// DELETED has none and its ACLs affect permissions again. `None` here is the former:
    /// [`DEFAULT_OBJECT_OWNERSHIP`], derived rather than stored so a create writes no copy
    /// of the default.
    ///
    /// Private, like `ownership_controls`, so the accessor pair below is the only way in or
    /// out.
    object_ownership: Option<String>,
    /// What makes this bucket a DIRECTORY bucket — the zone it is placed in and the
    /// redundancy it is stored with ([`DirectoryBucketPlacement`]) — or `None` for a
    /// general purpose bucket, which is every bucket a create that named no
    /// `CreateBucketConfiguration$Bucket` of type [`DIRECTORY_BUCKET_TYPE`] made.
    ///
    /// The two kinds of bucket are the same record because they hold objects the same way:
    /// what differs is what the API serves on them — a session (`CreateSession`), an
    /// appending write (`PutObjectRequest$WriteOffsetBytes`), an ARN on the create, and the
    /// storage class their objects are stored in — each of which reads this one field
    /// rather than guessing from the bucket's name.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    directory: Option<DirectoryBucketPlacement>,
    /// The access control list `PutBucketAcl` last set on the bucket — one entry per
    /// `Grant` of the `AccessControlPolicy` it applied, in the order that ACL names them
    /// — or `None` while no ACL has ever been set on it.
    ///
    /// `None` is NOT "no access": a bucket that has never had an ACL set carries the
    /// canned `private` ACL ([`default_acl_grants`]), which is what a read of it reports.
    /// Keeping that case as `None` rather than as a stored copy of the default is what
    /// lets `GetBucketAcl` report it without writing — the same distinction
    /// [`Self::encryption`] keeps for a never-configured bucket.
    ///
    /// The OWNER is not stored beside the grants: an ACL cannot transfer a bucket, so the
    /// owner a reader reports is always the account that created it
    /// ([`canonical_user_id`]), whatever `AccessControlPolicy$Owner` a request carries.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    acl: Option<Vec<AclGrant>>,
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
    /// The replication configuration the last `PutBucketReplication` associated with the
    /// bucket, or `None` while the bucket has none — the state a bucket is created in, and
    /// what makes `GetBucketReplication` answer
    /// [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] until a put sets one.
    ///
    /// The MODELLED container is stored whole, because that is exactly what the operation
    /// takes and what `GetBucketReplication` reports back: "Specify the replication
    /// configuration in the request body", and the read's payload is the same
    /// `ReplicationConfiguration` shape. Keeping the submitted container means a later read
    /// answers with the configuration the bucket actually carries rather than with a
    /// re-derived copy of it.
    ///
    /// Private, like `tags` and `encryption`, so the accessor pair below is the only way in
    /// or out.
    replication: Option<crate::ops::types::ReplicationConfiguration>,
    /// The lifecycle configuration the last `PutBucketLifecycleConfiguration` associated
    /// with the bucket, or `None` while the bucket has none — the state a bucket is created
    /// in, and what makes `GetBucketLifecycleConfiguration` answer
    /// [`NO_SUCH_LIFECYCLE_CONFIGURATION_CODE`] until a put sets one.
    ///
    /// Private, like `tags` and `replication`, so the accessor pair below and
    /// [`ComAmazonawsS3Amazons3State::put_lifecycle_configuration`] (which mints the rule
    /// identifiers a request omitted) are the only way in or out. See [`BucketLifecycle`]
    /// for what is kept.
    lifecycle: Option<BucketLifecycle>,
    /// The bucket's CloudWatch request-metrics configurations, by the id each one is
    /// identified by: "Sets a metrics configuration (specified by the metrics configuration
    /// ID) for the bucket" (`PutBucketMetricsConfiguration`'s own documentation).
    ///
    /// A MAP rather than one value, because a bucket carries MANY of them — "You can have
    /// up to 1,000 metrics configurations per bucket" (same documentation,
    /// [`MAX_METRICS_CONFIGURATIONS_PER_BUCKET`]) — each addressable on its own by the id
    /// the request named it with, which is what a read, a replacement and a delete of one
    /// configuration leave the others untouched by. A `BTreeMap`, so iterating it is
    /// ascending id order and a listing needs no sort step. Empty for a bucket no metrics
    /// configuration has been put on, which is the state a bucket is created in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: that is
    /// exactly what the put takes as its payload and what `GetBucketMetricsConfiguration`
    /// reports back, so a read answers the configuration the bucket actually carries rather
    /// than a re-derived copy of it.
    ///
    /// Private, like `tags` and `replication`, so the accessor pair below is the only way
    /// in or out.
    metrics_configurations: BTreeMap<String, crate::ops::types::MetricsConfiguration>,
    /// The bucket's storage class analysis configurations, by the id each one is identified
    /// by: "Sets an analytics configuration for the bucket (specified by the analytics
    /// configuration ID)" (`PutBucketAnalyticsConfiguration`'s own documentation).
    ///
    /// A MAP rather than one value, for the reason [`Self::metrics_configurations`] is one —
    /// "You can have up to 1,000 analytics configurations per bucket" (same documentation,
    /// [`MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET`]) — each addressable on its own by the id
    /// the request named it with, so a read, a replacement or a delete of one configuration
    /// leaves every other one exactly as it was. A `BTreeMap`, so iterating it IS ascending
    /// id order and a listing needs no sort step. Empty for a bucket no analytics
    /// configuration has been put on, the state a bucket is created in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: it is
    /// exactly what the put carries as its payload and what
    /// `GetBucketAnalyticsConfiguration` reports back, so a read answers the configuration
    /// the bucket actually carries — every member the request named, including the
    /// destination's own account id — rather than a re-derived copy of it.
    ///
    /// Private, like `tags` and `metrics_configurations`, so the accessor pair below is the
    /// only way in or out.
    analytics_configurations: BTreeMap<String, crate::ops::types::AnalyticsConfiguration>,
    /// The bucket's S3 Inventory configurations, by the inventory id each one is identified
    /// by: a put "adds an S3 Inventory configuration (identified by the inventory ID) to the
    /// bucket" (`PutBucketInventoryConfiguration`'s own documentation).
    ///
    /// A MAP rather than one value, for the reason [`Self::analytics_configurations`] is
    /// one — "You can have up to 1,000 inventory configurations per bucket" (same
    /// documentation, [`MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET`]) — each addressable on its
    /// own by the id the request named it with, so a read, a replacement or a delete of one
    /// configuration leaves every other one exactly as it was. A `BTreeMap`, so iterating it
    /// IS ascending id order and a listing needs no sort step. Empty for a bucket no
    /// inventory configuration has been put on, the state a bucket is created in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: it is
    /// exactly what the put carries as its payload and what
    /// `GetBucketInventoryConfiguration` reports back, so a read answers the configuration
    /// the bucket actually carries — the destination, the schedule, the optional fields it
    /// asked for — rather than a re-derived copy of it.
    ///
    /// Private, like `tags` and `analytics_configurations`, so the accessor pair below is
    /// the only way in or out.
    inventory_configurations: BTreeMap<String, crate::ops::types::InventoryConfiguration>,
    /// The S3 Intelligent-Tiering configurations the bucket carries, by the id each one is
    /// identified by — empty while the bucket carries none, which is the state it is
    /// created in.
    ///
    /// A bucket carries MANY of them, not one: "Puts a S3 Intelligent-Tiering
    /// configuration to the specified bucket. You can have up to 1,000 S3
    /// Intelligent-Tiering configurations per bucket"
    /// (`PutBucketIntelligentTieringConfiguration`'s own documentation), and every other
    /// operation on one names it by "the ID used to identify the S3 Intelligent-Tiering
    /// configuration" (that request's `Id`) — so the id is the key and a put under an id
    /// already present replaces that configuration alone.
    ///
    /// The MODELLED container is stored whole, for the reason [`Self::replication`] is:
    /// it is exactly what the put carries and what a read reports back, so a reader
    /// answers with the configuration the bucket actually carries rather than a re-derived
    /// copy. A `BTreeMap`, so iterating it IS ascending id order and a listing needs no
    /// sort step, and a page's resume point is simply the next id in it.
    ///
    /// Private, like `tags` and `replication`, so the accessors below are the only way in
    /// or out.
    intelligent_tiering: BTreeMap<String, crate::ops::types::IntelligentTieringConfiguration>,
    /// The bucket's website configuration, as `PutBucketWebsite` last set it, or `None`
    /// while no website configuration is associated with the bucket.
    ///
    /// `None` is the state a bucket is created in — a bucket is configured as a website
    /// only by "adding a website configuration" to it (`GetBucketWebsite`'s own
    /// documentation) — and it is what makes a read answer
    /// [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`] rather than report a configuration that does
    /// not exist. Nothing is derived for a never-configured bucket, so a read of one writes
    /// nothing, the way it does not for [`Self::encryption`].
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    website: Option<BucketWebsite>,
    /// The bucket's cors configuration — one entry per `CORSRule` the last
    /// `PutBucketCors` carried, in that order — or `None` while no cors configuration is
    /// associated with the bucket.
    ///
    /// `None` is the state a bucket is created in: a bucket services cross-origin requests
    /// only once "you add the cors subresource to the bucket" (`PutBucketCors`'s own
    /// documentation), and it is what makes a read answer
    /// [`NO_SUCH_CORS_CONFIGURATION_CODE`] rather than report a configuration that does not
    /// exist. An `Option` rather than a plain `Vec` for the reason `ownership_controls` is
    /// one: `CORSConfiguration$CORSRules` is REQUIRED, so a bucket either has a
    /// configuration naming at least one rule or has none at all.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    cors: Option<Vec<BucketCorsRule>>,
    /// Where the bucket's server access logs are delivered, as the last
    /// `PutBucketLogging` set it, or `None` while server access logging is not enabled on
    /// the bucket.
    ///
    /// `None` is the state a bucket is created in, and the state an empty
    /// `BucketLoggingStatus` puts it back into: "To enable logging, you use
    /// `LoggingEnabled` and its children request elements. To disable logging, you use an
    /// empty `BucketLoggingStatus` request element" (`PutBucketLogging`'s own
    /// documentation). It is what makes `GetBucketLogging` report a status container with
    /// no `LoggingEnabled` in it — a 200 rather than a not-found, since every bucket HAS a
    /// logging status and "not enabled" is one of the states it can be in.
    ///
    /// The MODELLED container is stored whole, the way [`Self::replication`] is: it is
    /// exactly what the put carries inside its payload and what `GetBucketLogging` reports
    /// back, so a read answers the configuration the bucket actually carries — every
    /// member the request named, its target grants and its key format included — rather
    /// than a re-derived copy of it.
    ///
    /// Private, like `tags`, so the accessor pair below is the only way in or out.
    logging: Option<crate::ops::types::LoggingEnabled>,
    /// Every VERSION the bucket holds of every key, NEWEST FIRST within each key.
    ///
    /// Entry 0 of a key's chain is its CURRENT version — what a reader that names no
    /// `versionId` answers with — and the rest are the noncurrent ones a later write or a
    /// delete displaced, each addressable by its own [`Object::version_id`]. A chain holds
    /// a [`StoredVersion::DeleteMarker`] wherever a delete that named no version was made
    /// on a bucket carrying a versioning state: "If bucket versioning is enabled, the
    /// operation inserts a delete marker, which becomes the current version of the object"
    /// (`DeleteObject`'s documentation), so the version it displaced is still stored behind
    /// it. A bucket that was never versioning-enabled holds exactly one entry per key, the
    /// `null` version, and no markers at all.
    ///
    /// A `BTreeMap`, so iterating it IS the ascending UTF-8 byte order S3 lists keys in
    /// (`String`'s `Ord` compares bytes) and a listing needs no sort step. A key is present
    /// only while its chain is non-empty, so "the bucket holds no version of this key" and
    /// "the bucket has no such key" are the same state.
    ///
    /// Private for the same reason as `tags`: a version is recorded only through
    /// [`ComAmazonawsS3Amazons3State::put_object`] / [`Self::remove_object`] /
    /// [`Self::remove_version`], which are the only places that see every ceiling and the
    /// store's version sequence.
    versions: BTreeMap<String, Vec<StoredVersion>>,
    /// The emulated service that provisioned this bucket on the account's behalf, or
    /// `None` for a bucket a client created over S3's own API
    /// ([`ComAmazonawsS3Amazons3State::create_bucket`]).
    ///
    /// Another AWS service whose own resource IS an S3-compatible object store (a
    /// Lightsail bucket is the one in this workspace: "Access keys grant full
    /// programmatic access to the specified bucket and its objects") stands that store up
    /// here, through [`ComAmazonawsS3Amazons3State::provision_bucket`], so the objects its
    /// resource holds are the objects THIS service's data plane serves — one object store
    /// in this emulator, not two. Such a bucket is nevertheless not a bucket of the
    /// caller's own S3 account (AWS keeps it in a service-managed account), so a reader
    /// that reports the account's buckets leaves it out:
    /// [`ComAmazonawsS3Amazons3State::account_buckets`].
    ///
    /// Private, like `tags`, and written only where the bucket comes into being: a stored
    /// bucket never changes hands.
    provisioned_by: Option<String>,
    /// Whether AWS itself publishes this bucket, rather than any caller owning it — one of
    /// the world-readable buckets the catalog in [`aws_published`] names.
    ///
    /// Such a bucket is in the store before the first request is served, because that is
    /// what "AWS publishes it" means: nothing creates it and nothing has to. It is served
    /// by this service's object data plane exactly as any other bucket is — that is the
    /// point of holding it — while it belongs to no caller, so
    /// [`ComAmazonawsS3Amazons3State::account_buckets`] leaves it out of the account's own
    /// buckets and the whole-store budgets subtract what the catalog itself holds
    /// ([`ComAmazonawsS3Amazons3State::published_retained_bytes`]).
    ///
    /// Private and written only where the bucket comes into being, like `provisioned_by`.
    aws_published: bool,
    /// The multipart uploads the bucket has in progress, by upload id.
    ///
    /// An upload lands here when `CreateMultipartUpload` initiates it and leaves only
    /// when it is completed or aborted: "After you initiate a multipart upload and upload
    /// one or more parts … you must either complete or abort the multipart upload"
    /// (`CreateMultipartUpload`'s own documentation). Keyed by the id the store minted
    /// ([`mint_upload_id`]), because that is what every later request names the upload by;
    /// sorted, because `ListMultipartUploads` reports uploads in key order and an id is
    /// what distinguishes two uploads of one key.
    ///
    /// Private, like `versions`, and written only through
    /// [`ComAmazonawsS3Amazons3State::create_multipart_upload`], which is the only place
    /// that sees the ceilings an in-progress upload counts against.
    multipart_uploads: BTreeMap<String, MultipartUpload>,
    /// The bucket's S3 Metadata configuration, as `CreateBucketMetadataConfiguration`
    /// recorded it, or `None` while the bucket carries none — the state a bucket is
    /// created in, and what makes `GetBucketMetadataConfiguration` answer
    /// [`NO_SUCH_CONFIGURATION_CODE`] until a create records one.
    ///
    /// At most one, so a value rather than a map: see [`BucketMetadataConfiguration`].
    ///
    /// Private, like `tags` and `encryption`, so the accessor pair below is the only way
    /// in or out.
    metadata_configuration: Option<BucketMetadataConfiguration>,
}

impl Bucket {
    /// When the bucket was created, as `create_bucket` recorded it. Stable across reads:
    /// nothing updates it. `None` only for a `Default`-built value, which this store
    /// never holds — a reader reports no creation date rather than inventing one.
    pub fn created_at(&self) -> Option<SystemTime> {
        self.created_at
    }

    /// The emulated service that provisioned the bucket as the object store of one of ITS
    /// resources, or `None` for a bucket created over S3's own API. See the field.
    pub fn provisioned_by(&self) -> Option<&str> {
        self.provisioned_by.as_deref()
    }

    /// Whether AWS publishes this bucket rather than a caller owning it. See the
    /// `aws_published` field and the [`aws_published`] catalog.
    pub fn is_aws_published(&self) -> bool {
        self.aws_published
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

    /// Whether S3 Object Lock is enabled on the bucket — see [`Self::object_lock_enabled`].
    ///
    /// A pure read: a bucket Object Lock was never enabled on carries no configuration,
    /// which is reported rather than stored.
    pub fn object_lock_enabled(&self) -> bool {
        self.object_lock_enabled
    }

    /// Enable S3 Object Lock on the bucket, and with it versioning: "Object Lock works only
    /// in versioned buckets" (Amazon S3 User Guide, *Using S3 Object Lock*), so a bucket
    /// that carries an Object Lock configuration carries the `Enabled` versioning status
    /// that lets a lock be addressed to a version at all.
    ///
    /// One place, so no caller can record half of that pair.
    pub fn enable_object_lock(&mut self) {
        self.object_lock_enabled = true;
        // A bucket already in that state keeps the versioning configuration it carries, MFA
        // delete included: there is nothing to set.
        if !self.versioning_enabled() {
            self.set_versioning(VERSIONING_ENABLED_STATUS, None);
        }
    }

    /// The default retention new objects written to this bucket are locked under, or `None`
    /// when its Object Lock configuration names no rule — see [`Self::object_lock_rule`].
    ///
    /// A pure read: a bucket whose configuration carries no rule is reported as carrying
    /// none rather than having one stored for it.
    pub fn object_lock_default_retention(&self) -> Option<&BucketDefaultRetention> {
        self.object_lock_rule.as_ref()
    }

    /// Record the `ObjectLockConfiguration` a `PutObjectLockConfiguration` carried: Object
    /// Lock enabled on the bucket (with the versioning state Object Lock needs, through
    /// [`Self::enable_object_lock`]), and `default_retention` as the rule its new objects are
    /// locked under.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::set_versioning`] replaces the
    /// versioning state: the request carries "the Object Lock configuration that you want to
    /// apply to the specified bucket"
    /// (`PutObjectLockConfigurationRequest$ObjectLockConfiguration`) as its complete
    /// payload, so a configuration naming no rule leaves the bucket carrying none.
    pub fn set_object_lock_configuration(
        &mut self,
        default_retention: Option<BucketDefaultRetention>,
    ) {
        self.enable_object_lock();
        self.object_lock_rule = default_retention;
    }

    /// Why S3 refuses a request that would permanently remove the stored version `object`
    /// of `key` — `bucket` being this bucket's own name, which the authorization decision
    /// names the object by — judged at `at`, or `None` when nothing bars it.
    /// `bypass_requested` is whether the request asked to bypass governance-mode
    /// restrictions; what is answered is the `<Message>` the 403 [`ACCESS_DENIED_CODE`]
    /// carries.
    ///
    /// S3 Object Lock is a WORM protection, and what it protects is the VERSION: "if you
    /// try to delete objects protected by … retention and the retention period has not
    /// expired, the operation fails" (Amazon S3 User Guide, *S3 Object Lock*). So a
    /// permanent delete is refused while
    ///
    /// * a legal hold is on the version — judged FIRST and independently of anything the
    ///   request asks for, because a legal hold "provides the same protection as a
    ///   retention period" while having "no expiration date" and is lifted only by
    ///   `PutObjectLegalHold`, never by the governance bypass, which asks about
    ///   "Governance-mode restrictions"
    ///   (`DeleteObjectRequest$BypassGovernanceRetention`) and nothing else; or
    /// * the version's retention period is still running
    ///   ([`ObjectRetention::in_force_at`]) — unless it is a [`GOVERNANCE_RETENTION_MODE`]
    ///   one, the request asked to bypass it, and the caller may perform
    ///   [`BYPASS_GOVERNANCE_RETENTION_ACTION`]
    ///   ([`Self::governance_bypass_denied`]), since the header alone is not the privilege.
    ///
    /// The two are refused with the two sentences S3 words them in: the lock itself bars the
    /// call ([`OBJECT_LOCK_ACCESS_DENIED_MESSAGE`]), while a caller the policy denies the
    /// override to is refused the OVERRIDE, which is an authorization decision about that
    /// action ([`explicit_resource_policy_denial`]) and leaves the retention standing.
    ///
    /// An expired retention protects nothing, which is what lets the same delete succeed
    /// once the period has lapsed. A version carrying neither is refused nothing here.
    pub fn object_lock_delete_refusal(
        &self,
        bucket: &str,
        key: &str,
        object: &Object,
        at: SystemTime,
        bypass_requested: bool,
    ) -> Option<String> {
        if object.legal_hold == Some(true) {
            return Some(OBJECT_LOCK_ACCESS_DENIED_MESSAGE.to_string());
        }
        let retention = object
            .retention
            .as_ref()
            .filter(|retention| retention.in_force_at(at))?;
        if !bypass_requested || retention.mode != GOVERNANCE_RETENTION_MODE {
            return Some(OBJECT_LOCK_ACCESS_DENIED_MESSAGE.to_string());
        }
        self.governance_bypass_denied(bucket, key).then(|| {
            explicit_resource_policy_denial(
                BYPASS_GOVERNANCE_RETENTION_ACTION,
                &object_arn(bucket, key),
            )
        })
    }

    /// Whether the caller may NOT lift the governance-mode retention on the object `key` of
    /// the bucket named `bucket`.
    ///
    /// The bypass a request asks for is a PRIVILEGED override rather than a self-service
    /// one: "to override or remove governance-mode retention settings you must have the
    /// `s3:BypassGovernanceRetention` permission" (Amazon S3 User Guide, *S3 Object Lock*),
    /// so every operation whose `BypassGovernanceRetention` member can lift a retention asks
    /// this before honouring it — the delete of a retained version
    /// ([`Self::object_lock_delete_refusal`]) and the retention write that would weaken one.
    pub fn governance_bypass_denied(&self, bucket: &str, key: &str) -> bool {
        self.caller_is_denied(BYPASS_GOVERNANCE_RETENTION_ACTION, &object_arn(bucket, key))
    }

    /// Whether this bucket's own policy REFUSES the calling principal `action` on
    /// `resource`.
    ///
    /// Decided by the tree's one evaluation of IAM policy documents
    /// ([`policy_decision_for_anonymous`] asks it for the caller that presents no
    /// credential), for the principal a credentialed request here speaks for
    /// ([`caller_principal_arn`]) together with the spellings that name its whole account —
    /// a statement denying the account refuses each of its principals.
    ///
    /// The request carries a value for every condition key this service can ANSWER
    /// ([`REQUEST_CONDITION_KEYS`]) and declares those keys as ones it models, so a clause
    /// over one of them is DECIDED: the guardrail clause AWS publishes for a bucket policy —
    /// a `Deny` of everything that did not arrive over a secure transport — holds for no
    /// request this emulator serves, and the same clause stated the other way holds for
    /// every one.
    ///
    /// Only a deny this evaluation could decide refuses: a clause over a key this service
    /// does not answer leaves the statement UNDECIDED, and the evaluation's reading of an
    /// undecided `Deny` is that it still refuses — fail-closed, which is right where a GRANT
    /// is at stake and wrong here, because it would refuse the bucket owner's own read of
    /// every bucket whose policy conditions anything on a key nothing in this emulator
    /// models. The keys a decision needed and the request did not carry are what says which
    /// case this is (`Outcome::required_context_keys`).
    ///
    /// Only a DENY is reported at all, because this emulator serves one account and the
    /// caller is that account: a document that says nothing about the request leaves the
    /// bucket's own owner able to act on it, which is how every other object operation here
    /// treats it. A bucket carrying no policy denies nothing.
    fn caller_is_denied(&self, action: &str, resource: &str) -> bool {
        let Some(policy) = self.policy.as_deref() else {
            return false;
        };
        let caller = caller_principal_arn();
        let mut principals = vec![caller.clone()];
        principals.extend(::service_common::iam_authorization::account_principals(
            &caller,
        ));
        let context = [(
            SECURE_TRANSPORT_KEY.to_string(),
            SECURE_TRANSPORT_VALUE.to_string(),
        )];
        let outcome = ::service_common::iam_authorization::resource_outcome_prepared(
            &::service_common::iam_authorization::PreparedDocument::read(policy),
            &::service_common::iam_authorization::Request::new(action, resource, &principals)
                .with_context(&context)
                .with_published_keys(REQUEST_CONDITION_KEYS),
        );
        outcome.decision == ::service_common::iam_authorization::Decision::Denied
            && outcome.required_context_keys.is_empty()
    }

    /// The object a delete that names NO `versionId` would remove PERMANENTLY, or `None`
    /// when such a delete takes nothing away.
    ///
    /// Which one it is follows [`Self::remove_object`]: on a versioning-ENABLED bucket the
    /// delete "inserts a delete marker, which becomes the current version of the object"
    /// and every stored version stays, so nothing is removed at all; on any other bucket it
    /// takes the `null` version away — the one entry carrying no minted id — which is a
    /// permanent removal like the one a `versionId` asks for, and so is subject to the same
    /// Object Lock protection ([`Self::object_lock_delete_refusal`]).
    pub fn permanently_deleted_version(&self, key: &str) -> Option<&Object> {
        if self.versioning_enabled() {
            return None;
        }
        let position = self.null_version_position(key)?;
        self.versions.get(key)?.get(position)?.as_object()
    }

    /// The bucket's Transfer Acceleration state — a `BucketAccelerateStatus` value — or
    /// `None` while no state has ever been set on it, which is what makes a read report no
    /// state value at all rather than a default. See [`Self::accelerate`].
    pub fn accelerate_status(&self) -> Option<&str> {
        self.accelerate.as_deref()
    }

    /// Set the bucket's Transfer Acceleration state from the `AccelerateConfiguration` a
    /// `PutBucketAccelerateConfiguration` request carried.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Bucket::set_versioning`] replaces
    /// the versioning state: `AccelerateConfiguration` is the "Container for setting the
    /// transfer acceleration state" the request carries as its whole payload, so the state
    /// the bucket is in afterwards is the one it names.
    pub fn set_accelerate_status(&mut self, status: &str) {
        self.accelerate = Some(status.to_string());
    }

    /// Who pays for downloads from the bucket: the `Payer` its request payment
    /// configuration was last set to, or [`DEFAULT_PAYER`] while it has never been set.
    ///
    /// Derived rather than stored for a never-configured bucket, so a read reports the
    /// default without writing one — the way [`Self::acl_grants`] reports the canned ACL
    /// a bucket starts with.
    pub fn payer(&self) -> &str {
        self.request_payment.as_deref().unwrap_or(DEFAULT_PAYER)
    }

    /// Associate `payer` with the bucket as its whole request payment configuration.
    ///
    /// A replacement, not a merge: `RequestPaymentConfiguration` is the "Container for
    /// Payer" a `PutBucketRequestPayment` carries as its payload, and it holds that one
    /// REQUIRED member (`smithy_query(query="shape",shape="RequestPaymentConfiguration")`),
    /// so what the request names becomes the bucket's configuration in full — the way
    /// [`Bucket::set_versioning`] replaces the versioning state.
    pub fn set_payer(&mut self, payer: &str) {
        self.request_payment = Some(payer.to_string());
    }

    /// The ABAC status the bucket carries: the one `PutBucketAbac` last set, or
    /// [`DEFAULT_BUCKET_ABAC_STATUS`] for a bucket no put has set one on.
    ///
    /// A pure read — the default is DERIVED here rather than stored, so reporting it
    /// writes nothing and a later caller sees exactly what this one saw.
    pub fn abac_status(&self) -> &str {
        self.abac.as_deref().unwrap_or(DEFAULT_BUCKET_ABAC_STATUS)
    }

    /// Set the bucket's ABAC status from the `AbacStatus` a `PutBucketAbac` request
    /// carried.
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Bucket::set_versioning`] replaces
    /// the versioning state: the request carries the complete `AbacStatus` container as
    /// its payload, so the state the bucket is in afterwards is the one it names.
    pub fn set_abac_status(&mut self, status: &str) {
        self.abac = Some(status.to_string());
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
    /// for an Amazon S3 bucket" — which leaves the bucket with no configuration at all, the
    /// one state in which a later `GetPublicAccessBlock` reports none. It is NOT the state
    /// the bucket was created in: a create installs
    /// [`BucketPublicAccessBlock::all_enabled`].
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
        // The rules the container names are what the bucket's effective setting becomes:
        // the LAST one wins, the way a later rule in one document overrides an earlier one.
        if let Some(last) = rules.last() {
            self.object_ownership = Some(last.clone());
        }
        self.ownership_controls = Some(rules);
    }

    /// Remove the bucket's ownership controls, leaving none associated with it.
    ///
    /// What `DeleteBucketOwnershipControls` does — "Removes OwnershipControls for an Amazon
    /// S3 bucket" — which puts the bucket back in the state it was created in, so a later
    /// `GetBucketOwnershipControls` reports no ownership controls again
    /// ([`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`]). Scoped to this one bucket's controls:
    /// nothing else on the record is touched.
    ///
    /// A bucket with no `OwnershipControls` is one whose ACLs are ENABLED, so the
    /// effective setting becomes [`OBJECT_WRITER`] rather than the
    /// [`DEFAULT_OBJECT_OWNERSHIP`] a create derives: that default belongs to the create,
    /// and withdrawing the controls is the documented way back to ACLs that affect
    /// permissions.
    pub fn clear_ownership_controls(&mut self) {
        self.ownership_controls = None;
        self.object_ownership = Some(OBJECT_WRITER.to_string());
    }

    /// The bucket's effective `ObjectOwnership` setting: what nothing has changed it from
    /// is [`DEFAULT_OBJECT_OWNERSHIP`].
    pub fn object_ownership(&self) -> &str {
        self.object_ownership
            .as_deref()
            .unwrap_or(DEFAULT_OBJECT_OWNERSHIP)
    }

    /// Set the bucket's effective `ObjectOwnership` setting — what `CreateBucket`'s
    /// `x-amz-object-ownership` header names, and what the last rule of a
    /// `PutBucketOwnershipControls` payload applies.
    pub fn set_object_ownership(&mut self, ownership: &str) {
        self.object_ownership = Some(ownership.to_string());
    }

    /// The zone and redundancy this bucket was placed with as a DIRECTORY bucket, or `None`
    /// for a general purpose bucket. See [`Bucket::directory`].
    pub fn directory_placement(&self) -> Option<&DirectoryBucketPlacement> {
        self.directory.as_ref()
    }

    /// Whether this is a directory bucket — the one question the operations that serve only
    /// directory buckets, and the ones that serve only general purpose buckets, ask.
    pub fn is_directory(&self) -> bool {
        self.directory.is_some()
    }

    /// Place this bucket in a zone as a directory bucket. Called by `CreateBucket` under the
    /// same guard that recorded the bucket, so no reader can see the bucket before its kind
    /// is settled.
    pub fn set_directory_placement(&mut self, placement: DirectoryBucketPlacement) {
        self.directory = Some(placement);
    }

    /// The storage class an object written into this bucket is stored in when the write
    /// names none: the S3 Express One Zone class for a directory bucket
    /// ([`EXPRESS_ONEZONE_STORAGE_CLASS`]) and [`DEFAULT_STORAGE_CLASS`] for a general
    /// purpose one — "if you don't specify, S3 Standard is the default storage class"
    /// (`PutObjectRequest$StorageClass`).
    pub fn default_storage_class(&self) -> &'static str {
        if self.is_directory() {
            EXPRESS_ONEZONE_STORAGE_CLASS
        } else {
            DEFAULT_STORAGE_CLASS
        }
    }

    /// The ARN of this bucket, held under the name `name` it is recorded in the store by:
    /// "the Amazon Resource Name (ARN) of the S3 bucket. ARNs uniquely identify Amazon Web
    /// Services resources" (`HeadBucketOutput$BucketArn`).
    ///
    /// Which form it takes is decided by the bucket's own KIND, not by the operation
    /// reporting it: a directory bucket is named in the `s3express` namespace, with the
    /// Region and account its zone is in ([`directory_bucket_arn`]), and a general purpose
    /// bucket by its globally unique name alone ([`bucket_arn`]). One place, so every
    /// reader that reports a bucket's ARN reports the same string for the same bucket.
    pub fn arn(&self, name: &str) -> String {
        if self.is_directory() {
            directory_bucket_arn(&self.region, name)
        } else {
            bucket_arn(name)
        }
    }

    /// Whether ACLs are DISABLED on the bucket, i.e. its effective `ObjectOwnership`
    /// setting is [`BUCKET_OWNER_ENFORCED`] — which is what a bucket is CREATED with
    /// unless the create or a later `PutBucketOwnershipControls` names another value.
    ///
    /// "If your bucket uses the bucket owner enforced setting for S3 Object Ownership,
    /// ACLs are disabled and no longer affect permissions" (`PutBucketAcl`'s own
    /// documentation): a request to SET an ACL on such a bucket is refused with
    /// [`ACL_NOT_SUPPORTED_CODE`], while a request to READ one still answers — with the
    /// bucket-owner-full-control ACL, which is [`default_acl_grants`].
    pub fn acls_disabled(&self) -> bool {
        self.object_ownership() == BUCKET_OWNER_ENFORCED
    }

    /// The grants of the bucket's access control list: the ACL last set on it, or the
    /// canned `private` ACL every bucket starts with ([`default_acl_grants`]).
    ///
    /// Derived rather than stored for a bucket that has never had an ACL set, so a read
    /// reports the default without writing one. A bucket whose ACLs are disabled
    /// ([`Self::acls_disabled`]) reports the default too: "requests to read ACLs are
    /// still supported and return the bucket-owner-full-control ACL with the owner being
    /// the account that created the bucket" (`GetBucketAcl`'s own documentation).
    pub fn acl_grants(&self) -> Vec<AclGrant> {
        let stored = match self.acl.as_deref() {
            Some(grants) if !self.acls_disabled() => grants.to_vec(),
            _ => default_acl_grants(),
        };
        self.reported_acl_grants(stored)
    }

    /// The grants of `stored` an ACL READ reports, given the bucket's Block Public Access
    /// settings: all of them, less every PUBLIC grant ([`acl_grant_is_public`]) while
    /// `IgnorePublicAcls` is on.
    ///
    /// "Setting this element to TRUE causes Amazon S3 to ignore all public ACLs on this
    /// bucket and any objects that it contains"
    /// (`PublicAccessBlockConfiguration$IgnorePublicAcls`): a grant S3 ignores gives nobody
    /// anything, so a read of the ACL does not report it. The stored grants are untouched —
    /// the setting "doesn't affect the persistence of any existing ACLs" (same
    /// documentation) — so they are reported again once it is turned off, exactly as
    /// disabling ACLs hides them without taking them away.
    ///
    /// ONE rule for the bucket's own ACL and for the ACL of any object it contains, which
    /// is what the setting says it covers.
    fn reported_acl_grants(&self, stored: Vec<AclGrant>) -> Vec<AclGrant> {
        if self
            .public_access_block()
            .is_some_and(|settings| settings.ignore_public_acls)
        {
            return stored
                .into_iter()
                .filter(|grant| !acl_grant_is_public(grant))
                .collect();
        }
        stored
    }

    /// The grants a `PutBucketAcl` (or the create's canned ACL) last set on the bucket,
    /// or `None` when none ever was — the raw record, NOT what a reader reports.
    ///
    /// [`Self::acl_grants`] reports the default for a bucket whose ACLs are disabled,
    /// because that is what `GetBucketAcl` answers; this is what the bucket actually
    /// carries, which is what decides whether switching Object Ownership to
    /// [`BUCKET_OWNER_ENFORCED`] would disable an ACL that still grants something.
    pub fn stored_acl_grants(&self) -> Option<&[AclGrant]> {
        self.acl.as_deref()
    }

    /// Associate `grants` with the bucket as its whole access control list.
    ///
    /// A replacement, not a merge: `PutBucketAcl` "sets the permissions on an existing
    /// bucket using access control lists" (that operation's own documentation) from the
    /// one ACL the request names — a canned ACL, the `x-amz-grant-*` headers or an
    /// `AccessControlPolicy` body — so what the request names becomes the bucket's ACL in
    /// full, the way [`Bucket::set_ownership_controls`] replaces the ownership controls.
    pub fn set_acl_grants(&mut self, grants: Vec<AclGrant>) {
        self.acl = Some(grants);
    }

    /// The grants of the access control list of `object`, one of the objects this bucket
    /// holds: the ACL last set on that version, or the owner's `FULL_CONTROL` alone
    /// ([`default_acl_grants`]) when none ever was.
    ///
    /// Derived rather than stored for an object nobody has set an ACL on, so a read reports
    /// the default without writing one — the way [`Self::acl_grants`] reports the bucket's
    /// own default. An object of a bucket whose ACLs are disabled
    /// ([`Self::acls_disabled`]) reports the default too: "If your bucket uses the bucket
    /// owner enforced setting for S3 Object Ownership, requests to read ACLs are still
    /// supported and return the bucket-owner-full-control ACL with the owner being the
    /// account that created the bucket" (`GetObjectAcl`'s own documentation). The grants
    /// the version stores are untouched by that, so they are reported again once ACLs
    /// affect permissions on the bucket.
    ///
    /// On the BUCKET rather than on [`Object`] because the answer turns on the bucket's
    /// Object Ownership setting and on its Block Public Access settings
    /// ([`Self::reported_acl_grants`], which drops the public grants S3 ignores), exactly as
    /// [`Self::object_expiration`] lives here because it turns on the bucket's lifecycle
    /// configuration.
    pub fn object_acl_grants(&self, object: &Object) -> Vec<AclGrant> {
        let stored = match object.acl.as_deref() {
            Some(grants) if !self.acls_disabled() => grants.to_vec(),
            _ => default_acl_grants(),
        };
        self.reported_acl_grants(stored)
    }

    /// Why S3 refuses a read of `key` — the bytes or the metadata of the version `object`
    /// names, `None` for an address this bucket holds no object at — to a caller that
    /// presents NO credential, or `None` when the public may read it.
    ///
    /// The ANONYMOUS caller is a principal the bucket has to grant access to explicitly,
    /// so what it may read is what this bucket's own configuration gives to EVERYONE, and
    /// everything else is refused ([`ACCESS_DENIED_MESSAGE`]) rather than served:
    ///
    /// * an ACL grant to `AllUsers` ([`acl_grants_to_all_users`]) carrying the `READ` the
    ///   read needs, judged on the grants an ACL read reports — which is where
    ///   `IgnorePublicAcls` has already dropped the public grants S3 ignores, so a grant
    ///   that is ignored admits nobody;
    /// * a bucket policy statement that allows [`GET_OBJECT_ACTION`] on this object to
    ///   every principal ([`policy_decision_for_anonymous`]) — unless
    ///   `RestrictPublicBuckets` is on, which "restricts access to … buckets with public
    ///   policies to only AWS service principals and authorized users within the bucket
    ///   owner's account" (`PublicAccessBlockConfiguration$RestrictPublicBuckets`), a set
    ///   this caller is in neither of. An explicit `Deny` refuses the read whatever the
    ///   ACL grants, the way IAM combines the two.
    ///
    /// A Requester Pays bucket refuses it ahead of all of that
    /// ([`Self::requester_pays_anonymous_refusal`]): there is no account to charge.
    pub fn anonymous_object_read_refusal(
        &self,
        bucket: &str,
        key: &str,
        object: Option<&Object>,
    ) -> Option<&'static str> {
        if let Some(message) = self.requester_pays_anonymous_refusal() {
            return Some(message);
        }
        let granted_by_acl = object.is_some_and(|object| {
            acl_grants_to_all_users(&self.object_acl_grants(object), PERMISSION_READ)
        });
        let restricted = self
            .public_access_block()
            .is_some_and(|settings| settings.restrict_public_buckets);
        let by_policy = self
            .policy
            .as_deref()
            .map(|policy| {
                policy_decision_for_anonymous(policy, GET_OBJECT_ACTION, &object_arn(bucket, key))
            })
            // A bucket carrying no policy has no policy to state anything, which is the
            // decision a document that says nothing about the request reaches.
            .unwrap_or_default();
        let granted = match by_policy {
            ::service_common::iam_authorization::Decision::Denied => false,
            ::service_common::iam_authorization::Decision::Allowed => !restricted || granted_by_acl,
            ::service_common::iam_authorization::Decision::NotStated => granted_by_acl,
        };
        (!granted).then_some(ACCESS_DENIED_MESSAGE)
    }

    /// Why S3 refuses a read of `key` — the bytes or the metadata of the version `addressed`
    /// names, `None` for an address this bucket holds no object at — to the CREDENTIALED
    /// caller, or `None` when this bucket's policy leaves the read to it.
    ///
    /// A bucket policy is a RESOURCE-BASED policy, so it is in force on every request against
    /// the bucket's resources and not only on the public ones
    /// ([`Self::anonymous_object_read_refusal`] is the same question asked for the caller that
    /// presents none): an explicit `Deny` about the request "wins, whatever any `Allow` says"
    /// (IAM's own evaluation order, which [`Self::caller_is_denied`] answers by), and this
    /// emulator serves one account that owns its buckets, so a document saying nothing about
    /// the request leaves the read to that owner.
    ///
    /// WHICH refusal it is depends on what the address resolved to, and the two are worded as
    /// S3 words them:
    ///
    /// * a version the read DID resolve, whose read the policy denies, is refused the read
    ///   itself — the `AccessDenied` [`ACCESS_DENIED_MESSAGE`] states, about
    ///   [`GET_OBJECT_ACTION`] on that object;
    /// * an address holding NOTHING is answered by what the caller may learn about the
    ///   bucket's contents at all: "if you have the `s3:ListBucket` permission on the bucket,
    ///   Amazon S3 returns an HTTP status code 404 Not Found error. If you don't have the
    ///   `s3:ListBucket` permission, Amazon S3 returns an HTTP status code 403 Access Denied
    ///   error" (`GetObject`'s own documentation) — so a policy denying
    ///   [`LIST_BUCKET_ACTION`] on this bucket refuses the read instead of reporting the key
    ///   absent, with the sentence an explicit deny carries
    ///   ([`explicit_resource_policy_denial`]), and one that does not leaves the 404 to be
    ///   answered.
    ///
    /// Asked BEFORE anything about the key is reported, which is what keeps a refusal from
    /// saying whether the object is there.
    pub fn caller_object_read_refusal(
        &self,
        bucket: &str,
        key: &str,
        addressed: Option<&Object>,
    ) -> Option<String> {
        if addressed.is_some() {
            return self
                .caller_is_denied(GET_OBJECT_ACTION, &object_arn(bucket, key))
                .then(|| ACCESS_DENIED_MESSAGE.to_string());
        }
        let resource = self.arn(bucket);
        self.caller_is_denied(LIST_BUCKET_ACTION, &resource)
            .then(|| explicit_resource_policy_denial(LIST_BUCKET_ACTION, &resource))
    }

    /// Why REQUESTER PAYS refuses a request against this bucket that carries no credential
    /// ([`REQUESTER_PAYS_ANONYMOUS_MESSAGE`]), or `None` for a bucket whose owner pays.
    ///
    /// "Requests to a Requester Pays bucket must be authenticated: you cannot make anonymous
    /// requests" (`PutBucketRequestPayment`'s own documentation of the feature), because such
    /// a request names no account to charge. So the refusal is about the CALLER rather than
    /// about what the bucket grants: it stands however public the object is, and whether or
    /// not the request acknowledges the charge, which is why it is answered ahead of every
    /// grant ([`Self::anonymous_object_read_refusal`]) and why the sentence is the same
    /// wherever an unauthenticated caller is refused — an operation's own authorization, or
    /// the signature boundary that refuses a request before a handler sees it
    /// ([`crate::service::sigv4`]).
    pub fn requester_pays_anonymous_refusal(&self) -> Option<&'static str> {
        (self.payer() == PAYER_REQUESTER).then_some(REQUESTER_PAYS_ANONYMOUS_MESSAGE)
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

    /// Take the bucket's own default encryption configuration away, reporting whether it
    /// carried one.
    ///
    /// What `DeleteBucketEncryption` does: it "resets the default encryption for the bucket
    /// as server-side encryption with Amazon S3 managed keys (SSE-S3)" (that operation's own
    /// documentation) — which is exactly the state a bucket that was never configured is in,
    /// so dropping the stored rules is what resets it: [`Self::effective_encryption_rules`]
    /// then computes [`default_encryption_rule`] again for every reader, and a write that
    /// names no encryption of its own inherits that default
    /// ([`Self::default_object_encryption`]).
    ///
    /// Scoped to this one subresource, the way [`Self::clear_replication`] is scoped to the
    /// replication one: every other configuration the bucket holds, and its objects, are
    /// left exactly as they were.
    pub fn clear_encryption(&mut self) -> bool {
        self.encryption.take().is_some()
    }

    /// The bucket's replication configuration, or `None` while no configuration is
    /// associated with it.
    pub fn replication(&self) -> Option<&crate::ops::types::ReplicationConfiguration> {
        self.replication.as_ref()
    }

    /// The bucket's S3 Metadata configuration, or `None` while it carries none — in which
    /// case a read of it answers [`NO_SUCH_CONFIGURATION_CODE`].
    pub fn metadata_configuration(&self) -> Option<&BucketMetadataConfiguration> {
        self.metadata_configuration.as_ref()
    }

    /// Associate `configuration` with the bucket as its whole S3 Metadata configuration.
    ///
    /// The caller decides whether the bucket may take one: a bucket already carrying a
    /// metadata configuration is one `CreateBucketMetadataConfiguration` refuses rather
    /// than overwrites, so this is reached only once per configuration — with
    /// `GetBucketMetadataConfiguration` reporting it from then on.
    pub fn set_metadata_configuration(&mut self, configuration: BucketMetadataConfiguration) {
        self.metadata_configuration = Some(configuration);
    }

    /// Take the bucket's S3 Metadata configuration away, reporting whether it carried one.
    ///
    /// What `DeleteBucketMetadataConfiguration` does to the BUCKET: it "deletes an S3
    /// Metadata configuration from a general purpose bucket" (that operation's own
    /// documentation), so the whole subresource goes and a read of it reports none again. The
    /// tables that configuration provisioned are NOT the bucket's: they are tables of the
    /// managed table bucket's namespace, which the store holds and which this delete leaves
    /// untouched ([`ComAmazonawsS3Amazons3State::delete_metadata_configuration`] is the one
    /// way in).
    pub fn clear_metadata_configuration(&mut self) -> bool {
        self.metadata_configuration.take().is_some()
    }

    /// Record `configuration_state` as the inventory table state of the bucket's S3 Metadata
    /// configuration, with `table_name` the name an inventory table this creates is created
    /// under and `failure` why such a table could not be created — reporting whether the
    /// bucket carries a configuration to record it on.
    ///
    /// What `UpdateBucketMetadataInventoryTableConfiguration` does: it "enables or disables a
    /// live inventory table for an S3 Metadata configuration on a general purpose bucket"
    /// (that operation's own documentation), so the state the request names replaces the one
    /// the configuration carried and the inventory table behind it follows
    /// ([`set_optional_metadata_table_state`]). Scoped to that one table: the journal table,
    /// its record expiration and the annotation table are left exactly as they were, the way
    /// [`Self::clear_replication`] is scoped to the one subresource it names.
    ///
    /// `false` is a bucket carrying NO metadata configuration, in which case nothing is
    /// written: there is no configuration to enable a table for, and the caller answers
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    ///
    /// PRIVATE: a table this provisions is a table of the managed table bucket's namespace,
    /// which the STORE holds, so [`ComAmazonawsS3Amazons3State::update_metadata_inventory_table`]
    /// is the one way in — it is what decides the name, judges it against that namespace and
    /// records it there, so the bucket's record and the namespace cannot be written apart.
    fn update_metadata_inventory_table(
        &mut self,
        configuration_state: String,
        table_name: &str,
        failure: Option<MetadataTableFailure>,
        now: SystemTime,
    ) -> bool {
        let Some(configuration) = self.metadata_configuration.as_mut() else {
            return false;
        };
        set_optional_metadata_table_state(
            &mut configuration.inventory,
            configuration_state,
            // `InventoryTableConfigurationUpdates` carries no `Role`: only the annotation
            // table is managed by an IAM role.
            None,
            table_name,
            failure,
            now,
        );
        true
    }

    /// Record `configuration_state` as the annotation table state of the bucket's S3 Metadata
    /// configuration and `role` as the IAM role that manages it, with `table_name` the name an
    /// annotation table this creates is created under — reporting whether the bucket carries a
    /// configuration to record them on.
    ///
    /// What `UpdateBucketMetadataAnnotationTableConfiguration` does: it is used "to enable or
    /// disable the annotation table, or to update its associated IAM role" (that operation's
    /// own documentation), so the state the request names replaces the one the configuration
    /// carried and the annotation table behind it follows
    /// ([`set_optional_metadata_table_state`]), while a `role` the request names replaces the
    /// role in effect and `None` leaves it as it was ("the new IAM role ARN to apply",
    /// `AnnotationTableConfigurationUpdates$Role`, which is optional). Scoped to that one
    /// table: the journal table, its record expiration and the inventory table are left
    /// exactly as they were, the way [`Self::update_metadata_inventory_table`] is scoped to the
    /// one table it names.
    ///
    /// `false` is a bucket carrying NO metadata configuration, in which case nothing is
    /// written: there is no configuration to enable a table for, and the caller answers
    /// [`METADATA_CONFIGURATION_NOT_FOUND_CODE`].
    pub fn update_metadata_annotation_table(
        &mut self,
        configuration_state: String,
        role: Option<String>,
        table_name: &str,
        now: SystemTime,
    ) -> bool {
        let Some(configuration) = self.metadata_configuration.as_mut() else {
            return false;
        };
        set_optional_metadata_table_state(
            &mut configuration.annotation,
            configuration_state,
            role,
            table_name,
            // No provisioning failure is reportable for an ANNOTATION table: the published V2
            // list of `ErrorDetails$ErrorCode` values names none for one, so there is no code
            // or sentence to report it with — which is where the create that provisions the
            // same table leaves it too.
            None,
            now,
        );
        true
    }

    /// Record `record_expiration` as the journal table record expiration of the bucket's S3
    /// Metadata configuration — reporting whether the bucket carries a configuration to
    /// record it on.
    ///
    /// What `UpdateBucketMetadataJournalTableConfiguration` does: it "enables or disables
    /// journal table record expiration for an S3 Metadata configuration on a general purpose
    /// bucket" (that operation's own documentation), and the only thing its payload carries
    /// is that setting (`JournalTableConfigurationUpdates` has `RecordExpiration` as its one,
    /// required member), so the settings the request names replace the ones the configuration
    /// carried in full. Scoped to that one setting: the journal TABLE itself — which every
    /// configuration has, so there is none to create or drop — its name and its status, and
    /// the inventory and annotation tables, are left exactly as they were, the way
    /// [`Self::update_metadata_inventory_table`] is scoped to the one table it names.
    ///
    /// `false` is a bucket carrying NO metadata configuration, in which case nothing is
    /// written: there is no journal table to expire records of, and the caller answers
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    pub fn update_metadata_journal_record_expiration(
        &mut self,
        record_expiration: crate::ops::types::RecordExpiration,
    ) -> bool {
        let Some(configuration) = self.metadata_configuration.as_mut() else {
            return false;
        };
        configuration.journal_record_expiration = record_expiration;
        true
    }

    /// Associate `configuration` with the bucket as its whole replication configuration.
    ///
    /// A replacement, not a merge: `PutBucketReplication` "creates a replication
    /// configuration or replaces an existing one" from the complete
    /// `ReplicationConfiguration` it carries as its payload, so what the request names
    /// becomes the bucket's configuration in full.
    pub fn set_replication(
        &mut self,
        configuration: crate::ops::types::ReplicationConfiguration,
    ) {
        self.replication = Some(configuration);
    }

    /// Remove the bucket's replication configuration, reporting whether it carried one.
    ///
    /// What `DeleteBucketReplication` does — it "deletes the replication configuration from
    /// the bucket" (that operation's own documentation) — which puts the bucket back in the
    /// state it was created in, so a later `GetBucketReplication` reports no configuration
    /// again ([`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`]). Scoped to this one
    /// subresource: the bucket's versioning, its objects and every other configuration are
    /// left exactly as they were, the way [`Self::clear_lifecycle`] is scoped to the
    /// lifecycle one.
    pub fn clear_replication(&mut self) -> bool {
        self.replication.take().is_some()
    }

    /// The bucket's lifecycle configuration, or `None` while no configuration is associated
    /// with it.
    pub fn lifecycle(&self) -> Option<&BucketLifecycle> {
        self.lifecycle.as_ref()
    }

    /// Remove the bucket's lifecycle configuration, reporting whether it carried one.
    ///
    /// What `DeleteBucketLifecycle` does: it "removes all the lifecycle configuration rules
    /// in the lifecycle subresource associated with the bucket" (that operation's own
    /// documentation), which leaves the bucket in the state it was created in — so a bucket
    /// that carried none is already there and nothing changes.
    pub fn clear_lifecycle(&mut self) -> bool {
        self.lifecycle.take().is_some()
    }

    /// When the bucket's lifecycle configuration expires the object stored at `key`, and
    /// which rule does it — or `None` when no rule in effect expires that object.
    ///
    /// "If the object expiration is configured … the response includes this header. It
    /// includes the expiry-date and rule-id key-value pairs providing object expiration
    /// information" (`GetObjectOutput$Expiration`), so every reader and writer of an object
    /// reports this one answer.
    ///
    /// The EARLIEST expiry wins when several rules apply, because that is when the object
    /// actually goes; rules that expire it at the same instant are settled by the order the
    /// configuration lists them. Only a rule whose `Status` is `Enabled` counts, since a
    /// disabled rule "is not currently being applied", and only an `Expiration` that names
    /// a day count or a date dates anything — `ExpiredObjectDeleteMarker` is about a delete
    /// marker rather than about this object.
    ///
    /// Derived, never stored: a later tagging call moves an object into or out of a
    /// tag-filtered rule, so the answer belongs to the object's CURRENT state.
    pub fn object_expiration(&self, key: &str, object: &Object) -> Option<ObjectExpiration> {
        let size = i64::try_from(object.body.len()).ok()?;
        self.lifecycle()?
            .rules
            .iter()
            .filter(|rule| rule.status == LIFECYCLE_RULE_ENABLED)
            .filter(|rule| lifecycle_rule_applies(rule, key, size, &object.tags))
            .filter_map(|rule| {
                let expiration = rule.expiration.as_ref()?;
                let expires_at = match (expiration.days, &expiration.date) {
                    (Some(days), _) => midnight_after_days(object.last_modified, days)?,
                    (None, Some(date)) => instant_of(date)?,
                    (None, None) => return None,
                };
                Some(ObjectExpiration {
                    expires_at,
                    rule_id: rule.id.clone().unwrap_or_default(),
                })
            })
            .min_by_key(|expiration| expiration.expires_at)
    }

    /// [`Self::object_expiration`] as the `Expiration` response member carries it, or `None`
    /// when no rule in effect expires the object.
    ///
    /// The one place the lookup and the rendering are put together, so every operation whose
    /// output documents that member reports the same string for the same object.
    pub fn object_expiration_header(&self, key: &str, object: &Object) -> Option<String> {
        self.object_expiration(key, object)
            .and_then(|expiration| expiration.header_value())
    }

    /// When the bucket's lifecycle configuration aborts the in-progress multipart upload of
    /// `key` initiated at `initiated_at`, and which rule does it — or `None` when no rule
    /// in effect aborts it.
    ///
    /// "If the bucket has a lifecycle rule configured with an action to abort incomplete
    /// multipart uploads and the prefix in the lifecycle rule matches the object name in
    /// the request, the response includes this header"
    /// (`CreateMultipartUploadOutput$AbortDate`): the PREFIX is what selects the upload,
    /// because an upload in progress is not yet an object and so has no tag set or size for
    /// a rule's other predicates to read.
    ///
    /// The earliest abort wins when several rules apply, for the reason the earliest
    /// expiry does in [`Self::object_expiration`].
    pub fn multipart_upload_abort(
        &self,
        key: &str,
        initiated_at: SystemTime,
    ) -> Option<MultipartUploadAbort> {
        self.lifecycle()?
            .rules
            .iter()
            .filter(|rule| rule.status == LIFECYCLE_RULE_ENABLED)
            .filter(|rule| key.starts_with(lifecycle_rule_prefix(rule)))
            .filter_map(|rule| {
                let days = rule
                    .abort_incomplete_multipart_upload
                    .as_ref()?
                    .days_after_initiation?;
                Some(MultipartUploadAbort {
                    aborts_at: midnight_after_days(initiated_at, days)?,
                    rule_id: rule.id.clone().unwrap_or_default(),
                })
            })
            .min_by_key(|abort| abort.aborts_at)
    }

    /// The metrics configuration the bucket carries under `id`, or `None` when it carries
    /// none under that id — the state every bucket starts in, and what makes a read answer
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    pub fn metrics_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::MetricsConfiguration> {
        self.metrics_configurations.get(id)
    }

    /// Drop the metrics configuration the bucket carries under `id`, reporting whether one
    /// was there to drop.
    ///
    /// Per-ID, like the put: a delete "specified by the metrics configuration ID" removes
    /// THAT configuration and leaves every other id the bucket carries in place. `false`
    /// means the bucket carried none under that id, which is what makes a delete answer
    /// [`NO_SUCH_CONFIGURATION_CODE`] rather than report a removal it did not make.
    pub fn delete_metrics_configuration(&mut self, id: &str) -> bool {
        self.metrics_configurations.remove(id).is_some()
    }

    /// Every metrics configuration the bucket carries, in ascending id order — empty for a
    /// bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), so two listings of an unchanged
    /// bucket report the same sequence and a page can resume at the next id; the stored
    /// containers themselves, so a listing reports what the puts recorded.
    pub fn metrics_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::MetricsConfiguration)> + '_ {
        self.metrics_configurations
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, and report what that did.
    ///
    /// A full replacement of the configuration of that id, and of that one only: "If you're
    /// updating an existing metrics configuration, note that this is a full replacement of
    /// the existing metrics configuration. If you don't include the elements you want to
    /// keep, they are erased" (`PutBucketMetricsConfiguration`'s own documentation). Every
    /// other id the bucket carries is left exactly as it was.
    ///
    /// The ceiling is checked HERE, where the bucket's configuration count is, so a put
    /// cannot reach past [`MAX_METRICS_CONFIGURATIONS_PER_BUCKET`]; replacing an id the
    /// bucket already carries adds no configuration and so is never refused for it.
    pub fn put_metrics_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::MetricsConfiguration,
    ) -> BucketConfigurationWrite {
        put_keyed_configuration(
            &mut self.metrics_configurations,
            id,
            configuration,
            MAX_METRICS_CONFIGURATIONS_PER_BUCKET,
        )
    }

    /// The analytics configuration the bucket carries under `id`, or `None` when it carries
    /// none under that id — the state every bucket starts in, and what makes a read answer
    /// [`NO_SUCH_CONFIGURATION_CODE`].
    ///
    /// The stored container itself, so what a reader reports is what the put recorded rather
    /// than a re-derived copy — the reading [`Self::metrics_configuration`] gives.
    pub fn analytics_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::AnalyticsConfiguration> {
        self.analytics_configurations.get(id)
    }

    /// Drop the analytics configuration the bucket carries under `id`, reporting whether one
    /// was there to drop.
    ///
    /// Per-ID, like the put: a delete "specified by the analytics configuration ID"
    /// (`DeleteBucketAnalyticsConfiguration`'s own documentation) removes THAT configuration
    /// and leaves every other id the bucket carries in place. `false` means the bucket
    /// carried none under that id, which is what makes a delete answer
    /// [`NO_SUCH_CONFIGURATION_CODE`] rather than report a removal it did not make.
    pub fn delete_analytics_configuration(&mut self, id: &str) -> bool {
        self.analytics_configurations.remove(id).is_some()
    }

    /// Every analytics configuration the bucket carries, in ascending id order — empty for a
    /// bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), the reading
    /// [`Self::metrics_configurations`] gives: two listings of an unchanged bucket report the
    /// same sequence and a page can resume at the next id. The stored containers themselves,
    /// so a listing reports what the puts recorded.
    pub fn analytics_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::AnalyticsConfiguration)> + '_ {
        self.analytics_configurations
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, and report what that did.
    ///
    /// Per-ID and a full replacement of that id alone: a put "specified by the analytics
    /// configuration ID" (`PutBucketAnalyticsConfiguration`'s own documentation) carries the
    /// whole container, so the configuration stored under `id` becomes exactly the one this
    /// request named and every other id the bucket carries is left as it was.
    ///
    /// The ceiling is checked HERE, where the bucket's configuration count is, so a put
    /// cannot reach past [`MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET`]; replacing an id the
    /// bucket already carries adds no configuration and so is never refused for it.
    pub fn put_analytics_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::AnalyticsConfiguration,
    ) -> BucketConfigurationWrite {
        put_keyed_configuration(
            &mut self.analytics_configurations,
            id,
            configuration,
            MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET,
        )
    }

    /// The S3 Inventory configuration the bucket carries under `id`, or `None` when it
    /// carries none under that id — the state every bucket starts in, and what makes a read
    /// answer [`NO_SUCH_CONFIGURATION_CODE`].
    ///
    /// The stored container itself, so what a reader reports is what the put recorded rather
    /// than a re-derived copy — the reading [`Self::analytics_configuration`] gives.
    pub fn inventory_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::InventoryConfiguration> {
        self.inventory_configurations.get(id)
    }

    /// Every S3 Inventory configuration the bucket carries, in ascending id order — empty
    /// for a bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), the reading
    /// [`Self::analytics_configurations`] gives: two listings of an unchanged bucket report
    /// the same sequence and a page can resume at the next id. The stored containers
    /// themselves, so a listing reports what the puts recorded.
    pub fn inventory_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::InventoryConfiguration)> + '_ {
        self.inventory_configurations
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, and report what that did.
    ///
    /// Per-ID and a full replacement of that id alone: a put "adds an S3 Inventory
    /// configuration (identified by the inventory ID) to the bucket"
    /// (`PutBucketInventoryConfiguration`'s own documentation) and carries the whole
    /// container, so the configuration stored under `id` becomes exactly the one this request
    /// named and every other id the bucket carries is left as it was.
    ///
    /// The ceiling is checked HERE, where the bucket's configuration count is, so a put
    /// cannot reach past [`MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET`]; replacing an id the
    /// bucket already carries adds no configuration and so is never refused for it.
    pub fn put_inventory_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::InventoryConfiguration,
    ) -> BucketConfigurationWrite {
        put_keyed_configuration(
            &mut self.inventory_configurations,
            id,
            configuration,
            MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET,
        )
    }

    /// Drop the S3 Inventory configuration the bucket carries under `id`, reporting whether
    /// there was one to drop.
    ///
    /// Per-ID, like the put: a delete "deletes an S3 Inventory configuration (identified by
    /// the inventory ID) from the bucket" (`DeleteBucketInventoryConfiguration`'s own
    /// documentation) removes THAT configuration and leaves every other id the bucket carries
    /// in place. `false` means the bucket carried none under that id, which is what makes a
    /// delete answer [`NO_SUCH_CONFIGURATION_CODE`] rather than report a removal it did not
    /// make.
    pub fn delete_inventory_configuration(&mut self, id: &str) -> bool {
        self.inventory_configurations.remove(id).is_some()
    }

    /// Every S3 Intelligent-Tiering configuration the bucket carries, in ascending id
    /// order — empty for a bucket that carries none.
    ///
    /// The order is the store's own (a `BTreeMap` of ids), so two listings of an unchanged
    /// bucket report the same sequence and a page can resume at the next id.
    pub fn intelligent_tiering_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &crate::ops::types::IntelligentTieringConfiguration)> + '_
    {
        self.intelligent_tiering
            .iter()
            .map(|(id, configuration)| (id.as_str(), configuration))
    }

    /// Associate `configuration` with the bucket under `id`, reporting what that did.
    ///
    /// Per-ID, not whole-bucket: `PutBucketIntelligentTieringConfiguration` "puts a S3
    /// Intelligent-Tiering configuration to the specified bucket" under the `Id` the
    /// request names, so a put under an id the bucket already carries replaces THAT
    /// configuration and leaves every other one in place.
    ///
    /// A put that would add a NEW id past
    /// [`MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET`] writes nothing and reports
    /// [`IntelligentTieringWrite::ConfigurationCountReached`], which is the published
    /// ceiling rather than this emulator's own; replacing an id already present is never
    /// refused by it, since it adds no configuration.
    pub fn put_intelligent_tiering_configuration(
        &mut self,
        id: String,
        configuration: crate::ops::types::IntelligentTieringConfiguration,
    ) -> IntelligentTieringWrite {
        if !self.intelligent_tiering.contains_key(&id)
            && self.intelligent_tiering.len() >= MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET
        {
            return IntelligentTieringWrite::ConfigurationCountReached;
        }
        self.intelligent_tiering.insert(id, configuration);
        IntelligentTieringWrite::Stored
    }

    /// The one S3 Intelligent-Tiering configuration the bucket carries under `id`, or
    /// `None` when it carries none under that id.
    ///
    /// The stored container itself, so what a reader reports is what the put recorded
    /// rather than a re-derived copy — the reading [`Self::replication`] gives.
    pub fn intelligent_tiering_configuration(
        &self,
        id: &str,
    ) -> Option<&crate::ops::types::IntelligentTieringConfiguration> {
        self.intelligent_tiering.get(id)
    }

    /// Drop the S3 Intelligent-Tiering configuration the bucket carries under `id`,
    /// reporting whether there was one to drop.
    ///
    /// Per-ID, the way [`Self::put_intelligent_tiering_configuration`] records one: every
    /// other configuration the bucket carries stays exactly as it was, so a bucket whose
    /// last configuration is removed carries none again.
    pub fn delete_intelligent_tiering_configuration(&mut self, id: &str) -> bool {
        self.intelligent_tiering.remove(id).is_some()
    }

    /// The default encryption rules IN FORCE on the bucket: the ones a
    /// `PutBucketEncryption` associated with it, or the single SSE-S3 rule every bucket
    /// carries until one does ([`default_encryption_rule`]).
    ///
    /// A configured rule that names NO `BlockedEncryptionTypes` leaves the bucket's blocked
    /// set at [`DEFAULT_BLOCKED_ENCRYPTION_TYPES`], which is what this materialises:
    /// blocking an encryption type is "a bucket-level setting … used to prevent the upload
    /// of new objects encrypted with the specified server-side encryption type", one whose
    /// own enum publishes the value that UNBLOCKS it (`NONE`, beside `SSE-C` —
    /// `smithy_query(query="shape",shape="EncryptionType")`), so a configuration that says
    /// nothing about it neither turns the blocking off nor hides it from a reader.
    ///
    /// COMPUTED, never stored: a bucket that was never configured keeps [`Self::encryption`]
    /// at `None`, so reading this changes nothing a later caller sees. This is what
    /// `GetBucketEncryption` reports and what [`Self::default_object_encryption`] resolves a
    /// write's inherited encryption from, so the two can never disagree.
    pub fn effective_encryption_rules(&self) -> Vec<BucketEncryptionRule> {
        match self.encryption() {
            Some(configured) => configured
                .iter()
                .map(|rule| BucketEncryptionRule {
                    blocked_encryption_types: rule
                        .blocked_encryption_types
                        .clone()
                        .or_else(|| Some(default_blocked_encryption_types())),
                    ..rule.clone()
                })
                .collect(),
            None => vec![default_encryption_rule()],
        }
    }

    /// The encryption an object written to this bucket WITHOUT naming any encryption of its
    /// own is stored under: the bucket's default encryption configuration.
    ///
    /// "Amazon S3 encrypts data with server-side encryption by using Amazon S3 managed keys
    /// (SSE-S3) by default. You can optionally tell Amazon S3 to encrypt data at rest by
    /// using server-side encryption with other key options"
    /// (`PutObjectRequest$ServerSideEncryption`) — the other key options being the ones
    /// `PutBucketEncryption` configures, which is why the rule applied here is the bucket's
    /// effective one rather than SSE-S3 always.
    ///
    /// The rule that decides it is the first of [`Self::effective_encryption_rules`] that
    /// names an `ApplyServerSideEncryptionByDefault` algorithm; a configuration whose rules
    /// name none (`ServerSideEncryptionRule` makes that container optional) leaves the
    /// bucket on the SSE-S3 base default. The Bucket Key setting travels with the algorithm
    /// only when that algorithm is SSE-KMS, because an S3 Bucket Key is "for object
    /// encryption with server-side encryption using Key Management Service (KMS) keys
    /// (SSE-KMS)" (`PutObjectRequest$BucketKeyEnabled`) and nothing else; an encryption
    /// CONTEXT is never inherited, because it is a per-request value no bucket
    /// configuration carries.
    pub fn default_object_encryption(&self) -> ObjectEncryption {
        let rules = self.effective_encryption_rules();
        let rule = rules
            .iter()
            .find(|rule| rule.sse_algorithm.is_some())
            .cloned()
            .unwrap_or_else(default_encryption_rule);
        let inherited = ObjectEncryption {
            sse_algorithm: rule.sse_algorithm,
            kms_key_id: rule.kms_master_key_id,
            kms_encryption_context: None,
            bucket_key_enabled: rule.bucket_key_enabled,
        };
        if inherited.uses_kms() {
            inherited
        } else {
            ObjectEncryption {
                sse_algorithm: inherited.sse_algorithm,
                ..ObjectEncryption::default()
            }
        }
    }

    /// The bucket's website configuration, or `None` when no website configuration is
    /// associated with the bucket.
    pub fn website(&self) -> Option<&BucketWebsite> {
        self.website.as_ref()
    }

    /// Associate `configuration` with the bucket as its whole website configuration.
    ///
    /// A replacement, not a merge: `PutBucketWebsite` "sets the configuration of the
    /// website that is specified in the website subresource" from the complete
    /// `WebsiteConfiguration` it carries as its payload (that operation's own
    /// documentation), so what the request names becomes the bucket's configuration in
    /// full — the way [`Bucket::set_ownership_controls`] replaces the ownership controls.
    pub fn set_website(&mut self, configuration: BucketWebsite) {
        self.website = Some(configuration);
    }

    /// Remove the bucket's website configuration, leaving none associated with it.
    ///
    /// What `DeleteBucketWebsite` does — "This action removes the website configuration for
    /// a bucket" (that operation's own documentation) — which puts the bucket back in the
    /// state it was created in, so a later `GetBucketWebsite` answers
    /// [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`] again. Scoped to this one bucket's website
    /// configuration: nothing else on the record is touched, the way [`Bucket::clear_cors`]
    /// is scoped to the cors configuration.
    pub fn clear_website(&mut self) {
        self.website = None;
    }

    /// The bucket's cors configuration, or `None` when no cors configuration is associated
    /// with the bucket.
    pub fn cors(&self) -> Option<&[BucketCorsRule]> {
        self.cors.as_deref()
    }

    /// Associate `rules` with the bucket as its whole cors configuration.
    ///
    /// A replacement, not a merge: `PutBucketCors` "sets the cors configuration for your
    /// bucket. If the configuration exists, Amazon S3 replaces it" (that operation's own
    /// documentation), so what the request names becomes the bucket's configuration in full
    /// — the way [`Bucket::set_website`] replaces the website configuration.
    pub fn set_cors(&mut self, rules: Vec<BucketCorsRule>) {
        self.cors = Some(rules);
    }

    /// Remove the bucket's cors configuration, leaving none associated with it.
    ///
    /// What `DeleteBucketCors` does — "Deletes the cors configuration information set for
    /// the bucket" (that operation's own documentation) — which puts the bucket back in the
    /// state it was created in, so a later `GetBucketCors` answers
    /// [`NO_SUCH_CORS_CONFIGURATION_CODE`] again. Scoped to this one bucket's cors
    /// configuration: nothing else on the record is touched, the way
    /// [`Bucket::clear_ownership_controls`] is scoped to the ownership controls.
    pub fn clear_cors(&mut self) {
        self.cors = None;
    }

    /// Where the bucket's server access logs are delivered, or `None` while logging is not
    /// enabled on it. See [`Self::logging`].
    pub fn logging(&self) -> Option<&crate::ops::types::LoggingEnabled> {
        self.logging.as_ref()
    }

    /// Set the bucket's logging status to `logging`: `Some` to enable server access
    /// logging with that configuration, `None` to disable it.
    ///
    /// A replacement of the whole subresource, the way [`Bucket::set_cors`] replaces the
    /// cors configuration: `BucketLoggingStatus` is the complete "container for logging
    /// status information" a `PutBucketLogging` carries as its payload, so what the bucket
    /// carries afterwards is exactly what that container named — and a container naming no
    /// `LoggingEnabled` is how logging is turned off again ("To disable logging, you use an
    /// empty `BucketLoggingStatus` request element", that operation's own documentation).
    pub fn set_logging(&mut self, logging: Option<crate::ops::types::LoggingEnabled>) {
        self.logging = logging;
    }

    /// The object stored at `key`, or `None` when the bucket holds no current object
    /// there.
    ///
    /// The CURRENT version: what a read that names no `versionId` answers with, and the
    /// only version a bucket that was never versioning-enabled has. `None` when the key's
    /// current version is a DELETE MARKER — "if the current version of the object is a
    /// delete marker, Amazon S3 behaves as if the object was deleted"
    /// (`GetObject`/`DeleteObject`) — and also when the bucket holds no such key at all:
    /// the two are the same thing to every reader of a current object.
    pub fn object(&self, key: &str) -> Option<&Object> {
        self.current_version(key)?.as_object()
    }

    /// The key's CURRENT version, object or delete marker, or `None` when the bucket holds
    /// no version of it.
    ///
    /// Entry 0 of the chain: "the delete marker … becomes the current version of the
    /// object", and a write makes what it stores current.
    pub fn current_version(&self, key: &str) -> Option<&StoredVersion> {
        self.versions.get(key)?.first()
    }

    /// The version of `key` that `version_id` names — current or noncurrent, object or
    /// delete marker — or `None` when the bucket holds no such version.
    ///
    /// [`NULL_VERSION_ID`] names the version a write made while the bucket was not
    /// versioning-enabled, which is stored with no minted id of its own.
    pub fn stored_version(&self, key: &str, version_id: &str) -> Option<&StoredVersion> {
        self.versions
            .get(key)?
            .iter()
            .find(|held| held.named_by(version_id))
    }

    /// The OBJECT version of `key` that `version_id` names, or `None` when the bucket
    /// holds no such version or that version is a delete marker (which holds no bytes to
    /// answer a read with — see [`Self::stored_version`] for the version itself).
    pub fn object_version(&self, key: &str, version_id: &str) -> Option<&Object> {
        self.stored_version(key, version_id)?.as_object()
    }

    /// The object version a request ADDRESSES: the one `version_id` names, or the key's
    /// current object when the request names no version.
    ///
    /// One place, so every operation that acts on "this object, optionally this version of
    /// it" resolves the address the same way — "the version ID for the object that you want
    /// to apply this Object Retention configuration to"
    /// (`PutObjectRetentionRequest$VersionId`), and with none the current version, which is
    /// what a reader naming no `versionId` answers with.
    pub fn addressed_object(&self, key: &str, version_id: Option<&str>) -> Option<&Object> {
        match version_id {
            None => self.object(key),
            Some(version_id) => self.object_version(key, version_id),
        }
    }

    /// Every version of `key` the bucket holds, NEWEST FIRST: the current one followed by
    /// the noncurrent ones, delete markers among them.
    ///
    /// The read seam for anything that reports a key's versions rather than its current
    /// object, so no caller reaches into the chain itself.
    pub fn object_versions(&self, key: &str) -> &[StoredVersion] {
        self.versions.get(key).map_or(&[], Vec::as_slice)
    }

    /// Every CURRENT OBJECT the bucket holds, as `(key, object)` in ascending key order.
    ///
    /// The read seam for a listing over current objects (`ListObjectsV2`): ascending UTF-8
    /// byte order, because that is the order S3 lists keys in ("returns some or all of the
    /// objects in a bucket … in lexicographical order"), and a key whose current version
    /// is a delete marker is NOT here — a delete marker is what makes a key stop appearing
    /// in such a listing while its versions are still stored.
    pub fn objects(&self) -> impl Iterator<Item = (&str, &Object)> + '_ {
        self.versions
            .iter()
            .filter_map(|(key, chain)| Some((key.as_str(), chain.first()?.as_object()?)))
    }

    /// The in-progress multipart upload `upload_id` names, or `None` when the bucket has
    /// no such upload in progress — which is what makes a read or a part upload naming an
    /// id this store never minted (or one a completion or an abort has since taken away)
    /// answer `NoSuchUpload`.
    pub fn multipart_upload(&self, upload_id: &str) -> Option<&MultipartUpload> {
        self.multipart_uploads.get(upload_id)
    }

    /// Every multipart upload the bucket has in progress, keyed by upload id.
    ///
    /// The read seam for anything that reports the bucket's uploads rather than one of
    /// them, so no caller reaches into the map. Read-only: an upload is recorded only
    /// through [`ComAmazonawsS3Amazons3State::create_multipart_upload`], the only place
    /// that sees every ceiling.
    pub fn multipart_uploads(&self) -> &BTreeMap<String, MultipartUpload> {
        &self.multipart_uploads
    }

    /// Delete `key` the way a `DeleteObject` that named NO `versionId` does, reporting
    /// what that did ([`ObjectDeletion`]).
    ///
    /// Which of the three things it does turns on the bucket's versioning state, and
    /// `PutBucketVersioning` / `DeleteObject` document each:
    ///
    /// * versioning status `Enabled` — "the operation inserts a delete marker, which
    ///   becomes the current version of the object. To permanently delete an object in a
    ///   versioned bucket, you must include the object's versionId in the request": the
    ///   marker is PREPENDED to the key's chain under the id `minted`, nothing stored is
    ///   taken away, and every version behind it stays addressable by its own id.
    /// * versioning status `Suspended` — "All objects added to the bucket receive the
    ///   version ID null", and a delete "removes the null version (if there is one) and
    ///   inserts a delete marker with version ID null": the `null` version goes, whether it
    ///   is an object or an older null marker, and a null marker becomes current. Every
    ///   version minted while versioning was enabled stays.
    /// * no versioning state at all — such a bucket has no version history to keep, so the
    ///   `null` version is removed in place and everything it retained is released.
    ///
    /// `minted` is the version id the delete marker of an `Enabled` bucket is stored under,
    /// which only [`ComAmazonawsS3Amazons3State::remove_object`] can mint (it holds the
    /// store's version sequence); it is ignored on any other bucket, whose marker is the
    /// `null` one.
    ///
    /// PRIVATE, so a delete goes through
    /// [`ComAmazonawsS3Amazons3State::remove_object`] and cannot name a bucket that does
    /// not exist. NOTHING is written when the answer is a ceiling: a delete marker is a
    /// stored version like any other, so it is refused rather than recorded once the bucket
    /// is at [`MAX_OBJECTS_PER_BUCKET`] stored versions.
    fn remove_object(&mut self, key: &str, minted: Option<String>) -> ObjectDeletion {
        let held_object = self.object(key).is_some();

        // A bucket with no versioning state keeps no history: the delete takes the `null`
        // version away, and with it the last entry of the key's chain.
        if self.versioning.is_none() {
            return if self.remove_null_version(key) {
                ObjectDeletion::Removed
            } else {
                ObjectDeletion::NothingThere
            };
        }

        // An `Enabled` bucket's marker is a version of its own ("All objects added to the
        // bucket receive a unique version ID"); a suspended bucket's is the `null` one,
        // which is why it displaces whatever null version the key held.
        let version_id = if self.versioning_enabled() { minted } else { None };
        let displaces_null = version_id.is_none() && self.null_version_position(key).is_some();
        // The marker is a stored version like any other, so it is refused rather than
        // recorded once the bucket is full — unless it takes the place of the null version
        // it displaces, which leaves the count where it was.
        if !displaces_null && self.stored_versions() >= MAX_OBJECTS_PER_BUCKET {
            return ObjectDeletion::VersionCountReached;
        }
        if displaces_null {
            self.remove_null_version(key);
        }
        self.versions
            .entry(key.to_string())
            .or_default()
            .insert(
                0,
                StoredVersion::DeleteMarker(DeleteMarker {
                    version_id: version_id.clone(),
                    last_modified: SystemTime::now(),
                }),
            );
        ObjectDeletion::MarkerLaid {
            version_id,
            displaced_object: held_object,
        }
    }

    /// Where in `key`'s chain the `null` version is — the one entry that carries no minted
    /// id, object or delete marker — or `None` when the bucket holds none.
    ///
    /// At most one such entry can exist: only a write or a delete made while the bucket was
    /// not versioning-enabled stores one, and each of those displaces the one before it
    /// ("All objects added to the bucket receive the version ID null").
    fn null_version_position(&self, key: &str) -> Option<usize> {
        self.versions
            .get(key)?
            .iter()
            .position(|held| held.version_id().is_none())
    }

    /// Remove the `null` version of `key`, reporting whether the bucket held one. See
    /// [`Self::null_version_position`].
    fn remove_null_version(&mut self, key: &str) -> bool {
        let Some(position) = self.null_version_position(key) else {
            return false;
        };
        let Some(chain) = self.versions.get_mut(key) else {
            return false;
        };
        chain.remove(position);
        if chain.is_empty() {
            self.versions.remove(key);
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
        self.versions.get_mut(key)?.first_mut()?.as_object_mut()
    }

    /// The object version a request addresses, for writing one of its subresources: the
    /// one `version_id` names, or the key's current object when it names none — the
    /// mutable twin of [`Self::addressed_object`].
    ///
    /// PRIVATE, and reached only through
    /// [`ComAmazonawsS3Amazons3State::replace_object_retention`] and
    /// [`ComAmazonawsS3Amazons3State::restore_object_version`], for the reason
    /// [`Self::object_mut`] is private: those are the places that see the byte budgets a
    /// subresource write also has to stay inside.
    fn addressed_object_mut(&mut self, key: &str, version_id: Option<&str>) -> Option<&mut Object> {
        match version_id {
            None => self.object_mut(key),
            Some(version_id) => self
                .versions
                .get_mut(key)?
                .iter_mut()
                .find(|held| held.named_by(version_id))?
                .as_object_mut(),
        }
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
        // "The rule specified in the Object Lock configuration will be applied by default to
        // every new object placed in the specified bucket"
        // (`PutObjectLockConfiguration`'s own documentation), so a bucket carrying a default
        // retention locks the version this write stores — counted from that version's own
        // creation time, and only where the write placed no retention itself, because a
        // retention the request names is the one the object is stored with.
        //
        // Here, because this is the only place that sees both the bucket's configuration and
        // the object being written: every writer gets the default rather than each one
        // re-deriving it. Before the budgets below, so the bytes it retains are inside them.
        if object.retention.is_none() {
            object.retention = self
                .object_lock_rule
                .as_ref()
                .and_then(|rule| rule.retention_at(object.last_modified));
        }
        let retaining = self.keeps_superseded(key);
        let held = self.object(key).is_some();
        // The write adds an entry unless it replaces the key's current version in place,
        // which it does only when that version is not retained and there is one.
        let adds_version = retaining || self.current_version(key).is_none();
        if adds_version && self.stored_versions() >= MAX_OBJECTS_PER_BUCKET {
            return ObjectWrite::ObjectCountReached;
        }
        let would_hold = self.retained_bytes_after_write(key, retaining) + object.retained_bytes();
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return ObjectWrite::ByteBudgetReached { would_hold };
        }
        let chain = self.versions.entry(key.to_string()).or_default();
        if adds_version {
            chain.insert(0, StoredVersion::Object(Box::new(object)));
        } else {
            chain[0] = StoredVersion::Object(Box::new(object));
        }
        if held {
            ObjectWrite::Replaced
        } else {
            ObjectWrite::Created
        }
    }

    /// Every key the bucket holds at least one version of, in ascending UTF-8 byte order
    /// — the keys with a current object, plus the keys whose only versions left are
    /// noncurrent ones or delete markers.
    ///
    /// The read seam for a listing over VERSIONS (`ListObjectVersions`), the way
    /// [`Self::objects`] is the seam for a listing over current objects. Ascending order,
    /// because that is the order S3 lists keys in.
    pub fn version_keys(&self) -> Vec<&str> {
        self.versions.keys().map(String::as_str).collect()
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
        let Some(chain) = self.versions.get_mut(key) else {
            return false;
        };
        let Some(position) = chain.iter().position(|held| held.named_by(version_id)) else {
            return false;
        };
        // Removing entry 0 promotes the version behind it to current, because the chain is
        // newest first: that is what makes a versioned bucket's delete reversible — the
        // version before it becomes what a read without a `versionId` answers with.
        chain.remove(position);
        if chain.is_empty() {
            self.versions.remove(key);
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
                .current_version(key)
                .is_some_and(|held| held.version_id().is_some())
    }

    /// Whether the bucket holds NOTHING at all: no version of any key — no current object,
    /// no noncurrent version retained behind one, and no delete marker.
    ///
    /// This is the emptiness `DeleteBucket` is over — "All objects (including all object
    /// versions and delete markers) in the bucket must be deleted before the bucket itself
    /// can be deleted" (that operation's own documentation) — so it spans the bucket's
    /// whole version history rather than its current view. [`Self::objects`] alone would
    /// report a versioning-enabled bucket whose every key has been deleted as empty, while
    /// each of those deletes laid a delete marker over a version that is still stored and
    /// still addressable by its id: both of them have to be deleted first.
    pub fn holds_no_versions(&self) -> bool {
        self.stored_versions() == 0
    }

    /// How many versions this bucket stores, over every key: each key's current version
    /// plus every noncurrent one it keeps, delete markers included.
    ///
    /// What [`MAX_OBJECTS_PER_BUCKET`] counts, because a retained noncurrent version holds
    /// this process's memory exactly as a current one does.
    pub fn stored_versions(&self) -> usize {
        self.versions.values().map(Vec::len).sum()
    }

    /// How many bytes this bucket's objects retain — payloads AND every string kept
    /// beside them, over every version it stores, so no part of a stored object escapes
    /// the budgets.
    ///
    /// The parts of the multipart uploads it has in progress are counted too: a part held
    /// against an upload occupies this process's memory exactly as a stored object's
    /// payload does, and is released only when a completion or an abort takes the upload
    /// away — so the two share one budget rather than each having its own.
    fn retained_bytes(&self) -> usize {
        self.versions
            .values()
            .flatten()
            .map(StoredVersion::retained_bytes)
            .sum::<usize>()
            + self
                .multipart_uploads
                .values()
                .map(MultipartUpload::retained_bytes)
                .sum::<usize>()
    }

    /// How many bytes this bucket would still retain with a write at `key` applied, not
    /// counting the incoming object itself.
    ///
    /// Everything it holds now, less the key's current version when that version is
    /// replaced in place — which is every write except one that keeps what it supersedes.
    fn retained_bytes_after_write(&self, key: &str, retaining_current: bool) -> usize {
        let released = if retaining_current {
            0
        } else {
            self.current_version(key)
                .map_or(0, StoredVersion::retained_bytes)
        };
        self.retained_bytes().saturating_sub(released)
    }
}

/// One stored VERSION of a key: the object a write recorded, or the delete marker a
/// delete that named no version laid over it.
///
/// A separate type rather than a flag on [`Object`], so a reader that wants an object's
/// bytes cannot be handed a marker by mistake: a marker holds no bytes, no metadata and no
/// entity tag, and `ListObjectVersions` reports it in its own `DeleteMarker` container.
#[derive(Debug, Clone)]
pub enum StoredVersion {
    /// A version that holds bytes — what a read answers with. BOXED: an [`Object`] is
    /// two orders of magnitude larger than a marker, and a chain of them would otherwise
    /// carry that size for every delete marker too.
    Object(Box<Object>),
    /// A delete marker: "If bucket versioning is enabled, the operation inserts a delete
    /// marker, which becomes the current version of the object" (`DeleteObject`'s
    /// documentation). While it is a key's current version every read that names no
    /// `versionId` behaves as if the object were deleted, and the versions behind it stay
    /// addressable by their own ids.
    DeleteMarker(DeleteMarker),
}

impl StoredVersion {
    /// The id this version is addressed by, or `None` for the `null` version — one stored
    /// while the bucket's versioning status was not `Enabled`, which a request names by
    /// [`NULL_VERSION_ID`].
    pub fn version_id(&self) -> Option<&str> {
        match self {
            Self::Object(object) => object.version_id.as_deref(),
            Self::DeleteMarker(marker) => marker.version_id.as_deref(),
        }
    }

    /// Whether `version_id` — as a request writes it — names this version.
    pub fn named_by(&self, version_id: &str) -> bool {
        match self.version_id() {
            Some(id) => id == version_id,
            None => version_id == NULL_VERSION_ID,
        }
    }

    /// The object this version holds, or `None` when it is a delete marker.
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(object) => Some(object.as_ref()),
            Self::DeleteMarker(_) => None,
        }
    }

    /// The object this version holds, for writing one of its subresources, or `None` when
    /// it is a delete marker.
    fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Self::Object(object) => Some(object.as_mut()),
            Self::DeleteMarker(_) => None,
        }
    }

    /// The delete marker this version is, or `None` when it holds an object.
    pub fn as_delete_marker(&self) -> Option<&DeleteMarker> {
        match self {
            Self::Object(_) => None,
            Self::DeleteMarker(marker) => Some(marker),
        }
    }

    /// When this version came into being: the write that stored the object, or the delete
    /// that laid the marker.
    pub fn last_modified(&self) -> SystemTime {
        match self {
            Self::Object(object) => object.last_modified,
            Self::DeleteMarker(marker) => marker.last_modified,
        }
    }

    /// How many bytes this version retains, which the per-bucket and service-wide budgets
    /// count. A marker retains only the id it is addressed by.
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Object(object) => object.retained_bytes(),
            Self::DeleteMarker(marker) => marker.version_id.as_ref().map_or(0, String::len),
        }
    }
}

/// One delete marker: "a placeholder (marker) for a versioned object that was named in a
/// simple DELETE request… it has a key name and version ID like any other object"
/// (Amazon S3 User Guide, "Working with delete markers"), and nothing else — it holds no
/// bytes, so `ListObjectVersions`' `DeleteMarkerEntry` reports neither a size nor an
/// entity tag for it.
#[derive(Debug, Clone)]
pub struct DeleteMarker {
    /// The version id the marker is addressed by, or `None` for the `null` marker a
    /// versioning-SUSPENDED bucket's delete lays ("All objects added to the bucket receive
    /// the version ID null").
    ///
    /// Minted by [`ComAmazonawsS3Amazons3State::remove_object`], the one place that sees
    /// both the bucket's versioning state and the store's version sequence, so no handler
    /// constructs one.
    pub version_id: Option<String>,
    /// When the delete that laid the marker was made, which is what a listing reports as
    /// its `LastModified`.
    pub last_modified: SystemTime,
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
    /// The object's tag set, in the order the request that set it carried it — the
    /// `x-amz-tagging` header of the write, or the modelled `Tagging` container of a later
    /// `PutObjectTagging`.
    ///
    /// Empty when no tag set is associated with the object: "no tags" and "an empty tag
    /// set" are the same state, which is why `PutObjectTagging` with an empty `TagSet`
    /// leaves an object where `DeleteObjectTagging` would. A `PutObject` REPLACES the
    /// whole object ("You cannot use PutObject to only update a single piece of metadata
    /// for an existing object"), so the tag set of the object it replaces goes with it:
    /// the write's own header decides the new one, and
    /// [`ComAmazonawsS3Amazons3State::replace_object_tags`] is what sets one afterwards.
    pub tags: Vec<ResourceTag>,
    /// How this object is encrypted at rest — see [`ObjectEncryption`]. Decided by the
    /// write that stored it and never afterwards, like every other member here: "You
    /// cannot use PutObject to only update a single piece of metadata for an existing
    /// object."
    pub encryption: ObjectEncryption,
    /// The checksum the write that stored this object computed over the bytes it stored —
    /// see [`ObjectChecksum`] — or `None` when that write named no checksum algorithm,
    /// which is the state an object whose reader reports no checksum at all is in.
    ///
    /// Decided by that write, like [`Self::encryption`]: "this checksum is only present if
    /// the checksum was uploaded with the object" (`GetObjectOutput$ChecksumSHA256`), so
    /// nothing afterwards computes one for an object that was stored without it.
    pub checksum: Option<ObjectChecksum>,
    /// The parts a multipart upload assembled this object from — see [`ObjectPartRecord`] —
    /// or empty for an object one write stored whole.
    ///
    /// "A collection of parts associated with a multipart upload"
    /// (`GetObjectAttributesParts`) is what a read of the object's attributes reports from
    /// this: the parts are gone as parts once the upload is completed (no `ListParts` reaches
    /// them), but the object remembers how it was assembled, which is the only way that
    /// collection can be reported at all.
    pub parts: Vec<ObjectPartRecord>,
    /// The Object Lock retention in force on this stored version — see
    /// [`ObjectRetention`] — or `None` while no retention is associated with it, which is
    /// the state a write leaves a version in.
    ///
    /// Unlike every member above it, this one is set AFTER the write: a retention is
    /// "placed" on an object by `PutObjectRetention` ("Places an Object Retention
    /// configuration on an object"), through
    /// [`ComAmazonawsS3Amazons3State::replace_object_retention`], which is the one place
    /// that sees the byte budgets it has to stay inside.
    pub retention: Option<ObjectRetention>,
    /// The Object Lock LEGAL HOLD on this stored version: `Some(true)` while a hold is in
    /// place, `Some(false)` once one was taken off, and `None` while no legal hold has ever
    /// been applied to it — the state a write leaves a version in.
    ///
    /// Three states rather than two, because "never applied" is one S3 reports apart from
    /// `OFF`: `HeadObject`'s `ObjectLockLegalHoldStatus` "is not returned if the specified
    /// version of this object has never had a legal hold applied"
    /// (`smithy_query(query="doc",shape="HeadObjectOutput",
    /// member="ObjectLockLegalHoldStatus")`), so a reader of a version nobody has placed a
    /// hold on has no status to report.
    ///
    /// Set AFTER the write, like [`Self::retention`]: a legal hold is applied to an object
    /// by `PutObjectLegalHold` ("applies a legal hold configuration to the specified
    /// object"), through
    /// [`ComAmazonawsS3Amazons3State::replace_object_legal_hold`].
    pub legal_hold: Option<bool>,
    /// The restore `RestoreObject` last initiated on this stored version — see
    /// [`ObjectRestore`] — or `None` while no restore has been asked for, which is the
    /// state a write leaves a version in.
    ///
    /// Set AFTER the write, like [`Self::retention`]: a restore is initiated on an
    /// already-archived object ("you must first initiate a restore request, and then wait
    /// until a temporary copy of the object is available", `RestoreObject`'s own
    /// documentation), through
    /// [`ComAmazonawsS3Amazons3State::restore_object_version`], which is the one place that
    /// sees the byte budgets it has to stay inside.
    pub restore: Option<ObjectRestore>,
    /// The annotations attached to this stored object version, by name.
    ///
    /// "An annotation is a named payload of 1 byte to 1 MiB that you can associate with a
    /// specific object or object version" (`PutObjectAnnotation`'s own documentation), so
    /// the annotations of one version live WITH that version and a write of the object does
    /// not carry them forward — "You cannot use PutObject to only update a single piece of
    /// metadata for an existing object", so the object a write replaces takes its
    /// annotations with it, exactly as it takes its tag set.
    ///
    /// Keyed by name, which is how every annotation operation addresses one, and sorted,
    /// because that is the order a listing of them reports. Set AFTER the write, like
    /// [`Self::retention`] and [`Self::restore`], through
    /// [`ComAmazonawsS3Amazons3State::put_object_annotation`] — the one place that sees the
    /// per-object ceiling and the byte budgets an annotation has to stay inside.
    pub annotations: BTreeMap<String, Annotation>,
    /// The grants of the access control list last set on this stored version, or `None`
    /// while no ACL has ever been set on it — the state a write leaves a version in, and
    /// the state whose reader reports the owner's `FULL_CONTROL` alone
    /// ([`default_acl_grants`]), because an object is private until an ACL says otherwise.
    ///
    /// Set AFTER the write, like [`Self::retention`]: an ACL is set on an object by
    /// `PutObjectAcl` ("Uses the acl subresource to set the access control list (ACL)
    /// permissions for … an existing object in an S3 bucket", that operation's own
    /// documentation), through
    /// [`ComAmazonawsS3Amazons3State::replace_object_acl`] — the one place that sees the
    /// byte budgets an ACL has to stay inside.
    ///
    /// The RAW record, kept whatever the bucket's Object Ownership setting is:
    /// [`Bucket::object_acl_grants`] is what a reader reports, and it reports the default
    /// while the bucket's ACLs are disabled — disabling them takes nothing away, so these
    /// grants are reported again once ACLs affect permissions on the bucket.
    pub acl: Option<Vec<AclGrant>>,
    /// What replication did with this stored version — see [`ObjectReplication`] — or
    /// `None` for a version no replication rule covers, which is the state of every object
    /// in a bucket with no replication configuration.
    ///
    /// "If your request involves a bucket that is either a source or a destination in a
    /// replication rule" the reader reports a `ReplicationStatus`
    /// (`GetObjectOutput$ReplicationStatus`); for every other object there is nothing to
    /// report, which is why this is an `Option` and why a reader of an object outside every
    /// rule reports no status at all.
    ///
    /// Settled by the write that stored the version, through
    /// [`ComAmazonawsS3Amazons3State::replicate_write`] — the one place that performs the
    /// replication and records its outcome, so the status describes a copy that was really
    /// made.
    pub replication: Option<ObjectReplication>,
}

/// One annotation attached to one stored object version: the payload
/// `PutObjectAnnotation` recorded under a name, which `GetObjectAnnotation` answers with.
///
/// The name is not here — it is the key the object holds this record under
/// ([`Object::annotations`]) — and neither is a checksum: nothing in this emulator computes
/// one, so none is recorded and none is reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    /// The annotation's bytes, exactly as the request carried them.
    pub payload: Vec<u8>,
    /// The annotation's entity tag, including the quotes S3 reports it with — the digest of
    /// its payload, computed the one way every entity tag here is ([`object_etag`]).
    pub etag: String,
    /// When the annotation was last written, which is what a read of it reports as
    /// `Last-Modified` and a listing of them as `AnnotationEntry$LastModified`.
    pub last_modified: SystemTime,
}

impl Annotation {
    /// The annotation `payload` makes, written at `at`: the payload with the entity tag of
    /// those bytes beside it, so no caller derives a tag of its own.
    pub fn new(payload: Vec<u8>, at: SystemTime) -> Self {
        Self {
            etag: object_etag(&payload),
            last_modified: at,
            payload,
        }
    }

    /// The size of the annotation payload, in bytes — what `AnnotationEntry$Size` reports
    /// and what a read of it answers as `Content-Length`.
    pub fn size(&self) -> i64 {
        i64::try_from(self.payload.len()).unwrap_or(i64::MAX)
    }

    /// How many bytes this annotation retains: its payload AND the entity tag kept beside
    /// it, the way [`UploadedPart::retained_bytes`] counts a part's, so no annotation
    /// escapes the budgets a stored object's bytes are bounded by.
    pub fn retained_bytes(&self) -> usize {
        self.payload.len() + self.etag.len()
    }
}

/// The server-side encryption in force on one stored object: the configuration the write
/// that stored it settled on, which every reader of that object reports back.
///
/// The members are `PutObject`'s own encryption members
/// (`smithy_query(query="operation",operation="PutObject")`): `ServerSideEncryption`
/// (`x-amz-server-side-encryption`), `SSEKMSKeyId`, `SSEKMSEncryptionContext` and
/// `BucketKeyEnabled` — the four `PutObject`, `GetObject` and `HeadObject` all report
/// back as bound response headers.
///
/// A write that names one or more of them is stored under what it named; one that names
/// none inherits the bucket's default encryption configuration
/// ([`Bucket::default_object_encryption`]), which is what makes every object in a bucket
/// encrypted even when no client ever asked.
///
/// What this records is an object's encryption CONFIGURATION, not a key and not a
/// ciphertext: this emulator holds an object's bytes in this process's memory and
/// performs no cryptography, so the only thing it can answer faithfully is which
/// configuration the object was stored under — which is exactly what those response
/// members report. The SSE-C members (a customer-provided KEY, which S3 itself never
/// stores) are NOT here: `PutObject` refuses a write that carries them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectEncryption {
    /// A `ServerSideEncryption` value (`AES256`, `aws:kms`, …), or `None` for an object
    /// stored before any encryption state was recorded for it.
    pub sse_algorithm: Option<String>,
    /// `SSEKMSKeyId`, verbatim as the request carried it — "You can specify the key ID,
    /// key alias, or the Amazon Resource Name (ARN) of the KMS key" — or, for a write that
    /// named no encryption member at all, the `KMSMasterKeyID` of the bucket's default
    /// encryption configuration ([`Bucket::default_object_encryption`]), and `None` when
    /// neither names one.
    pub kms_key_id: Option<String>,
    /// `SSEKMSEncryptionContext`: "a Base64-encoded string holding a UTF-8 encoded JSON
    /// of additional encryption context key-value pairs", verbatim.
    pub kms_encryption_context: Option<String>,
    /// `BucketKeyEnabled`: whether the object uses an S3 Bucket Key for SSE-KMS.
    pub bucket_key_enabled: Option<bool>,
}

impl ObjectEncryption {
    /// Whether this record names NO encryption member at all — what a write that leaves
    /// every one of `PutObject`'s encryption headers off parses to, and exactly the case
    /// where the bucket's own default encryption configuration is what the object is
    /// stored under ([`Bucket::default_object_encryption`]).
    pub fn names_no_member(&self) -> bool {
        self == &Self::default()
    }

    /// Whether this object's encryption is SSE-KMS — the only kind a KMS key id, an
    /// encryption context or an S3 Bucket Key belongs to
    /// (`ApplyServerSideEncryptionByDefault$KMSMasterKeyID`: "This parameter is allowed if
    /// and only if `SSEAlgorithm` is set to `aws:kms` or `aws:kms:dsse`", the two values
    /// [`KMS_SSE_ALGORITHMS`] holds).
    pub fn uses_kms(&self) -> bool {
        self.sse_algorithm
            .as_deref()
            .is_some_and(|algorithm| KMS_SSE_ALGORITHMS.contains(&algorithm))
    }

    /// The header name of the first member here that only SSE-KMS has, or `None` when this
    /// record names none of them: what a caller is told it may not combine with another
    /// algorithm. An S3 Bucket Key counts only when it is switched ON — "Setting this
    /// header to true causes Amazon S3 to use an S3 Bucket Key"
    /// (`PutObjectRequest$BucketKeyEnabled`), so `false` asks for nothing KMS-only.
    pub fn kms_only_member(&self) -> Option<&'static str> {
        if self.kms_key_id.is_some() {
            return Some("x-amz-server-side-encryption-aws-kms-key-id");
        }
        if self.kms_encryption_context.is_some() {
            return Some("x-amz-server-side-encryption-context");
        }
        if self.bucket_key_enabled == Some(true) {
            return Some("x-amz-server-side-encryption-bucket-key-enabled");
        }
        None
    }

    /// How many bytes this record retains — the caller-supplied strings it holds, which
    /// the per-bucket and service-wide budgets must count like every other string kept
    /// beside an object.
    pub fn retained_bytes(&self) -> usize {
        [
            &self.sse_algorithm,
            &self.kms_key_id,
            &self.kms_encryption_context,
        ]
        .iter()
        .filter_map(|member| member.as_ref())
        .map(String::len)
        .sum()
    }
}

/// The `ObjectLockLegalHoldStatus` value that means a legal hold IS in place on an object
/// version — one of that enum's two values
/// (`smithy_query(query="shape",shape="ObjectLockLegalHoldStatus")`, which
/// [`crate::ops::types::OBJECT_LOCK_LEGAL_HOLD_STATUS_VALUES`] carries).
pub const LEGAL_HOLD_ON_STATUS: &str = "ON";

/// The other `ObjectLockLegalHoldStatus` value: no legal hold is in place on the version.
pub const LEGAL_HOLD_OFF_STATUS: &str = "OFF";

/// The `ObjectLockRetentionMode` value whose retention bars a weakening write unless the
/// caller asks to bypass it — one of that enum's two values
/// (`smithy_query(query="shape",shape="ObjectLockRetentionMode")`, which
/// [`crate::ops::types::OBJECT_LOCK_RETENTION_MODE_VALUES`] carries).
pub const GOVERNANCE_RETENTION_MODE: &str = "GOVERNANCE";

/// The other `ObjectLockRetentionMode` value: the one whose retention nothing may weaken
/// while it is in force, bypass asked for or not.
pub const COMPLIANCE_RETENTION_MODE: &str = "COMPLIANCE";

/// The Object Lock RETENTION in force on one stored object version: the configuration
/// `PutObjectRetention` last placed on it, which `GetObjectRetention` reports back.
///
/// The members are `ObjectLockRetention`'s own
/// (`smithy_query(query="shape",shape="ObjectLockRetention")`): the `Mode` the version is
/// locked in and the `RetainUntilDate` that lock runs to. Both together or neither — "the
/// container element for the Object Retention configuration"
/// (`PutObjectRetentionRequest$Retention`) describes one period in one mode, so a version
/// either carries a retention or carries none, which is why this lives behind an
/// `Option` on [`Object`].
///
/// What it records is a retention SETTING, not an enforcement engine: the instant the
/// period runs to is kept so every reader answers the same date, and
/// [`Self::in_force_at`] is what a write that would weaken it is judged against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRetention {
    /// An `ObjectLockRetentionMode` value — [`GOVERNANCE_RETENTION_MODE`] or
    /// [`COMPLIANCE_RETENTION_MODE`].
    pub mode: String,
    /// When the retention period runs to: `ObjectLockRetention$RetainUntilDate`, "the date
    /// on which this Object Lock Retention will expire".
    pub retain_until: SystemTime,
}

impl ObjectRetention {
    /// Whether the period is still running at `instant`, which is what makes the retention
    /// a restriction on a later write rather than a record of one that has run out: a
    /// retention "will expire" at its `RetainUntilDate`
    /// (`ObjectLockRetention$RetainUntilDate`), so the instant it names is the first one it
    /// no longer covers.
    pub fn in_force_at(&self, instant: SystemTime) -> bool {
        instant < self.retain_until
    }

    /// Whether this retention protects LESS than `other` does — the comparison a
    /// `PutObjectRetention` over an existing retention is judged by: a period that ends
    /// earlier (shortening it) or a mode that can itself be lifted where the one in force
    /// cannot (`GOVERNANCE` placed over `COMPLIANCE`).
    pub fn weakens(&self, other: &Self) -> bool {
        self.retain_until < other.retain_until
            || (other.mode == COMPLIANCE_RETENTION_MODE && self.mode != COMPLIANCE_RETENTION_MODE)
    }

    /// How many bytes this record retains — the caller-supplied string it holds, which the
    /// per-bucket and service-wide budgets must count like every other string kept beside
    /// an object ([`ObjectEncryption::retained_bytes`] counts its own the same way).
    pub fn retained_bytes(&self) -> usize {
        self.mode.len()
    }
}

/// The request field a `RetainUntilDate` that does not end in the future is reported
/// against: the member of the `Retention` document that carries it
/// (`smithy_query(query="shape",shape="ObjectLockRetention")` reports
/// `RetainUntilDate`), which is how S3's `InvalidArgument` document names the argument it
/// rejected.
pub const RETAIN_UNTIL_DATE_ARGUMENT: &str = "RetainUntilDate";

/// The `<Message>` S3 refuses a retention whose period does not end in the FUTURE with —
/// fixed text, with no variable part.
///
/// A retention period is the span a version is protected FOR, and it "will expire" at the
/// date it names (`ObjectLockRetention$RetainUntilDate`), so a date at or before now
/// describes a period that protects nothing: the value is a bad ARGUMENT rather than a
/// permission the caller lacks, which is why it is refused with
/// [`INVALID_ARGUMENT_CODE`] and HTTP 400 and not with [`ACCESS_DENIED_CODE`].
pub const RETAIN_UNTIL_DATE_NOT_IN_FUTURE_MESSAGE: &str =
    "The retain until date must be in the future!";

/// The request field a `x-amz-object-lock-retain-until-date` a WRITE carries is reported
/// against: the header itself, which is where that write's retention date arrives
/// (`smithy_query(query="operation",operation="PutObject")` reports
/// `ObjectLockRetainUntilDate: {httpHeader: "x-amz-object-lock-retain-until-date"}`).
pub const OBJECT_LOCK_RETAIN_UNTIL_DATE_HEADER: &str = "x-amz-object-lock-retain-until-date";

/// The text a request spelled a HEADER in, for a refusal that must echo the value that
/// header carried ([`echoed_retain_until_date`] is the one that does).
///
/// A header's value reaches a handler already parsed into the member's own type, so the
/// spelling is gone; `parts` is where it is still the caller's own text.
pub fn header_text<'p>(parts: &'p ::http::request::Parts, name: &str) -> Option<&'p str> {
    parts
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
}

/// A retention date as a refusal ECHOES it: the text the REQUEST spelled it in, and a
/// rendering of the parsed instant only where that text cannot be recovered.
///
/// S3's `InvalidArgument` names the field it rejected and the value that field CARRIED, so
/// the echo is the caller's own text: a timestamp parses to an instant, which can be written
/// several ways and is no longer what the caller sent, and a client matching the envelope
/// against what it sent reads that text. The fallback — for a date whose spelling the
/// request did not keep — is the ISO 8601 `date-time` the member is bound in
/// (`ObjectLockRetention$RetainUntilDate`'s `timestampFormat`), which is the spelling a
/// request carrying the canonical form sent.
///
/// One place, so the header-borne date of a write and the document-borne date of a retention
/// write are echoed identically.
pub fn echoed_retain_until_date(
    spelled: Option<&str>,
    date: Option<&::aws_smithy_types::DateTime>,
) -> String {
    if let Some(spelled) = spelled.map(str::trim).filter(|text| !text.is_empty()) {
        return spelled.to_string();
    }
    date.and_then(|date| {
        date.fmt(::aws_smithy_types::date_time::Format::DateTime)
            .ok()
    })
    .unwrap_or_default()
}

/// The IAM action that LIFTING a governance-mode retention is authorized under: "to
/// override or remove governance-mode retention settings you must have the
/// `s3:BypassGovernanceRetention` permission" (Amazon S3 User Guide, *S3 Object Lock*),
/// which is what a request carrying `x-amz-bypass-governance-retention` asks the service to
/// do.
pub const BYPASS_GOVERNANCE_RETENTION_ACTION: &str = "s3:BypassGovernanceRetention";

/// How S3 words the 403 [`ACCESS_DENIED_CODE`] of a request a policy in force EXPLICITLY
/// DENIES the caller: who was refused, which IAM `action` on which `resource`, and that it
/// was an explicit deny in the resource's own policy that refused it.
///
/// The same sentence shape every authorization refusal of this crate is worded in
/// ([`block_public_access_denial`] states the guardrail half of it), with the reason clause
/// IAM publishes for this decision rather than for an absent grant
/// (`service_common::iam_authorization::not_authorized_by_resource_policy` states that
/// other one). Every value in it comes from the request and from this emulator's own
/// identity — the principal is the caller ([`caller_principal_arn`]), the action is the one
/// the policy denied and the resource is the one the request addressed — so nothing in it
/// is a recorded value.
pub fn explicit_resource_policy_denial(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: \"{resource}\" with an \
         explicit deny in a resource-based policy",
        caller_principal_arn()
    )
}

/// The Object Lock state a read of one stored object version REPORTS: the three
/// `x-amz-object-lock-*` headers `GetObject` and `HeadObject` both declare, as
/// [`Object::reported_object_lock`] renders them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportedObjectLock {
    /// `ObjectLockMode` — an [`ObjectRetention::mode`] value, for a version carrying a
    /// retention.
    pub mode: Option<String>,
    /// `ObjectLockRetainUntilDate` — the instant that retention runs to.
    pub retain_until: Option<SystemTime>,
    /// `ObjectLockLegalHoldStatus` — [`LEGAL_HOLD_ON_STATUS`] or [`LEGAL_HOLD_OFF_STATUS`]
    /// for a version a legal hold has been applied to, and nothing for one that never has.
    pub legal_hold_status: Option<String>,
}

/// The Object Lock a WRITE asked for, as [`requested_object_lock`] read it off the three
/// `x-amz-object-lock-*` members every object write declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestedObjectLock {
    /// The retention the write named, for [`Object::retention`] — `None` when it named
    /// neither a mode nor a date, in which case the bucket's own default retention is what
    /// locks the version ([`Bucket::put_object`] applies it).
    pub retention: Option<ObjectRetention>,
    /// The legal hold the write named, for [`Object::legal_hold`] — `None` when it named no
    /// status, which leaves the version with no hold ever applied.
    pub legal_hold: Option<bool>,
}

impl RequestedObjectLock {
    /// Whether the write named any Object Lock member at all — what decides whether the
    /// bucket has to carry an Object Lock configuration for it to be served.
    pub fn names_a_member(&self) -> bool {
        self.retention.is_some() || self.legal_hold.is_some()
    }
}

/// Why the Object Lock a write asked for cannot be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectLockRequestRejection {
    /// One of `x-amz-object-lock-mode` and `x-amz-object-lock-retain-until-date` without the
    /// other: a retention runs in a mode TO a date, so half of one describes no period.
    IncompleteRetention,
    /// A `x-amz-object-lock-retain-until-date` outside the range of instants this platform
    /// can hold, which no period a version is locked for can run to.
    UnrepresentableRetainUntilDate,
    /// A `x-amz-object-lock-retain-until-date` that is not in the FUTURE, which describes a
    /// period that protects nothing — see [`RETAIN_UNTIL_DATE_NOT_IN_FUTURE_MESSAGE`].
    RetainUntilDateNotInFuture,
}

/// The Object Lock an object write asked for, read off the three members that carry it, or
/// why it cannot be recorded.
///
/// "The Object Lock mode that you want to apply to this object" with "the date and time when
/// you want this object's Object Lock to expire" beside it
/// (`PutObjectRequest$ObjectLockMode` and `$ObjectLockRetainUntilDate`), and "specifies
/// whether a legal hold will be applied to this object"
/// (`$ObjectLockLegalHoldStatus`) — so a write that names them stores them ON the version it
/// writes, which is what a later delete of that version is judged against
/// ([`Bucket::object_lock_delete_refusal`]), and `PutObjectRetention` /
/// `PutObjectLegalHold` are what change them afterwards.
///
/// ONE place for the rule, so every write that declares these members — the one-shot write,
/// the copy and the multipart initiation whose settings the completed object carries —
/// records the same lock from the same values.
/// `now` is the instant the period is judged against: a retention "will expire" at the date
/// it names, so one that does not end after `now` protects nothing and is refused rather
/// than recorded ([`RETAIN_UNTIL_DATE_NOT_IN_FUTURE_MESSAGE`] is the sentence, the same rule
/// `PutObjectRetention` judges its own `RetainUntilDate` by).
pub fn requested_object_lock(
    mode: Option<&str>,
    retain_until_date: Option<&::aws_smithy_types::DateTime>,
    legal_hold_status: Option<&str>,
    now: SystemTime,
) -> Result<RequestedObjectLock, ObjectLockRequestRejection> {
    let retention = match (mode, retain_until_date) {
        (None, None) => None,
        (Some(mode), Some(date)) => {
            let Some(retain_until) = instant_of(date) else {
                return Err(ObjectLockRequestRejection::UnrepresentableRetainUntilDate);
            };
            if retain_until <= now {
                return Err(ObjectLockRequestRejection::RetainUntilDateNotInFuture);
            }
            Some(ObjectRetention {
                mode: mode.to_string(),
                retain_until,
            })
        }
        _ => return Err(ObjectLockRequestRejection::IncompleteRetention),
    };
    Ok(RequestedObjectLock {
        retention,
        // "ON" is a hold in place and "OFF" is one that is not; a write naming no status
        // leaves the version with none ever applied, which is a third state a reader reports
        // apart from `OFF` (see [`Object::legal_hold`]).
        legal_hold: legal_hold_status.map(|status| status == LEGAL_HOLD_ON_STATUS),
    })
}

/// The one value the `ObjectLockEnabled` enum publishes
/// (`smithy_query(query="shape",shape="ObjectLockEnabled")`, whose values
/// [`crate::ops::types::OBJECT_LOCK_ENABLED_VALUES`] carries): "indicates whether this
/// bucket has an Object Lock configuration enabled"
/// (`ObjectLockConfiguration$ObjectLockEnabled`).
///
/// What a reader of a bucket's Object Lock configuration reports for it, and the only value
/// a put of one may name — there is no second state for that member to be in, which is why
/// [`Bucket::object_lock_enabled`] keeps it as a `bool`.
pub const OBJECT_LOCK_ENABLED_STATUS: &str = "Enabled";

/// How many days a default retention period of one YEAR runs for.
///
/// A default retention period is "the number of days" or "the number of years that you want
/// to specify for the default retention period" (`DefaultRetention$Days` and
/// `DefaultRetention$Years`) counted from the object's own creation time. The API publishes
/// no calendar for the second of those, so a year is this many days here wherever it falls.
const DAYS_PER_RETENTION_YEAR: i64 = 365;

/// The longest default retention period a rule may name, in YEARS: "you can set a retention
/// period of up to 100 years" (Amazon S3 User Guide, *S3 Object Lock*).
///
/// A put naming a longer period is refused rather than recorded, so every period a bucket
/// carries is one an object written into it can really be locked for.
pub const MAX_DEFAULT_RETENTION_YEARS: i32 = 100;

/// The same ceiling in DAYS, the other unit a period may be given in: derived from
/// [`MAX_DEFAULT_RETENTION_YEARS`] through [`DAYS_PER_RETENTION_YEAR`], so the two bounds
/// cannot drift apart.
pub const MAX_DEFAULT_RETENTION_DAYS: i32 =
    MAX_DEFAULT_RETENTION_YEARS * DAYS_PER_RETENTION_YEAR as i32;

/// The PERIOD a bucket's default Object Lock retention runs for.
///
/// "The DefaultRetention period can be either Days or Years but you must select one. You
/// cannot specify Days and Years at the same time" (`PutObjectLockConfiguration`'s own
/// documentation), so this is one of the two and never both nor neither — which a pair of
/// `Option`s could not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultRetentionPeriod {
    /// `DefaultRetention$Days` — "the number of days that you want to specify for the
    /// default retention period".
    Days(i32),
    /// `DefaultRetention$Years` — "the number of years that you want to specify for the
    /// default retention period".
    Years(i32),
}

impl DefaultRetentionPeriod {
    /// Whether the period is one a rule may name: it has to run FORWARD from the write it
    /// locks, so a count of zero or fewer is no period at all, and no further than the
    /// published ceiling ([`MAX_DEFAULT_RETENTION_YEARS`]).
    ///
    /// The one place that judgement lives, so what a bucket carries is always a period an
    /// object written into it can be locked for — the write path never has to drop a lock it
    /// cannot compute.
    pub fn is_within_published_bounds(self) -> bool {
        match self {
            Self::Days(days) => (1..=MAX_DEFAULT_RETENTION_DAYS).contains(&days),
            Self::Years(years) => (1..=MAX_DEFAULT_RETENTION_YEARS).contains(&years),
        }
    }

    /// How many days the period runs for — a year counted as [`DAYS_PER_RETENTION_YEAR`] of
    /// them — or `None` when that product leaves the range this type can hold.
    fn days(self) -> Option<i64> {
        match self {
            Self::Days(days) => Some(i64::from(days)),
            Self::Years(years) => i64::from(years).checked_mul(DAYS_PER_RETENTION_YEAR),
        }
    }

    /// How many seconds the period runs for, or `None` when the arithmetic leaves the range
    /// this type can hold.
    fn seconds(self) -> Option<i64> {
        self.days()?.checked_mul(SECONDS_PER_DAY)
    }
}

/// The DEFAULT RETENTION a bucket's Object Lock configuration carries: the retention every
/// new object placed in the bucket is locked under.
///
/// The members are `DefaultRetention`'s own
/// (`smithy_query(query="shape",shape="DefaultRetention")`) less its event hold, which
/// nothing in this emulator carries out: the `Mode` new objects are locked in and the period
/// that lock runs for. "The DefaultRetention settings require both a mode and a period"
/// (`PutObjectLockConfiguration`'s own documentation), so the pair is ONE value here rather
/// than two members that could disagree, and a bucket either carries a default retention or
/// carries none — which is why this lives behind an `Option` on [`Bucket`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketDefaultRetention {
    /// An `ObjectLockRetentionMode` value — [`GOVERNANCE_RETENTION_MODE`] or
    /// [`COMPLIANCE_RETENTION_MODE`].
    pub mode: String,
    /// How long after it is written an object stays locked.
    pub period: DefaultRetentionPeriod,
}

impl BucketDefaultRetention {
    /// The retention an object written at `written_at` is locked under by this rule.
    ///
    /// `None` only for a date outside the range this platform can hold, which a stored rule
    /// cannot reach: [`DefaultRetentionPeriod::is_within_published_bounds`] is what every put
    /// of a configuration passes through, so the period here is at most the published
    /// ceiling.
    ///
    /// "The rule specified in the Object Lock configuration will be applied by default to
    /// every new object placed in the specified bucket"
    /// (`PutObjectLockConfiguration`'s own documentation), and the period is counted from
    /// the object's own creation time, so the date each version is locked to is its own
    /// rather than a property of the bucket.
    pub fn retention_at(&self, written_at: SystemTime) -> Option<ObjectRetention> {
        let seconds = u64::try_from(self.period.seconds()?).ok()?;
        Some(ObjectRetention {
            mode: self.mode.clone(),
            retain_until: written_at.checked_add(std::time::Duration::from_secs(seconds))?,
        })
    }
}

/// How long after the `RestoreObject` that initiated it a temporary copy of an archived
/// object becomes available in this emulator.
///
/// NOT a fidelity claim about retrieval time: a real Glacier retrieval takes minutes to
/// hours depending on the tier, which no emulated workflow could wait for. What IS faithful
/// is that the copy is NOT there when the call returns — "you must first initiate a restore
/// request, and then wait until a temporary copy of the object is available"
/// (`RestoreObject`'s own documentation) — so the restore is reported as ongoing for a
/// span, and a caller reading the object's state straight afterwards sees
/// `ongoing-request="true"` exactly as it would against AWS. The instant it falls due is
/// stored ([`ObjectRestore::available_at`]) and nothing sleeps for it.
pub const RESTORE_COMPLETION_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
/// The restore in force on one stored object version: what the last `RestoreObject` on it
/// asked for, and when the temporary copy it initiated falls due.
///
/// The members are `RestoreRequest`'s own
/// (`smithy_query(query="shape",shape="RestoreRequest")`): the `Days` the active copy lives
/// for and the `Tier` the retrieval is processed at. A restore is one job on one version —
/// "To restore a specific object version, you can provide a version ID. If you don't
/// provide a version ID, Amazon S3 restores the current version" — so a version either
/// carries one or carries none, which is why this lives behind an `Option` on [`Object`].
///
/// Nothing here is a copy of the object: the archived bytes are the ones the store already
/// holds. What it records is the STATE of the restore, which is what every reader of the
/// object reports through its `x-amz-restore` header ([`Self::header_value`]): ongoing
/// until the copy falls due, then available until it expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRestore {
    /// A `Tier` value — the retrieval tier the restore is processed at
    /// ([`DEFAULT_RESTORE_TIER`] when the request named none).
    pub tier: String,
    /// "Lifetime of the active copy in days" (`RestoreRequest$Days`): how long the
    /// temporary copy lives once it is available.
    pub days: i32,
    /// When the temporary copy falls due — the instant the restore stops being reported as
    /// ongoing, [`RESTORE_COMPLETION_DELAY`] after the call that initiated it.
    pub available_at: SystemTime,
}

impl ObjectRestore {
    /// Whether the retrieval is still running at `instant`: "If the object restoration is
    /// in progress, the header returns the value `ongoing-request="true"`"
    /// (`HeadObjectOutput$Restore`).
    pub fn in_progress_at(&self, instant: SystemTime) -> bool {
        instant < self.available_at
    }

    /// When Amazon S3 is scheduled to delete the temporary copy: [`Self::days`] days after
    /// it became available, rounded up to midnight UTC the way every S3 lifetime date is
    /// ([`midnight_after_days`]), or `None` when that instant is outside the range this
    /// platform can hold.
    pub fn expires_at(&self) -> Option<SystemTime> {
        midnight_after_days(self.available_at, self.days)
    }

    /// That expiry as the HTTP date the `Restore` member reports it as, or `None` when the
    /// lifetime names an instant no date this store can render falls on.
    ///
    /// The one place the date is rendered, so what a reader reports and what
    /// `RestoreObject` checks a lifetime against cannot drift: a restore whose expiry
    /// cannot be rendered is refused rather than recorded, which is what keeps every
    /// recorded restore reportable.
    pub fn expiry_date(&self) -> Option<String> {
        ::aws_smithy_types::DateTime::from(self.expires_at()?)
            .fmt(::aws_smithy_types::date_time::Format::HttpDate)
            .ok()
    }

    /// This restore as the `Restore` response member carries it at `instant`, or `None`
    /// once the temporary copy has expired (the version is archived again, and the header
    /// is reported only "if either the archive restoration is in progress … or an archive
    /// copy is already restored").
    ///
    /// The two documented renderings, verbatim from `HeadObjectOutput$Restore`:
    /// `ongoing-request="true"` while the retrieval runs, and
    /// `ongoing-request="false", expiry-date="Fri, 21 Dec 2012 00:00:00 GMT"` once the copy
    /// is there — the expiry date as an HTTP date, the way [`ObjectExpiration::header_value`]
    /// renders its own.
    pub fn header_value(&self, instant: SystemTime) -> Option<String> {
        if self.in_progress_at(instant) {
            return Some(String::from("ongoing-request=\"true\""));
        }
        let expires_at = self.expires_at()?;
        if instant >= expires_at {
            return None;
        }
        let expiry_date = self.expiry_date()?;
        Some(format!(
            "ongoing-request=\"false\", expiry-date=\"{expiry_date}\""
        ))
    }

    /// How many bytes this record retains — the caller-supplied string it holds, which the
    /// per-bucket and service-wide budgets must count like every other string kept beside
    /// an object ([`ObjectRetention::retained_bytes`] counts its own the same way).
    pub fn retained_bytes(&self) -> usize {
        self.tier.len()
    }
}

/// The `Tier` a restore is processed at when the request names none.
///
/// From `RestoreObject`'s own documentation: "Standard - Standard retrievals allow you to
/// access any of your archived objects within several hours. This is the default option for
/// retrieval requests that do not specify the retrieval option." `Standard` is one of the
/// `Tier` enum's values (`smithy_query(query="shape",shape="Tier")`).
pub const DEFAULT_RESTORE_TIER: &str = "Standard";

/// The `ReplicationStatus` a SOURCE object version carries once its replication is done:
/// every rule that covers it copied it to its destination.
///
/// One of the values the enum publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ReplicationStatus")` reports
/// `[COMPLETE, PENDING, FAILED, REPLICA, COMPLETED]`).
pub const REPLICATION_STATUS_COMPLETED: &str = "COMPLETED";

/// The `ReplicationStatus` a source object version carries while the replication its
/// bucket's configuration calls for has not been reported done yet.
pub const REPLICATION_STATUS_PENDING: &str = "PENDING";

/// The `ReplicationStatus` a source object version carries when the replication a rule
/// calls for could not be performed — a destination this emulator cannot write.
pub const REPLICATION_STATUS_FAILED: &str = "FAILED";

/// The `ReplicationStatus` the COPY carries: "the object in the destination bucket is a
/// replica" (`HeadObjectOutput$ReplicationStatus`), which is a finished object from the
/// moment it is written.
pub const REPLICATION_STATUS_REPLICA: &str = "REPLICA";

/// How long after the write that started it a replication is reported done.
///
/// Replication is asynchronous — "Amazon S3 replicates objects asynchronously"
/// (`PutBucketReplication`'s own documentation), with no published duration — so the span a
/// source object spends in [`REPLICATION_STATUS_PENDING`] is this emulator's own, chosen
/// short enough that a caller polling the status sees it settle and long enough that a
/// caller reading straight after the write still sees the transient state.
pub const REPLICATION_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// What replication did with one stored object version: the status it settles on, and when
/// that status is reported.
///
/// The copy itself is made by the write that recorded the version
/// ([`ComAmazonawsS3Amazons3State::replicate_write`]), so this records an outcome rather
/// than a promise; what the clock still owes is only the moment the source version stops
/// reporting [`REPLICATION_STATUS_PENDING`] and reports that outcome
/// ([`Self::status_at`]) — the asynchrony AWS's own replication has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectReplication {
    /// The status this version settles on: [`REPLICATION_STATUS_COMPLETED`] or
    /// [`REPLICATION_STATUS_FAILED`] for a source version, and
    /// [`REPLICATION_STATUS_REPLICA`] for the copy.
    pub settled: String,
    /// The instant from which [`Self::status_at`] reports [`Self::settled`]; before it, a
    /// source version reports [`REPLICATION_STATUS_PENDING`]. The write's own instant for a
    /// replica, which is finished the moment it exists.
    pub settles_at: SystemTime,
}

impl ObjectReplication {
    /// The outcome `settled` of a SOURCE version written at `at`, reported from
    /// [`REPLICATION_SETTLES_AFTER`] past that write.
    pub fn source(settled: &str, at: SystemTime) -> Self {
        Self {
            settled: settled.to_string(),
            settles_at: at + REPLICATION_SETTLES_AFTER,
        }
    }

    /// The replica written at `at` — [`REPLICATION_STATUS_REPLICA`] from the moment it is
    /// stored, because the copy is what the status describes and it is already there.
    pub fn replica(at: SystemTime) -> Self {
        Self {
            settled: REPLICATION_STATUS_REPLICA.to_string(),
            settles_at: at,
        }
    }

    /// The `ReplicationStatus` a reader at `now` reports for this version:
    /// [`REPLICATION_STATUS_PENDING`] until [`Self::settles_at`], and the settled outcome
    /// from then on.
    ///
    /// A pure function of the clock: a read reports it and writes nothing.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            REPLICATION_STATUS_PENDING
        } else {
            &self.settled
        }
    }

    /// How many bytes this record retains — the status string kept beside the object, which
    /// the per-bucket and service-wide budgets count like every other string an object
    /// holds.
    pub fn retained_bytes(&self) -> usize {
        self.settled.len()
    }
}


impl Object {
    /// A freshly written object of `body`, as a write that names nothing about it beyond its
    /// content type stores it.
    ///
    /// A convenience for a writer that carries only bytes — the data plane of another service
    /// of this emulator delivering into a bucket — so it does not restate this struct's whole
    /// member list: the entity tag is computed from the bytes the object is stored with, the
    /// storage class is [`DEFAULT_STORAGE_CLASS`], the version id is minted by
    /// [`ComAmazonawsS3Amazons3State::put_object`] rather than here, and the members a later
    /// call places on a stored version — its tag set, its Object Lock retention and legal
    /// hold, its restore, its annotations and its ACL — are unset, which is the state a write
    /// leaves a version in. S3's
    /// own `PutObject` and `CopyObject` build the object they store themselves, because they
    /// carry the cache-control, metadata, tag set and encryption configuration of the request.
    pub fn stored(body: Vec<u8>, content_type: Option<String>) -> Self {
        Self {
            etag: object_etag(&body),
            body,
            version_id: None,
            last_modified: SystemTime::now(),
            storage_class: DEFAULT_STORAGE_CLASS.to_string(),
            content_type,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            expires: None,
            website_redirect_location: None,
            metadata: BTreeMap::new(),
            tags: Vec::new(),
            encryption: ObjectEncryption::default(),
            retention: None,
            legal_hold: None,
            restore: None,
            annotations: BTreeMap::new(),
            acl: None,
            // A writer that carries only bytes names no checksum algorithm, so the object
            // it stores carries no checksum either.
            checksum: None,
            // Nor is it assembled from parts: one write stored it whole.
            parts: Vec::new(),
            // Replication is settled by the store once the version is recorded (see
            // [`ComAmazonawsS3Amazons3State::replicate_write`]), not by the writer.
            replication: None,
        }
    }

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
            + self.annotation_bytes()
            + acl_grants_bytes(self.acl.as_deref().unwrap_or_default())
            + self.encryption.retained_bytes()
            + self
                .checksum
                .as_ref()
                .map_or(0, ObjectChecksum::retained_bytes)
            + self
                .parts
                .iter()
                .map(ObjectPartRecord::retained_bytes)
                .sum::<usize>()
            + self
                .retention
                .as_ref()
                .map_or(0, ObjectRetention::retained_bytes)
            + self
                .restore
                .as_ref()
                .map_or(0, ObjectRestore::retained_bytes)
            + self
                .replication
                .as_ref()
                .map_or(0, ObjectReplication::retained_bytes)
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

    /// How many bytes this object's annotations retain: each one's name, with the payload
    /// and entity tag of the annotation held under it ([`Annotation::retained_bytes`]).
    ///
    /// Counted by [`Self::retained_bytes`] like every other string kept beside an object,
    /// so an annotation is bounded by the same per-bucket and service-wide budgets the
    /// object's own bytes are.
    pub fn annotation_bytes(&self) -> usize {
        self.annotations
            .iter()
            .map(|(name, annotation)| name.len() + annotation.retained_bytes())
            .sum()
    }

    /// The annotation attached to this object under `name`, or `None` when it carries none
    /// — which is what makes a read or a delete naming a name this object does not hold
    /// answer `NoSuchAnnotation`.
    pub fn annotation(&self, name: &str) -> Option<&Annotation> {
        self.annotations.get(name)
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

    /// The Object Lock state a read of this stored version reports — see
    /// [`ReportedObjectLock`].
    ///
    /// "The Object Lock mode, if any, that's in effect for this object" and "the date and
    /// time when the Object Lock retention period expires"
    /// (`GetObjectOutput$ObjectLockMode` / `$ObjectLockRetainUntilDate`) are the retention
    /// `PutObjectRetention` placed on THIS version, reported as the record stands — the
    /// same record `GetObjectRetention` reports, so the readers of one retention cannot
    /// disagree about it. The legal hold status "is not returned if the specified version
    /// of this object has never had a legal hold applied"
    /// (`GetObjectOutput$ObjectLockLegalHoldStatus`), so a version nobody has placed one on
    /// reports nothing rather than [`LEGAL_HOLD_OFF_STATUS`].
    ///
    /// ONE place, so `GetObject` and `HeadObject` report one version's lock identically.
    pub fn reported_object_lock(&self) -> ReportedObjectLock {
        ReportedObjectLock {
            mode: self
                .retention
                .as_ref()
                .map(|retention| retention.mode.clone()),
            retain_until: self.retention.as_ref().map(|retention| retention.retain_until),
            legal_hold_status: self.legal_hold.map(|held| {
                if held {
                    LEGAL_HOLD_ON_STATUS.to_string()
                } else {
                    LEGAL_HOLD_OFF_STATUS.to_string()
                }
            }),
        }
    }

    /// Whether this object's bytes are ARCHIVED at `instant` — in a storage class that
    /// holds no copy a read can serve, with no restored temporary copy available.
    ///
    /// "The storage class of the object can't be `GLACIER` or `DEEP_ARCHIVE` unless you first
    /// restore a copy of the object" (`InvalidObjectState`'s own condition, from
    /// `GetObject`'s documentation of it): the two classes [`ARCHIVED_STORAGE_CLASSES`]
    /// names, and the restore that lifts the bar is the one `RestoreObject` initiated on THIS
    /// version — in force only once the temporary copy has fallen due
    /// ([`ObjectRestore::header_value`] reports the same three states) and only until it
    /// expires, after which the version is archived again.
    pub fn archived_at(&self, instant: SystemTime) -> bool {
        if !ARCHIVED_STORAGE_CLASSES.contains(&self.storage_class.as_str()) {
            return false;
        }
        self.restore
            .as_ref()
            .is_none_or(|restore| restore.in_progress_at(instant) || restore.header_value(instant).is_none())
    }

    /// How many parts a multipart upload assembled this object from, or `None` for an object
    /// one write stored whole.
    ///
    /// What a part-scoped read reports as `PartsCount`: "the count of parts this object has.
    /// This value is only returned if you specify `partNumber` in your request and the object
    /// was uploaded as a multipart upload" (`GetObjectOutput$PartsCount`) — so an object that
    /// was NOT uploaded as a multipart upload reports no count, whatever the read asked for.
    pub fn parts_count(&self) -> Option<i32> {
        (!self.parts.is_empty())
            .then(|| i32::try_from(self.parts.len()).unwrap_or(i32::MAX))
    }

    /// The span of this object's bytes that `part_number` names, or `None` when the object
    /// has no such part.
    ///
    /// A part-scoped read "effectively performs a 'ranged' GET request for the part
    /// specified" (`GetObjectRequest$PartNumber`), so the span is the part's own place in the
    /// object: the parts were concatenated "in ascending order by part number"
    /// (`CompleteMultipartUpload`'s own documentation), which makes a part's first byte the
    /// sum of the sizes before it.
    ///
    /// An object stored by ONE write has a single part — the whole object — because that is
    /// what a part-scoped read of it answers; it reports no `PartsCount`
    /// ([`Self::parts_count`]) all the same, since it was not uploaded as a multipart upload.
    /// A part holding no bytes names no span, because `Content-Range` cannot state one.
    pub fn part_span(&self, part_number: i32) -> Option<ByteSpan> {
        if self.parts.is_empty() {
            return (part_number == MIN_PART_NUMBER)
                .then(|| ByteSpan::new(0, self.body.len().checked_sub(1)?))
                .flatten();
        }
        let mut first = 0usize;
        for part in &self.parts {
            let size = usize::try_from(part.size).unwrap_or(0);
            if part.part_number == part_number {
                return ByteSpan::new(first, first + size.checked_sub(1)?);
            }
            first += size;
        }
        None
    }

    /// The checksum a response carrying `span` of this object reports — `None` for a span
    /// no checksum of this store's describes, and for an object written under no checksum
    /// algorithm at all.
    ///
    /// Every `Checksum*` member is "the Base64 encoded … checksum of the object" (each
    /// member's own documentation), so the value a response advertises has to be the digest
    /// of the bytes that response CARRIES: a reader validating it recomputes the digest over
    /// the body it received. So
    ///
    /// * a read of the whole object (`span` of `None`, or a span covering every byte)
    ///   reports the checksum the write recorded;
    /// * a read narrowed to exactly one part of the object reports THAT part's own digest,
    ///   which the completion recorded beside it ([`ObjectPartRecord::checksum`]), under the
    ///   object's algorithm;
    /// * any other narrowed read reports no checksum, because this store holds no digest of
    ///   those bytes and the object's own would describe a body the response does not carry.
    ///
    /// ONE place for the rule, so a GET and a HEAD of the same span cannot advertise
    /// different checksums.
    pub fn checksum_of_span(&self, span: Option<ByteSpan>) -> Option<ObjectChecksum> {
        let stored = self.checksum.as_ref()?;
        let Some(span) = span else {
            return Some(stored.clone());
        };
        if span.first() == 0 && span.len() >= self.body.len() {
            return Some(stored.clone());
        }
        let part = self
            .parts
            .iter()
            .find(|part| self.part_span(part.part_number) == Some(span))?;
        Some(ObjectChecksum {
            algorithm: stored.algorithm.clone(),
            value: part.checksum.clone()?,
            // The object's own type, which "determines how part-level checksums are
            // combined to create an object-level checksum for multipart objects"
            // (`GetObjectOutput$ChecksumType`) — a property of the object, not of the span
            // this response carries of it.
            checksum_type: stored.checksum_type.clone(),
        })
    }
}

/// One in-progress multipart upload this emulator holds, as the
/// `CreateMultipartUpload` that initiated it recorded it.
///
/// A multipart upload is a RESOURCE of its own, distinct from the object it will become:
/// "This action initiates a multipart upload and returns an upload ID … You also include
/// this upload ID in the final request to either complete or abort the multipart upload
/// request" (`CreateMultipartUpload`'s own documentation). So everything the initiating
/// request settles about the FINISHED object is kept here until a later
/// `CompleteMultipartUpload` applies it — the same members [`Object`] keeps, because that
/// is what the completed object is made of — and the upload is addressed by its id in the
/// meantime.
///
/// The parts uploaded into it so far are here too ([`Self::parts`]): a part belongs to the
/// upload rather than to the object, because nothing of it is addressable as an object
/// until a completion assembles it.
#[derive(Debug, Clone)]
pub struct MultipartUpload {
    /// The object key the upload was initiated for — "Object key for which the multipart
    /// upload is to be initiated" (`CreateMultipartUploadRequest$Key`). An upload is
    /// addressed by its id AND its key, so a request naming the id under another key
    /// names no upload.
    pub key: String,
    /// When the upload was initiated: the instant
    /// [`ComAmazonawsS3Amazons3State::create_multipart_upload`] recorded it, which is what
    /// `MultipartUpload$Initiated` reports. Never reassigned, so the value a reader sees
    /// does not move while the upload is in progress.
    pub initiated_at: SystemTime,
    /// A `StorageClass` value; [`DEFAULT_STORAGE_CLASS`] when the request named none —
    /// what `ListParts` reports as the "class of storage used to store the uploaded
    /// object" (`ListPartsOutput$StorageClass`).
    pub storage_class: String,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
    pub content_disposition: Option<String>,
    pub content_encoding: Option<String>,
    pub content_language: Option<String>,
    pub expires: Option<String>,
    pub website_redirect_location: Option<String>,
    /// The user-defined metadata the initiating request carried in its `x-amz-meta-`
    /// headers: "A map of metadata to store with the object in S3"
    /// (`CreateMultipartUploadRequest$Metadata`) — the object the upload completes into,
    /// so it is held with the upload until then.
    pub metadata: BTreeMap<String, String>,
    /// The tag set the initiating request's `x-amz-tagging` header named, judged against
    /// an OBJECT's published tag-set limits ([`OBJECT_TAG_SET_LIMITS`]) exactly as
    /// `PutObject`'s own header is. Empty when the request carried none.
    pub tags: Vec<ResourceTag>,
    /// The server-side encryption configuration the upload was initiated under, which
    /// every reader of the upload reports back — see [`ObjectEncryption`].
    pub encryption: ObjectEncryption,
    /// The `ChecksumAlgorithm` the initiating request asked S3 to create the object's
    /// checksum with, or `None` when it named none. The CONFIGURATION, not a checksum:
    /// nothing here computes one, so no checksum VALUE is recorded or reported.
    pub checksum_algorithm: Option<String>,
    /// The `ChecksumType` the initiating request asked for, kept for the same reason as
    /// [`Self::checksum_algorithm`].
    pub checksum_type: Option<String>,
    /// The parts `UploadPart` has recorded against this upload, by part number.
    ///
    /// Keyed by part number because that is what addresses a part — "A part number uniquely
    /// identifies a part and also defines its position within the object being created"
    /// (`UploadPart`'s own documentation) — so a second upload of one number replaces what
    /// that number held rather than adding a part beside it. Sorted, because every reader
    /// of an upload reports its parts in ascending part-number order
    /// (`ListPartsRequest$PartNumberMarker`: "Only parts with higher part numbers will be
    /// listed").
    ///
    /// A part is recorded only through [`ComAmazonawsS3Amazons3State::upload_part`], the
    /// only place that sees the byte budgets a retained part counts against; an initiation
    /// records an upload with no part at all.
    pub parts: BTreeMap<i32, UploadedPart>,
    /// The Object Lock the INITIATING request asked the completed object to be stored with
    /// — see [`RequestedObjectLock`].
    ///
    /// "When you initiate a multipart upload, you specify the options you want on the
    /// finished object" (`CreateMultipartUpload`'s own documentation), and the Object Lock
    /// mode, retain-until date and legal hold are three of those options, so they are held
    /// with the upload until the completion stores them on the version it writes.
    pub object_lock: RequestedObjectLock,
}

impl MultipartUpload {
    /// How large this upload's user-defined metadata is, in UTF-8 bytes summed over every
    /// key and value — what [`MAX_OBJECT_METADATA_BYTES`] bounds, measured the way
    /// [`Object::metadata_bytes`] measures a stored object's.
    pub fn metadata_bytes(&self) -> usize {
        self.metadata
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum()
    }

    /// The part `part_number` names, or `None` when this upload holds no such part — which
    /// is what makes a read of an upload report only the parts something really uploaded.
    pub fn part(&self, part_number: i32) -> Option<&UploadedPart> {
        self.parts.get(&part_number)
    }

    /// How many bytes the parts of this upload retain — what the per-bucket and
    /// service-wide byte budgets count for an upload, measured the way
    /// [`Object::retained_bytes`] measures a stored object's: the parts, plus the
    /// caller-supplied strings held beside them.
    pub fn retained_bytes(&self) -> usize {
        self.parts
            .values()
            .map(UploadedPart::retained_bytes)
            .sum::<usize>()
            + self
                .object_lock
                .retention
                .as_ref()
                .map_or(0, ObjectRetention::retained_bytes)
    }
}

/// One part `UploadPart` recorded against an in-progress multipart upload.
///
/// A part is the data "you provide … as a part of an object in your request"
/// (`UploadPart`'s own documentation) plus what every reader of the upload reports about
/// it: the members `Part` declares (`smithy_query(query="shape",shape="Part")`) — its
/// number, the instant it was uploaded, its entity tag and its size — so `ListParts`
/// answers from the recorded part rather than from anything re-derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedPart {
    /// "Part number identifying the part. This is a positive integer between 1 and 10,000"
    /// (`Part$PartNumber`), inside [`MIN_PART_NUMBER`]..=[`MAX_PART_NUMBER`]. Kept on the
    /// part as well as in the map's key so a reader that holds a part holds its number.
    pub part_number: i32,
    /// "Entity tag returned when the part was uploaded" (`Part$ETag`) — the quoted MD5 of
    /// the part's own bytes, as [`object_etag`] renders it, so a later completion can be
    /// judged against the tag the upload really answered.
    pub etag: String,
    /// "Date and time at which the part was uploaded" (`Part$LastModified`): the instant
    /// the store recorded it. Reassigned only by an upload that overwrites the part, so a
    /// reader sees the moment the bytes it is reporting arrived.
    pub uploaded_at: SystemTime,
    /// The part's data, which a completion assembles into the object's body. "Size in
    /// bytes of the uploaded part data" (`Part$Size`) is its length, so nothing records a
    /// size beside it that could disagree with the bytes.
    pub body: Vec<u8>,
    /// The digest of THIS part's own bytes, under the algorithm the part write settled, or
    /// `None` when neither the part write nor the initiation named one.
    ///
    /// A part-level checksum is per-part: "the Base64 encoded, … checksum of the part"
    /// (`Part$ChecksumSHA256` and its nine siblings), never the object's composition over
    /// the parts — so its [`ObjectChecksum::checksum_type`] is
    /// [`CHECKSUM_TYPE_FULL_OBJECT`]: the value is the digest of ALL of the part's bytes.
    /// Recorded by the write that received those bytes, so `UploadPart`'s answer and every
    /// later `ListParts` report the same value.
    pub checksum: Option<ObjectChecksum>,
}

impl UploadedPart {
    /// "Size in bytes of the uploaded part data" (`Part$Size`) — the length of the bytes
    /// recorded, as the member's `long` type carries it.
    pub fn size(&self) -> i64 {
        i64::try_from(self.body.len()).unwrap_or(i64::MAX)
    }

    /// How many bytes this part retains: its data AND the strings kept beside it — the
    /// entity tag and the recorded checksum — the way [`Object::retained_bytes`] counts a
    /// stored object's, so no part of an upload escapes the budgets.
    pub fn retained_bytes(&self) -> usize {
        self.body.len()
            + self.etag.len()
            + self
                .checksum
                .as_ref()
                .map_or(0, ObjectChecksum::retained_bytes)
    }
}

/// The id an initiated multipart upload is addressed by.
///
/// S3's upload ids are OPAQUE: `MultipartUploadId` is a plain string in the model
/// (`smithy_query(query="shape",shape="MultipartUploadId")` reports no pattern and no
/// length), and the only thing documented about one is that it identifies the upload every
/// later part, completion and abort request names ("This upload ID is used to associate
/// all of the parts in the specific multipart upload"). So this is DERIVED rather than
/// drawn from a generator, exactly as [`mint_version_id`] is: the SHA-256 of the bucket,
/// the key and the store's monotonic mint sequence, URL-safe Base64 encoded so the id
/// survives the round trip through the `uploadId` query parameter a client sends it back
/// in. Two uploads — of one key or anywhere in the store — therefore never collide, and
/// the id stays what it was once recorded, because nothing re-derives it.
pub fn mint_upload_id(bucket: &str, key: &str, sequence: u64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(bucket.as_bytes());
    // A separator, so ("a", "bc") and ("ab", "c") cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(key.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    ::aws_smithy_types::base64::encode(digest.finalize().as_slice())
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "")
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
///
/// One PART of a multipart upload carries the same tag over its own bytes — "Entity tag
/// returned when the part was uploaded" (`Part$ETag`) — so `UploadPart` renders a part's
/// tag through here too, rather than deriving one of its own.
pub fn object_etag(body: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Md5::digest(body)))
}

/// `etag` WITHOUT the double quotes S3 stores and reports it in.
///
/// An entity tag is held and reported quoted (see [`object_etag`]) — but an event
/// notification reports the digest alone: the `etag` of an EventBridge detail and the
/// `eTag` of a `Records` document both carry it without the surrounding quotes. One place,
/// so both documents report the same value and no caller has to know the difference
/// between the stored form and the published one.
pub fn unquoted_etag(etag: &str) -> &str {
    etag.strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(etag)
}

/// The entity tag S3 reports for an object a multipart upload was completed into, over the
/// parts it was assembled from, in the order they were assembled.
///
/// A multipart object's tag is NOT the digest of its bytes: "if an object is uploaded as a
/// multipart upload, then the ETag of the object is not an MD5 digest of the entire object.
/// Amazon S3 calculates the MD5 digest of each individual part as it is uploaded. The MD5
/// digests are used to determine the ETag for the final object. Amazon S3 concatenates the
/// bytes for the MD5 digests together and then calculates the MD5 digest of these
/// concatenated values" (Amazon S3 User Guide, *Checking object integrity*). S3 then
/// "appends a dash with the total number of parts to the ETag", which is what makes a
/// multipart tag recognisable as one.
///
/// So the digest is over the raw digest BYTES of each part, not over their hex renderings,
/// and the count is the number of parts the completion assembled — one part included, which
/// still carries the dashed form.
pub fn multipart_object_etag(parts: &[&[u8]]) -> String {
    let mut digest = Md5::new();
    for part in parts {
        digest.update(Md5::digest(part));
    }
    format!("\"{}-{}\"", hex::encode(digest.finalize()), parts.len())
}

/// The entity tag's own VALUE: the digest [`object_etag`] rendered, without the double
/// quotes around it.
///
/// Those quotes are HTTP's quoted-string syntax for the `ETag` response header — "An
/// entity-tag consists of an opaque quoted string" (RFC 9110 §8.8.3) — so they belong to
/// the renderings that reproduce that header verbatim (the header itself, and the legacy
/// listing elements that echo it), not to the tag. A field that carries the tag as its own
/// typed value in a response document, and a precondition comparison against a tag a
/// client may send back with or without the quotes, work on this value instead.
pub fn etag_value(etag: &str) -> &str {
    etag.trim_matches('"')
}

/// The conditional WRITE member a request carries that does NOT hold at a key whose current
/// object carries `held_etag` (`None` for a key holding nothing), named by its header, or
/// `None` when every one of them holds.
///
/// The two documented conditions, each quoted from the member that carries it:
///
/// * "Uploads the object only if the ETag (entity tag) value provided during the WRITE
///   operation matches the ETag of the object in S3. If the ETag values do not match, the
///   operation returns a 412 Precondition Failed error" (`PutObjectRequest$IfMatch`) — so a
///   key holding nothing at all matches no tag either.
/// * "Uploads the object only if the object key name does not already exist in the bucket
///   specified. Otherwise, Amazon S3 returns a 412 Precondition Failed error"
///   (`PutObjectRequest$IfNoneMatch`), whose value "expects the '*' (asterisk) character":
///   what it asks is that the key hold nothing.
///
/// The tags are compared by VALUE ([`etag_value`]): a client may send a tag back with or
/// without the double quotes HTTP's quoted-string syntax wraps it in, and both name the same
/// object.
///
/// ONE place for the rule, so every conditional write in this service — the one-shot write
/// and the completion of a multipart upload — refuses exactly the same requests and names
/// the same header in the refusal's `<Condition>`.
pub fn unmet_write_precondition(
    held_etag: Option<&str>,
    if_match: Option<&str>,
    if_none_match: Option<&str>,
) -> Option<&'static str> {
    if if_none_match.is_some() && held_etag.is_some() {
        return Some(IF_NONE_MATCH_CONDITION);
    }
    let match_fails = if_match.is_some_and(|expected| {
        held_etag.is_none_or(|held| etag_value(held) != etag_value(expected))
    });
    match_fails.then_some(IF_MATCH_CONDITION)
}

/// The `If-Match` value that matches whatever entity tag the addressed object carries:
/// "`If-Match` does accept a string value of an '*' (asterisk) character to denote a match
/// of any ETag" (`DeleteObjectRequest$IfMatch`).
pub const ANY_ENTITY_TAG: &str = "*";

/// The conditional DELETE member a request carries that does NOT hold against the object
/// version it addresses, named by its header, or `None` when every one of them holds —
/// `held` being that version, or `None` when the request addresses nothing the store holds.
///
/// The three conditions, each quoted from the member that carries it:
///
/// * "Deletes the object if the ETag (entity tag) value provided during the delete operation
///   matches the ETag of the object in S3. If the ETag values do not match, the operation
///   returns a `412 Precondition Failed` error" (`DeleteObjectRequest$IfMatch`), with
///   [`ANY_ENTITY_TAG`] matching any tag — so a request addressing nothing has no tag to
///   match and is refused;
/// * "the object is deleted only if its modification times matches the provided `Timestamp`.
///   If the `Timestamp` values do not match, the operation returns a `412 Precondition
///   Failed` error. If the `Timestamp` matches or if the object doesn't exist, the operation
///   returns a `204 Success (No Content)` response"
///   (`$IfMatchLastModifiedTime`), compared at the one-second resolution an HTTP date
///   carries (RFC 9110 §5.6.7), which is the format that member is bound in;
/// * "the object is deleted only if its size matches the provided size in bytes"
///   (`$IfMatchSize`), with the same "or if the object doesn't exist" success.
///
/// "You can use the `If-Match`, `x-amz-if-match-last-modified-time` and
/// `x-amz-if-match-size` conditional headers in conjunction with each-other or individually"
/// (`$IfMatchSize`): every member the request carries has to hold, so one failing condition
/// refuses the delete and is the one the refusal's `<Condition>` names.
///
/// The `If-Match` value is judged by the one rule every entity-tag precondition in this
/// module is judged by ([`entity_tag_matches`]): by VALUE, with the wildcard, the weak marker
/// and the comma-separated list RFC 7232 §3.1 admits.
pub fn unmet_delete_precondition(
    held: Option<&Object>,
    if_match: Option<&str>,
    if_match_last_modified_time: Option<SystemTime>,
    if_match_size: Option<i64>,
) -> Option<&'static str> {
    let match_fails = if_match.is_some_and(|expected| {
        held.is_none_or(|object| !entity_tag_matches(expected, &object.etag))
    });
    if match_fails {
        return Some(IF_MATCH_CONDITION);
    }
    let time_fails = if_match_last_modified_time.is_some_and(|expected| {
        held.is_some_and(|object| {
            whole_seconds(object.last_modified) != whole_seconds(expected)
        })
    });
    if time_fails {
        return Some(IF_MATCH_LAST_MODIFIED_TIME_CONDITION);
    }
    let size_fails = if_match_size.is_some_and(|expected| {
        held.is_some_and(|object| {
            i64::try_from(object.body.len()).unwrap_or(i64::MAX) != expected
        })
    });
    size_fails.then_some(IF_MATCH_SIZE_CONDITION)
}

/// The conditional READ member a request carries that does NOT hold against an object whose
/// entity tag is `etag` and which was last modified at `last_modified`, named by its header,
/// or `None` when every one of them holds.
///
/// The two conditions a read is REFUSED for, as `GetObjectRequest` documents them: "Return
/// the object only if its entity tag (ETag) is the same as the one specified in this header;
/// otherwise, return a 412 Precondition Failed error" (`$IfMatch`), and "Return the object
/// only if it has not been modified since the specified time; otherwise, return a 412
/// Precondition Failed error" (`$IfUnmodifiedSince`). The same member documents the
/// precedence when both are present: "If-Match condition evaluates to true, and;
/// If-Unmodified-Since condition evaluates to false; then, S3 returns 200 OK and the data
/// requested" — so an `If-Match` that holds decides the read on its own.
///
/// `If-Unmodified-Since` is compared at the one-second resolution an HTTP date has (RFC 9110
/// §5.6.7), so an object stored within the same second as the date it is judged against
/// counts as unmodified since it.
///
/// ONE place for the rule, so a GET and a HEAD of the same object cannot disagree about
/// whether its preconditions hold or about which header a refusal names.
pub fn unmet_read_precondition(
    etag: &str,
    last_modified: SystemTime,
    if_match: Option<&str>,
    if_unmodified_since: Option<SystemTime>,
) -> Option<&'static str> {
    if let Some(expected) = if_match {
        return (etag_value(etag) != etag_value(expected)).then_some(IF_MATCH_CONDITION);
    }
    if_unmodified_since
        .is_some_and(|since| whole_seconds(last_modified) > whole_seconds(since))
        .then_some(IF_UNMODIFIED_SINCE_CONDITION)
}

/// `instant` truncated to whole seconds since the epoch, the resolution an HTTP date carries
/// — `None` for an instant before the epoch, which no object this store holds was written
/// at.
fn whole_seconds(instant: SystemTime) -> Option<u64> {
    instant
        .duration_since(::std::time::UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

/// The source one `x-amz-copy-source` value names: the bucket, the key and the stored
/// version it addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopySource {
    /// The source bucket, URL-decoded.
    pub bucket: String,
    /// The source object's key, URL-decoded.
    pub key: String,
    /// The stored version the value's `?versionId=` named, or `None` when it named none —
    /// in which case the copy is of the key's current version.
    pub version_id: Option<String>,
}

/// Read an `x-amz-copy-source` value into the bucket, key and version it names, or report
/// why it names none.
///
/// "Specify the name of the source bucket and the key of the source object, separated by a
/// slash (/). For example, to copy the object `reports/january.pdf` from the general
/// purpose bucket `awsexamplebucket`, use `awsexamplebucket/reports/january.pdf`. The
/// value must be URL-encoded." A stored version is addressed by appending
/// `?versionId=<version-id>` to that value
/// (`smithy_query(query="doc",shape="CopyObjectRequest",member="CopySource")`), and the
/// member's own pattern (`^\/?.+\/.+$`) allows one leading slash, so the FIRST slash after
/// it separates the bucket from the key — a key with slashes of its own keeps them.
///
/// ONE place for the rule, so every operation that takes a copy source — the whole-object
/// copy and the part copy, which document the member identically — addresses exactly the
/// same objects and refuses exactly the same values.
pub fn parse_copy_source(value: &str) -> Result<CopySource, String> {
    let rejection = || {
        format!(
            "x-amz-copy-source must name the source bucket and the source object's key \
             separated by a slash, URL-encoded, optionally followed by \
             ?versionId=<version-id>, got {value:?}"
        )
    };
    // The value is URL-encoded, so a `?` in it is the query delimiter and a `?` in a key
    // arrives as `%3F` — which is what makes the FIRST one the end of the key, and makes
    // `versionId` the only subresource the value may carry.
    let (path, version_id) = match value.split_once('?') {
        None => (value, None),
        Some((path, query)) => match query.strip_prefix("versionId=") {
            Some(version_id) if !version_id.is_empty() => (path, Some(version_id)),
            _ => return Err(rejection()),
        },
    };
    let path = path.strip_prefix('/').unwrap_or(path);
    let Some((bucket, key)) = path.split_once('/') else {
        return Err(rejection());
    };
    if bucket.is_empty() || key.is_empty() {
        return Err(rejection());
    }
    let Some(bucket) = url_decoded(bucket) else {
        return Err(rejection());
    };
    let Some(key) = url_decoded(key) else {
        return Err(rejection());
    };
    let version_id = match version_id {
        None => None,
        Some(version_id) => match url_decoded(version_id) {
            Some(version_id) => Some(version_id),
            None => return Err(rejection()),
        },
    };
    Ok(CopySource {
        bucket,
        key,
        version_id,
    })
}

/// One percent-encoded element of a copy source, decoded — or `None` when its escapes do
/// not decode to UTF-8, which no bucket name or object key this store holds is.
fn url_decoded(value: &str) -> Option<String> {
    ::percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(::std::borrow::Cow::into_owned)
}

/// The prefix of the second documented `x-amz-copy-source` form — the ARN of an object "as
/// accessed through the access point", or through S3 on Outposts, which this emulator holds
/// none of.
pub const ACCESS_POINT_ARN_PREFIX: &str = "arn:";

/// Whether a copy source names an object by ARN rather than by `bucket/key`.
///
/// The one leading slash the member's pattern allows is stripped the way
/// [`parse_copy_source`] strips it, so a value is classified the same here and there.
pub fn copy_source_is_arn(value: &str) -> bool {
    value
        .strip_prefix('/')
        .unwrap_or(value)
        .starts_with(ACCESS_POINT_ARN_PREFIX)
}

/// The four copy-source conditional members one request carried, as
/// [`unmet_copy_source_precondition`] judges them.
///
/// Every operation that takes a copy source declares the same four
/// (`x-amz-copy-source-if-match`, `-if-none-match`, `-if-modified-since` and
/// `-if-unmodified-since`), so the values travel as one struct and the rule lives in one
/// place.
#[derive(Debug, Clone, Copy, Default)]
pub struct CopySourceConditions<'a> {
    pub if_match: Option<&'a str>,
    pub if_none_match: Option<&'a str>,
    /// The instant `x-amz-copy-source-if-modified-since` named, in whole seconds since the
    /// epoch — the unit the HTTP date it arrives as carries.
    pub if_modified_since: Option<i64>,
    /// The same for `x-amz-copy-source-if-unmodified-since`.
    pub if_unmodified_since: Option<i64>,
}

/// The copy-source precondition a request carries that does NOT hold against `source`,
/// named by its header, or `None` when every one of them holds.
///
/// The four members are evaluated in the order RFC 7232 gives them — an entity-tag
/// precondition first, and the date precondition beside it only when that tag member is
/// absent — which is exactly what the two combinations the members document describe:
/// "if both the x-amz-copy-source-if-match and x-amz-copy-source-if-unmodified-since
/// headers are present … x-amz-copy-source-if-match condition evaluates to true
/// [and] x-amz-copy-source-if-unmodified-since condition evaluates to false" answers 200
/// and copies, while "x-amz-copy-source-if-none-match condition evaluates to false [and]
/// x-amz-copy-source-if-modified-since condition evaluates to true" answers 412.
pub fn unmet_copy_source_precondition(
    conditions: &CopySourceConditions<'_>,
    source: &Object,
) -> Option<&'static str> {
    // The stored time is compared at whole-second granularity, because that is the
    // granularity of the HTTP-date these members carry and of the `Last-Modified` the
    // client read the value from.
    let modified_at = i64::try_from(whole_seconds(source.last_modified).unwrap_or_default())
        .unwrap_or(i64::MAX);
    if let Some(header) = conditions.if_match {
        if !entity_tag_matches(header, &source.etag) {
            return Some(COPY_SOURCE_IF_MATCH_CONDITION);
        }
    } else if let Some(since) = conditions.if_unmodified_since {
        // "Copies the object if it hasn't been modified since the specified time."
        if modified_at > since {
            return Some(COPY_SOURCE_IF_UNMODIFIED_SINCE_CONDITION);
        }
    }
    if let Some(header) = conditions.if_none_match {
        if entity_tag_matches(header, &source.etag) {
            return Some(COPY_SOURCE_IF_NONE_MATCH_CONDITION);
        }
    } else if let Some(since) = conditions.if_modified_since {
        // "Copies the object if it has been modified since the specified time."
        if modified_at <= since {
            return Some(COPY_SOURCE_IF_MODIFIED_SINCE_CONDITION);
        }
    }
    None
}

/// Whether an entity-tag precondition value names `etag`.
///
/// An `ETag` is reported inside quotes, and a client may send the value back with or
/// without them, as a comma-separated list, or as the [`ANY_ENTITY_TAG`] wildcard that names
/// any existing entity (RFC 7232 §2.3, §3.1) — so each candidate is compared on the tag's
/// own value ([`etag_value`]), with the weak marker stripped too.
pub fn entity_tag_matches(header: &str, etag: &str) -> bool {
    let stored = etag_value(etag);
    header.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == ANY_ENTITY_TAG || etag_value(candidate.trim_start_matches("W/")) == stored
    })
}

/// The same digest as [`object_etag`], Base64 encoded the way the `Content-MD5` header
/// carries it: "The Base64 encoded 128-bit MD5 digest of the data"
/// (`PutObjectRequest$ContentMD5`), which is what makes that header checkable.
pub fn content_md5(body: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(Md5::digest(body))
}

/// The `ChecksumType` of a checksum computed over the object's own bytes, whole.
///
/// One of the two values `ChecksumType` carries
/// (`smithy_query(query="shape",shape="ChecksumType")` publishes `[COMPOSITE,
/// FULL_OBJECT]`), and the one a single write's checksum has: there are no parts for it to
/// be composed of.
pub const CHECKSUM_TYPE_FULL_OBJECT: &str = "FULL_OBJECT";

/// The `ChecksumType` of a checksum computed over the CHECKSUMS of an object's parts —
/// "the checksum type … determines how part-level checksums are combined to create an
/// object-level checksum for multipart objects"
/// (`CompleteMultipartUploadRequest$ChecksumType`) — and the one a completed multipart
/// upload's checksum has unless the algorithm admits no such composition.
pub const CHECKSUM_TYPE_COMPOSITE: &str = "COMPOSITE";

/// The `ChecksumAlgorithm` values whose checksum is only ever over the whole object.
///
/// "The CRC64NVME checksum is always a full object checksum"
/// (`CompleteMultipartUploadRequest$ChecksumCRC64NVME`), so a multipart object checksummed
/// with it carries [`CHECKSUM_TYPE_FULL_OBJECT`] rather than a composition of its parts'
/// checksums.
pub const FULL_OBJECT_CHECKSUM_ALGORITHMS: [&str; 1] = ["CRC64NVME"];

/// The `ChecksumAlgorithm` values a multipart object may carry a
/// [`CHECKSUM_TYPE_FULL_OBJECT`] checksum under.
///
/// A multipart upload's object-level checksum is settled from its PARTS, so a full-object
/// value is only available for an algorithm whose part digests combine into the digest of
/// the whole object — the CRC-based ones. The Amazon S3 User Guide (*Checking object
/// integrity*) publishes full object checksums for exactly those (`CRC32`, `CRC32C` and
/// `CRC64NVME`) and composite checksums for the others, which is why a multipart upload
/// asking for `FULL_OBJECT` under any other algorithm names a pair S3 does not settle (see
/// [`settle_multipart_checksum_type`]).
pub const FULL_OBJECT_CAPABLE_CHECKSUM_ALGORITHMS: [&str; 3] = ["CRC32", "CRC32C", "CRC64NVME"];

/// The `ChecksumType` a multipart upload under `algorithm` is settled with when its
/// initiating request asked for `requested` (`None` when it asked for no type at all), or
/// `None` when the two name a combination S3 does not settle.
///
/// The rule, for every `ChecksumAlgorithm` the model publishes:
///
/// * an algorithm whose checksum "is always a full object checksum"
///   ([`FULL_OBJECT_CHECKSUM_ALGORITHMS`]) settles [`CHECKSUM_TYPE_FULL_OBJECT`], and
///   admits no other type;
/// * [`CHECKSUM_TYPE_FULL_OBJECT`] is settled for an algorithm whose parts' checksums
///   combine into the whole object's ([`FULL_OBJECT_CAPABLE_CHECKSUM_ALGORITHMS`]) when the
///   request asks for it, and for no other algorithm;
/// * every other algorithm settles [`CHECKSUM_TYPE_COMPOSITE`] — the composition over the
///   parts' checksums that "determines how part-level checksums are combined to create an
///   object-level checksum for multipart objects"
///   (`CreateMultipartUploadRequest$ChecksumType`).
///
/// ONE place for the rule, so the type an initiation reports and records, the type a later
/// completion composes the value under ([`multipart_checksum`]), and the type a reader of
/// the upload reports cannot disagree.
pub fn settle_multipart_checksum_type(
    algorithm: &str,
    requested: Option<&str>,
) -> Option<&'static str> {
    let always_full_object = FULL_OBJECT_CHECKSUM_ALGORITHMS.contains(&algorithm);
    let may_be_full_object = FULL_OBJECT_CAPABLE_CHECKSUM_ALGORITHMS.contains(&algorithm);
    match requested {
        Some(CHECKSUM_TYPE_FULL_OBJECT) => may_be_full_object.then_some(CHECKSUM_TYPE_FULL_OBJECT),
        Some(CHECKSUM_TYPE_COMPOSITE) => (!always_full_object).then_some(CHECKSUM_TYPE_COMPOSITE),
        // The request named no type at all: the one its algorithm settles.
        None if always_full_object => Some(CHECKSUM_TYPE_FULL_OBJECT),
        None => Some(CHECKSUM_TYPE_COMPOSITE),
        // A value outside the two the enum publishes is no type this settles, so it is
        // refused like any other pair it does not settle rather than folded into "the
        // request named none", which would settle a type the request never asked for.
        Some(_) => None,
    }
}

/// The RAW digest bytes the `ChecksumAlgorithm` value `algorithm` computes over `data`, or
/// `None` for a value this emulator has no implementation of.
///
/// Each algorithm is computed by a crate that implements it rather than by anything written
/// here: the five S3 has long published (`CRC32`, `CRC32C`, `CRC64NVME`, `SHA1`, `SHA256`)
/// through `aws-smithy-checksums`, the AWS SDK's own implementation of exactly these
/// checksums, and the others through the hash crate for each (`sha2` for `SHA512`, `md-5`
/// for `MD5`, `xxhash-rust` for the three `XXHASH` values). The bytes are the digest's own,
/// big-endian for the integer-valued ones, because that is what a Base64 checksum value and
/// a composition over part checksums are both built from.
fn checksum_digest(algorithm: &str, data: &[u8]) -> Option<Vec<u8>> {
    match algorithm {
        "CRC32" | "CRC32C" | "CRC64NVME" | "SHA1" | "SHA256" => {
            // The SDK crate names these in lowercase, which is the spelling its
            // `FromStr` accepts (and the spelling the header names carry).
            let parsed: ::aws_smithy_checksums::ChecksumAlgorithm =
                algorithm.to_ascii_lowercase().parse().ok()?;
            let mut checksum = parsed.into_impl();
            checksum.update(data);
            Some(checksum.finalize().to_vec())
        }
        "SHA512" => {
            use ::sha2::Digest as _;
            Some(::sha2::Sha512::digest(data).to_vec())
        }
        "MD5" => Some(Md5::digest(data).to_vec()),
        "XXHASH64" => Some(::xxhash_rust::xxh64::xxh64(data, 0).to_be_bytes().to_vec()),
        "XXHASH3" => Some(::xxhash_rust::xxh3::xxh3_64(data).to_be_bytes().to_vec()),
        "XXHASH128" => Some(::xxhash_rust::xxh3::xxh3_128(data).to_be_bytes().to_vec()),
        _ => None,
    }
}

/// The checksum of `data` under `algorithm`, as a response reports it: "the Base64 encoded
/// … checksum of the object" (every `Checksum*` member's own documentation), over the whole
/// of the object's bytes.
///
/// `None` for an algorithm [`checksum_digest`] has no implementation of, in which case
/// nothing is recorded and nothing is reported — never a value under another algorithm.
pub fn full_object_checksum(algorithm: &str, data: &[u8]) -> Option<String> {
    Some(::aws_smithy_types::base64::encode(checksum_digest(
        algorithm, data,
    )?))
}

/// The COMPOSITE checksum of an object assembled from `parts`, under `algorithm`.
///
/// The rule is the one the Amazon S3 User Guide states for a multipart object's
/// object-level checksum (*Checking object integrity*): S3 checksums each part, then
/// "concatenates the … checksums … and then calculates the checksum of these concatenated
/// values", reporting it "with the total number of parts" after a dash — the same
/// composition an entity tag of a multipart object is built with ([`multipart_object_etag`]),
/// over the request's checksum algorithm instead of MD5. So a COMPOSITE value is the
/// algorithm's digest of the parts' RAW digests, Base64 encoded, followed by `-<parts>`.
pub fn composite_checksum(algorithm: &str, parts: &[&[u8]]) -> Option<String> {
    let mut digests: Vec<u8> = Vec::new();
    for part in parts {
        digests.extend_from_slice(&checksum_digest(algorithm, part)?);
    }
    let combined = checksum_digest(algorithm, &digests)?;
    Some(format!(
        "{}-{}",
        ::aws_smithy_types::base64::encode(combined),
        parts.len()
    ))
}

/// The checksum one completed multipart upload's object carries: its value and its
/// `ChecksumType`, under the algorithm the upload was initiated with — or `None` when the
/// upload named no algorithm, or named one this emulator cannot compute, in which case the
/// object carries no checksum at all rather than one it did not compute.
///
/// The type is the one [`settle_multipart_checksum_type`] settles for the upload's
/// algorithm and the type it asked for — the same rule the initiation reported and recorded
/// — so an upload whose pair S3 does not settle carries no checksum rather than one under a
/// type it never agreed to. The value follows from the type — a composition over the parts,
/// or the digest of the whole assembled object — so the two can never describe different
/// computations.
pub fn multipart_checksum(
    algorithm: Option<&str>,
    requested_type: Option<&str>,
    parts: &[&[u8]],
) -> Option<ObjectChecksum> {
    let algorithm = algorithm?;
    let checksum_type = settle_multipart_checksum_type(algorithm, requested_type)?;
    let value = if checksum_type == CHECKSUM_TYPE_COMPOSITE {
        composite_checksum(algorithm, parts)?
    } else {
        full_object_checksum(algorithm, &parts.concat())?
    };
    Some(ObjectChecksum {
        algorithm: algorithm.to_string(),
        value,
        checksum_type: checksum_type.to_string(),
    })
}

/// The checksum one stored object was written with: which algorithm computed it, the value
/// a reader reports, and whether it was computed over the object's bytes or over its parts'
/// checksums.
///
/// Recorded by the write that stored the object, from the bytes it stored, so every later
/// read reports the same value — "this checksum is only present if the checksum was uploaded
/// with the object" (`Checksum*`'s own documentation), which is why an object written with no
/// checksum algorithm carries none of this and reports no checksum at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectChecksum {
    /// The `ChecksumAlgorithm` value the write named, which decides WHICH `Checksum*`
    /// member carries [`Self::value`].
    pub algorithm: String,
    /// The Base64 checksum, with the `-<parts>` suffix a composition carries.
    pub value: String,
    /// [`CHECKSUM_TYPE_FULL_OBJECT`] or [`CHECKSUM_TYPE_COMPOSITE`].
    pub checksum_type: String,
}

impl ObjectChecksum {
    /// How many bytes this record retains — the strings kept beside the object, which the
    /// per-bucket and service-wide budgets count the way they count every other string an
    /// object holds ([`ObjectEncryption::retained_bytes`] counts its own the same way).
    pub fn retained_bytes(&self) -> usize {
        self.algorithm.len() + self.value.len() + self.checksum_type.len()
    }
}

/// One part a completed multipart upload assembled an object from, as a read of the
/// object's attributes reports it.
///
/// The members `ObjectPart` declares for a general purpose bucket
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ObjectPart")`): "the part number
/// identifying the part", "the size of the uploaded part in bytes", and the part's own
/// checksum under the algorithm the upload was initiated with — nothing else of the part
/// survives the completion, because its bytes become the object's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPartRecord {
    /// "The part number identifying the part. This value is a positive integer between 1
    /// and 10,000" (`ObjectPart$PartNumber`) — the number the completion assembled it under.
    pub part_number: i32,
    /// "The size of the uploaded part in bytes" (`ObjectPart$Size`).
    pub size: i64,
    /// The Base64 checksum of this part's own bytes, under the object's checksum algorithm,
    /// or `None` when the upload named no algorithm — in which case a read of the object's
    /// attributes reports no `Part` element at all for a general purpose bucket ("if an
    /// additional checksum … isn't applied to the object specified in the request, the
    /// response doesn't return the Part element", `GetObjectAttributesParts$Parts`).
    pub checksum: Option<String>,
}

impl ObjectPartRecord {
    /// How many bytes this record retains: the checksum string kept with it, which the
    /// budgets count like every other string beside an object.
    pub fn retained_bytes(&self) -> usize {
        self.checksum.as_ref().map_or(0, String::len)
    }
}

/// One span of an object's bytes: the first and last byte INCLUSIVE, the way HTTP's
/// `Content-Range` and `Range` both state a span.
///
/// Non-empty by construction ([`ByteSpan::new`]), because an empty span is not a span HTTP
/// can state: `Content-Range` names a first and a last byte, so a response can only report
/// one that holds at least one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteSpan {
    /// The first byte of the span, counted from zero.
    first: usize,
    /// The LAST byte of the span, inclusive.
    last: usize,
}

impl ByteSpan {
    /// The span from `first` to `last` inclusive, or `None` when that is no span at all
    /// (`last` before `first`).
    pub fn new(first: usize, last: usize) -> Option<Self> {
        (first <= last).then_some(Self { first, last })
    }

    /// The first byte of the span.
    pub fn first(self) -> usize {
        self.first
    }

    /// The last byte of the span, inclusive.
    pub fn last(self) -> usize {
        self.last
    }

    /// How many bytes the span holds — what `Content-Length` reports for a response carrying
    /// it, since "Content-Length is the length of the body actually being returned".
    pub fn len(self) -> usize {
        self.last - self.first + 1
    }

    /// Never empty: see [`ByteSpan`].
    pub fn is_empty(self) -> bool {
        false
    }

    /// Those bytes of `body`, clamped to what it holds so a span can never read past it.
    pub fn slice(self, body: &[u8]) -> Vec<u8> {
        let first = self.first.min(body.len());
        let end = (self.last + 1).min(body.len());
        body[first..end].to_vec()
    }

    /// The `Content-Range` header value for this span of a representation of `total` bytes:
    /// `bytes <first>-<last>/<total>`, the one form RFC 9110 (HTTP Semantics, "Content-Range")
    /// defines for a `bytes` range.
    pub fn content_range(self, total: usize) -> String {
        format!("bytes {}-{}/{}", self.first, self.last, total)
    }
}

/// What a `Range` header asks of a representation of a given size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeRequest {
    /// The header asks for exactly this span.
    Span(ByteSpan),
    /// The header is well formed but asks for bytes the representation does not have, which
    /// is the one range condition HTTP answers with a refusal ([`INVALID_RANGE_CODE`]).
    Unsatisfiable,
    /// The header asks for nothing this emulator can answer as a single range — a form the
    /// `bytes` unit does not define, or several ranges at once, which "Amazon S3 doesn't
    /// support retrieving multiple ranges of data per `GET` request"
    /// (`GetObjectRequest$Range`). RFC 9110 lets a server "ignore a Range header field",
    /// which is what S3 does: the whole representation is served.
    Ignored,
}

/// What the `Range` header `header` asks of a representation of `total` bytes.
///
/// The `bytes` unit's three forms, from RFC 9110's "Range" grammar: `bytes=<first>-<last>`,
/// `bytes=<first>-` (to the end) and `bytes=-<suffix>` (the last `<suffix>` bytes). A
/// `<last>` past the end is clamped to it, because the span is "the remaining bytes of the
/// representation" in that case, and a suffix longer than the representation selects the
/// whole of it.
///
/// One place for the rule, so every reader that serves a ranged request ([`Object`]'s own
/// readers) answers the same span, the same refusal and the same ignored header.
pub fn requested_range(header: &str, total: usize) -> RangeRequest {
    let Some(set) = header.trim().strip_prefix("bytes=") else {
        // Another unit entirely: "An origin server MUST ignore a Range header field that
        // contains a range unit it does not understand" (RFC 9110).
        return RangeRequest::Ignored;
    };
    let mut spans = set.split(',');
    let (Some(single), None) = (spans.next(), spans.next()) else {
        return RangeRequest::Ignored;
    };
    let Some((first, last)) = single.trim().split_once('-') else {
        return RangeRequest::Ignored;
    };
    let (first, last) = (first.trim(), last.trim());
    match (first.parse::<usize>(), last.parse::<usize>()) {
        // `bytes=-<suffix>`: the last `suffix` bytes. A zero-length suffix selects nothing,
        // which is unsatisfiable, and so is any suffix of an empty representation.
        (Err(_), Ok(suffix)) if first.is_empty() => {
            if suffix == 0 || total == 0 {
                return RangeRequest::Unsatisfiable;
            }
            let first = total.saturating_sub(suffix);
            ByteSpan::new(first, total - 1).map_or(RangeRequest::Unsatisfiable, RangeRequest::Span)
        }
        // `bytes=<first>-` : from `first` to the end of the representation.
        (Ok(first), Err(_)) if last.is_empty() => {
            if first >= total {
                return RangeRequest::Unsatisfiable;
            }
            ByteSpan::new(first, total - 1).map_or(RangeRequest::Unsatisfiable, RangeRequest::Span)
        }
        // `bytes=<first>-<last>` : the span, with `last` clamped to the final byte. A
        // `<last>` BEFORE `<first>` makes the spec "syntactically invalid" (RFC 9110's
        // `byte-range-spec` grammar), which is a header to ignore rather than a range to
        // refuse: the refusal is for a valid range the representation cannot satisfy.
        (Ok(first), Ok(last)) => {
            if last < first {
                return RangeRequest::Ignored;
            }
            if first >= total {
                return RangeRequest::Unsatisfiable;
            }
            ByteSpan::new(first, last.min(total - 1))
                .map_or(RangeRequest::Unsatisfiable, RangeRequest::Span)
        }
        _ => RangeRequest::Ignored,
    }
}

/// The code S3 answers a read whose `Range` asks for bytes the object does not have:
/// `InvalidRange`, HTTP 416 — not a shape in this model, so it travels through the
/// unmodelled channel.
pub const INVALID_RANGE_CODE: &str = "InvalidRange";
/// See [`INVALID_RANGE_CODE`] — the status that code's entry in Amazon S3's "Error
/// responses" list publishes: "416 Requested Range Not Satisfiable".
pub const INVALID_RANGE_STATUS: u16 = 416;
/// The `<Message>` that same entry publishes — FIXED text, naming neither the range asked
/// for nor the object's size.
pub const INVALID_RANGE_MESSAGE: &str = "The requested range is not satisfiable";

/// The code S3 answers a read whose `partNumber` names no part of the object:
/// `InvalidPartNumber`, HTTP 416 — also unmodelled in this model, from the same published
/// list.
pub const INVALID_PART_NUMBER_CODE: &str = "InvalidPartNumber";
/// See [`INVALID_PART_NUMBER_CODE`].
pub const INVALID_PART_NUMBER_STATUS: u16 = 416;
/// The `<Message>` that code's entry publishes — FIXED text, which is why it names no part
/// number.
pub const INVALID_PART_NUMBER_MESSAGE: &str = "The requested partnumber is not satisfiable";

/// The value `accept-ranges` carries for an object S3 can serve byte ranges of: "Indicates
/// that a range of bytes was specified in the request" (`GetObjectOutput$AcceptRanges`).
///
/// The one range unit HTTP defines over a representation's bytes, and the unit
/// [`requested_range`] reads a `Range` header in — so the header a read ADVERTISES and the
/// unit a read is SERVED in cannot drift apart, and the two readers that advertise it
/// (`GetObject` and `HeadObject`) answer the same value.
pub const ACCEPT_RANGES_BYTES: &str = "bytes";


/// The `Checksum*` members a reader reports for a stored checksum: the one its algorithm
/// names, and the `ChecksumType` beside it. Every other member is `None`, because an object
/// carries the checksum of ONE algorithm — the one its write named.
///
/// One place for the algorithm-to-member mapping, so the four readers that report a stored
/// checksum (the write that computed it, `GetObject`, `HeadObject` and the completion of a
/// multipart upload) cannot map it differently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportedChecksums {
    pub crc32: Option<String>,
    pub crc32c: Option<String>,
    pub crc64nvme: Option<String>,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub sha512: Option<String>,
    pub md5: Option<String>,
    pub xxhash64: Option<String>,
    pub xxhash3: Option<String>,
    pub xxhash128: Option<String>,
    pub checksum_type: Option<String>,
}

/// `value` without the `-<parts>` suffix a COMPOSITE checksum carries.
///
/// A composition is reported "with the total number of parts" after a dash (Amazon S3 User
/// Guide, *Checking object integrity*) wherever the VALUE itself is reported — the
/// `x-amz-checksum-*` header of a write or a read, and the element a completion answers —
/// which is what [`composite_checksum`] renders. A read of the object's ATTRIBUTES is the
/// one place that reports the digest ALONE: `GetObjectAttributes` states how many parts the
/// object holds in `ObjectParts$TotalPartsCount` beside the checksum container, so the count
/// is not repeated inside the value there.
///
/// Only a trailing dash followed by digits is dropped: a Base64 checksum value is written in
/// the standard alphabet, which has no `-`, so no part of a digest can be mistaken for the
/// suffix.
pub fn checksum_without_part_count(value: &str) -> &str {
    match value.rsplit_once('-') {
        Some((digest, parts))
            if !parts.is_empty() && parts.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            digest
        }
        _ => value,
    }
}

/// The members a reader reports for `checksum`, or nothing at all for an object that
/// carries none — see [`ReportedChecksums`].
pub fn reported_checksums(checksum: Option<&ObjectChecksum>) -> ReportedChecksums {
    let Some(checksum) = checksum else {
        return ReportedChecksums::default();
    };
    let value = Some(checksum.value.clone());
    let mut reported = ReportedChecksums {
        checksum_type: Some(checksum.checksum_type.clone()),
        ..ReportedChecksums::default()
    };
    match checksum.algorithm.as_str() {
        "CRC32" => reported.crc32 = value,
        "CRC32C" => reported.crc32c = value,
        "CRC64NVME" => reported.crc64nvme = value,
        "SHA1" => reported.sha1 = value,
        "SHA256" => reported.sha256 = value,
        "SHA512" => reported.sha512 = value,
        "MD5" => reported.md5 = value,
        "XXHASH64" => reported.xxhash64 = value,
        "XXHASH3" => reported.xxhash3 = value,
        "XXHASH128" => reported.xxhash128 = value,
        // An algorithm no member of the model names cannot be reported under one; nothing
        // records such a checksum ([`checksum_digest`] computes none for it), so this is
        // unreachable rather than a silent drop.
        _ => return ReportedChecksums::default(),
    }
    reported
}

/// The `x-amz-checksum-*` request header one `ChecksumAlgorithm` value's digest arrives
/// on.
///
/// Derived rather than tabulated: every `Checksum*` member of this model binds
/// `x-amz-checksum-` followed by its algorithm in lowercase
/// (`smithy_query(query="operation",operation="PutObject")` reports `ChecksumSHA256 ->
/// x-amz-checksum-sha256`, `ChecksumCRC64NVME -> x-amz-checksum-crc64nvme`, and so on for
/// all ten), so a refusal names the header the value really arrived on and the two cannot
/// drift apart.
pub fn checksum_header_name(algorithm: &str) -> String {
    format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase())
}

/// The values of the ten `Checksum*` members one write declared — what
/// [`verify_declared_integrity`] judges.
///
/// One struct rather than a table per operation, because the algorithm each member stands
/// for is a property of the MODEL, not of the operation: every write that accepts these
/// headers (`PutObject`, `UploadPart`, `PutObjectAnnotation`) hands over the members it
/// deserialized and the mapping from member to algorithm lives here, in one place, so a
/// `ChecksumAlgorithm` value added to the model is added once.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeclaredChecksums<'a> {
    pub crc32: Option<&'a str>,
    pub crc32c: Option<&'a str>,
    pub crc64nvme: Option<&'a str>,
    pub sha1: Option<&'a str>,
    pub sha256: Option<&'a str>,
    pub sha512: Option<&'a str>,
    pub md5: Option<&'a str>,
    pub xxhash64: Option<&'a str>,
    pub xxhash3: Option<&'a str>,
    pub xxhash128: Option<&'a str>,
}

impl<'a> DeclaredChecksums<'a> {
    /// Each `ChecksumAlgorithm` value paired with the value its member carried, in the
    /// model's own member order (`smithy_query(query="operation",operation="PutObject")`:
    /// `ChecksumCRC32`, `ChecksumCRC32C`, `ChecksumCRC64NVME`, `ChecksumSHA1`,
    /// `ChecksumSHA256`, `ChecksumSHA512`, `ChecksumMD5`, `ChecksumXXHASH64`,
    /// `ChecksumXXHASH3`, `ChecksumXXHASH128`) — which is the order a request naming more
    /// than one of them is judged in.
    pub fn pairs(&self) -> [(&'static str, Option<&'a str>); 10] {
        [
            ("CRC32", self.crc32),
            ("CRC32C", self.crc32c),
            ("CRC64NVME", self.crc64nvme),
            ("SHA1", self.sha1),
            ("SHA256", self.sha256),
            ("SHA512", self.sha512),
            ("MD5", self.md5),
            ("XXHASH64", self.xxhash64),
            ("XXHASH3", self.xxhash3),
            ("XXHASH128", self.xxhash128),
        ]
    }

    /// How many of the ten members carried a value — what decides whether the write
    /// declared ONE checksum type or several, which S3 refuses
    /// ([`IntegrityRejection::MultipleChecksumTypes`]).
    pub fn declared_count(&self) -> usize {
        self.pairs()
            .into_iter()
            .filter(|(_, declared)| declared.is_some())
            .count()
    }

    /// The `ChecksumAlgorithm` value of the FIRST member that carried one, in that same
    /// order, or `None` when the write declared no individual checksum — which is what makes
    /// an individual checksum name the algorithm a write's own checksum is computed under
    /// ("if you provide an individual checksum, Amazon S3 ignores any provided
    /// ChecksumAlgorithm parameter", `PutObjectRequest$ChecksumAlgorithm`).
    pub fn named_algorithm(&self) -> Option<&'static str> {
        self.pairs()
            .into_iter()
            .find_map(|(algorithm, declared)| declared.map(|_| algorithm))
    }
}

/// Why a write's declared integrity value cannot be accepted.
///
/// Two classes, because a client branches on them differently: a value that cannot
/// POSSIBLY be a digest of the named width is a malformed request (fix the client), while a
/// well-formed digest that is not the digest of the bytes received says the bytes are not
/// the bytes the client hashed (recompute and retry the same payload). Neither code is a
/// shape in this model, so a handler renders one through the unmodelled channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityRejection {
    /// An `x-amz-checksum-*` value that does not Base64-decode, or decodes to a width the
    /// named algorithm's digest does not have.
    MalformedChecksum {
        /// The `ChecksumAlgorithm` value whose member carried it.
        algorithm: String,
    },
    /// A well-formed digest that is not the one the algorithm computes over the payload.
    ChecksumMismatch {
        /// The `ChecksumAlgorithm` value whose member carried it.
        algorithm: String,
    },
    /// A `Content-MD5` that does not Base64-decode to a 128-bit digest.
    MalformedContentMd5,
    /// More than one `x-amz-checksum-*` header on one write: each names a checksum TYPE for
    /// the same bytes, and S3 computes and stores exactly one per object, so a request
    /// carrying several asks for something no stored object can report.
    MultipleChecksumTypes,
    /// A well-formed `Content-MD5` that is not the MD5 of the payload.
    ContentMd5Mismatch,
}

impl IntegrityRejection {
    /// The wire error code S3 answers this rejection with.
    ///
    /// A malformed `x-amz-checksum-*` header is `InvalidRequest` — "a parameter or header
    /// in your request isn't valid" — and a malformed `Content-MD5` is `InvalidDigest`, the
    /// code S3's *Error responses* list publishes for that header specifically. Either
    /// value failing the COMPARISON is `BadDigest`, the code that list publishes for a
    /// digest that does not describe the bytes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::MalformedChecksum { .. } | Self::MultipleChecksumTypes => INVALID_REQUEST_CODE,
            Self::MalformedContentMd5 => INVALID_DIGEST_CODE,
            Self::ChecksumMismatch { .. } | Self::ContentMd5Mismatch => BAD_DIGEST_CODE,
        }
    }

    /// The HTTP status every one of them is rendered with: 400, from that same list.
    pub fn status(&self) -> u16 {
        BAD_DIGEST_STATUS
    }
}

impl std::fmt::Display for IntegrityRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The header the value arrived on is the only variable part: S3 names the
            // header it could not read and nothing about the value or the width expected.
            Self::MalformedChecksum { algorithm } => write!(
                f,
                "Value for {} header is invalid.",
                checksum_header_name(algorithm)
            ),
            // The algorithm whose digest did not match, as the `ChecksumAlgorithm` enum
            // spells it, which is how S3 names it in this sentence.
            Self::ChecksumMismatch { algorithm } => write!(
                f,
                "The {algorithm} you specified did not match the calculated checksum."
            ),
            Self::MalformedContentMd5 => write!(f, "{INVALID_DIGEST_MESSAGE}"),
            Self::ContentMd5Mismatch => write!(f, "{CONTENT_MD5_MISMATCH_MESSAGE}"),
            Self::MultipleChecksumTypes => write!(f, "{MULTIPLE_CHECKSUM_TYPES_MESSAGE}"),
        }
    }
}

/// The code S3 answers a `Content-MD5` that is not a Base64 128-bit digest at all:
/// "InvalidDigest — The Content-MD5 or checksum value that you specified is not valid"
/// (Amazon S3 *Error responses*, list of error codes), HTTP 400.
pub const INVALID_DIGEST_CODE: &str = "InvalidDigest";

/// See [`INVALID_DIGEST_CODE`] — the text that list publishes for it.
pub const INVALID_DIGEST_MESSAGE: &str =
    "The Content-MD5 or checksum value that you specified is not valid.";

/// The `<Message>` S3 sends with [`INVALID_REQUEST_CODE`] for a write that declared more
/// than one `x-amz-checksum-*` header: "Expecting a single x-amz-checksum- header. Multiple
/// checksum Types are not allowed."
///
/// Fixed text that names the header PREFIX rather than any of the members the request
/// carried, so nothing of the request is interpolated into it.
pub const MULTIPLE_CHECKSUM_TYPES_MESSAGE: &str =
    "Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed.";

/// The `<Message>` S3 sends with [`BAD_DIGEST_CODE`] for a `Content-MD5` that IS a digest
/// but not this payload's: fixed text, with no variable part — it names neither the digest
/// declared nor the one computed.
pub const CONTENT_MD5_MISMATCH_MESSAGE: &str =
    "The Content-MD5 you specified did not match what we received.";

/// Whether the integrity values one write declared describe the bytes it carried.
///
/// ONE rule for every write that accepts them, so the code and the sentence a client
/// branches on are the same whichever operation and whichever header carried the value:
/// each value's SHAPE is judged first — a digest of the wrong width cannot be the digest of
/// anything, so it is refused before any comparison — and only then is it compared against
/// the digest of the payload actually received. `Content-MD5` is judged first because it is
/// the integrity check HTTP itself defines over the body.
///
/// A declared algorithm this emulator cannot compute ([`checksum_digest`] implements every
/// value the model's `ChecksumAlgorithm` publishes) is left unjudged rather than refused: no
/// comparison is possible, so neither answer would be a true statement about the value.
pub fn verify_declared_integrity(
    content_md5: Option<&str>,
    checksums: &DeclaredChecksums<'_>,
    body: &[u8],
) -> Result<(), IntegrityRejection> {
    // One write declares ONE checksum type: "you can only specify one checksum algorithm per
    // request" (`PutObjectRequest$ChecksumAlgorithm` names the single algorithm a write's
    // checksum is computed under), and a stored object carries one checksum
    // ([`Object::checksum`]), so several `x-amz-checksum-*` headers at once ask for something
    // no object can report. Judged before any value is read, because it is a property of the
    // request alone.
    if checksums.declared_count() > 1 {
        return Err(IntegrityRejection::MultipleChecksumTypes);
    }
    if let Some(declared) = content_md5 {
        // "The Base64 encoded 128-bit MD5 digest of the data" (`PutObjectRequest$ContentMD5`)
        // — so a value that is not Base64 at all, or is Base64 of anything other than 16
        // bytes, is not a digest the header may carry.
        let decoded = ::aws_smithy_types::base64::decode(declared.trim())
            .map_err(|_| IntegrityRejection::MalformedContentMd5)?;
        if decoded.len() != MD5_DIGEST_BYTES {
            return Err(IntegrityRejection::MalformedContentMd5);
        }
        if decoded != Md5::digest(body).as_slice() {
            return Err(IntegrityRejection::ContentMd5Mismatch);
        }
    }
    for (algorithm, declared) in checksums.pairs() {
        let Some(declared) = declared else { continue };
        let Some(expected) = checksum_digest(algorithm, body) else {
            continue;
        };
        let decoded = ::aws_smithy_types::base64::decode(declared.trim()).map_err(|_| {
            IntegrityRejection::MalformedChecksum {
                algorithm: algorithm.to_string(),
            }
        })?;
        if decoded.len() != expected.len() {
            return Err(IntegrityRejection::MalformedChecksum {
                algorithm: algorithm.to_string(),
            });
        }
        if decoded != expected {
            return Err(IntegrityRejection::ChecksumMismatch {
                algorithm: algorithm.to_string(),
            });
        }
    }
    Ok(())
}

/// How many bytes an MD5 digest is wide: "the Base64 encoded 128-bit MD5 digest of the
/// data" (`PutObjectRequest$ContentMD5`), which is 16 bytes.
const MD5_DIGEST_BYTES: usize = 128 / 8;

/// Why the `Checksum*` members a write declared for a whole MULTIPART object cannot be
/// accepted, or `None` when they describe `computed` — the checksum the assembled object
/// really carries ([`multipart_checksum`]).
///
/// The same rule [`verify_declared_integrity`] applies to the bytes of a one-shot write,
/// over the value a COMPOSITION produces instead: a completion "can be used as a data
/// integrity check to verify that the data received is the same data that was originally
/// sent" (`CompleteMultipartUploadRequest$ChecksumCRC64NVME` and its siblings), and the
/// digest the object carries is the one the initiation's algorithm and type settled — so a
/// declared value that is not that one, and a value declared under any OTHER algorithm, is
/// a digest the object does not have. Either is [`BAD_DIGEST_CODE`], the code that class
/// publishes for a digest that does not describe what the service holds.
///
/// Declaring several at once is refused before any comparison, exactly as it is on a
/// one-shot write: one object carries one checksum.
pub fn verify_declared_object_checksum(
    declared: &DeclaredChecksums<'_>,
    computed: Option<&ObjectChecksum>,
) -> Result<(), ObjectChecksumRejection> {
    if declared.declared_count() > 1 {
        return Err(ObjectChecksumRejection::MultipleChecksumTypes);
    }
    for (algorithm, value) in declared.pairs() {
        let Some(value) = value else { continue };
        let describes = computed.is_some_and(|checksum| {
            checksum.algorithm == algorithm && checksum.value == value.trim()
        });
        if !describes {
            return Err(ObjectChecksumRejection::Mismatch {
                algorithm: algorithm.to_string(),
            });
        }
    }
    Ok(())
}

/// Why the object checksum a COMPLETION declared cannot be accepted — see
/// [`verify_declared_object_checksum`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectChecksumRejection {
    /// A declared value that is not the digest the assembled object carries.
    Mismatch {
        /// The `ChecksumAlgorithm` value whose member carried it.
        algorithm: String,
    },
    /// More than one `x-amz-checksum-*` header on one completion, which asks for something
    /// no stored object can report — the condition
    /// [`IntegrityRejection::MultipleChecksumTypes`] states for a one-shot write, worded
    /// from the same place.
    MultipleChecksumTypes,
}

impl ObjectChecksumRejection {
    /// The wire error code S3 answers this rejection with: the codes
    /// [`IntegrityRejection`] answers the same two conditions with on a one-shot write.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Mismatch { .. } => BAD_DIGEST_CODE,
            Self::MultipleChecksumTypes => INVALID_REQUEST_CODE,
        }
    }

    /// The HTTP status both are rendered with: 400, from that same published list.
    pub fn status(&self) -> u16 {
        BAD_DIGEST_STATUS
    }
}

impl std::fmt::Display for ObjectChecksumRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The algorithm as the HEADER the value arrived on spells it — lowercase —
            // which is how a COMPLETION names it; a one-shot write's own mismatch names the
            // `ChecksumAlgorithm` enum value instead
            // ([`IntegrityRejection::ChecksumMismatch`]), and the two sentences are
            // otherwise the same words.
            Self::Mismatch { algorithm } => write!(
                f,
                "The {} you specified did not match the calculated checksum.",
                algorithm.to_ascii_lowercase()
            ),
            Self::MultipleChecksumTypes => write!(f, "{MULTIPLE_CHECKSUM_TYPES_MESSAGE}"),
        }
    }
}

/// The one `EncodingType` the model declares: `url`
/// (`smithy_query(query="shape",shape="EncodingType")` reports exactly one value), which
/// is also the one the AWS CLI sends on every listing.
pub const ENCODING_TYPE_URL: &str = "url";

/// The characters percent-encoding LEAVES ALONE: RFC 3986's unreserved set,
/// `A-Z a-z 0-9 - _ . ~`.
///
/// Everything else in a key name is encoded as `%XX` per UTF-8 code value, which is what
/// `encoding-type=url` asks for: "non-ASCII characters that are used in an object's key
/// name will be percent-encoded according to UTF-8 code values. For example, the object
/// `test_file(3).png` will appear as `test_file%283%29.png`" (the `EncodingType` enum's
/// own documentation) — so even an ASCII character outside that set (`(`, `)`, and the
/// `/` of a key prefix) is encoded, which is why the set is subtractive rather than a
/// hand-written list of characters to escape.
const KEY_UNRESERVED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The characters a LISTING leaves literal in a key-name element it writes under
/// `encoding-type=url`: [`KEY_UNRESERVED`]'s set plus the `/` that separates the elements of
/// a key prefix, which S3 reports as itself so a prefix reads as the prefix it is. The space
/// is not here — it is written `+` — see [`listing_encoded_key`].
const LISTING_KEY_LITERAL: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// `value` as a URI PATH carries it: percent-encoded per UTF-8 code value, leaving RFC
/// 3986's unreserved set alone.
///
/// What a URI reads a key out of — the path of the `Location` a completed multipart upload
/// reports, and the `rule-id` of an `x-amz-expiration` header ("the value of the rule-id is
/// URL-encoded", `GetObjectOutput$Expiration`). A LISTING writes its key-name elements in the
/// other form S3 uses ([`listing_encoded_key`]), which is not this one.
pub fn url_encoded_key(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, KEY_UNRESERVED).to_string()
}

/// `value` as a listing reports it when the request asked for `encoding-type=url`.
///
/// The elements this applies to are the ones `ListObjectsV2Output$EncodingType` names —
/// "returns encoded key name values in the following response elements: Delimiter,
/// Prefix, Key, and StartAfter" — plus the same key-name elements of the other listings
/// (`ListObjectVersions`' markers and its `CommonPrefixes`). A listing that asked for no
/// encoding type reports every one of them verbatim.
///
/// The form is the one S3 answers with, which is a FORM encoding rather than a path one:
/// a character outside the unreserved set becomes `%XX` per UTF-8 code value — "the object
/// `test_file(3).png` will appear as `test_file%283%29.png`" (the `EncodingType` enum's own
/// documentation) — a SPACE becomes `+`, and the `/` that separates the elements of a key
/// prefix is left as it is. The two exceptions are what makes the element decodable back to
/// the stored key by the plus-aware unquoting every SDK applies to it (botocore's
/// `decode_list_object_v2` passes each key-name element through `unquote_plus`), and they
/// are what a recorded listing of keys carrying a space and a `/` shows.
pub fn listing_encoded_key(value: &str) -> String {
    value
        .split(' ')
        .map(|run| {
            ::percent_encoding::utf8_percent_encode(run, LISTING_KEY_LITERAL).to_string()
        })
        .collect::<Vec<String>>()
        .join("+")
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
/// store's monotonic mint sequence, rendered as the [`VERSION_ID_CHARS`] characters an S3
/// version id is shaped like. Two versions of a key — and two versions anywhere in the
/// store — therefore never collide, and the value stays what it was once stored, because
/// nothing re-derives it.
///
/// Hex, so every character is an ASCII alphanumeric: the id survives a round trip through
/// the `versionId` query parameter a client sends it back in, where `+` would arrive
/// decoded as a space and `/` would end the path segment, and it carries only characters
/// [`version_id_is_well_formed`] accepts.
pub fn mint_version_id(bucket: &str, key: &str, sequence: u64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(bucket.as_bytes());
    // A separator, so ("a", "bc") and ("ab", "c") cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(key.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    // Each byte renders as two hex digits, so half of `VERSION_ID_CHARS` digest bytes
    // fill the id exactly.
    ::hex::encode(&digest.finalize()[..VERSION_ID_CHARS / 2])
}

/// Per-service state for com-amazonaws-s3-amazons3. A handler reaches it through [`service_state`].
#[derive(Debug)]
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
    /// How many lifecycle rule identifiers this store has minted.
    ///
    /// Only ever increases, so [`Self::mint_lifecycle_rule_id`] cannot hand out the same
    /// identifier twice however many configurations are put — on one bucket or across them.
    minted_lifecycle_rule_ids: u64,
    /// How many multipart upload ids this store has minted, ever.
    ///
    /// The sequence [`mint_upload_id`] derives each id from, so two uploads of one key —
    /// and two uploads anywhere in the store — can never carry the same id, whatever the
    /// clock does. Only [`Self::create_multipart_upload`] advances it.
    minted_upload_ids: u64,
    /// What the objects AWS publishes ([`aws_published`]) retain, and how many stored
    /// versions they are, measured ONCE when the store was seeded with them.
    ///
    /// The whole-store ceilings ([`MAX_OBJECTS_TOTAL`] / [`MAX_OBJECT_BYTES_TOTAL`]) bound
    /// what this process holds because a CALLER asked it to, and the published catalog is
    /// fixed by the code rather than by anything a caller does — so this fixed amount is
    /// subtracted from the store's footprint ([`Self::caller_retained_bytes`] /
    /// [`Self::caller_stored_versions`]) instead of a whole bucket being skipped. Measured
    /// rather than exempted per bucket so that bytes a caller writes INTO a published
    /// bucket count against those budgets exactly as they would anywhere else.
    ///
    /// Written once, by [`Self::record_published_buckets`]; nothing else moves it, so it is
    /// the CEILING on what the budgets subtract — [`Self::published_allowance`] takes the
    /// smaller of it and what those buckets hold now.
    published_retained_bytes: usize,
    /// How many stored versions the published catalog is. See
    /// [`Self::published_retained_bytes`].
    published_versions: usize,
    /// The sessions `CreateSession` has vended and not released, as `credential id ->
    /// the instant it expires`.
    ///
    /// Kept because a session has a LIFETIME: "temporary security credentials are scoped to
    /// the bucket and expire after 5 minutes" (`CreateSession`'s own documentation), and a
    /// credential nothing releases would be accepted for signing for as long as the process
    /// lives. [`Self::release_expired_sessions`] reports the ones the clock has already
    /// expired so the vending operation can forget them, and
    /// [`Self::record_vended_session`] bounds how many this process holds at once.
    ///
    /// Private: the two methods below are the only way in or out, so the quota and the
    /// release cannot be applied in one place and skipped in another.
    vended_sessions: BTreeMap<String, SystemTime>,
    /// The names of the metadata tables that exist in the managed table bucket's namespace
    /// ([`METADATA_TABLE_NAMESPACE`]).
    ///
    /// ONE namespace for the whole store, because that is its scope: the managed table
    /// bucket is one per Region and every bucket's metadata tables are created inside it, so
    /// a table name is taken or free for the store rather than for one bucket.
    ///
    /// Kept apart from the bucket's own configuration because its SCOPE is different, and
    /// because a table OUTLIVES it: the name of a table is taken or free for the whole
    /// namespace, so a create asks this set rather than the bucket whether the name it would
    /// provision under is free — and provisions into [`METADATA_TABLE_STATUS_FAILED`] with
    /// [`METADATA_JOURNAL_TABLE_EXISTS_CODE`] when a table already holds it. Deleting the
    /// configuration that created a table does not delete the table (the published failure
    /// above asks the caller to delete it through the table API that owns it), and neither
    /// does deleting the general purpose bucket, so no operation of this service takes a name
    /// out of this set; a ceiling is what bounds it
    /// ([`MAX_METADATA_NAMESPACE_TABLES`]).
    metadata_namespace_tables: BTreeSet<String>,
}

impl Default for ComAmazonawsS3Amazons3State {
    /// The store a fresh engine hands this service: empty of everything a caller writes,
    /// and already holding the buckets AWS publishes ([`aws_published`]).
    ///
    /// Those buckets are what the store STARTS with because that is what publishing one
    /// means: no client creates `sagemaker-sample-files`, and a client that follows a
    /// documented path reads sample data out of it before it creates a bucket of its own.
    /// Seeding them here — the one place a store comes into being — is what lets every
    /// reader resolve them through the one bucket map, with no operation carrying a case
    /// for a managed name.
    fn default() -> Self {
        let mut store = Self {
            buckets: BTreeMap::new(),
            minted_versions: 0,
            minted_notification_configuration_ids: 0,
            minted_lifecycle_rule_ids: 0,
            minted_upload_ids: 0,
            published_retained_bytes: 0,
            published_versions: 0,
            vended_sessions: BTreeMap::new(),
            metadata_namespace_tables: BTreeSet::new(),
        };
        store.record_published_buckets();
        store
    }
}

impl ComAmazonawsS3Amazons3State {
    /// Forget every session whose expiry the clock has already passed, reporting the
    /// credential ids so the caller can delete them from the credential store too.
    ///
    /// A session "expire[s] after 5 minutes. After the expiration time, any calls that you
    /// make with those credentials will fail" (`CreateSession`'s own documentation), so this
    /// materialises only what the CLOCK already made due: the release was owed at the same
    /// instant whether or not anybody called. Nothing else about the store moves.
    pub fn release_expired_sessions(&mut self, now: SystemTime) -> Vec<String> {
        let expired: Vec<String> = self
            .vended_sessions
            .iter()
            .filter(|(_, expires_at)| **expires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.vended_sessions.remove(id);
        }
        expired
    }

    /// Record the session `id` as vended until `expires_at`, or report that this process
    /// already holds as many sessions as it keeps at once ([`MAX_VENDED_SESSIONS`]).
    ///
    /// The bound is this emulator's, not an AWS quota: a caller may ask for a session as
    /// often as it likes, and each one is a credential this process has to keep accepting
    /// until it expires — so the count is what keeps that memory bounded whatever a caller
    /// does. Read and written under the one guard the caller already holds, so two
    /// concurrent vends cannot both pass a bound only one of them fits in.
    pub fn record_vended_session(&mut self, id: &str, expires_at: SystemTime) -> bool {
        if self.vended_sessions.len() >= MAX_VENDED_SESSIONS {
            return false;
        }
        self.vended_sessions.insert(id.to_string(), expires_at);
        true
    }

    /// Whether `access_key_id` names a session this service vended and has not released —
    /// so a request presenting it presents a credential the emulator ISSUED.
    pub fn holds_vended_session(&self, access_key_id: &str) -> bool {
        self.vended_sessions.contains_key(access_key_id)
    }

    /// Whether the managed table bucket's namespace already holds a metadata table named
    /// `table` — see [`Self::metadata_namespace_tables`].
    ///
    /// What a create of a metadata configuration asks before it provisions a table: a name
    /// already in the namespace is one S3 cannot create a second table under, which is the
    /// `*AlreadyExists` provisioning failure the configuration then reports.
    pub fn metadata_namespace_holds(&self, table: &str) -> bool {
        self.metadata_namespace_tables.contains(table)
    }

    /// How many more tables the managed table bucket's namespace has room for
    /// ([`MAX_METADATA_NAMESPACE_TABLES`]).
    ///
    /// Nothing releases a table — it outlives the configuration that created it
    /// ([`Self::delete_metadata_configuration`]) — so a caller creating configurations for
    /// bucket after bucket would grow the namespace without bound; what keeps it bounded is
    /// this ceiling, measured against the names a create would ADD before it records any of
    /// them: a create that would carry the namespace past the maximum is refused instead, with
    /// nothing recorded. Read under the guard the caller already holds, so two concurrent
    /// creates cannot both fit in room only one of them has.
    pub fn metadata_namespace_room(&self) -> usize {
        MAX_METADATA_NAMESPACE_TABLES.saturating_sub(self.metadata_namespace_tables.len())
    }

    /// Record `table` as a table of the managed table bucket's namespace.
    ///
    /// Called for each table a create really provisions, so the namespace holds exactly the
    /// tables this store has created, for as long as this store lives: no operation of this
    /// service deletes a table from it. A name the namespace already holds is already
    /// recorded, so recording it again changes nothing.
    ///
    /// The CEILING is the caller's: [`MAX_METADATA_NAMESPACE_TABLES`] is measured through
    /// [`Self::metadata_namespace_room`] against the names a create would add, before any of
    /// them is recorded and under the same guard — one place owning the bound, rather than a
    /// second check here that the caller would have to interpret.
    pub fn record_metadata_namespace_table(&mut self, table: &str) {
        self.metadata_namespace_tables.insert(table.to_string());
    }

    /// Take the S3 Metadata configuration of the bucket named `bucket` away, reporting
    /// whether this store holds that bucket at all — the one outcome the caller answers
    /// differently; a bucket carrying no configuration has nothing to delete and is already
    /// in the asked-for state, so it reports the same `true` a delete that removed one does.
    ///
    /// The CONFIGURATION is what goes: the tables it provisioned stay in the managed table
    /// bucket's namespace, because a metadata table is a table of an S3 Tables bucket and is
    /// deleted through the table API that owns it — "a journal table already exists in the
    /// Amazon Web Services managed table bucket's namespace. Delete the journal table, and
    /// then try again. To create a new metadata table, you must delete the metadata
    /// configuration for this bucket, and then create a new metadata configuration"
    /// ([`METADATA_JOURNAL_TABLE_EXISTS_MESSAGE`], the provisioning failure the published V2
    /// list carries) states both halves: the configuration delete is a PRECONDITION for a new
    /// configuration, and the leftover table is the separate deletion a caller still owes. So
    /// a create of the same configuration afterwards is accepted and its journal table
    /// provisions into [`METADATA_TABLE_STATUS_FAILED`] carrying that code, exactly as a
    /// create colliding with any other table of the namespace does. Nothing in this service's
    /// API takes a table out of the namespace, which is why it is bounded by a ceiling
    /// ([`MAX_METADATA_NAMESPACE_TABLES`]) rather than by what a delete releases.
    pub fn delete_metadata_configuration(&mut self, bucket: &str) -> bool {
        let Some(held) = self.buckets.get_mut(bucket) else {
            return false;
        };
        held.clear_metadata_configuration();
        true
    }

    /// Record `configuration_state` as the inventory table state of the S3 Metadata
    /// configuration the general purpose bucket `bucket` carries, provisioning or dropping the
    /// inventory table behind it and keeping the managed table bucket's namespace in step.
    ///
    /// The ONE way in, because an inventory table is a table of that namespace rather than of
    /// the bucket: `UpdateBucketMetadataInventoryTableConfiguration` really does PROVISION one
    /// — `s3tables:CreateTable` is among the permissions that operation requires, and none of
    /// the permissions it lists deletes a table (its own documentation) — so the name an
    /// enable creates a table under joins the namespace and stays there, exactly as the names
    /// a create provisions do ([`Self::record_metadata_namespace_table`]). An enable whose
    /// name the namespace ALREADY holds therefore provisions nothing: the table is recorded
    /// [`METADATA_TABLE_STATUS_FAILED`] with the published failure for the name being taken,
    /// the same verdict a create colliding with that name reports
    /// ([`metadata_inventory_table_failure`], the one rule both paths read). Enabling the
    /// table that is already there is not a provisioning at all, so it leaves both the table
    /// and the namespace exactly as they are.
    pub fn update_metadata_inventory_table(
        &mut self,
        bucket: &str,
        configuration_state: String,
        now: SystemTime,
    ) -> MetadataInventoryTableUpdate {
        let Some(held) = self.buckets.get(bucket) else {
            return MetadataInventoryTableUpdate::NoSuchBucket;
        };
        let Some(configuration) = held.metadata_configuration() else {
            return MetadataInventoryTableUpdate::NoConfiguration;
        };
        let table_name = metadata_table_name(bucket, METADATA_INVENTORY_TABLE_SUFFIX);
        // Only an enable that finds the configuration carrying no table of its own provisions
        // one; the journal table it would read from is the configuration's own.
        let provisions = configuration_state == METADATA_CONFIGURATION_STATE_ENABLED
            && configuration
                .inventory
                .as_ref()
                .is_none_or(|inventory| inventory.table.is_none());
        let failure = provisions
            .then(|| {
                metadata_inventory_table_failure(
                    self.metadata_namespace_holds(&table_name),
                    configuration.journal.failure.is_some(),
                )
            })
            .flatten();
        // A table that could not be provisioned adds nothing to the namespace: either the name
        // is already held by the table that is there, or no table was created under it. Only a
        // table this really creates takes room, measured against the ceiling under the guard
        // the caller already holds — the way the create measures the names it would add — and
        // an enable that would carry the namespace past the maximum writes NOTHING.
        let takes_room = provisions && failure.is_none();
        if takes_room && self.metadata_namespace_room() == 0 {
            return MetadataInventoryTableUpdate::NamespaceFull;
        }
        let Some(held) = self.buckets.get_mut(bucket) else {
            // Unreachable: the lookup above ran against this same map, under this same guard.
            return MetadataInventoryTableUpdate::NoSuchBucket;
        };
        if !held.update_metadata_inventory_table(configuration_state, &table_name, failure, now) {
            // Unreachable for the same reason: the configuration was read above.
            return MetadataInventoryTableUpdate::NoConfiguration;
        }
        if takes_room {
            self.record_metadata_namespace_table(&table_name);
        }
        MetadataInventoryTableUpdate::Recorded
    }

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

    /// Every bucket of the CALLER'S OWN S3 account, as `(name, bucket)` in bucket-name
    /// order.
    ///
    /// The read seam for any operation that reports "the buckets in your account" — S3's
    /// own `ListBuckets` and the peer services that report the account's buckets. It is
    /// [`Self::buckets`] LESS the buckets another emulated service provisioned as the
    /// object store of one of its own resources ([`Bucket::provisioned_by`]): those are
    /// served by this service's object data plane, because that is what they are for, but
    /// they are not buckets of the caller's S3 account — AWS holds a Lightsail bucket in a
    /// service-managed account, so it is not in the account's own `ListBuckets` either.
    pub fn account_buckets(&self) -> impl Iterator<Item = (&str, &Bucket)> + '_ {
        self.buckets()
            .filter(|(_, bucket)| bucket.provisioned_by.is_none() && !bucket.aws_published)
    }

    /// How many stored versions the caller-stored half of this store holds, which is what
    /// [`MAX_OBJECTS_TOTAL`] bounds: every version in the store, less the published
    /// catalog's own ([`Self::published_allowance`]).
    fn caller_stored_versions(&self) -> usize {
        let (_, versions) = self.published_allowance();
        self.buckets
            .values()
            .map(Bucket::stored_versions)
            .sum::<usize>()
            .saturating_sub(versions)
    }

    /// How many bytes the caller-stored half of this store retains, which is what
    /// [`MAX_OBJECT_BYTES_TOTAL`] bounds: everything the store retains, less the published
    /// catalog's own ([`Self::published_allowance`]).
    fn caller_retained_bytes(&self) -> usize {
        let (bytes, _) = self.published_allowance();
        self.buckets
            .values()
            .map(Bucket::retained_bytes)
            .sum::<usize>()
            .saturating_sub(bytes)
    }

    /// What the whole-store budgets subtract for the published catalog, as
    /// `(bytes, versions)`: the seeded amount ([`Self::published_retained_bytes`]) or what
    /// the published buckets hold NOW, whichever is SMALLER.
    ///
    /// Both directions matter, which is why it is neither the cached amount alone nor the
    /// live footprint alone. A caller that writes into a published bucket pushes the live
    /// footprint ABOVE the seeded amount, so the subtraction stays at the seeded amount and
    /// everything it wrote counts against the budgets like a write anywhere else. A caller
    /// that deletes a published object pushes it BELOW, so the subtraction shrinks with it
    /// and no room is handed out for bytes the store no longer holds.
    fn published_allowance(&self) -> (usize, usize) {
        let (mut bytes, mut versions) = (0usize, 0usize);
        for bucket in self.buckets.values().filter(|held| held.aws_published) {
            bytes += bucket.retained_bytes();
            versions += bucket.stored_versions();
        }
        (
            bytes.min(self.published_retained_bytes),
            versions.min(self.published_versions),
        )
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
        self.record_object(bucket, key, object, None)
    }

    /// Store the REPLICA `object` at `key` in the bucket named `bucket`, under
    /// `replicated_version` — the id of the SOURCE version it is a copy of — rather than an
    /// id of its own.
    ///
    /// A replica keeps its source's version id: the copy is the same version of the same
    /// object in another bucket, which is what lets a reader address it there by the id the
    /// source reported. Everything else is [`Self::put_object`]'s: the same per-bucket and
    /// service-wide ceilings bound a replica, so a destination that is full refuses the copy
    /// rather than growing past them — which is what the source version then reports as a
    /// replication that did not happen (see [`Self::replicate_write`]).
    pub fn put_replica(
        &mut self,
        bucket: &str,
        key: &str,
        object: Object,
        replicated_version: Option<&str>,
    ) -> Option<ObjectWrite> {
        self.record_object(bucket, key, object, replicated_version)
    }

    /// [`Self::put_object`] and [`Self::put_replica`] in one: `replicated_version` is the
    /// version id a REPLICA keeps, and `None` asks for the id the bucket's versioning state
    /// calls for.
    fn record_object(
        &mut self,
        bucket: &str,
        key: &str,
        object: Object,
        replicated_version: Option<&str>,
    ) -> Option<ObjectWrite> {
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
        // version there is kept rather than replaced. Measured over the CALLER-stored half
        // of the store ([`Self::caller_retained_bytes`]), which is what these budgets
        // bound; a write into a bucket AWS publishes counts here like any other.
        let held_versions: usize = self.caller_stored_versions();
        if (keeps || !held) && held_versions >= MAX_OBJECTS_TOTAL {
            return Some(ObjectWrite::TotalObjectCountReached);
        }
        // The id is minted before the budgets are measured, so the bytes it retains are
        // counted; a refusal below leaves it unused, which costs nothing but the sequence
        // number it consumed (ids are opaque, and nothing numbers them). A REPLICA mints
        // none: it keeps the id of the source version it copies.
        let minted = match replicated_version {
            Some(replicated) => Some(replicated.to_string()),
            None => versioned.then(|| {
                self.minted_versions = self.minted_versions.saturating_add(1);
                mint_version_id(bucket, key, self.minted_versions)
            }),
        };
        let incoming = object.retained_bytes() + minted.as_ref().map_or(0, String::len);
        // The same allowance the five other whole-store checks subtract
        // ([`Self::published_allowance`]), so this path hands out no room for published
        // bytes the store no longer holds either.
        let (published_bytes, _) = self.published_allowance();
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
            .saturating_sub(published_bytes)
            + incoming;
        if would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectWrite::TotalByteBudgetReached { would_hold });
        }

        // Then the per-bucket ceilings, which also perform the write.
        self.buckets
            .get_mut(bucket)
            .map(|held| held.put_object(key, object, minted))
    }

    /// Carry out the replication the bucket named `bucket` owes for the version its last
    /// write stored at `key`, reporting what that version now carries — or `None` when no
    /// enabled rule of that bucket covers the key, which is the state of every object in a
    /// bucket with no replication configuration (and so of every object whose reader reports
    /// no `ReplicationStatus` at all).
    ///
    /// What it does, for each destination `state::replication::destinations` names: the
    /// stored version is copied there byte-identically, under the same key and the same
    /// version id ([`Self::put_replica`]), where it is a replica. The source version then
    /// carries [`REPLICATION_STATUS_COMPLETED`] when every one of those copies was made and
    /// [`REPLICATION_STATUS_FAILED`] when one could not be — a destination this emulator
    /// does not hold, one whose versioning is not enabled (replication copies object
    /// versions, so a destination without them cannot hold one), or one already at a
    /// ceiling. Either way the status is reported from
    /// [`REPLICATION_SETTLES_AFTER`] past the write ([`ObjectReplication::status_at`]).
    ///
    /// Called by the write that recorded the version, AFTER the store accepted it, so a
    /// refused write replicates nothing. A REPLICA is never replicated again, because the
    /// copy is written through [`Self::put_replica`] — which records it and replicates
    /// nothing itself — rather than through the write path that calls this: a configuration
    /// pointing back at the source therefore cannot copy a replica home.
    ///
    /// The WORK one write may do here is bounded, because both of its factors are a
    /// caller's to choose: how many destinations the stored configuration names and how
    /// large the object is. At most
    /// [`MAX_REPLICATION_DESTINATIONS_PER_WRITE`] copies are made and at most
    /// [`MAX_REPLICATION_BYTES_PER_WRITE`] bytes are copied in one call, and the FIRST copy
    /// the store refuses ends the fan-out rather than letting every further destination pay
    /// for a copy that cannot be stored. A write past either ceiling records
    /// [`REPLICATION_STATUS_FAILED`], which is what the source version then reports: the
    /// copies past it were not made.
    pub fn replicate_write(&mut self, bucket: &str, key: &str) -> Option<ObjectReplication> {
        // The tag set alone, because that is all the SELECTION needs: a bucket with no
        // configuration — and a key no rule covers — pays no copy of the object at all.
        let tags = self.bucket(bucket)?.object(key)?.tags.clone();
        let destinations = replication::destinations(self, bucket, key, &tags);
        if destinations.is_empty() {
            return None;
        }
        let asked = destinations.len();
        let source_version = self.bucket(bucket)?.object(key)?.clone();
        let body_bytes = source_version.body.len();

        // More destinations than one write replicates to is work this emulator does not do:
        // the copies past the ceiling are not made, and the source version says so.
        let mut every_copy_made = asked <= MAX_REPLICATION_DESTINATIONS_PER_WRITE;
        let mut copied_bytes = 0usize;
        for target in destinations
            .into_iter()
            .take(MAX_REPLICATION_DESTINATIONS_PER_WRITE)
        {
            // A destination this emulator cannot hold a version in replicates nothing: a
            // bucket it does not hold, one whose versioning is not enabled, and the SOURCE
            // bucket itself (a bucket cannot be its own destination, which would replace
            // the very version being replicated with a replica of it).
            let writable = self
                .bucket(&target.bucket)
                .is_some_and(Bucket::versioning_enabled);
            if !writable || target.bucket == bucket {
                every_copy_made = false;
                continue;
            }
            // The byte ceiling is consulted BEFORE the copy is made, so the bytes this one
            // call copies are bounded whatever the configuration asks for.
            if copied_bytes.saturating_add(body_bytes) > MAX_REPLICATION_BYTES_PER_WRITE {
                every_copy_made = false;
                break;
            }
            let replica = replication::replica_of(
                &source_version,
                target.storage_class.as_deref(),
                SystemTime::now(),
            );
            let written = self.put_replica(
                &target.bucket,
                key,
                replica,
                source_version.version_id.as_deref(),
            );
            if matches!(
                written,
                Some(ObjectWrite::Created) | Some(ObjectWrite::Replaced)
            ) {
                copied_bytes = copied_bytes.saturating_add(body_bytes);
            } else {
                // A store that cannot hold this copy cannot hold the next one either, so
                // the fan-out ends here instead of copying a body per remaining
                // destination only to have each one refused.
                every_copy_made = false;
                break;
            }
        }

        let settled = if every_copy_made {
            REPLICATION_STATUS_COMPLETED
        } else {
            REPLICATION_STATUS_FAILED
        };
        let record = ObjectReplication::source(settled, source_version.last_modified);
        self.buckets
            .get_mut(bucket)?
            .addressed_object_mut(key, source_version.version_id.as_deref())?
            .replication = Some(record.clone());
        Some(record)
    }

    /// Whether the bucket named `bucket` carries a replication configuration at all — the
    /// ONE question a caller with many keys asks once, instead of asking after each of them
    /// what a bucket that replicates nothing owes.
    ///
    /// Read-only, and cheap: it looks at the stored container rather than at any rule.
    pub fn replicates_objects(&self, bucket: &str) -> bool {
        self.bucket(bucket)
            .and_then(Bucket::replication)
            .is_some()
    }

    /// Replicate the DELETE MARKER a delete just laid on `key` in the bucket named
    /// `bucket`, reporting how many destinations it was laid in.    ///
    /// "Specifies whether Amazon S3 replicates delete markers"
    /// (`DeleteMarkerReplication$Status`): a marker goes only to the destinations of the
    /// covering rules that ask for it, so a rule carrying no `DeleteMarkerReplication` — or
    /// one whose status is not `Enabled` — leaves its destination unmarked. The marker laid
    /// in the destination is that bucket's own ([`Self::remove_object`] mints its id there),
    /// because a delete marker carries no bytes to copy.
    ///
    /// Called by the delete that laid the marker. The PERMANENT delete of one version never
    /// reaches here, which is the published rule: a delete naming a version id is not
    /// replicated. A batch delete asks [`Self::replicates_objects`] once and calls this only
    /// for a bucket that has a configuration at all, so a bucket with none pays nothing per
    /// key; and one call marks at most [`MAX_REPLICATION_DESTINATIONS_PER_WRITE`]
    /// destinations, the same ceiling a write's fan-out is bounded by.
    pub fn replicate_delete_marker(&mut self, bucket: &str, key: &str) -> usize {
        let marked: Vec<String> = replication::destinations(self, bucket, key, &[])
            .into_iter()
            .filter(|target| target.delete_markers && target.bucket != bucket)
            .map(|target| target.bucket)
            .take(MAX_REPLICATION_DESTINATIONS_PER_WRITE)
            .collect();
        let mut laid = 0usize;
        for destination in marked {
            if !self
                .bucket(&destination)
                .is_some_and(Bucket::versioning_enabled)
            {
                continue;
            }
            if matches!(
                self.remove_object(&destination, key),
                Some(ObjectDeletion::MarkerLaid { .. })
            ) {
                laid += 1;
            }
        }
        laid
    }

    /// Record `upload` as a new multipart upload in progress in the bucket named
    /// `bucket`, reporting what that did and the id the upload is addressed by — or
    /// `None` when this emulator holds no such bucket (so an initiation refuses a name
    /// that is not a bucket instead of creating one).
    ///
    /// This is also where the UPLOAD ID is minted, because this is the only place that
    /// sees the store's mint sequence: "This action initiates a multipart upload and
    /// returns an upload ID" (`CreateMultipartUpload`'s own documentation), and the id is
    /// recorded with the upload, so every later reader reports the id the upload really
    /// is addressed by rather than one re-derived from the request. Two initiations of one
    /// key are two uploads, each with its own id — "You specify this upload ID in each of
    /// your subsequent upload part requests" — so nothing here replaces an upload the
    /// bucket already has in progress.
    ///
    /// This is the ONLY way an upload is recorded, because it is the only place that can
    /// see BOTH the per-bucket ceiling ([`MAX_MULTIPART_UPLOADS_PER_BUCKET`]) and the
    /// service-wide one ([`MAX_MULTIPART_UPLOADS_TOTAL`]), which together bound the
    /// uploads this process holds whatever a caller does. Nothing is written unless the
    /// answer is [`MultipartUploadCreation::Initiated`]: an over-long key, metadata past
    /// [`MAX_OBJECT_METADATA_BYTES`] and either ceiling are each refused here, leaving
    /// every recorded upload exactly as it was.
    pub fn create_multipart_upload(
        &mut self,
        bucket: &str,
        upload: MultipartUpload,
    ) -> Option<MultipartUploadCreation> {
        let held_bucket = self.buckets.get(bucket)?;
        if upload.key.len() > MAX_OBJECT_KEY_BYTES {
            return Some(MultipartUploadCreation::KeyTooLong {
                bytes: upload.key.len(),
            });
        }
        let metadata_bytes = upload.metadata_bytes();
        if metadata_bytes > MAX_OBJECT_METADATA_BYTES {
            return Some(MultipartUploadCreation::MetadataTooLarge {
                bytes: metadata_bytes,
            });
        }
        if held_bucket.multipart_uploads.len() >= MAX_MULTIPART_UPLOADS_PER_BUCKET {
            return Some(MultipartUploadCreation::UploadCountReached);
        }
        let held_total: usize = self
            .buckets
            .values()
            .map(|held| held.multipart_uploads.len())
            .sum();
        if held_total >= MAX_MULTIPART_UPLOADS_TOTAL {
            return Some(MultipartUploadCreation::TotalUploadCountReached);
        }

        self.minted_upload_ids = self.minted_upload_ids.saturating_add(1);
        let upload_id = mint_upload_id(bucket, &upload.key, self.minted_upload_ids);
        self.buckets
            .get_mut(bucket)?
            .multipart_uploads
            .insert(upload_id.clone(), upload);
        Some(MultipartUploadCreation::Initiated { upload_id })
    }

    /// Record `part` against the multipart upload `upload_id` names in the bucket named
    /// `bucket`, reporting what that did — or `None` when this emulator holds no such
    /// bucket (so a part upload refuses a name that is not a bucket instead of creating
    /// one).
    ///
    /// An upload is addressed by its id AND the key it was initiated for, so a request
    /// naming the id under another key names no upload
    /// ([`PartUpload::NoSuchUpload`]) — the same address a read of the upload is answered
    /// by, so neither can reach an upload the other cannot.
    ///
    /// A part REPLACES what its number held: "If you upload a new part using the same part
    /// number that was used with a previous part, the previously uploaded part is
    /// overwritten" (`UploadPart`'s own documentation), and the bytes the overwritten part
    /// retained are released with it.
    ///
    /// This is the ONLY way a part is recorded, because it is the only place that can see
    /// BOTH the per-bucket byte budget ([`MAX_OBJECT_BYTES_PER_BUCKET`]) and the
    /// service-wide one ([`MAX_OBJECT_BYTES_TOTAL`]) — the same two budgets a stored
    /// object is measured against, since a retained part holds this process's memory the
    /// same way. Nothing is written unless the answer is [`PartUpload::Stored`].
    pub fn upload_part(
        &mut self,
        bucket: &str,
        upload_id: &str,
        key: &str,
        part: UploadedPart,
    ) -> Option<PartUpload> {
        let held_bucket = self.buckets.get(bucket)?;
        let Some(upload) = held_bucket
            .multipart_upload(upload_id)
            .filter(|upload| upload.key == key)
        else {
            return Some(PartUpload::NoSuchUpload);
        };

        // What this write releases: the part the number already held, whose bytes go away
        // as it is overwritten — so the budgets are over what the store WOULD hold, not
        // over everything ever uploaded.
        let released = upload
            .part(part.part_number)
            .map_or(0, UploadedPart::retained_bytes);
        let incoming = part.retained_bytes();

        let would_hold_in_bucket = held_bucket.retained_bytes().saturating_sub(released) + incoming;
        if would_hold_in_bucket > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(PartUpload::ByteBudgetReached {
                would_hold: would_hold_in_bucket,
            });
        }
        // The whole-store budget measures the caller-stored half of the store
        // ([`Self::caller_retained_bytes`]); a part uploaded into a bucket AWS publishes
        // counts against it like any other.
        let would_hold: usize = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + incoming;
        if would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(PartUpload::TotalByteBudgetReached { would_hold });
        }

        let replaced = self
            .buckets
            .get_mut(bucket)?
            .multipart_uploads
            .get_mut(upload_id)?
            .parts
            .insert(part.part_number, part)
            .is_some();
        Some(PartUpload::Stored { replaced })
    }

    /// Discard the multipart upload `upload_id` names in the bucket named `bucket`,
    /// reporting what that did — or `None` when this emulator holds no such bucket (so an
    /// abort refuses a name that is not a bucket instead of reporting success for it).
    ///
    /// An upload is addressed by its id AND the key it was initiated for, exactly as
    /// [`Self::upload_part`] and a read of the upload address it, so an id named under
    /// another key names no upload ([`UploadAbort::NoSuchUpload`]) and nothing is removed.
    ///
    /// The upload goes away with every part it held — "the storage consumed by any
    /// previously uploaded parts will be freed" (`AbortMultipartUpload`'s own
    /// documentation) — so the bytes those parts retained stop counting against the
    /// bucket's budget ([`Bucket::retained_bytes`]) and the upload stops being listed. ONE
    /// upload is discarded: every other upload the bucket has in progress, including
    /// another upload of the same key, is left exactly as it was.
    pub fn abort_multipart_upload(
        &mut self,
        bucket: &str,
        upload_id: &str,
        key: &str,
    ) -> Option<UploadAbort> {
        let held_bucket = self.buckets.get_mut(bucket)?;
        if held_bucket
            .multipart_upload(upload_id)
            .is_none_or(|upload| upload.key != key)
        {
            return Some(UploadAbort::NoSuchUpload);
        }
        held_bucket.multipart_uploads.remove(upload_id);
        Some(UploadAbort::Discarded)
    }

    /// Turn the multipart upload `upload_id` names in the bucket named `bucket` into
    /// `object` at `key`, reporting what that did — or `None` when this emulator holds no
    /// such bucket (so a completion refuses a name that is not a bucket instead of creating
    /// one).
    ///
    /// An upload is addressed by its id AND the key it was initiated for, exactly as
    /// [`Self::upload_part`] and [`Self::abort_multipart_upload`] address it, so an id named
    /// under another key names no upload ([`UploadCompletion::NoSuchUpload`]) and nothing is
    /// written.
    ///
    /// The upload is taken out BEFORE the object is written, because the parts BECOME the
    /// object: "upon receiving this request, Amazon S3 concatenates all the parts in
    /// ascending order by part number to create a new object"
    /// (`CompleteMultipartUpload`'s own documentation), and the storage the parts held is
    /// not held twice. That is also what keeps the completion inside the byte budgets a
    /// store this size has room for: the bytes the parts retained are released as the same
    /// bytes are recorded under the key.
    ///
    /// The write goes through [`Self::put_object`], the one place an object is recorded, so
    /// a completed upload is subject to exactly the ceilings a one-shot write is and mints
    /// its version id the same way. A write that is REFUSED leaves the upload in progress
    /// with every part it held, so a caller can retry it rather than losing the parts.
    pub fn complete_multipart_upload(
        &mut self,
        bucket: &str,
        upload_id: &str,
        key: &str,
        object: Object,
    ) -> Option<UploadCompletion> {
        let held_bucket = self.buckets.get_mut(bucket)?;
        if held_bucket
            .multipart_upload(upload_id)
            .is_none_or(|upload| upload.key != key)
        {
            return Some(UploadCompletion::NoSuchUpload);
        }
        let Some(upload) = held_bucket.multipart_uploads.remove(upload_id) else {
            return Some(UploadCompletion::NoSuchUpload);
        };

        let write = self.put_object(bucket, key, object);
        match write {
            Some(write @ (ObjectWrite::Created | ObjectWrite::Replaced)) => {
                Some(UploadCompletion::Completed(write))
            }
            // Either refusal leaves the upload where it was: the bucket is reached again
            // because the write borrowed the whole store, and the upload goes back under
            // the id it was addressed by. The bucket is still held — the write above was
            // measured against it and nothing in between removes a bucket — so the upload
            // really does go back; the conditional is what keeps a store that somehow no
            // longer held it from panicking on a request path, there being nothing left to
            // put the upload into.
            refused => {
                if let Some(held_bucket) = self.buckets.get_mut(bucket) {
                    held_bucket
                        .multipart_uploads
                        .insert(upload_id.to_string(), upload);
                }
                refused.map(UploadCompletion::Refused)
            }
        }
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

    /// Record `rules` as the whole lifecycle configuration of the bucket named `bucket`,
    /// under the `transition_default_minimum_object_size` behaviour the put applied,
    /// reporting `None` when this emulator holds no such bucket (so a put refuses a name
    /// that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT, not a merge: "this will overwrite an existing lifecycle
    /// configuration, so if you want to retain any configuration details, they must be
    /// included in the new lifecycle configuration"
    /// (`PutBucketLifecycleConfiguration`'s own documentation), so what the bucket carries
    /// afterwards is exactly what this request carried.
    ///
    /// Every rule that arrives without an `ID` is given one here, because the member is the
    /// rule's "unique identifier" and a read reports one for every rule. A minted
    /// identifier is recorded with the configuration, so every later read reports the same
    /// one, and it never collides with an identifier the request named itself.
    pub fn put_lifecycle_configuration(
        &mut self,
        bucket: &str,
        mut rules: Vec<crate::ops::types::LifecycleRule>,
        transition_default_minimum_object_size: String,
    ) -> Option<()> {
        if !self.buckets.contains_key(bucket) {
            return None;
        }

        let mut taken: BTreeSet<String> = rules.iter().filter_map(|rule| rule.id.clone()).collect();
        for rule in &mut rules {
            if rule.id.is_none() {
                let id = self.mint_lifecycle_rule_id(&taken);
                taken.insert(id.clone());
                rule.id = Some(id);
            }
        }

        self.buckets.get_mut(bucket)?.lifecycle = Some(BucketLifecycle {
            rules,
            transition_default_minimum_object_size,
        });
        Some(())
    }

    /// An opaque rule identifier no rule of this configuration carries.
    ///
    /// S3's own assigned identifiers are opaque strings, so this is derived the way
    /// [`Self::mint_notification_configuration_id`] derives a notification id: the MD5
    /// digest of the store's monotonic counter, rendered as hex. Uniqueness comes from the
    /// counter, not from the digest, and an identifier a request named itself (`taken`) is
    /// stepped over rather than reused.
    fn mint_lifecycle_rule_id(&mut self, taken: &BTreeSet<String>) -> String {
        loop {
            self.minted_lifecycle_rule_ids += 1;
            let mut digest = Md5::new();
            digest.update(b"s3-lifecycle-rule-id:");
            digest.update(self.minted_lifecycle_rule_ids.to_be_bytes());
            let id = hex::encode(digest.finalize());
            if !taken.contains(&id) {
                return id;
            }
        }
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
            .caller_retained_bytes()
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

    /// Replace the Object Lock retention of the object version `version_id` names in
    /// `bucket` — the key's current object when it names none — with `retention`, or take
    /// the retention away when that is `None`. Reports what the write did, or `None` when
    /// this emulator holds no such bucket (so a retention write refuses a name that is not
    /// a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::replace_object_tags`]
    /// replaces a tag set: `Retention` is "the container element for the Object Retention
    /// configuration" (`PutObjectRetentionRequest$Retention`), so what the version carries
    /// afterwards is exactly what this call carried and nothing of the configuration
    /// before it.
    ///
    /// This decides NOTHING about whether the write is ALLOWED: whether a retention in
    /// force bars it, and whether the caller asked to bypass a governance one, is the
    /// handler's judgement over [`ObjectRetention::in_force_at`] and
    /// [`ObjectRetention::weakens`] — this is the storage seam those answers end at.
    ///
    /// It is the ONLY way an object's retention is recorded, because it is the only place
    /// that can see BOTH the per-bucket byte budget and the service-wide one: a retention
    /// is retained with the object ([`Object::retained_bytes`]) and is not bounded by
    /// anything [`Self::put_object`] checked. Nothing is written unless the answer is
    /// [`ObjectRetentionWrite::Applied`].
    pub fn replace_object_retention(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        retention: Option<ObjectRetention>,
    ) -> Option<ObjectRetentionWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectRetentionWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this retention in place of
        // the version's current one: everything they retain now, less the bytes the record
        // being replaced holds, plus the bytes of the replacement.
        let released = object
            .retention
            .as_ref()
            .map_or(0, ObjectRetention::retained_bytes);
        let taken = retention
            .as_ref()
            .map_or(0, ObjectRetention::retained_bytes);
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectRetentionWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectRetentionWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.retention = retention;
                ObjectRetentionWrite::Applied
            })
    }

    /// Record `encryption` as the server-side encryption configuration of the object
    /// version `version_id` names in `bucket` — the key's current object when it names
    /// none. Reports what the write did, or `None` when this emulator holds no such bucket
    /// (so an encryption update refuses a name that is not a bucket instead of creating
    /// one).
    ///
    /// A REPLACEMENT of the object's whole encryption record, the way
    /// [`Self::replace_object_retention`] replaces a retention: `ObjectEncryption` is "the
    /// updated server-side encryption type for this object"
    /// (`UpdateObjectEncryptionRequest$ObjectEncryption`), so what the version carries
    /// afterwards is exactly what this call carried. Nothing ELSE about the object changes:
    /// "the UpdateObjectEncryption operation preserves all object metadata properties,
    /// including the storage class, creation date, last modified date, ETag, and checksum
    /// properties" (that operation's own documentation).
    ///
    /// It is the ONLY way an existing object's encryption is re-recorded, because it is the
    /// only place that can see BOTH the per-bucket byte budget and the service-wide one: an
    /// encryption configuration is retained with the object
    /// ([`ObjectEncryption::retained_bytes`]). Nothing is written unless the answer is
    /// [`ObjectEncryptionWrite::Applied`].
    pub fn replace_object_encryption(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        encryption: ObjectEncryption,
    ) -> Option<ObjectEncryptionWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectEncryptionWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this configuration in place
        // of the object's current one, measured exactly as a retention replacement is.
        let released = object.encryption.retained_bytes();
        let taken = encryption.retained_bytes();
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectEncryptionWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self.caller_retained_bytes().saturating_sub(released) + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectEncryptionWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.encryption = encryption;
                ObjectEncryptionWrite::Applied
            })
    }

    /// Record `legal_hold` as the Object Lock legal hold status of the object version
    /// `version_id` names in `bucket` — the key's current object when it names none.
    /// Reports what the write did, or `None` when this emulator holds no such bucket (so a
    /// legal hold write refuses a name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::replace_object_retention`]
    /// replaces a retention: `LegalHold` is the "container element for the legal hold
    /// configuration you want to apply to the specified object"
    /// (`PutObjectLegalHoldRequest$LegalHold`), which holds one status, so what the version
    /// carries afterwards is exactly the status this call named.
    ///
    /// No byte budget is consulted, unlike the retention seam: what is kept is a flag, not
    /// a caller-supplied string, so a hold adds nothing to what the bucket retains.
    pub fn replace_object_legal_hold(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        legal_hold: bool,
    ) -> Option<ObjectLegalHoldWrite> {
        let held = self.buckets.get_mut(bucket)?;
        let Some(object) = held.addressed_object_mut(key, version_id) else {
            return Some(ObjectLegalHoldWrite::NoSuchObject);
        };
        object.legal_hold = Some(legal_hold);
        Some(ObjectLegalHoldWrite::Applied)
    }

    /// Replace the access control list of the object version `version_id` names in
    /// `bucket` — the key's current object when it names none — with `grants`. Reports what
    /// the write did, or `None` when this emulator holds no such bucket (so an ACL write
    /// refuses a name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the whole subresource, the way [`Self::replace_object_tags`]
    /// replaces a tag set: `PutObjectAcl` "uses the acl subresource to set the access
    /// control list (ACL) permissions for a new or existing object" from the ONE ACL the
    /// request names, so what the version carries afterwards is exactly those grants and
    /// nothing of the ACL before it.
    ///
    /// This decides NOTHING about whether the write is ALLOWED: whether the bucket's Object
    /// Ownership setting disables ACLs ([`Bucket::acls_disabled`]) and whether its Block
    /// Public Access settings bar a public one ([`acl_grants_public_access`]) are the
    /// handler's judgement — this is the storage seam those answers end at.
    ///
    /// It is the ONLY way an object's ACL is recorded, because it is the only place that
    /// can see BOTH the per-bucket byte budget and the service-wide one: an ACL is retained
    /// with the object ([`Object::retained_bytes`]) and is not bounded by anything
    /// [`Self::put_object`] checked. Nothing is written unless the answer is
    /// [`ObjectAclWrite::Applied`].
    pub fn replace_object_acl(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        grants: Vec<AclGrant>,
    ) -> Option<ObjectAclWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectAclWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this ACL in place of the
        // version's current one: everything they retain now, less the bytes the ACL being
        // replaced holds, plus the bytes of the replacement.
        let released = acl_grants_bytes(object.acl.as_deref().unwrap_or_default());
        let taken = acl_grants_bytes(&grants);
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectAclWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self.caller_retained_bytes().saturating_sub(released) + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectAclWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.acl = Some(grants);
                ObjectAclWrite::Applied
            })
    }

    /// Record `restore` on the object version `version_id` names in `bucket` — the key's
    /// current object when it names none. Reports what the write did, or `None` when this
    /// emulator holds no such bucket (so a restore refuses a name that is not a bucket
    /// instead of creating one).
    ///
    /// A REPLACEMENT of the whole record, the way [`Self::replace_object_retention`]
    /// replaces a retention: a second `RestoreObject` on a version states the lifetime and
    /// the tier of the copy afresh, so what the version carries afterwards is exactly what
    /// this call carried.
    ///
    /// It decides NOTHING about whether the restore is ALLOWED: whether the version is
    /// archived at all, and whether a retrieval is already running on it, is the handler's
    /// judgement over [`ARCHIVED_STORAGE_CLASSES`] and [`ObjectRestore::in_progress_at`] —
    /// this is the storage seam those answers end at. It restores no BYTES either: an
    /// archived version's bytes are the ones this store already holds, and what a restore
    /// changes is the state every reader reports through `x-amz-restore`.
    ///
    /// It is the ONLY way a restore is recorded, because it is the only place that can see
    /// BOTH the per-bucket byte budget and the service-wide one: the record is retained
    /// with the object ([`Object::retained_bytes`]) and is not bounded by anything
    /// [`Self::put_object`] checked. Nothing is written unless the answer is
    /// [`ObjectRestoreWrite::Applied`].
    pub fn restore_object_version(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        restore: ObjectRestore,
    ) -> Option<ObjectRestoreWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(ObjectRestoreWrite::NoSuchObject);
        };

        // What the bucket and the whole store would retain with this record in place of the
        // version's current one: everything they retain now, less the bytes the record
        // being replaced holds, plus the bytes of the replacement.
        let released = object
            .restore
            .as_ref()
            .map_or(0, ObjectRestore::retained_bytes);
        let taken = restore.retained_bytes();
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(ObjectRestoreWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(ObjectRestoreWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.restore = Some(restore);
                ObjectRestoreWrite::Applied
            })
    }

    /// Record `annotation` under `name` on the object version `version_id` names in
    /// `bucket` — the key's current object when it names none. Reports what the write did,
    /// or `None` when this emulator holds no such bucket (so an annotation write refuses a
    /// name that is not a bucket instead of creating one).
    ///
    /// A REPLACEMENT of the one annotation `name` addresses, and of nothing else: an
    /// annotation is "a named payload … that you can associate with a specific object or
    /// object version" (`PutObjectAnnotation`'s own documentation), so a second write under
    /// the same name is the whole new payload of that name and leaves every other
    /// annotation of the object where it was.
    ///
    /// It decides NOTHING about whether the name or the payload is acceptable: the name's
    /// bounds are [`annotation_name_rejection`]'s judgement and the payload's are the
    /// handler's over [`MIN_ANNOTATION_PAYLOAD_BYTES`] and
    /// [`MAX_ANNOTATION_PAYLOAD_BYTES`] — this is the storage seam those answers end at.
    ///
    /// It is the ONLY way an annotation is recorded, because it is the only place that can
    /// see BOTH the per-object ceiling ([`MAX_ANNOTATIONS_PER_OBJECT`]) and the two byte
    /// budgets: an annotation is retained with the object ([`Object::retained_bytes`]) and
    /// is not bounded by anything [`Self::put_object`] checked. Nothing is written unless
    /// the answer is [`AnnotationWrite::Applied`].
    pub fn put_object_annotation(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        name: &str,
        annotation: Annotation,
    ) -> Option<AnnotationWrite> {
        let held = self.buckets.get(bucket)?;
        let Some(object) = held.addressed_object(key, version_id) else {
            return Some(AnnotationWrite::NoSuchObject);
        };

        // "Each object can have up to 1,000 annotations", so only a write that names a
        // name the object does not carry yet can take it past the ceiling: one that
        // replaces an existing annotation leaves the count where it is.
        let replaces = object.annotation(name);
        if replaces.is_none() && object.annotations.len() >= MAX_ANNOTATIONS_PER_OBJECT {
            return Some(AnnotationWrite::LimitReached);
        }

        // What the bucket and the whole store would retain with this annotation in place
        // of the one that name carries: everything they retain now, less the bytes that
        // annotation holds, plus the bytes of the replacement — and the NAME's own bytes
        // too when the name is new, since those are retained with it.
        let released = replaces.map_or(0, Annotation::retained_bytes);
        let taken = annotation.retained_bytes()
            + if replaces.is_none() { name.len() } else { 0 };
        let would_hold = held.retained_bytes().saturating_sub(released) + taken;
        if would_hold > MAX_OBJECT_BYTES_PER_BUCKET {
            return Some(AnnotationWrite::ByteBudgetReached { would_hold });
        }
        let total_would_hold = self
            .caller_retained_bytes()
            .saturating_sub(released)
            + taken;
        if total_would_hold > MAX_OBJECT_BYTES_TOTAL {
            return Some(AnnotationWrite::TotalByteBudgetReached {
                would_hold: total_would_hold,
            });
        }

        self.buckets
            .get_mut(bucket)
            .and_then(|held| held.addressed_object_mut(key, version_id))
            .map(|object| {
                object.annotations.insert(name.to_string(), annotation);
                AnnotationWrite::Applied
            })
    }

    /// Remove the annotation `name` addresses from the object version `version_id` names
    /// in `bucket` — the key's current object when it names none. Reports what the removal
    /// did, or `None` when this emulator holds no such bucket (so a delete refuses a name
    /// that is not a bucket instead of reporting a removal from nothing).
    ///
    /// PERMANENT, and of the one annotation `name` addresses alone: "Deleting an annotation
    /// is permanent. Annotations are not independently versioned, so there is no delete
    /// marker or way to recover a deleted annotation" (`DeleteObjectAnnotation`'s own
    /// documentation), so the payload and everything it retained are released and every
    /// other annotation of the object stays where it was.
    ///
    /// The counterpart of [`Self::put_object_annotation`] and the ONLY way an annotation is
    /// taken away, so the bytes an annotation retains are released in exactly one place;
    /// unlike a write it is bounded by nothing, because a removal only gives room back.
    pub fn remove_object_annotation(
        &mut self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
        name: &str,
    ) -> Option<AnnotationRemoval> {
        let held = self.buckets.get_mut(bucket)?;
        let Some(object) = held.addressed_object_mut(key, version_id) else {
            return Some(AnnotationRemoval::NoSuchObject);
        };
        Some(match object.annotations.remove(name) {
            Some(_) => AnnotationRemoval::Removed,
            None => AnnotationRemoval::NoSuchAnnotation,
        })
    }

    /// Record a bucket named `name` in `region`, reporting what happened.
    ///
    /// Nothing is written unless the answer is [`BucketCreation::Created`]: a taken name
    /// leaves the existing bucket alone, and a create past
    /// [`MAX_BUCKETS_PER_ACCOUNT`] is refused, which is what bounds this map.
    pub fn create_bucket(&mut self, name: &str, region: &str) -> BucketCreation {
        self.record_bucket(name, region, None, false)
    }

    /// Record a bucket named `name` in `region` that `provisioned_by` — another emulated
    /// service — stands up as the object store of one of ITS resources, reporting what
    /// happened.
    ///
    /// This is the one way a service other than S3 gives a resource of its own an
    /// S3-compatible object data plane: every object operation here serves the bucket
    /// (that is the point of provisioning it), while [`Self::account_buckets`] leaves it
    /// out. See [`Bucket::provisioned_by`].
    ///
    /// The same guarantees as [`Self::create_bucket`], which it shares its body with: a
    /// name this store already holds is [`BucketCreation::NameTaken`] and writes NOTHING,
    /// so a provisioning service learns the name is taken instead of taking over the
    /// bucket that holds it — one bucket namespace, whoever provisioned the bucket in it.
    /// What it does NOT share is the account's bucket quota: see
    /// [`MAX_BUCKETS_PER_ACCOUNT`].
    pub fn provision_bucket(
        &mut self,
        name: &str,
        region: &str,
        provisioned_by: &str,
    ) -> BucketCreation {
        self.record_bucket(name, region, Some(provisioned_by), false)
    }

    /// Record every bucket AWS publishes ([`aws_published::PUBLISHED_BUCKETS`]) and store
    /// the objects it publishes under it.
    ///
    /// Called from [`Default`] and nowhere else: a published bucket is in the store before
    /// the first request is served, because nothing creates it. Each object goes in through
    /// [`Self::put_object`], the one way an object is recorded, so a published object is
    /// held exactly the way a caller's is and every reader serves it through the same path.
    ///
    /// Nothing here can refuse: the catalog is fixed at compile time, so whether each entry
    /// is recorded is settled by the catalog rather than by anything a caller does — which
    /// is why the outcomes are dropped instead of raising anything on a request's path. That
    /// every entry IS recorded, with the bytes the catalog publishes, is pinned by
    /// `the_default_store_holds_every_published_object` below and over the wire by
    /// `tests/all/aws_published_sample_data.rs`.
    fn record_published_buckets(&mut self) {
        for published in aws_published::PUBLISHED_BUCKETS {
            let _ = self.record_bucket(published.name, published.region, None, true);
            for object in published.objects {
                let body = object.body.as_bytes().to_vec();
                let stored = Object {
                    etag: object_etag(&body),
                    body,
                    // The instant the store came into being, which is the only instant
                    // this emulator can report for a published object; nothing moves it
                    // afterwards, so every read reports the same `Last-Modified`.
                    last_modified: SystemTime::now(),
                    storage_class: DEFAULT_STORAGE_CLASS.to_string(),
                    content_type: Some(object.content_type.to_string()),
                    cache_control: None,
                    content_disposition: None,
                    content_encoding: None,
                    content_language: None,
                    expires: None,
                    website_redirect_location: None,
                    metadata: BTreeMap::new(),
                    tags: Vec::new(),
                    encryption: ObjectEncryption::default(),
                    // The store mints a version id only for a versioning-enabled bucket,
                    // and a published bucket carries no versioning state.
                    version_id: None,
                    // No Object Lock retention, no legal hold, no restore and no
                    // annotations: nothing placed one, and a published object is readable
                    // without one being asked for.
                    retention: None,
                    legal_hold: None,
                    restore: None,
                    annotations: BTreeMap::new(),
                    // Nor an ACL: a published object carries the ACL every object starts
                    // with, the owner's `FULL_CONTROL`, which a reader derives rather than
                    // this recording it.
                    acl: None,
                    // And no checksum: the catalog publishes bytes and a content type, so
                    // nothing named a checksum algorithm for these objects to be stored
                    // under.
                    checksum: None,
                    // Nor parts: a published object is stored whole.
                    parts: Vec::new(),
                    // And no replication: a published bucket carries no replication
                    // configuration, so nothing replicates its objects.
                    replication: None,
                };
                let _ = self.put_object(published.name, object.key, stored);
            }
        }

        // What the catalog itself holds, measured once now that it is all in: the fixed
        // amount the whole-store budgets subtract, so the catalog takes no room from a
        // caller's own objects while anything a caller writes into a published bucket
        // still counts. See [`Self::published_retained_bytes`].
        self.published_retained_bytes = self
            .buckets
            .values()
            .filter(|bucket| bucket.aws_published)
            .map(Bucket::retained_bytes)
            .sum();
        self.published_versions = self
            .buckets
            .values()
            .filter(|bucket| bucket.aws_published)
            .map(Bucket::stored_versions)
            .sum();
    }

    /// Record a bucket named `name` in `region`, provisioned by `provisioned_by` (`None`
    /// for one created over S3's own API) and published by AWS when `aws_published`,
    /// reporting what happened.
    ///
    /// Private: a create comes in through [`Self::create_bucket`] or
    /// [`Self::provision_bucket`], which is what keeps both subject to the same taken-name
    /// and quota checks, and the published catalog through
    /// [`Self::record_published_buckets`], which runs before any request does.
    fn record_bucket(
        &mut self,
        name: &str,
        region: &str,
        provisioned_by: Option<&str>,
        aws_published: bool,
    ) -> BucketCreation {
        if self.buckets.contains_key(name) {
            return BucketCreation::NameTaken;
        }
        // The quota is the ACCOUNT's, so it bounds the account's own buckets and is
        // measured over them: a bucket another service provisioned is not one of them (it
        // is not even reported among them, see [`Self::account_buckets`]) and is bounded
        // by that service's own limit on the resource it belongs to, so it neither
        // consumes this quota nor is refused by it. Neither is a bucket AWS publishes,
        // which is nobody's account's and is fixed by the catalog.
        if provisioned_by.is_none()
            && !aws_published
            && self.account_buckets().count() >= MAX_BUCKETS_PER_ACCOUNT
        {
            return BucketCreation::QuotaReached;
        }
        self.buckets.insert(
            name.to_string(),
            Bucket {
                region: region.to_string(),
                provisioned_by: provisioned_by.map(str::to_string),
                aws_published,
                // The creation instant is read here, where the bucket comes into being,
                // so `CreationDate` is the moment the store accepted it.
                created_at: Some(SystemTime::now()),
                // "Amazon S3 automatically enables S3 Block Public Access … for new
                // buckets" (Amazon S3 User Guide, Blocking public access to your Amazon S3
                // storage): a bucket is fully blocked from the moment it exists, so the
                // subresource is associated with it here — the same way its Object
                // Ownership setting is — and a read of it reports four switches rather than
                // "there is none". A bucket from AWS's published catalog predates that
                // default and is public, so it carries no configuration.
                public_access_block: (!aws_published)
                    .then(BucketPublicAccessBlock::all_enabled),
                // A fresh bucket carries no policy, no tags and no ownership controls, and
                // holds no objects; `PutBucketPolicy`, `PutBucketTagging` and
                // `PutBucketOwnershipControls` are what associate those subresources with
                // it and `PutObject` what puts an object in it.
                ..Bucket::default()
            },
        );
        BucketCreation::Created
    }

    /// Delete `key` in the bucket named `bucket` the way a `DeleteObject` that named no
    /// `versionId` does, reporting what that did — or `None` when this emulator holds no
    /// such bucket, so a delete can tell "no such bucket" from "nothing at that key".
    ///
    /// What it does turns on the bucket's versioning state, which [`Bucket::remove_object`]
    /// documents case by case. This is where the DELETE MARKER's version id is minted,
    /// because this is the only place that sees both that state and the store's version
    /// sequence: a delete on a bucket whose status is `Enabled` mints a fresh id for the
    /// marker it lays, exactly as a write mints one for the version it stores.
    ///
    /// A marker is a stored version, so it is bounded like one: this refuses to lay it
    /// once the whole store holds [`MAX_OBJECTS_TOTAL`] versions, and the per-bucket
    /// ceiling is checked inside. Nothing is written when the answer is a ceiling.
    pub fn remove_object(&mut self, bucket: &str, key: &str) -> Option<ObjectDeletion> {
        let held = self.buckets.get(bucket)?;
        let versioned = held.versioning_enabled();
        // Only a delete that lays a marker the store does not already hold a slot for adds
        // an entry: a bucket with no versioning state takes one away, and a `null` marker
        // displaces the `null` version it covers. Just that case is bounded here, and only
        // that case pays for the pass over every bucket.
        let adds_version = held.versioning.is_some()
            && (versioned || held.null_version_position(key).is_none());
        if adds_version {
            let held_versions: usize = self.caller_stored_versions();
            if held_versions >= MAX_OBJECTS_TOTAL {
                return Some(ObjectDeletion::TotalVersionCountReached);
            }
        }
        // Minted before the ceilings below are measured, the way a write's id is; a
        // refusal leaves it unused, which costs nothing but the sequence number.
        let minted = versioned.then(|| {
            self.minted_versions = self.minted_versions.saturating_add(1);
            mint_version_id(bucket, key, self.minted_versions)
        });
        self.buckets
            .get_mut(bucket)
            .map(|held| held.remove_object(key, minted))
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

/// How many characters a version id this store MINTS carries ([`mint_version_id`]): the
/// width of the opaque token S3 issues for a version, which is also the SHORTEST a version
/// id may be ([`version_id_is_well_formed`]).
pub const VERSION_ID_CHARS: usize = 32;

/// The most characters a version id may carry: `ObjectVersionId` is a plain string in the
/// model (`smithy_query(query="shape",shape="ObjectVersionId")` reports no length), and S3
/// documents a version id of "no more than 1,024 bytes" (Amazon S3 User Guide, *Using
/// versioning in S3 buckets*), so a longer token cannot be one.
pub const MAX_VERSION_ID_CHARS: usize = 1024;

/// The characters a version id is drawn from, beside the ASCII alphanumerics.
///
/// The token is opaque and travels in a `versionId` query parameter, so S3 draws it from
/// the alphanumerics and the unreserved characters that survive that round trip.
const VERSION_ID_EXTRA_CHARS: [char; 3] = ['-', '_', '.'];

/// The character a version id's alphabet writes the value ZERO as: the first digit of the
/// URL-safe Base64 alphabet S3's own published ids are written in.
///
/// A token made of nothing but it carries the zero value — see
/// [`version_id_is_well_formed`].
const VERSION_ID_ZERO_DIGIT: char = 'A';

/// Whether `version_id` is SHAPED like a version id S3 issues.
///
/// A `versionId` is one of exactly two things: the literal [`NULL_VERSION_ID`], which names
/// the version a write made while the bucket had no versioning state, or a token of the
/// published shape — between [`VERSION_ID_CHARS`] and [`MAX_VERSION_ID_CHARS`] characters,
/// each an ASCII alphanumeric or one of [`VERSION_ID_EXTRA_CHARS`]. The width is a RANGE
/// rather than one number because the token is opaque: this store mints ids of its own
/// width, and an id another S3 issued is just as much a version id.
///
/// An opaque token's text IS the identity it names, and one identity is not nameable: the
/// ZERO value, which a token writes as nothing but [`VERSION_ID_ZERO_DIGIT`]. No issuer
/// mints it — an issued id carries the moment and the sequence the version was recorded at
/// — so a caller that sends it has invented a token rather than named a version, which is
/// what S3 reports by refusing the ARGUMENT instead of the version.
///
/// Anything outside that is not a version id of any object in any bucket either, so both are
/// a malformed ARGUMENT rather than an absent version: S3 answers the former
/// [`INVALID_ARGUMENT_CODE`] with HTTP 400 and reserves [`NO_SUCH_VERSION_CODE`] with HTTP
/// 404 for a well-formed id that names no stored version — which is why an id of the
/// published shape that this store never minted is a 404 and not a 400. Checked by every
/// operation that takes one `versionId` as an argument, so one id is well-formed or
/// malformed everywhere; the batch delete reports per identifier what it could not do with
/// that pair instead (`ops/delete_objects.rs`).
pub fn version_id_is_well_formed(version_id: &str) -> bool {
    if version_id == NULL_VERSION_ID {
        return true;
    }
    let characters = version_id.chars().count();
    (VERSION_ID_CHARS..=MAX_VERSION_ID_CHARS).contains(&characters)
        && version_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || VERSION_ID_EXTRA_CHARS.contains(&character)
        })
        && version_id
            .chars()
            .any(|character| character != VERSION_ID_ZERO_DIGIT)
}

/// The request field a malformed `versionId` is reported against — the `versionId` query
/// parameter every operation that takes one binds it from
/// (`smithy_query(query="operation",operation="GetObject")` reports
/// `VersionId: {httpQuery: "versionId"}`), which is how S3's `InvalidArgument` document names
/// the argument it rejected.
pub const VERSION_ID_ARGUMENT: &str = "versionId";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for a `versionId` that is not
/// shaped like a version id ([`version_id_is_well_formed`]).
pub const INVALID_VERSION_ID_MESSAGE: &str = "Invalid version id specified";
/// The code S3 answers a `DeleteBucket` of a bucket that still holds something, with HTTP
/// 409 Conflict: "All objects (including all object versions and delete markers) in the
/// bucket must be deleted before the bucket itself can be deleted" (`DeleteBucket`'s own
/// documentation). Not a shape in this model, so it goes out through the unmodelled
/// channel.
pub const BUCKET_NOT_EMPTY_CODE: &str = "BucketNotEmpty";
/// See [`BUCKET_NOT_EMPTY_CODE`] — the wording for a bucket that has no versioning state,
/// whose only contents are the current object at each key.
pub const BUCKET_NOT_EMPTY_MESSAGE: &str = "The bucket you tried to delete is not empty";
/// See [`BUCKET_NOT_EMPTY_CODE`] — the wording for a bucket that HAS a versioning state,
/// where what is left to delete is the stored versions rather than a current object. The
/// code and the 409 are the requirement; this sentence is the recorded example of how S3
/// says "versions" to a versioned bucket's caller.
pub const BUCKET_NOT_EMPTY_VERSIONED_MESSAGE: &str =
    "The bucket you tried to delete is not empty. You must delete all versions in the bucket.";
/// The code S3 answers a write whose declared digest is not the digest of the bytes it
/// carried — a `Content-MD5` or an `x-amz-checksum-*` value that is well formed but does
/// not describe the payload ([`IntegrityRejection`] decides which sentence it carries).
pub const BAD_DIGEST_CODE: &str = "BadDigest";
/// The HTTP status [`BAD_DIGEST_CODE`] is rendered with: 400, from Amazon S3's list of
/// error codes ("400 Bad Request").
pub const BAD_DIGEST_STATUS: u16 = 400;
/// The code S3 answers a write whose body is larger than one request may carry
/// ([`MAX_OBJECT_BYTES`]).
pub const ENTITY_TOO_LARGE_CODE: &str = "EntityTooLarge";
/// The code S3 answers a conditional write whose precondition does not hold, with HTTP
/// 412 (`PutObjectRequest$IfMatch`: "If the ETag values do not match, the operation
/// returns a 412 Precondition Failed error").
pub const PRECONDITION_FAILED_CODE: &str = "PreconditionFailed";
/// See [`PRECONDITION_FAILED_CODE`] — the text Amazon S3 "Error responses" publishes for
/// that code in its list of error codes.
pub const PRECONDITION_FAILED_MESSAGE: &str =
    "At least one of the pre-conditions you specified did not hold";
/// The HTTP status [`PRECONDITION_FAILED_CODE`] is rendered with: 412, from the same
/// list ("412 Precondition Failed").
pub const PRECONDITION_FAILED_STATUS: u16 = 412;

/// The `<Condition>` S3's `PreconditionFailed` document names the conditional header that
/// did not hold by, for each conditional member this service judges.
///
/// The message that code carries is FIXED ([`PRECONDITION_FAILED_MESSAGE`]) and states that
/// "at least one" condition failed, so the member is what says WHICH one — and its value is
/// the request HEADER the failing member is bound to, as the model binds it
/// (`smithy_query(query="operation",operation="GetObject,PutObject,DeleteObject,CopyObject")`
/// reports `IfMatch: {httpHeader: "If-Match"}`, `IfNoneMatch: {httpHeader: "If-None-Match"}`,
/// `IfUnmodifiedSince: {httpHeader: "If-Unmodified-Since"}`,
/// `IfMatchLastModifiedTime: {httpHeader: "x-amz-if-match-last-modified-time"}`,
/// `IfMatchSize: {httpHeader: "x-amz-if-match-size"}` and the four `CopySourceIf*` members),
/// which is the name the client wrote on the request it has to change.
///
/// The copy-source conditions carry S3's `x-amz-copy-source-` prefix in front of the
/// standard HTTP condition's own canonical spelling, which is how S3's document words them;
/// a header S3 defines itself ([`IF_MATCH_LAST_MODIFIED_TIME_CONDITION`] and
/// [`IF_MATCH_SIZE_CONDITION`], and the annotation operations'
/// [`OBJECT_IF_MATCH_CONDITION`]) has no canonical HTTP spelling and is named as the model
/// publishes it.
pub const IF_MATCH_CONDITION: &str = "If-Match";
/// See [`IF_MATCH_CONDITION`].
pub const IF_NONE_MATCH_CONDITION: &str = "If-None-Match";
/// See [`IF_MATCH_CONDITION`].
pub const IF_UNMODIFIED_SINCE_CONDITION: &str = "If-Unmodified-Since";
/// See [`IF_MATCH_CONDITION`].
pub const IF_MATCH_LAST_MODIFIED_TIME_CONDITION: &str = "x-amz-if-match-last-modified-time";
/// See [`IF_MATCH_CONDITION`].
pub const IF_MATCH_SIZE_CONDITION: &str = "x-amz-if-match-size";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_MATCH_CONDITION: &str = "x-amz-copy-source-If-Match";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_NONE_MATCH_CONDITION: &str = "x-amz-copy-source-If-None-Match";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_MODIFIED_SINCE_CONDITION: &str = "x-amz-copy-source-If-Modified-Since";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_UNMODIFIED_SINCE_CONDITION: &str =
    "x-amz-copy-source-If-Unmodified-Since";
/// See [`IF_MATCH_CONDITION`] — the header the annotation operations bind their own
/// conditional member from (`PutObjectAnnotationRequest$ObjectIfMatch`).
pub const OBJECT_IF_MATCH_CONDITION: &str = "x-amz-object-if-match";
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
/// The code S3 answers a read of a bucket that has no website configuration associated
/// with it, with HTTP 404: "NoSuchWebsiteConfiguration - The specified bucket does not have
/// a website configuration" (Amazon S3 "Error responses", list of error codes). The
/// subresource-not-there case, answered the way [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`]
/// answers it for ownership controls, because `GetBucketWebsite` reports the configuration
/// as its WHOLE output document and so has no way to report "there is none" in a 200.
pub const NO_SUCH_WEBSITE_CONFIGURATION_CODE: &str = "NoSuchWebsiteConfiguration";
/// See [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`] — the text that same list publishes for it.
pub const NO_SUCH_WEBSITE_CONFIGURATION_MESSAGE: &str =
    "The specified bucket does not have a website configuration";

/// The code S3 answers a request whose XML payload does not validate against the schema
/// the API publishes for it: "MalformedXML - The XML provided does not match the schema"
/// (`PutBucketTagging`'s own list of special errors), with HTTP 400.
pub const MALFORMED_XML_CODE: &str = "MalformedXML";

/// The HTTP status [`MALFORMED_XML_CODE`] is rendered with: 400, from that same list.
pub const MALFORMED_XML_STATUS: u16 = 400;

/// The `<Message>` S3 sends with [`MALFORMED_XML_CODE`]: "The XML you provided was not well
/// formed or did not validate against our published schema" — the FIXED text that code's
/// entry in Amazon S3's "Error responses" list publishes, which is what a schema refusal
/// carries however the document failed to validate.
pub const MALFORMED_XML_MESSAGE: &str =
    "The XML you provided was not well-formed or did not validate against our published \
     schema";

/// The first character of `document` that the XML 1.0 character set does not allow, or
/// `None` when every one of them is a `Char`.
///
/// XML 1.0 defines the characters a document may contain as
/// `#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]` (W3C XML 1.0,
/// §2.2 "Characters"), so a C0 control other than tab, line feed and carriage return — and
/// the two non-characters `#xFFFE`/`#xFFFF` — cannot appear in one at all. A request
/// document carrying one is not well-formed XML whatever it otherwise says, which is
/// [`MALFORMED_XML_CODE`]. Surrogates are unrepresentable in a Rust `str`, so there is
/// nothing to check for them.
pub fn first_forbidden_xml_char(document: &str) -> Option<char> {
    document.chars().find(|character| {
        matches!(character, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}')
            || matches!(character, '\u{fffe}' | '\u{ffff}')
    })
}

/// The `Content-Type` an object is stored with when the write that created it named none.
///
/// S3 never infers a type from the key: an object put with no `Content-Type` is stored and
/// reported as `binary/octet-stream` (the default `Content-Type` the Amazon S3 User Guide
/// documents for an upload that specifies none), so a later `GetObject` or `HeadObject`
/// reports exactly that rather than a type the caller never sent.
pub const DEFAULT_CONTENT_TYPE: &str = "binary/octet-stream";

/// The code S3 answers a `CreateBucket` whose location constraint cannot be served by the
/// endpoint the request was sent to: "IllegalLocationConstraintException - The specified
/// location-constraint is not valid", HTTP 400 (Amazon S3 "Error responses", list of error
/// codes).
pub const ILLEGAL_LOCATION_CONSTRAINT_CODE: &str = "IllegalLocationConstraintException";

/// What stands in the message below for a create that names NO location constraint, which
/// is the one thing that sentence varies in.
pub const UNSPECIFIED_LOCATION_CONSTRAINT: &str = "unspecified";

/// See [`ILLEGAL_LOCATION_CONSTRAINT_CODE`] — the wording S3 uses for a create whose
/// location constraint the endpoint it was sent to cannot serve.
///
/// Fixed text with ONE variable part: the constraint the request named, spelled as the
/// request spelled it, and [`UNSPECIFIED_LOCATION_CONSTRAINT`] for a create that named
/// none — the two readings of one sentence, so neither can drift from the other.
pub fn illegal_location_constraint_message(location_constraint: Option<&str>) -> String {
    let named = location_constraint.unwrap_or(UNSPECIFIED_LOCATION_CONSTRAINT);
    format!(
        "The {named} location constraint is incompatible for the region specific endpoint \
         this request was sent to."
    )
}

/// The host of S3's GLOBAL endpoint, which is not an endpoint for any one Region: "if you
/// send your `CreateBucket` request to the s3.amazonaws.com global endpoint, the request
/// goes to the us-east-1 Region" (`CreateBucket`'s own documentation).
const GLOBAL_ENDPOINT_HOST: &str = "s3.amazonaws.com";

/// Whether `parts` was addressed to the GLOBAL endpoint rather than to an endpoint for one
/// Region.
///
/// Read off the host the request carries, because that is the only thing that tells the two
/// apart: a request to the global endpoint is signed for [`DEFAULT_BUCKET_REGION`] ("the
/// signature calculations in Signature Version 4 must use us-east-1 as the Region, even if
/// the location constraint in the request specifies another Region", `CreateBucket`'s own
/// documentation), exactly as a request to that Region's own endpoint is. The
/// virtual-hosted-style form of the same endpoint carries the bucket as a further label in
/// front of it, so a host that ENDS in the global one is addressed to it too.
pub fn addressed_global_endpoint(parts: &::http::request::Parts) -> bool {
    let host = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())
        .unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host == GLOBAL_ENDPOINT_HOST
        || host
            .strip_suffix(GLOBAL_ENDPOINT_HOST)
            .is_some_and(|label| label.ends_with('.'))
}

/// The Region the request was signed for: the third field of its SigV4 credential scope,
/// which is the endpoint a client addressed this emulator as.
///
/// The parsing lives in `emulator_server::sigv4_credential_scope`, the one place this
/// whole emulator reads a credential scope, so no service re-splits the header. A request
/// carrying no parseable scope is read as the Region the unspecified location constraint
/// means ([`DEFAULT_BUCKET_REGION`]), which is the endpoint a client that named none
/// reached.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_BUCKET_REGION.to_string())
}

/// The ABAC status a general purpose bucket carries until a `PutBucketAbac` sets one.
///
/// `BucketAbacStatus` names the two states the property can be in — `Enabled` and
/// `Disabled` (`smithy_query(query="shape",shape="BucketAbacStatus")`) — and a bucket
/// comes into being in neither of them by request: `CreateBucket` carries no ABAC member
/// (`smithy_query(query="operation",operation="CreateBucket")`), and ABAC takes effect
/// only once it is enabled ("If ABAC is enabled on your bucket, you can use tags on the
/// bucket for access control", `GetBucketAbac`'s own documentation). So the state a
/// never-configured bucket is in is the other one.
pub const DEFAULT_BUCKET_ABAC_STATUS: &str = "Disabled";

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

/// The `EncryptionType` values a general purpose bucket blocks until a
/// `PutBucketEncryption` names another set.
///
/// `BlockedEncryptionTypes` is "a bucket-level setting for Amazon S3 general purpose
/// buckets used to prevent the upload of new objects encrypted with the specified
/// server-side encryption type", and "currently, this parameter only supports blocking or
/// unblocking server-side encryption with customer-provided keys (SSE-C)"
/// (`ServerSideEncryptionRule$BlockedEncryptionTypes`). A general purpose bucket blocks
/// SSE-C uploads by default, which is why this is what a never-configured bucket reports.
pub const DEFAULT_BLOCKED_ENCRYPTION_TYPES: [&str; 1] = [SSE_C_ENCRYPTION_TYPE];

/// The `EncryptionType` value that names server-side encryption with customer-provided
/// keys — one of the two values the enum publishes
/// (`smithy_query(query="shape",shape="EncryptionType")` reports `[NONE, SSE-C]`), which
/// `tests/all/bucket_encryption.rs` re-derives from that value list.
pub const SSE_C_ENCRYPTION_TYPE: &str = "SSE-C";

/// The `ServerSideEncryption` values a `KMSMasterKeyID` may accompany in a default
/// encryption rule.
///
/// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID`'s own documentation: "General
/// purpose buckets - This parameter is allowed if and only if `SSEAlgorithm` is set to
/// `aws:kms` or `aws:kms:dsse`." Both are values of the `ServerSideEncryption` enum, so a
/// rule naming a key beside any other algorithm is one S3 refuses rather than records.
pub const KMS_SSE_ALGORITHMS: [&str; 2] = ["aws:kms", "aws:kms:dsse"];

/// The code a read of an object version's RETENTION answers with when no retention is
/// associated with that version, with HTTP 404.
///
/// The subresource-not-there case, answered the way [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`]
/// answers it for a bucket's website configuration: `GetObjectRetention` reports the
/// `Retention` container as its WHOLE @httpPayload
/// (`smithy_query(query="operation",operation="GetObjectRetention")`), so a 200 has no way
/// to report "there is none". The code is named for the subresource the request addressed,
/// as every row of that family is.
pub const NO_SUCH_OBJECT_LOCK_CONFIGURATION_CODE: &str = "NoSuchObjectLockConfiguration";
/// See [`NO_SUCH_OBJECT_LOCK_CONFIGURATION_CODE`] — the sentence S3 sends beside it,
/// verbatim, article and all: "The specified object does not have a ObjectLock
/// configuration". Fixed text, naming neither the bucket, the key nor the version.
pub const NO_SUCH_OBJECT_LOCK_CONFIGURATION_MESSAGE: &str =
    "The specified object does not have a ObjectLock configuration";

/// The `<Message>` S3 sends beside the 403 `AccessDenied` of a call an Object Lock
/// retention bars: "Access Denied because object protected by object lock."
///
/// A fixed sentence for that one condition — it names neither the object, the mode nor the
/// date, so nothing in it is derived from the request — which is why it lives here beside
/// the other code/message pairs rather than being built in the handler.
pub const OBJECT_LOCK_ACCESS_DENIED_MESSAGE: &str =
    "Access Denied because object protected by object lock.";

/// The `<Message>` S3 sends beside the 400 [`INVALID_REQUEST_CODE`] of an object-level
/// Object Lock call — a legal hold or a retention, read or written — addressed to a bucket
/// that has no Object Lock configuration: "Bucket is missing Object Lock Configuration".
///
/// A fixed sentence for that one condition, naming neither the bucket nor the key, so
/// nothing in it is derived from the request — which is why it lives here beside the other
/// code/message pairs rather than being built in each of the handlers that answer it.
pub const OBJECT_LOCK_CONFIGURATION_MISSING_MESSAGE: &str =
    "Bucket is missing Object Lock Configuration";

/// The code a read of a BUCKET's Object Lock configuration answers with when the bucket
/// carries none, with HTTP 404.
///
/// The subresource-not-there case for this subresource, named the way the rest of that
/// family is ([`NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE`],
/// `ReplicationConfigurationNotFoundError`): `GetObjectLockConfiguration` reports the
/// `ObjectLockConfiguration` container as its WHOLE @httpPayload
/// (`smithy_query(query="operation",operation="GetObjectLockConfiguration")`), so a 200 has
/// no way to report "there is none".
pub const OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE: &str = "ObjectLockConfigurationNotFoundError";
/// See [`OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE`] — the fixed sentence S3 sends beside it,
/// which names neither the bucket nor anything else of the request.
pub const OBJECT_LOCK_CONFIGURATION_NOT_FOUND_MESSAGE: &str =
    "Object Lock configuration does not exist for this bucket";

/// The code S3 answers a bucket-level call the bucket's own state cannot serve:
/// `InvalidBucketState`, HTTP 409 — the status S3's "Error responses" list carries for it
/// ("the request is not valid with the current state of the bucket").
///
/// The model declares no shape for it, so it travels through the unmodelled channel with
/// the message of the condition that was refused.
pub const INVALID_BUCKET_STATE_CODE: &str = "InvalidBucketState";
/// The HTTP status [`INVALID_BUCKET_STATE_CODE`] is rendered with: 409.
pub const INVALID_BUCKET_STATE_STATUS: u16 = 409;

/// The `<Message>` S3 sends beside the 409 [`INVALID_BUCKET_STATE_CODE`] of a
/// `PutObjectLockConfiguration` addressed to a bucket whose versioning status is not
/// `Enabled`: "Object Lock works only in versioned buckets" (Amazon S3 User Guide, *Using S3
/// Object Lock*), so the configuration cannot be applied to such a bucket at all.
///
/// A fixed sentence for that one condition, naming nothing of the request — which is why it
/// lives here beside the other code/message pairs rather than being built in the handler.
pub const OBJECT_LOCK_VERSIONING_REQUIRED_MESSAGE: &str =
    "Versioning must be 'Enabled' on the bucket to apply a Object Lock configuration";

/// The code S3 answers a call that cannot be served because of the state the object is in:
/// `InvalidObjectState`, HTTP 403 — the status the shape itself carries
/// (`smithy_query(query="shape",shape="InvalidObjectState")`).
///
/// The shape is in this model, but only `GetObject` lists it, and the `<Message>` depends
/// on WHICH state barred the call: a read of an archived object is told the object is
/// archived ([`crate::scaffolded::DOCUMENTED_ERROR_MESSAGES`] carries that one), while a
/// restore of an object that is not archived is told the storage class does not allow one
/// ([`RESTORE_NOT_ALLOWED_MESSAGE`]). So a handler that must word its own message answers
/// this code through the unmodelled channel, where the message travels with it.
pub const INVALID_OBJECT_STATE_CODE: &str = "InvalidObjectState";

/// The HTTP status [`INVALID_OBJECT_STATE_CODE`] is rendered with: 403, from the shape's
/// own `@httpError`.
pub const INVALID_OBJECT_STATE_STATUS: u16 = 403;

/// The `<Message>` S3 sends beside [`INVALID_OBJECT_STATE_CODE`] when a restore is asked
/// for an object whose storage class has nothing to restore: "Restore is not allowed for
/// the object's current storage class."
///
/// A fixed sentence for that one condition — it names neither the object nor its class, so
/// nothing in it is derived from the request — which is why it lives here beside the other
/// code/message pairs rather than being built in the handler.
pub const RESTORE_NOT_ALLOWED_MESSAGE: &str =
    "Restore is not allowed for the object's current storage class";

/// The `<Message>` S3 sends beside [`INVALID_OBJECT_STATE_CODE`] when a read is barred by
/// the storage class the object's bytes are in: "The operation is not valid for the object's
/// storage class."
///
/// A fixed sentence for that one condition — it names neither the object nor its class, so
/// nothing in it is derived from the request — which is why it lives here beside the other
/// code/message pairs rather than being built in the handler.
pub const INVALID_OBJECT_STATE_READ_MESSAGE: &str =
    "The operation is not valid for the object's storage class";

/// The `<Message>` S3 sends beside [`INVALID_OBJECT_STATE_CODE`] when the call is barred by
/// the storage class the SOURCE object's bytes are in: "Operation is not valid for the
/// source object's storage class."
///
/// The same condition [`INVALID_OBJECT_STATE_READ_MESSAGE`] words for the object being
/// read, worded for the object being copied FROM — a fixed sentence naming neither the
/// object nor its class, which is why it lives here beside the other code/message pairs.
pub const INVALID_OBJECT_STATE_COPY_SOURCE_MESSAGE: &str =
    "Operation is not valid for the source object's storage class";

/// The code `RestoreObject` answers when a retrieval is already running on the version it
/// names: `RestoreAlreadyInProgress`, with HTTP 409 Conflict — the special error that
/// operation publishes for the cause "Object restore is already in progress". Not a shape
/// in this model, so it goes out through the unmodelled channel.
pub const RESTORE_ALREADY_IN_PROGRESS_CODE: &str = "RestoreAlreadyInProgress";

/// The HTTP status [`RESTORE_ALREADY_IN_PROGRESS_CODE`] is rendered with: 409, from the
/// same list of special errors.
pub const RESTORE_ALREADY_IN_PROGRESS_STATUS: u16 = 409;

/// See [`RESTORE_ALREADY_IN_PROGRESS_CODE`] — the cause that list publishes for it.
pub const RESTORE_ALREADY_IN_PROGRESS_MESSAGE: &str = "Object restore is already in progress";

/// The code S3 answers a request whose value is not valid for the parameter carrying it:
/// "InvalidArgument - Invalid Argument", HTTP 400 (Amazon S3 "Error responses", list of
/// error codes).
pub const INVALID_ARGUMENT_CODE: &str = "InvalidArgument";
/// The HTTP status [`INVALID_ARGUMENT_CODE`] is rendered with: 400, from the same list.
pub const INVALID_ARGUMENT_STATUS: u16 = 400;

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for a
/// `NotificationConfiguration` naming a destination it could not validate.
///
/// A FIXED sentence: nothing about the request is interpolated into it. WHICH destinations
/// failed, and why each did, travel in the numbered `<ArgumentName{n}>` /
/// `<ArgumentValue{n}>` series beside it
/// ([`crate::scaffolded::Scaffolded::invalid_arguments`]).
pub const UNABLE_TO_VALIDATE_DESTINATIONS_MESSAGE: &str =
    "Unable to validate the following destination configurations";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for an `Events` entry that is
/// not one of the published `Event` values.
///
/// Also FIXED text — the event that is not published is not interpolated into it — and the
/// sentence both paths that can refuse such a value answer with: the boundary refusal of
/// the modelled enum (`crate::service`) and
/// [`NotificationConfigurationRejection::UnsupportedEvent`], which guards the store.
pub const UNSUPPORTED_EVENT_MESSAGE: &str = "The event is not supported for notifications";

/// The code S3 answers a request whose content is not one it can carry out as given:
/// "InvalidRequest", HTTP 400 (Amazon S3 "Error responses", list of error codes).
///
/// What a cors configuration breaking one of the limits its API publishes is refused with
/// — see [`CorsConfigurationRejection::code`] — where [`INVALID_ARGUMENT_CODE`] is the
/// code for a value that is wrong for the PARAMETER carrying it.
pub const INVALID_REQUEST_CODE: &str = "InvalidRequest";

/// The code S3 answers a request whose HEADERS ask for functionality the service does not
/// implement for the resource addressed: "NotImplemented — A header you provided implies
/// functionality that is not implemented", HTTP 501 (Amazon S3 "Error responses", list of
/// error codes).
///
/// What a member S3 documents for DIRECTORY buckets alone is answered with when the request
/// addresses a general purpose bucket: the service implements that functionality for one
/// kind of bucket and not for the other, so the header, not the value in it, is what cannot
/// be served. The model declares no shape for the code, so it travels through the unmodelled
/// channel.
pub const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";
/// The HTTP status [`NOT_IMPLEMENTED_CODE`] is rendered with: 501, from the same list.
pub const NOT_IMPLEMENTED_STATUS: u16 = 501;
/// See [`NOT_IMPLEMENTED_CODE`] — the text that same list publishes for it, which names the
/// header class rather than the member, so nothing of the request is interpolated into it.
pub const NOT_IMPLEMENTED_HEADER_MESSAGE: &str =
    "A header you provided implies functionality that is not implemented";

/// "The CORS configuration does not exist": the code `GetBucketCors` answers for a bucket
/// no cors configuration has been set on, with HTTP 404 (Amazon S3 "Error responses").
///
/// The subresource-not-there case, answered the way [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`]
/// answers it for a website configuration, because `GetBucketCors` reports the rules as its
/// WHOLE output document and so has no way to report "there are none" in a 200.
pub const NO_SUCH_CORS_CONFIGURATION_CODE: &str = "NoSuchCORSConfiguration";
/// See [`NO_SUCH_CORS_CONFIGURATION_CODE`] — the text that same list publishes for it.
pub const NO_SUCH_CORS_CONFIGURATION_MESSAGE: &str = "The CORS configuration does not exist";

/// The code `GetBucketLifecycleConfiguration` answers for a bucket no lifecycle
/// configuration has been set on: "Error code: NoSuchLifecycleConfiguration Description:
/// The lifecycle configuration does not exist. HTTP Status Code: 404 Not Found" — the
/// special error that operation's own documentation publishes for it.
///
/// The subresource-not-there case, answered the way [`NO_SUCH_CORS_CONFIGURATION_CODE`]
/// answers it for a cors configuration, because the read reports the rules as its WHOLE
/// output document and so has no way to report "there are none" in a 200.
pub const NO_SUCH_LIFECYCLE_CONFIGURATION_CODE: &str = "NoSuchLifecycleConfiguration";
/// See [`NO_SUCH_LIFECYCLE_CONFIGURATION_CODE`] — the text that same documentation
/// publishes for it.
pub const NO_SUCH_LIFECYCLE_CONFIGURATION_MESSAGE: &str = "The lifecycle configuration does not exist";
/// The HTTP status [`NO_SUCH_LIFECYCLE_CONFIGURATION_CODE`] is rendered with: 404, from the
/// same documentation.
pub const NO_SUCH_LIFECYCLE_CONFIGURATION_STATUS: u16 = 404;

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

/// How many replication rules one configuration may carry.
///
/// AWS publishes the bound on the operation itself: "A replication configuration must
/// include at least one rule, and can contain a maximum of 1,000"
/// (`PutBucketReplication`'s own documentation). The lower end is the same sentence: a
/// configuration naming no rule is not one S3 records.
pub const MAX_REPLICATION_RULES: usize = 1_000;

/// How many destinations ONE write replicates to, and how many bytes of replicas it copies.
///
/// This emulator's own ceilings, not AWS quotas: a stored configuration may name up to
/// [`MAX_REPLICATION_RULES`] destinations and an object may be as large as
/// [`MAX_OBJECT_BYTES_PER_BUCKET`], and the work ONE write would do is the PRODUCT of the
/// two — both of them a caller's to choose. So the fan-out one call performs is bounded
/// here, in the place that performs it ([`ComAmazonawsS3Amazons3State::replicate_write`]),
/// rather than by each factor separately: at most this many copies, carrying at most
/// [`MAX_REPLICATION_BYTES_PER_WRITE`] bytes between them. Past either, the copies are not
/// made and the source version reports [`REPLICATION_STATUS_FAILED`], which is what
/// "replication did not happen" says — nothing claims a copy that was not written.
pub const MAX_REPLICATION_DESTINATIONS_PER_WRITE: usize = 10;

/// See [`MAX_REPLICATION_DESTINATIONS_PER_WRITE`] — the bytes half of the same bound, so a
/// write of a large object fans out to fewer destinations rather than copying the product of
/// the two ceilings.
pub const MAX_REPLICATION_BYTES_PER_WRITE: usize = 4 * MAX_OBJECT_BYTES_PER_BUCKET;

/// How many S3 Intelligent-Tiering configurations one bucket may carry.
///
/// AWS publishes the bound on the operation that records them: "You can have up to 1,000
/// S3 Intelligent-Tiering configurations per bucket"
/// (`PutBucketIntelligentTieringConfiguration`'s own documentation), which the same
/// documentation's `TooManyConfigurations` error names again as "the 1,000-configuration
/// limit". A put that would add a configuration past it is refused rather than recorded,
/// so what one bucket's map holds is bounded whatever a caller does.
pub const MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// What [`Bucket::put_intelligent_tiering_configuration`] did, so the handler answers each
/// outcome without re-deriving it — and cannot reach the stored case without having gone
/// through the configuration-count check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntelligentTieringWrite {
    /// The configuration is now the one the bucket carries under that id.
    Stored,
    /// The bucket already carries [`MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET`]
    /// configurations and this put named a new id; nothing was written.
    ConfigurationCountReached,
}

/// The code S3 answers a put that would take a bucket past the configuration limit with:
/// "TooManyConfigurations … You are attempting to create a new configuration but have
/// already reached the 1,000-configuration limit"
/// (`PutBucketIntelligentTieringConfiguration`'s and `PutBucketMetricsConfiguration`'s own
/// special errors, which publish the same code and text), under "HTTP 400 Bad
/// Request Error".
pub const TOO_MANY_CONFIGURATIONS_CODE: &str = "TooManyConfigurations";

/// See [`TOO_MANY_CONFIGURATIONS_CODE`] — the description AWS documents for that code,
/// nothing of which comes from the request.
pub const TOO_MANY_CONFIGURATIONS_MESSAGE: &str =
    "You are attempting to create a new configuration but have already reached the \
     1,000-configuration limit";

/// The HTTP status [`TOO_MANY_CONFIGURATIONS_CODE`] is rendered with: 400, from that same
/// special-errors list.
pub const TOO_MANY_CONFIGURATIONS_STATUS: u16 = 400;

/// The code S3 answers a `Tiering` whose `Days` is not a number of days that tier accepts
/// with: `InvalidAccessTier`, under "HTTP 400 Bad Request".
///
/// The range a tiering may name is a property of the ACCESS TIER it names — "the minimum
/// number of days specified for Archive Access tier must be at least 90 days and Deep
/// Archive Access tier must be at least 180 days. The maximum can be up to 2 years (730
/// days)" (`Tiering$Days`) — so a pair outside it is reported against the tier rather than
/// as a bare bad argument.
pub const INVALID_ACCESS_TIER_CODE: &str = "InvalidAccessTier";

/// The HTTP status [`INVALID_ACCESS_TIER_CODE`] is rendered with: 400, the status S3 gives
/// a configuration it cannot apply.
pub const INVALID_ACCESS_TIER_STATUS: u16 = 400;

/// The code S3 answers a put whose configuration id is not one it can record with:
/// `InvalidConfigId`, under "HTTP 400 Bad Request".
///
/// The id is what identifies the configuration on the bucket, so a refusal about the id
/// itself — one longer than [`MAX_CONFIGURATION_ID_CHARS`] — is reported against the id
/// rather than as a bare bad argument.
pub const INVALID_CONFIG_ID_CODE: &str = "InvalidConfigId";

/// See [`INVALID_CONFIG_ID_CODE`] — the fixed text S3 sends for that condition, which
/// states the bound rather than repeating the id the request named, so nothing of the
/// request reaches it.
///
/// Its wording is NOT literal at either end, and the enforced bound is the inclusive range
/// `1..=`[`MAX_CONFIGURATION_ID_CHARS`]: an id of exactly that many characters is recorded,
/// and the shortest id S3 records is one character. Do not narrow the check to match the
/// prose.
pub const INVALID_CONFIG_ID_MESSAGE: &str =
    "Config Id should be more than 1 character and less than 64 characters in length.";

/// The HTTP status [`INVALID_CONFIG_ID_CODE`] is rendered with: 400, the status S3 gives a
/// configuration it cannot apply.
pub const INVALID_CONFIG_ID_STATUS: u16 = 400;

/// The `<Message>` S3 sends beside [`INVALID_CONFIG_ID_CODE`] for an id spelled with a
/// character a configuration id may not carry: the sentence QUOTES the pattern the id has to
/// match ([`CONFIGURATION_ID_PATTERN`]), and nothing in it comes from the request.
///
/// A second sentence for the same code, beside [`INVALID_CONFIG_ID_MESSAGE`]: the code names
/// the id as the field at fault and the sentence says WHICH rule about it the value broke —
/// the character class here, the length bound there.
pub const INVALID_CONFIG_ID_PATTERN_MESSAGE: &str =
    r"Config Id does not match the pattern ^[a-zA-Z0-9\-_.]+$";

/// The pattern a bucket configuration id has to match, as the refusal that quotes it spells
/// it ([`INVALID_CONFIG_ID_PATTERN_MESSAGE`]): one or more of a letter, a number, a dash, an
/// underscore or a period.
///
/// It is the character class the id members themselves publish — "can only contain letters,
/// numbers, periods, dashes, and underscores" (`MetricsConfiguration$Id`) — written the way
/// the wire refusal states it, and [`configuration_id_fault`] is the one reading of it.
pub const CONFIGURATION_ID_PATTERN: &str = r"^[a-zA-Z0-9\-_.]+$";

/// The code S3 answers a call on a bucket configuration that states NO id it could identify
/// one by: `InvalidConfigurationId`, under HTTP 400.
///
/// Distinct from [`INVALID_CONFIG_ID_CODE`], which reports an id that WAS stated and broke a
/// rule about its spelling: with no id at all there is no configuration to report a rule
/// against, so S3 names the id itself as the thing that is not one.
pub const INVALID_CONFIGURATION_ID_CODE: &str = "InvalidConfigurationId";

/// See [`INVALID_CONFIGURATION_ID_CODE`] — the fixed text S3 sends for that condition,
/// nothing of which comes from the request.
pub const INVALID_CONFIGURATION_ID_MESSAGE: &str = "The specified configuration id is invalid.";

/// The HTTP status [`INVALID_CONFIGURATION_ID_CODE`] is rendered with: 400, the status S3
/// gives a request naming an argument it cannot act on.
pub const INVALID_CONFIGURATION_ID_STATUS: u16 = 400;

/// Why a value is not an id a bucket configuration may be identified by — which is what
/// decides the refusal a call naming it answers with.
///
/// ONE type for every id-addressed bucket configuration family (analytics, metrics, S3
/// Inventory and S3 Intelligent-Tiering), because the id rule is one rule: the families
/// cannot drift apart on which values they record, nor on how a value outside the rule is
/// reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationIdFault {
    /// The call states no id at all, or an empty one: there is no value to judge against the
    /// id rule, so the id itself is what the refusal names
    /// ([`INVALID_CONFIGURATION_ID_CODE`]).
    Unstated,
    /// The id carries a character a configuration id may not
    /// ([`CONFIGURATION_ID_PATTERN`]).
    OutsidePattern,
    /// The id is longer than [`MAX_CONFIGURATION_ID_CHARS`] characters.
    TooLong,
}

impl ConfigurationIdFault {
    /// The S3 error code a call naming such an id is refused with.
    pub fn code(self) -> &'static str {
        match self {
            Self::Unstated => INVALID_CONFIGURATION_ID_CODE,
            Self::OutsidePattern | Self::TooLong => INVALID_CONFIG_ID_CODE,
        }
    }

    /// The HTTP status that refusal is rendered with — 400 for every one of them: the id is
    /// an argument of the request, and a request S3 cannot act on is a bad request.
    pub fn status(self) -> u16 {
        match self {
            Self::Unstated => INVALID_CONFIGURATION_ID_STATUS,
            Self::OutsidePattern | Self::TooLong => INVALID_CONFIG_ID_STATUS,
        }
    }

    /// The `<Message>` that refusal carries: the fixed sentence S3 publishes for the rule the
    /// value broke, with nothing of the request in it.
    pub fn message(self) -> &'static str {
        match self {
            Self::Unstated => INVALID_CONFIGURATION_ID_MESSAGE,
            Self::OutsidePattern => INVALID_CONFIG_ID_PATTERN_MESSAGE,
            Self::TooLong => INVALID_CONFIG_ID_MESSAGE,
        }
    }
}

/// Why `value` is not an id a bucket configuration may be identified by, or `None` when it
/// is one.
///
/// The one reading of the id rule: non-empty, spelled in [`CONFIGURATION_ID_PATTERN`]'s
/// character class, and at most [`MAX_CONFIGURATION_ID_CHARS`] characters. The model
/// constrains none of the id shapes (`smithy_query(query="shape",shape="AnalyticsId")`
/// reports a plain string), so nothing refuses a value outside it before a handler does, and
/// an id is what one bucket's configuration maps key on.
pub fn configuration_id_fault(value: &str) -> Option<ConfigurationIdFault> {
    if value.is_empty() {
        return Some(ConfigurationIdFault::Unstated);
    }
    if !value.chars().all(is_configuration_id_character) {
        return Some(ConfigurationIdFault::OutsidePattern);
    }
    if value.chars().count() > MAX_CONFIGURATION_ID_CHARS {
        return Some(ConfigurationIdFault::TooLong);
    }
    None
}

/// Whether `character` is one [`CONFIGURATION_ID_PATTERN`]'s class holds: a letter, a
/// number, a period, a dash or an underscore.
fn is_configuration_id_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
}

/// The code S3 answers a listing whose `continuation-token` is not one it issued:
/// `MalformedContinuationToken`, under HTTP 400 — the token is an argument of the request
/// and a token the service cannot resume from names no page of the listing.
pub const MALFORMED_CONTINUATION_TOKEN_CODE: &str = "MalformedContinuationToken";

/// See [`MALFORMED_CONTINUATION_TOKEN_CODE`] — the fixed text S3 sends for that condition,
/// as the service words it on the wire (its grammar is S3's own), with nothing of the
/// request in it: the token is "an opaque value that Amazon S3 understands"
/// (`ListBucketAnalyticsConfigurationsRequest$ContinuationToken`), so a refusal does not
/// echo it back.
pub const MALFORMED_CONTINUATION_TOKEN_MESSAGE: &str = "The continuation-token you provided invalid.";

/// The HTTP status [`MALFORMED_CONTINUATION_TOKEN_CODE`] is rendered with: 400.
pub const MALFORMED_CONTINUATION_TOKEN_STATUS: u16 = 400;

/// The most characters a bucket configuration id may carry.
///
/// 64, the limit AWS publishes on the id members themselves: "The ID has a 64 character
/// limit" (`MetricsConfiguration$Id`), and the same 64 for an S3 Intelligent-Tiering
/// configuration id. Neither `MetricsId` nor `IntelligentTieringId` is constrained in the
/// model — `smithy_query(query="shape",shape="IntelligentTieringId")` reports a bare string
/// — so nothing refuses a longer id before a handler does, and ONE constant serves both
/// families so the two cannot drift. A put naming a longer id is refused rather than
/// recorded, so what one bucket's maps key on is bounded whatever a caller does.
///
/// The LENGTH is shared; the CODE a refusal carries is each family's own, since AWS answers
/// an over-long id per family ([`INVALID_CONFIG_ID_CODE`] for an S3 Intelligent-Tiering id,
/// and [`MALFORMED_XML_CODE`] for a metrics id, whose rule also constrains the character set
/// and whose operation publishes no error of its own for a document it cannot take, so the
/// document is refused for not validating against the published schema).
pub const MAX_CONFIGURATION_ID_CHARS: usize = 64;

/// The code S3 answers a `GetBucketReplication` for a bucket that has no replication
/// configuration with: `ReplicationConfigurationNotFoundError` (Amazon S3 "Error
/// responses", list of error codes), with HTTP 404.
pub const REPLICATION_CONFIGURATION_NOT_FOUND_CODE: &str = "ReplicationConfigurationNotFoundError";
/// The message S3 answers [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] with: "The
/// replication configuration was not found" — fixed text, nothing of which comes from the
/// request, phrased the way the service words it on the wire rather than the way the
/// error-code list describes the condition ("there is no replication configuration for this
/// bucket"), which is a description of the case and not the `<Message>` a client reads.
pub const REPLICATION_CONFIGURATION_NOT_FOUND_MESSAGE: &str =
    "The replication configuration was not found";
/// The HTTP status [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] is rendered with: 404,
/// from the same list.
pub const REPLICATION_CONFIGURATION_NOT_FOUND_STATUS: u16 = 404;

/// The code S3 answers a `PutBucketLogging` whose `LoggingEnabled$TargetBucket` is not a
/// bucket log files can be delivered to with: "InvalidTargetBucketForLogging - The target
/// bucket for logging does not exist, is not owned by you, or does not have the appropriate
/// grants for the log-delivery group." (Amazon S3 "Error responses", list of error codes),
/// with HTTP 400.
pub const INVALID_TARGET_BUCKET_FOR_LOGGING_CODE: &str = "InvalidTargetBucketForLogging";
/// See [`INVALID_TARGET_BUCKET_FOR_LOGGING_CODE`] — the fixed text S3 publishes for that
/// code, nothing of which comes from the request.
pub const INVALID_TARGET_BUCKET_FOR_LOGGING_MESSAGE: &str =
    "The target bucket for logging does not exist, is not owned by you, or does not have \
     the appropriate grants for the log-delivery group.";
/// The HTTP status [`INVALID_TARGET_BUCKET_FOR_LOGGING_CODE`] is rendered with: 400, from
/// the same list.
pub const INVALID_TARGET_BUCKET_FOR_LOGGING_STATUS: u16 = 400;

/// The code S3 answers a `PutBucketLogging` whose target bucket is in another Region than
/// the bucket being logged with: "CrossLocationLoggingProhibited - Cross-location logging
/// not allowed. Buckets in one geographic location cannot log information to a bucket in
/// another location." (Amazon S3 "Error responses", list of error codes), with HTTP 403 —
/// the refusal behind "All logs are saved to buckets in the same Amazon Web Services Region
/// as the source bucket" (`PutBucketLogging`'s own documentation).
pub const CROSS_LOCATION_LOGGING_PROHIBITED_CODE: &str = "CrossLocationLoggingProhibited";
/// See [`CROSS_LOCATION_LOGGING_PROHIBITED_CODE`] — the fixed text S3 publishes for that
/// code.
pub const CROSS_LOCATION_LOGGING_PROHIBITED_MESSAGE: &str =
    "Cross-location logging not allowed. Buckets in one geographic location cannot log \
     information to a bucket in another location.";
/// The HTTP status [`CROSS_LOCATION_LOGGING_PROHIBITED_CODE`] is rendered with: 403, from
/// the same list.
pub const CROSS_LOCATION_LOGGING_PROHIBITED_STATUS: u16 = 403;

/// How many metrics configurations one bucket may hold, and so how many entries one
/// bucket's metrics record can ever carry.
///
/// AWS publishes the bound on the operation itself: "You can have up to 1,000 metrics
/// configurations per bucket" (`PutBucketMetricsConfiguration`'s own documentation), which
/// also names the refusal past it ([`TOO_MANY_CONFIGURATIONS_CODE`]). A put past it is
/// refused rather than recorded, so a caller looping on fresh ids cannot grow one bucket's
/// record without end.
pub const MAX_METRICS_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// The most characters a metrics configuration id may carry.
///
/// From the member's own documentation: "The ID has a 64 character limit and can only
/// contain letters, numbers, periods, dashes, and underscores" (`MetricsConfiguration$Id`).
/// The model constrains `MetricsId` no further — `smithy_query(query="shape",
/// shape="MetricsId")` reports a plain string with no `length` or `pattern` — so nothing
/// refuses a longer or oddly spelled id before [`is_valid_metrics_id`] does. The length
/// half is [`MAX_CONFIGURATION_ID_CHARS`], the bound every bucket configuration id shares;
/// the character set is this family's own.
pub const MAX_METRICS_ID_CHARS: usize = MAX_CONFIGURATION_ID_CHARS;

/// Whether `value` is an id a metrics configuration may be identified by: a configuration
/// id by length ([`MAX_METRICS_ID_CHARS`]), every character of which is a letter, a number,
/// a period, a dash or an underscore — the rule quoted on [`MAX_METRICS_ID_CHARS`].
///
/// An EMPTY id identifies no configuration, so it is not one either. The reading is
/// [`configuration_id_fault`]'s, the one place the id rule lives, so this family and the
/// others cannot drift apart on which values a bucket's map may key on.
pub fn is_valid_metrics_id(value: &str) -> bool {
    configuration_id_fault(value).is_none()
}

/// Whether `value` is long enough and short enough to identify a bucket configuration:
/// between one and [`MAX_CONFIGURATION_ID_CHARS`] characters.
///
/// The LENGTH half every bucket configuration id shares, so the families cannot drift apart
/// on it: [`configuration_id_fault`] composes it with the character set the id members
/// document, and [`is_valid_inventory_id`] is it alone. An EMPTY id identifies no
/// configuration, so it is never one.
pub fn is_valid_configuration_id(value: &str) -> bool {
    let characters = value.chars().count();
    characters > 0 && characters <= MAX_CONFIGURATION_ID_CHARS
}

/// The code S3 answers a read that names a bucket subresource configuration the bucket
/// carries none of under that id: "NoSuchConfiguration - The specified configuration does
/// not exist." (Amazon S3 "Error responses", list of error codes), with HTTP 404 — the
/// subresource-not-there case for a configuration a put identifies by an id, answered the
/// way [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] answers it for the one a bucket carries
/// at most one of.
pub const NO_SUCH_CONFIGURATION_CODE: &str = "NoSuchConfiguration";
/// See [`NO_SUCH_CONFIGURATION_CODE`] — the fixed text S3 publishes for that code, nothing
/// of which comes from the request.
pub const NO_SUCH_CONFIGURATION_MESSAGE: &str = "The specified configuration does not exist.";
/// The HTTP status [`NO_SUCH_CONFIGURATION_CODE`] is rendered with: 404, from the same
/// list.
pub const NO_SUCH_CONFIGURATION_STATUS: u16 = 404;

/// The code S3 answers a `CreateBucketMetadataConfiguration` for a bucket that already
/// carries one: the create cannot be carried out because the resource it would create is
/// already there, and the code names that condition.
///
/// AWS's text for it is fixed, with nothing from the request in it:
/// [`METADATA_CONFIGURATION_ALREADY_EXISTS_MESSAGE`].
pub const METADATA_CONFIGURATION_ALREADY_EXISTS_CODE: &str = "MetadataConfigurationAlreadyExists";
/// See [`METADATA_CONFIGURATION_ALREADY_EXISTS_CODE`] — the text S3 sends for that code.
pub const METADATA_CONFIGURATION_ALREADY_EXISTS_MESSAGE: &str =
    "The metadata configuration already exists";
/// The HTTP status [`METADATA_CONFIGURATION_ALREADY_EXISTS_CODE`] is rendered with: 409
/// Conflict, the status S3 renders every "the resource is already there" code with
/// (`BucketAlreadyExists` and `BucketAlreadyOwnedByYou` carry the same one).
pub const METADATA_CONFIGURATION_ALREADY_EXISTS_STATUS: u16 = 409;

/// The code S3 answers a call that addresses the S3 Metadata configuration of a general
/// purpose bucket which carries none — a bucket that never had one, or whose configuration
/// a `DeleteBucketMetadataConfiguration` has removed.
///
/// The metadata-configuration family names its own not-there condition rather than sharing
/// the id-addressed [`NO_SUCH_CONFIGURATION_CODE`] of the other bucket subresources, the
/// same way it names its own already-there condition
/// ([`METADATA_CONFIGURATION_ALREADY_EXISTS_CODE`]): a configuration a bucket carries at
/// most one of, addressed by the bucket alone.
pub const METADATA_CONFIGURATION_NOT_FOUND_CODE: &str = "MetadataConfigurationNotFound";
/// See [`METADATA_CONFIGURATION_NOT_FOUND_CODE`] — the text S3 sends for that code, fixed,
/// with nothing from the request in it.
pub const METADATA_CONFIGURATION_NOT_FOUND_MESSAGE: &str = "The metadata configuration was not found";
/// The HTTP status [`METADATA_CONFIGURATION_NOT_FOUND_CODE`] is rendered with: 404, the
/// status S3 renders every "the subresource is not there" code with (the sibling
/// [`NO_SUCH_CONFIGURATION_STATUS`] is the same one).
pub const METADATA_CONFIGURATION_NOT_FOUND_STATUS: u16 = 404;

/// What [`Bucket::put_metrics_configuration`], [`Bucket::put_analytics_configuration`] or
/// [`Bucket::put_inventory_configuration`] did, so the caller answers each outcome without
/// re-deriving it — and cannot reach the stored cases without having gone through the
/// per-bucket ceiling.
///
/// ONE type for every one of those families, because a put of any of them is the same
/// transaction: a full replacement of the configuration of the id it names, bounded by how
/// many of them one bucket may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketConfigurationWrite {
    /// The bucket carried no configuration under that id; this one was recorded.
    Stored,
    /// A configuration of that id was already there and this one replaced it IN FULL.
    Replaced,
    /// The bucket already holds as many configurations of that family as it may
    /// ([`MAX_METRICS_CONFIGURATIONS_PER_BUCKET`],
    /// [`MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET`],
    /// [`MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET`]) and this put would add one more;
    /// NOTHING was written.
    QuotaReached,
}

/// Record `configuration` under `id` in one of a bucket's per-id configuration maps,
/// refusing a NEW id once the map holds `ceiling` of them.
///
/// The one implementation behind [`Bucket::put_metrics_configuration`],
/// [`Bucket::put_analytics_configuration`] and [`Bucket::put_inventory_configuration`]: each
/// stores the modelled container whole under the id the request identifies it by, so
/// replacing an id already present overwrites that entry alone and counts against no
/// ceiling.
fn put_keyed_configuration<T>(
    configurations: &mut BTreeMap<String, T>,
    id: String,
    configuration: T,
    ceiling: usize,
) -> BucketConfigurationWrite {
    // The ceiling is read BEFORE the entry is taken, because taking it borrows the map.
    let at_the_ceiling = configurations.len() >= ceiling;
    match configurations.entry(id) {
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            entry.insert(configuration);
            BucketConfigurationWrite::Replaced
        }
        std::collections::btree_map::Entry::Vacant(_) if at_the_ceiling => {
            BucketConfigurationWrite::QuotaReached
        }
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(configuration);
            BucketConfigurationWrite::Stored
        }
    }
}

/// How many analytics configurations one bucket may hold, and so how many entries one
/// bucket's analytics record can ever carry.
///
/// AWS publishes the bound on the operation itself: "You can have up to 1,000 analytics
/// configurations per bucket" (`PutBucketAnalyticsConfiguration`'s own documentation), which
/// also names the refusal past it ([`TOO_MANY_CONFIGURATIONS_CODE`]). A put past it is
/// refused rather than recorded, so a caller looping on fresh ids cannot grow one bucket's
/// record without end.
pub const MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// The most characters an analytics configuration id may carry.
///
/// [`MAX_CONFIGURATION_ID_CHARS`], the bound every bucket configuration id shares: the model
/// constrains `AnalyticsId` no further — `smithy_query(query="shape",shape="AnalyticsId")`
/// reports a plain string with no `length` or `pattern` — so nothing refuses a longer id
/// before a handler does, and an id is what one bucket's analytics map keys on.
pub const MAX_ANALYTICS_ID_CHARS: usize = MAX_CONFIGURATION_ID_CHARS;

/// How many S3 Inventory configurations one bucket may hold, and so how many entries one
/// bucket's inventory record can ever carry.
///
/// AWS publishes the bound on the operation itself: "You can have up to 1,000 inventory
/// configurations per bucket" (`PutBucketInventoryConfiguration`'s own documentation), which
/// also names the refusal past it ([`TOO_MANY_CONFIGURATIONS_CODE`]). A put past it is
/// refused rather than recorded, so a caller looping on fresh ids cannot grow one bucket's
/// record without end.
pub const MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// The most characters an S3 Inventory configuration id may carry.
///
/// [`MAX_CONFIGURATION_ID_CHARS`], the bound every bucket configuration id shares: the model
/// constrains `InventoryId` no further — `smithy_query(query="shape",shape="InventoryId")`
/// reports a plain string with no `length` or `pattern` — so nothing refuses a longer id
/// before a handler does, and an id is what one bucket's inventory map keys on.
pub const MAX_INVENTORY_ID_CHARS: usize = MAX_CONFIGURATION_ID_CHARS;

/// Whether `value` is an id an S3 Inventory configuration may be identified by: a
/// configuration id by length ([`is_valid_configuration_id`], i.e. between one and
/// [`MAX_INVENTORY_ID_CHARS`] characters), which is all `InventoryId` constrains.
pub fn is_valid_inventory_id(value: &str) -> bool {
    is_valid_configuration_id(value)
}

/// The HTTP status [`INVALID_REQUEST_CODE`] is rendered with: 400, from Amazon S3's "Error
/// responses" list of error codes. It is the status the replication prerequisite is refused
/// under: a replication configuration applies only to a bucket whose versioning is enabled.
pub const INVALID_REQUEST_STATUS: u16 = 400;

/// The `<Message>` S3 sends beside the 400 [`INVALID_REQUEST_CODE`] of a copy whose source
/// and destination name the same object and which would change nothing about it: "This copy
/// request is illegal because it is trying to copy an object to itself without changing the
/// object's metadata, storage class, website redirect location or encryption attributes."
///
/// The sentence S3 publishes for that one condition, and it states the condition itself: the
/// attributes a self-copy has to change to be served. Nothing of the request is interpolated
/// into it — it names neither the bucket nor the key — which is why it lives here beside the
/// other code/message pairs rather than being worded in the handler, and why the generic
/// sentence the `InvalidRequest` SHAPE carries ([`INVALID_REQUEST_MESSAGE`]) is not what this
/// condition answers with.
pub const SELF_COPY_WITHOUT_CHANGE_MESSAGE: &str =
    "This copy request is illegal because it is trying to copy an object to itself without \
     changing the object's metadata, storage class, website redirect location or encryption \
     attributes.";

/// "The bucket policy does not exist."
pub const NO_SUCH_BUCKET_POLICY_CODE: &str = "NoSuchBucketPolicy";
/// See [`NO_SUCH_BUCKET_POLICY_CODE`].
pub const NO_SUCH_BUCKET_POLICY_MESSAGE: &str = "The bucket policy does not exist";
/// The code `GetBucketTagging` answers for a bucket no tag set has been stored on, with
/// HTTP 404 — the subresource-not-there case, answered the way
/// [`NO_SUCH_BUCKET_POLICY_CODE`] answers it for a bucket policy, because `TagSet` is a
/// required output member and so a 200 has no way to report "there is none".
pub const NO_SUCH_TAG_SET_CODE: &str = "NoSuchTagSet";
/// See [`NO_SUCH_TAG_SET_CODE`] — the fixed text S3 publishes for that code in its "Error
/// responses" list of error codes, which is what the service sends for this condition:
/// nothing in it comes from the request.
pub const NO_SUCH_TAG_SET_MESSAGE: &str = "The TagSet does not exist";
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

/// The statements `policy` carries, or `None` when the document is not a JSON object with
/// a `Statement` of the shape a policy writes it in: a list of statements, or a single
/// statement object.
///
/// One place locates a document's statements, so the grammar check
/// ([`is_valid_policy_document`]) and the public-access check
/// ([`policy_grants_public_access`]) cannot drift on what they are reading.
fn policy_statements(parsed: &::serde_json::Value) -> Option<Vec<&::serde_json::Value>> {
    match parsed.as_object()?.get("Statement")? {
        ::serde_json::Value::Array(list) => Some(list.iter().collect()),
        single @ ::serde_json::Value::Object(_) => Some(vec![single]),
        _ => None,
    }
}

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
    let Some(statements) = policy_statements(&parsed) else {
        return false;
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

/// The condition keys that, when a statement's `Condition` names one, keep that statement
/// from granting access to EVERYONE: each of them limits the grant to a set of fixed
/// values (an account, an organization, a VPC, an access point, a caller, a key), so the
/// statement reaches those callers rather than the public.
///
/// The catalog is the one Amazon S3 publishes under "The Meaning of 'Public'" (Amazon S3
/// User Guide, Blocking public access): a policy that grants access to a wildcard
/// principal is public "unless it contains a `Condition` that restricts access to a set of
/// fixed values" for one of these keys. Lower-case because IAM condition keys are
/// case-insensitive, so lookups fold the policy's spelling to this one.
const PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS: [&str; 13] = [
    "aws:principalarn",
    "aws:principalorgid",
    "aws:principalorgpaths",
    "aws:sourceaccount",
    "aws:sourcearn",
    "aws:sourceip",
    "aws:sourceowner",
    "aws:sourcevpc",
    "aws:sourcevpce",
    "aws:userid",
    "s3:dataaccesspointaccount",
    "s3:dataaccesspointarn",
    "s3:x-amz-server-side-encryption-aws-kms-key-id",
];

/// The one entry of [`PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS`] whose values are address
/// ranges rather than identifiers, so what "every value" means for it is
/// [`WHOLE_INTERNET_CIDRS`] rather than the bare wildcard.
const SOURCE_IP_CONDITION_KEY: &str = "aws:sourceip";

/// The `aws:SourceIp` values that name no fixed set of addresses: the whole IPv4 and IPv6
/// space.
///
/// From the same guide: an `aws:SourceIp` condition keeps a statement non-public only when
/// the addresses it names are not the entire internet, so a statement conditioned on one
/// of these still grants access to everyone.
const WHOLE_INTERNET_CIDRS: [&str; 2] = ["0.0.0.0/0", "::/0"];

/// The operand value that names no fixed set at all: it matches every value the key could
/// take, so a condition on it restricts nothing.
const MATCHES_EVERY_VALUE: &str = "*";

/// Whether `policy` grants access to the PUBLIC — the question
/// `GetBucketPolicyStatus`'s `PolicyStatus$IsPublic` answers about the bucket's policy.
///
/// The rule, from "The Meaning of 'Public'": a policy is public when ANY of its statements
/// both allows and reaches everyone — an `Effect` of `Allow` whose principal is the
/// wildcard (`"*"`, or `"*"` among the values of a principal map such as `{"AWS": "*"}`),
/// or an `Allow` that names a `NotPrincipal` and so grants to every principal it does not
/// list — and is not held back by a `Condition` restricting it to FIXED values of one of
/// [`PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS`]. A statement that grants nobody anything
/// (no principal at all), one whose `Effect` is `Deny`, and a document that is not a policy
/// are all non-public, as is a bucket carrying no policy — which is why the caller passes
/// the policy it HAS, and reports a bucket without one as not public.
pub fn policy_grants_public_access(policy: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(statements) = policy_statements(&parsed) else {
        return false;
    };
    statements.iter().any(|statement| {
        let Some(statement) = statement.as_object() else {
            return false;
        };
        let allows = statement
            .get("Effect")
            .and_then(::serde_json::Value::as_str)
            .is_some_and(|effect| effect == "Allow");
        allows && grants_every_principal(statement) && !condition_restricts_principals(statement)
    })
}

/// Whether `statement` grants to every principal: a wildcard `Principal`, or a
/// `NotPrincipal` (which names the principals the statement does NOT reach, so an `Allow`
/// carrying one reaches all the others).
fn grants_every_principal(statement: &::serde_json::Map<String, ::serde_json::Value>) -> bool {
    if statement.contains_key("NotPrincipal") {
        return true;
    }
    let Some(principal) = statement.get("Principal") else {
        // A statement with no principal grants nothing to anybody.
        return false;
    };
    principal_names_wildcard(principal)
}

/// Whether a `Principal` value names the wildcard, in any of the forms a policy writes it:
/// the bare string, a list carrying it, or a principal map (`{"AWS": "*"}`,
/// `{"AWS": ["*", …]}`) one of whose values is it.
fn principal_names_wildcard(principal: &::serde_json::Value) -> bool {
    match principal {
        ::serde_json::Value::String(value) => value == "*",
        ::serde_json::Value::Array(list) => list.iter().any(principal_names_wildcard),
        ::serde_json::Value::Object(map) => map.values().any(principal_names_wildcard),
        _ => false,
    }
}

/// Whether `statement`'s `Condition` holds its grant back from the public: it restricts
/// one of [`PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS`] to FIXED values, which is what the
/// guide requires — a key whose operand names every value it could take (the bare
/// wildcard, or the whole address space for `aws:SourceIp`) leaves the grant public, and a
/// list restricts only while every value in it is fixed.
fn condition_restricts_principals(
    statement: &::serde_json::Map<String, ::serde_json::Value>,
) -> bool {
    let Some(condition) = statement.get("Condition").and_then(::serde_json::Value::as_object) else {
        return false;
    };
    condition.values().any(|operands| {
        operands
            .as_object()
            .into_iter()
            .flatten()
            .any(|(key, values)| {
                let key = key.to_ascii_lowercase();
                PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS.contains(&key.as_str())
                    && names_fixed_values(&key, values)
            })
    })
}

/// Whether a condition key's operand names fixed values: a single fixed value, or a
/// non-empty list of them.
fn names_fixed_values(key: &str, values: &::serde_json::Value) -> bool {
    match values {
        ::serde_json::Value::String(value) => is_fixed_value(key, value.trim()),
        ::serde_json::Value::Array(list) => {
            !list.is_empty() && list.iter().all(|value| names_fixed_values(key, value))
        }
        _ => false,
    }
}

/// Whether one operand value of `key` is a fixed value rather than one matching everything
/// the key could be: the bare wildcard names no set, and for `aws:SourceIp` neither does
/// the whole address space ([`WHOLE_INTERNET_CIDRS`]).
fn is_fixed_value(key: &str, value: &str) -> bool {
    if value.is_empty() || value == MATCHES_EVERY_VALUE {
        return false;
    }
    key != SOURCE_IP_CONDITION_KEY || !WHOLE_INTERNET_CIDRS.contains(&value)
}

/// What a bucket policy says about `action` on `resource` for a caller that presents NO
/// credential.
///
/// Decided by the tree's ONE evaluation of IAM policy documents
/// (`service_common::iam_authorization`), which is the same rule every other emulated
/// service that enforces a resource policy answers with — an explicit `Deny` about the
/// request wins, otherwise an `Allow` about it grants, and a document that says nothing
/// about it states nothing. The caller is passed with NO principal spellings at all, which
/// is exactly what the anonymous caller is: only a statement whose principal element reaches
/// EVERY principal is about its request, and one naming an account or a role is not.
pub fn policy_decision_for_anonymous(
    policy: &str,
    action: &str,
    resource: &str,
) -> ::service_common::iam_authorization::Decision {
    ::service_common::iam_authorization::resource_decision(
        policy,
        &::service_common::iam_authorization::Request::new(action, resource, &[]),
    )
}

/// The IAM action an object READ is authorized under: "To use `GetObject`, you must have
/// the `READ` access to the object (or version)" (`GetObject`'s own documentation), which
/// is the same permission a read of that object's metadata needs — "A `HEAD` request has
/// the same options as a `GET` operation on an object" (`HeadObject`'s own documentation).
pub const GET_OBJECT_ACTION: &str = "s3:GetObject";

/// The IAM action that LISTING a bucket's contents is authorized under, and the one that
/// decides what a read of an address holding nothing is TOLD: "if the object that you
/// request doesn't exist, the error that Amazon S3 returns depends on whether you also have
/// the `s3:ListBucket` permission. If you have the `s3:ListBucket` permission on the bucket,
/// Amazon S3 returns an HTTP status code 404 Not Found error. If you don't have the
/// `s3:ListBucket` permission, Amazon S3 returns an HTTP status code 403 Access Denied
/// error" (`GetObject`'s own documentation).
pub const LIST_BUCKET_ACTION: &str = "s3:ListBucket";

/// The condition key saying whether the request arrived over a secure transport: "whether
/// the request was sent using SSL" (AWS General Reference, *Global condition context keys*).
///
/// This service ANSWERS it rather than leaving it open, because it knows: the emulator
/// serves every endpoint over TLS, so every request it judges arrived that way. A bucket
/// whose policy carries the guardrail clause AWS publishes for every bucket — a `Deny` of
/// everything that did NOT arrive over a secure transport — therefore refuses nothing here,
/// where a key with no value would leave that clause undecided and an undecided refusal
/// still refuses.
const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// The value that key carries, as IAM's boolean literals spell it.
const SECURE_TRANSPORT_VALUE: &str = "true";

/// The condition keys a request judged here carries a value for, and so DECLARES it models
/// — the keys whose absence from a policy's clause is decided rather than unknown. Every
/// other key keeps the fail-closed reading ([`Bucket::caller_is_denied`]).
const REQUEST_CONDITION_KEYS: &[&str] = &[SECURE_TRANSPORT_KEY];

/// The `Payer` value under which the REQUESTER, rather than the bucket owner, pays for a
/// download — one of the two values `Payer` takes (`PAYER_VALUES`), the other being
/// [`DEFAULT_PAYER`].
pub const PAYER_REQUESTER: &str = "Requester";

/// The code S3 answers a request its authorization refuses with, and the status and message
/// that code carries (Amazon S3 "Error responses", list of error codes: "AccessDenied —
/// Access Denied", HTTP 403).
pub const ACCESS_DENIED_CODE: &str = "AccessDenied";
/// See [`ACCESS_DENIED_CODE`].
pub const ACCESS_DENIED_STATUS: u16 = 403;
/// See [`ACCESS_DENIED_CODE`] — the text that same published list carries for it.
pub const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The message S3 refuses a request carrying no credential against a Requester Pays bucket
/// with: "Anonymous users cannot invoke requests against Requester Pays buckets" — fixed
/// text with no variable parts, because there is no principal to name.
///
/// A Requester Pays bucket charges the REQUESTER, and "requests to a Requester Pays bucket
/// must be authenticated: you cannot make anonymous requests"
/// (`PutBucketRequestPayment`'s own documentation of the feature), since an anonymous
/// request names no account to charge. The acknowledgement header does not rescue it for
/// the same reason.
pub const REQUESTER_PAYS_ANONYMOUS_MESSAGE: &str =
    "Anonymous users cannot invoke requests against Requester Pays buckets. Please \
     authenticate.";

/// The query parameters a request's CREDENTIAL can travel in when it carries no
/// `Authorization` header: the SigV4 presigned fields ("`X-Amz-Credential`", "`X-Amz-
/// Signature`") and the legacy presigned form's access key id (AWS General Reference,
/// *Authenticating requests: using query parameters*).
///
/// `pub(crate)`: the refusal wording asks the same question of the request it is answering
/// for ([`crate::service::sigv4`]), so both read one catalog.
pub(crate) const CREDENTIAL_QUERY_PARAMETERS: [&str; 3] =
    ["X-Amz-Credential", "X-Amz-Signature", "AWSAccessKeyId"];

/// Whether `parts` presents NO credential at all — the ANONYMOUS caller, which is what
/// decides whether an operation judges the request against what the bucket grants to
/// everyone.
///
/// "If you grant READ access to the anonymous user, the `GetObject` operation returns the
/// object without using an authorization header" (`GetObject`'s own documentation): a
/// request carrying a credential speaks for the account that credential names, whatever the
/// signature then verifies as, and one carrying none speaks for nobody. A credential travels
/// in the `Authorization` header of a signed request or in the query string of a presigned
/// one ([`CREDENTIAL_QUERY_PARAMETERS`]).
pub fn request_is_anonymous(parts: &::http::request::Parts) -> bool {
    let signed = parts
        .headers
        .get(::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.trim().is_empty());
    if signed {
        return false;
    }
    let query = parts.uri.query().unwrap_or_default();
    !CREDENTIAL_QUERY_PARAMETERS
        .iter()
        .any(|parameter| crate::service::query_carries(query, parameter))
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
            encryption: ObjectEncryption::default(),
            // The store mints the id; an object handed to `put_object` carries none.
            version_id: None,
            // A write places no retention; `replace_object_retention` is what does.
            retention: None,
            // Nor a legal hold; `replace_object_legal_hold` is what does.
            legal_hold: None,
            // A write initiates no restore; `restore_object_version` is what does.
            restore: None,
            // And it attaches no annotation; `put_object_annotation` is what does.
            annotations: BTreeMap::new(),
            // Nor an ACL; `replace_object_acl` is what does.
            acl: None,
            // And no checksum: a write that names no checksum algorithm stores none.
            checksum: None,
            // Nor parts: this object is stored whole.
            parts: Vec::new(),
            // And no replication: `replicate_write` is what settles one, from the bucket's
            // own replication configuration.
            replication: None,
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

    /// The same, filtering on one key-name `prefix` — what makes two configurations of one
    /// event distinguish the objects they publish.
    fn prefixed_queue_target(id: &str, prefix: &str, events: &[&str]) -> NotificationTarget {
        NotificationTarget {
            filter_rules: Some(vec![NotificationFilterRule {
                name: Some(FILTER_RULE_PREFIX.to_string()),
                value: Some(prefix.to_string()),
            }]),
            ..queue_target(Some(id), events)
        }
    }

    /// The published `Event` catalog fits in the one mask a configuration's events are
    /// read into, which is what lets the document-wide ambiguity walk cost one AND per
    /// pair of configurations.
    #[test]
    fn the_published_event_catalog_fits_one_event_mask() {
        assert!(
            EVENT_VALUES.len() <= EVENT_VALUES_FIT_ONE_MASK,
            "case \"the catalog's width\": {} published events do not fit the {} bits the \
             ambiguity walk reads them into",
            EVENT_VALUES.len(),
            EVENT_VALUES_FIT_ONE_MASK
        );
        // And every value of it is a distinct bit, so two configurations named by two
        // different events never read as sharing one.
        for (index, published) in EVENT_VALUES.iter().enumerate() {
            let target = queue_target(None, &[published]);
            assert_eq!(
                event_mask(&target),
                1 << index,
                "case {published:?}: expected the bit of its position in the catalog"
            );
        }
    }

    /// Every published `FilterRuleName` has a canonical spelling, each differs from the
    /// wire value only in case, and the canonical spelling resolves to itself — which is
    /// what makes a stored configuration as readable as the request that put it.
    #[test]
    fn every_published_filter_rule_name_has_a_canonical_spelling() {
        for published in FILTER_RULE_NAME_VALUES {
            let canonical = canonical_filter_rule_name(published).unwrap_or_else(|| {
                panic!("case {published:?}: a published filter rule name must canonicalise")
            });
            assert!(
                canonical.eq_ignore_ascii_case(published),
                "case {published:?}: the canonical spelling must differ only in case, got \
                 {canonical:?}"
            );
            assert_eq!(
                canonical_filter_rule_name(canonical),
                Some(canonical),
                "case {published:?}: the canonical spelling must resolve to itself, got {:?}",
                canonical_filter_rule_name(canonical)
            );
            assert_eq!(
                filter_rule_kind(canonical),
                Some(*published),
                "case {published:?}: the canonical spelling must name the wire kind, got {:?}",
                filter_rule_kind(canonical)
            );
        }
        assert_eq!(
            canonical_filter_rule_name("extension"),
            None,
            "case \"a name outside the enum\": nothing outside the published set may \
             canonicalise"
        );
    }

    #[test]
    fn the_default_store_holds_every_published_object() {
        // The store a fresh engine hands this service already holds what AWS publishes,
        // because nothing creates those buckets. Walked over the whole catalog, so an
        // entry added to it is covered without a row being written here.
        let store = ComAmazonawsS3Amazons3State::default();

        for published in aws_published::PUBLISHED_BUCKETS {
            let held = store
                .bucket(published.name)
                .unwrap_or_else(|| panic!("the published bucket {} is held", published.name));
            assert!(
                held.is_aws_published(),
                "case {:?}: a published bucket is marked as AWS's own",
                published.name
            );
            assert_eq!(
                held.region, published.region,
                "case {:?}: expected the published Region {:?}, got {:?}",
                published.name, published.region, held.region
            );
            assert!(
                store
                    .account_buckets()
                    .all(|(name, _)| name != published.name),
                "case {:?}: a published bucket is not one of the account's own",
                published.name
            );

            let keys: Vec<&str> = held.objects().map(|(key, _)| key).collect();
            let mut expected: Vec<&str> =
                published.objects.iter().map(|object| object.key).collect();
            expected.sort_unstable();
            assert_eq!(
                keys, expected,
                "case {:?}: expected exactly the published keys {expected:?}, got {keys:?}",
                published.name
            );

            for object in published.objects {
                let stored = held
                    .object(object.key)
                    .unwrap_or_else(|| panic!("the published key {} is held", object.key));
                assert_eq!(
                    stored.body,
                    object.body.as_bytes(),
                    "case {:?}: expected the published bytes",
                    object.key
                );
                assert_eq!(
                    stored.etag,
                    object_etag(object.body.as_bytes()),
                    "case {:?}: expected the entity tag of the published bytes, got {:?}",
                    object.key,
                    stored.etag
                );
                assert_eq!(
                    stored.content_type.as_deref(),
                    Some(object.content_type),
                    "case {:?}: expected the published Content-Type, got {:?}",
                    object.key,
                    stored.content_type
                );
            }
        }
    }

    #[test]
    fn the_published_catalog_takes_no_room_from_the_callers_own_objects() {
        // The whole-store budgets bound what a CALLER stored; the published catalog is
        // fixed by the code, so it neither consumes them nor is bounded by them. A fresh
        // store therefore reports an empty caller half even while it holds the catalog.
        let store = ComAmazonawsS3Amazons3State::default();
        assert!(
            store.buckets().count() > 0,
            "the default store holds the published catalog"
        );
        assert_eq!(
            store.caller_retained_bytes(),
            0,
            "a store nobody has written to retains no caller bytes, got {}",
            store.caller_retained_bytes()
        );
        assert_eq!(
            store.caller_stored_versions(),
            0,
            "a store nobody has written to holds no caller versions, got {}",
            store.caller_stored_versions()
        );
    }

    #[test]
    fn a_caller_write_into_a_published_bucket_counts_against_the_whole_store_budgets() {
        // The allowance the whole-store budgets subtract for the published catalog is
        // bounded BOTH ways, and this is the branch that pins it.
        let mut store = ComAmazonawsS3Amazons3State::default();
        let published = aws_published::PUBLISHED_BUCKETS
            .first()
            .expect("the catalog publishes at least one bucket");
        assert_eq!(
            store.caller_retained_bytes(),
            0,
            "a store nobody has written to retains no caller bytes"
        );

        // A caller's write lands in a published bucket, so the live footprint rises ABOVE
        // the seeded amount: the subtraction stays at the seeded amount and every byte the
        // caller wrote is measured, exactly as it would be in a bucket of its own.
        let written = object(64);
        let footprint = written.retained_bytes();
        assert_eq!(
            store.put_object(published.name, "written-by-the-caller", written),
            Some(ObjectWrite::Created),
            "the write into a published bucket is recorded"
        );
        assert_eq!(
            store.caller_retained_bytes(),
            footprint,
            "a caller's write into a published bucket counts in full, got {}",
            store.caller_retained_bytes()
        );
        assert_eq!(
            store.caller_stored_versions(),
            1,
            "a caller's write into a published bucket counts as one version, got {}",
            store.caller_stored_versions()
        );

        // And a delete OUT of a published bucket takes the live footprint below the seeded
        // amount, so the subtraction shrinks with it rather than handing out room for
        // bytes the store no longer holds.
        let key = published
            .objects
            .first()
            .expect("the catalog publishes at least one object")
            .key;
        assert_eq!(
            store.remove_object(published.name, key),
            Some(ObjectDeletion::Removed),
            "the published key was there to delete"
        );
        let total: usize = store.buckets.values().map(Bucket::retained_bytes).sum();
        let still_published: usize = store
            .buckets
            .values()
            .filter(|held| held.aws_published)
            .map(Bucket::retained_bytes)
            .sum();
        assert!(
            store.caller_retained_bytes() >= total - still_published,
            "the subtraction may not exceed what the published buckets still hold: store \
             retains {total}, published half {still_published}, caller half reported as {}",
            store.caller_retained_bytes()
        );

        // Taking the caller's own object away again leaves the caller half empty.
        assert_eq!(
            store.remove_object(published.name, "written-by-the-caller"),
            Some(ObjectDeletion::Removed),
            "the caller's own object was there to delete"
        );
        assert_eq!(
            store.caller_retained_bytes(),
            0,
            "with the caller's object gone the caller half is empty again, got {}",
            store.caller_retained_bytes()
        );
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
                        name: Some("extension".to_string()),
                        value: Some(".json".to_string()),
                    }]),
                    ..queue_target(None, &["s3:ObjectCreated:Put"])
                }]),
                Some("prefix or suffix"),
            ),
            (
                // The spelling a STORED rule is held under, which this same function has
                // to admit: the store is what it guards, and the store holds the canonical
                // name.
                "a filter rule under its canonical name",
                queue_configuration(vec![NotificationTarget {
                    filter_rules: Some(vec![NotificationFilterRule {
                        name: Some("Prefix".to_string()),
                        value: Some("logs/".to_string()),
                    }]),
                    ..queue_target(None, &["s3:ObjectCreated:Put"])
                }]),
                None,
            ),
            (
                "one filter carrying two rules of one kind",
                queue_configuration(vec![NotificationTarget {
                    filter_rules: Some(vec![
                        NotificationFilterRule {
                            name: Some("Suffix".to_string()),
                            value: Some(".csv".to_string()),
                        },
                        NotificationFilterRule {
                            name: Some("suffix".to_string()),
                            value: Some(".tsv".to_string()),
                        },
                    ]),
                    ..queue_target(None, &["s3:ObjectCreated:Put"])
                }]),
                Some("Cannot specify more than one suffix rule in a filter."),
            ),
            (
                "two configurations of one event whose prefixes overlap",
                queue_configuration(vec![
                    prefixed_queue_target("a", "CategoryA/", &["s3:ObjectCreated:*"]),
                    prefixed_queue_target("b", "CategoryA/sub/", &["s3:ObjectCreated:*"]),
                ]),
                Some("ambiguously defined"),
            ),
            (
                "two configurations of one event whose prefixes are disjoint",
                queue_configuration(vec![
                    prefixed_queue_target("a", "CategoryA/", &["s3:ObjectCreated:*"]),
                    prefixed_queue_target("b", "CategoryC/", &["s3:ObjectCreated:*"]),
                ]),
                None,
            ),
            (
                // The wildcard form and a literal value of its type are two distinct
                // rules, so a bucket may route an event class and one event of it to
                // different destinations.
                "a wildcard event beside a literal one of the same type",
                queue_configuration(vec![
                    prefixed_queue_target("a", "shared/", &["s3:ObjectCreated:*"]),
                    prefixed_queue_target("b", "shared/", &["s3:ObjectCreated:Put"]),
                ]),
                None,
            ),
            (
                "two configurations naming the same event with no filter at all",
                queue_configuration(vec![
                    queue_target(Some("a"), &["s3:ObjectCreated:Put"]),
                    queue_target(Some("b"), &["s3:ObjectCreated:Put"]),
                ]),
                Some("ambiguously defined"),
            ),
            (
                // The suffixes decide it: two configurations of one event whose prefixes
                // overlap are kept when no key can carry both suffixes.
                "overlapping prefixes with disjoint suffixes",
                queue_configuration(vec![
                    NotificationTarget {
                        filter_rules: Some(vec![
                            NotificationFilterRule {
                                name: Some("Prefix".to_string()),
                                value: Some("CategoryA/".to_string()),
                            },
                            NotificationFilterRule {
                                name: Some("Suffix".to_string()),
                                value: Some(".csv".to_string()),
                            },
                        ]),
                        ..queue_target(Some("a"), &["s3:ObjectCreated:*"])
                    },
                    NotificationTarget {
                        filter_rules: Some(vec![
                            NotificationFilterRule {
                                name: Some("Prefix".to_string()),
                                value: Some("CategoryA/sub/".to_string()),
                            },
                            NotificationFilterRule {
                                name: Some("Suffix".to_string()),
                                value: Some(".json".to_string()),
                            },
                        ]),
                        ..queue_target(Some("b"), &["s3:ObjectCreated:*"])
                    },
                ]),
                None,
            ),
            (
                "the same prefix for two events that cannot both happen",
                queue_configuration(vec![
                    prefixed_queue_target("a", "shared/", &["s3:ObjectCreated:Put"]),
                    prefixed_queue_target("b", "shared/", &["s3:ObjectRemoved:Delete"]),
                ]),
                None,
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
                    prefixed_queue_target("twice", "one/", &["s3:ObjectCreated:Put"]),
                    prefixed_queue_target("twice", "two/", &["s3:ObjectCreated:Post"]),
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
                // Each on a prefix of its own, because two configurations of one event
                // that both match every key are ambiguous however many there are.
                "exactly as many configurations as a bucket may carry",
                queue_configuration(
                    (0..MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET)
                        .map(|index| {
                            prefixed_queue_target(
                                &format!("at-the-bound-{index}"),
                                &format!("category-{index}/"),
                                &["s3:ObjectCreated:Put"],
                            )
                        })
                        .collect(),
                ),
                None,
            ),
            (
                // The COUNT is what this row is about, and the count is judged before the
                // document's own rules.
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
            .map(|(key, _)| key)
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
            Some(ObjectDeletion::Removed),
            "removing a stored key on a bucket with no versioning state takes it away"
        );
        assert_eq!(
            store.remove_object("one", "doomed"),
            Some(ObjectDeletion::NothingThere),
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
            Some(ObjectDeletion::Removed),
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
            .map(StoredVersion::version_id)
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

        // Only a delete that NAMES a version releases it; a delete that names none lays a
        // delete marker over the key and keeps every version behind it.
        let displaced = ids[0].expect("the newest version carries an id").to_string();
        let laid = store.remove_object("one", "key");
        let Some(ObjectDeletion::MarkerLaid {
            version_id: Some(marker_id),
            displaced_object,
        }) = laid
        else {
            panic!("a delete on a versioning-enabled bucket lays a delete marker, got {laid:?}");
        };
        assert!(
            displaced_object,
            "the marker hid a current object, got {displaced_object}"
        );
        let bucket = store.bucket("one").expect("bucket one exists");
        assert!(
            bucket.object("key").is_none(),
            "the delete took the key out of the current view"
        );
        assert!(
            bucket
                .current_version("key")
                .is_some_and(|held| held.as_delete_marker().is_some()),
            "the delete marker is the key's current version"
        );
        assert_eq!(
            bucket.object_versions("key").len(),
            4,
            "the marker is stored beside the three versions it hides, got {}",
            bucket.object_versions("key").len()
        );
        assert!(
            bucket.object_version("key", &displaced).is_some(),
            "the displaced version is still addressable by its id"
        );
        assert!(
            bucket.object_version("key", &marker_id).is_none(),
            "the marker holds no object to read by its id"
        );
        assert!(
            bucket
                .stored_version("key", &marker_id)
                .is_some_and(|held| held.as_delete_marker().is_some()),
            "the marker is addressable by the id it was minted with"
        );
        assert_eq!(
            store.remove_object_version("one", "key", &marker_id),
            Some(true),
            "naming the marker's id removes the marker"
        );
        let bucket = store.bucket("one").expect("bucket one exists");
        assert!(
            bucket.object("key").is_some(),
            "removing the marker makes the version it hid current again"
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
