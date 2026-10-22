// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3-amazons3 service.

use std::collections::BTreeMap;

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

/// One general purpose bucket this emulator holds.
#[derive(Debug, Clone)]
pub struct Bucket {
    /// The Region the bucket lives in — what `HeadBucket` reports as
    /// `x-amz-bucket-region` and what decides the form of `CreateBucket`'s `Location`.
    pub region: String,
    /// The bucket's tag set, in the order the tagging request carried it.
    ///
    /// Private, and empty when no tag set is associated with the bucket: "no tags" and
    /// "an empty tag set" are the same state on AWS, which is why `PutBucketTagging` with
    /// an empty `TagSet` and `DeleteBucketTagging` leave a bucket in the same place.
    tags: Vec<BucketTag>,
}

impl Bucket {
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

    /// Remove the bucket's tag set, leaving no tags associated with it.
    pub fn clear_tags(&mut self) {
        self.tags.clear();
    }
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
    /// The bucket named `name`, or `None` when this emulator holds no such bucket.
    pub fn bucket(&self, name: &str) -> Option<&Bucket> {
        self.buckets.get(name)
    }

    /// The bucket named `name` for writing, or `None` when this emulator holds no such
    /// bucket — so a subresource write (a tag set, say) refuses a name that is not a
    /// bucket instead of creating one.
    pub fn bucket_mut(&mut self, name: &str) -> Option<&mut Bucket> {
        self.buckets.get_mut(name)
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
                // A fresh bucket carries no tags; `PutBucketTagging` is what associates a
                // tag set with it.
                tags: Vec::new(),
            },
        );
        BucketCreation::Created
    }
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
