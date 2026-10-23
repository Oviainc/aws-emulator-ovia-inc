// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecr-amazonec2containerregistry-v20150921 service.
//!
//! One registry (this emulator's account) holding repositories by name, each holding
//! its images by digest plus a tag index, beside the registry-wide settings a caller can
//! configure (its replication rules, its signing configuration) and the registry-level
//! resources ECR keys by prefix or principal (pull through cache rules, repository
//! creation templates and pull time update exclusions). Everything an operation answers
//! with is derived from here; nothing is re-synthesised from a request.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    EncryptionConfiguration, EncryptionConfigurationForRepositoryCreationTemplate, ImageDetail,
    ImageIdentifier, ImageScanningConfiguration, ImageTagMutabilityExclusionFilter,
    LifecyclePolicyPreviewResult, LifecyclePolicyPreviewSummary, LifecyclePolicyRuleAction,
    PullThroughCacheRule, ReplicationConfiguration, ReplicationRule, Repository,
    RepositoryCreationTemplate, RepositoryScanningConfiguration, SigningConfiguration, Tag,
    TransitioningImageTotalCount, IMAGE_ACTION_TYPE_VALUES,
    LIFECYCLE_POLICY_TARGET_STORAGE_CLASS_VALUES, TAG_STATUS_VALUES,
};

/// `CreateRepository`'s documented default: "If this parameter is omitted, the default
/// setting of MUTABLE will be used which will allow image tags to be overwritten."
pub const DEFAULT_IMAGE_TAG_MUTABILITY: &str = "MUTABLE";

/// The default `encryptionConfiguration.encryptionType`: ECR encrypts repository
/// contents with `AES256` unless the caller asks for a KMS key.
pub const DEFAULT_ENCRYPTION_TYPE: &str = S3_MANAGED_ENCRYPTION_TYPE;

/// The encryption type that encrypts with an Amazon S3-managed key, and therefore the
/// one that takes no `kmsKey` of the caller's: the `KMS` types are the ones a key
/// belongs to.
pub const S3_MANAGED_ENCRYPTION_TYPE: &str = "AES256";

/// The ceiling on how many repositories this store holds, how many images one
/// repository holds, and how many tags point into one repository: the three caps that
/// bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create or a push is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeRepositories` and `DescribeImages` must still answer what `CreateRepository`
/// and `PutImage` stored. The two operations that grow the store refuse past these caps
/// with the `LimitExceededException` both of them model ("the operation did not succeed
/// because it would have exceeded a service limit for your account") and no read can
/// return. The image cap is the one that matters most: `imageManifest` is modelled
/// `@length(1..4194304)`, so each image carries up to 4 MiB of caller-supplied text.
/// The numbers are the emulator's own bound, not a published AWS default — AWS
/// publishes the repository and image quotas as adjustable, so no single value is the
/// contract — and they are set high enough that no ordinary workflow meets one.
pub const MAX_REPOSITORIES: usize = 1_000;

/// See [`MAX_REPOSITORIES`].
pub const MAX_IMAGES_PER_REPOSITORY: usize = 1_000;

/// See [`MAX_REPOSITORIES`]. This one bounds `tag_index`, which grows by one entry per
/// push that introduces a tag the repository did not carry.
pub const MAX_TAGS_PER_REPOSITORY: usize = 10_000;

/// See [`MAX_REPOSITORIES`]: the same reasoning for the pull through cache rule table,
/// which `CreatePullThroughCacheRule` grows by one entry per rule. The number is this
/// emulator's own bound rather than a published AWS default (AWS publishes the pull
/// through cache rule quota as adjustable), and a create past it is refused with the
/// `LimitExceededException` the operation models.
pub const MAX_PULL_THROUGH_CACHE_RULES: usize = 1_000;

/// See [`MAX_REPOSITORIES`]. This one bounds `creation_templates`, which grows by one
/// entry per `CreateRepositoryCreationTemplate` that introduces a new namespace prefix;
/// past it the create is refused with the `LimitExceededException` that operation models.
pub const MAX_REPOSITORY_CREATION_TEMPLATES: usize = 1_000;

/// See [`MAX_REPOSITORIES`]. This one bounds `pull_time_update_exclusions`, which grows
/// by one entry per `RegisterPullTimeUpdateExclusion` naming a principal the registry
/// does not already exclude; past it a register is refused with the
/// `LimitExceededException` that operation models. AWS publishes no fixed quota for
/// pull time update exclusions, so this is the emulator's own bound.
pub const MAX_PULL_TIME_UPDATE_EXCLUSIONS: usize = 1_000;

/// The `ImageStatus` of an image in standard storage. Every image this emulator
/// stores is in standard storage (moving one to archive is `UpdateImageStorageClass`,
/// still a 501 stub), so every stored image reports `ACTIVE`.
pub const ACTIVE_IMAGE_STATUS: &str = "ACTIVE";

/// The `ScanFrequency` of a repository whose scan on push is switched on: a push is what
/// triggers its scan.
pub const SCAN_ON_PUSH_FREQUENCY: &str = "SCAN_ON_PUSH";

/// The `ScanFrequency` of a repository that is scanned only when asked: "If scan on push
/// is not specified, then the MANUAL scan frequency is set by default"
/// (`RegistryScanningRule.scanFrequency`).
pub const MANUAL_SCAN_FREQUENCY: &str = "MANUAL";

/// Page size used when a paginated operation's `maxResults` is absent, and the ceiling
/// on the page a request may ask for: the ECR API reference documents 100 as the default
/// and as the maximum number of results per page for `DescribeRepositories`,
/// `ListImages` and `DescribeImages`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// "The maximum number of tags that can be applied to a repository is 50."
/// (`TooManyTagsException`'s own documentation).
pub const MAX_TAGS: usize = 50;

/// "Tag keys can have a maximum character length of 128 characters, and tag values can
/// have a maximum length of 256 characters." (`InvalidTagParameterException`).
pub const MAX_TAG_KEY_LEN: usize = 128;
/// See [`MAX_TAG_KEY_LEN`].
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The tag-key namespace AWS keeps for itself: "the `aws:` prefix is reserved for AWS
/// use [...] you can't edit or delete a tag key or value with this prefix" (AWS General
/// Reference, "Tagging AWS resources"). A request that tries to create or remove a key
/// in it is refused rather than stored, so no caller-supplied tag can ever impersonate
/// one AWS set.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The region an unsigned request is served in: a request that carries no SigV4
/// credential scope names no region, and ECR's ARNs and repository URIs are regional.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the `StartLifecyclePolicyPreview` call that starts it the clock owes a
/// lifecycle policy preview its `IN_PROGRESS` -> `COMPLETE` transition.
///
/// AWS publishes no duration for a preview run, so this is a CHOSEN value inside the
/// 250–750 ms band this project fixes for a clock-owed transition. It is never zero, so a
/// caller that starts a second preview straight after the first still meets the
/// `IN_PROGRESS` state and the `LifecyclePolicyPreviewInProgressException` it answers
/// with. It is stored as a due instant and never slept for: the completion is owed
/// whether or not anybody calls again.
pub const LIFECYCLE_PREVIEW_DELAY: Duration = Duration::from_millis(400);

/// The `LifecyclePolicyPreviewStatus` a preview carries while it is still running.
///
/// The other two values the enum carries are not reachable here: a policy document that
/// does not parse is refused at the start call rather than stored as a `FAILED` preview,
/// and no published lifetime after which a stored preview goes `EXPIRED` exists to drive
/// that one.
pub const PREVIEW_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `LifecyclePolicyPreviewStatus` a preview carries once the clock has made its
/// completion due. See [`PREVIEW_STATUS_IN_PROGRESS`].
pub const PREVIEW_STATUS_COMPLETE: &str = "COMPLETE";

/// The `LifecyclePolicyStorageClass` every image this emulator stores is in: moving one
/// to archive is `UpdateImageStorageClass`, still a 501 stub, so every previewed image
/// reports standard storage.
pub const STANDARD_STORAGE_CLASS: &str = "STANDARD";

/// The `ImageActionType` an `expire` rule applies — the modelled spelling (from
/// `IMAGE_ACTION_TYPE_VALUES`) of what a policy document writes in lower case.
pub const EXPIRE_ACTION_TYPE: &str = "EXPIRE";

/// The `ImageActionType` a `transition` rule applies. See [`EXPIRE_ACTION_TYPE`].
pub const TRANSITION_ACTION_TYPE: &str = "TRANSITION";

/// The `TagStatus` a selection with no tag list asks for.
pub const ANY_TAG_STATUS: &str = "ANY";

/// The only `selection.countUnit` a lifecycle policy rule carries: a `sinceImagePushed`
/// count is a number of days.
pub const COUNT_UNIT_DAYS: &str = "days";

/// The unit a `sinceImagePushed` rule counts in, in seconds.
const SECONDS_PER_DAY: u64 = 86_400;

/// One account setting of this registry: the setting's name, the values
/// `PutAccountSetting` accepts for it, and the value the registry reports for it before
/// anybody sets one.
#[derive(Debug, Clone, Copy)]
pub struct AccountSettingSpec {
    /// The name `GetAccountSetting`/`PutAccountSetting` address this setting by.
    pub name: &'static str,
    /// The values `PutAccountSetting` may set this setting to.
    pub settable_values: &'static [&'static str],
    /// The value this setting has until a `PutAccountSetting` changes it.
    pub default_value: &'static str,
}

/// Every account setting this registry serves.
///
/// The names come from the model's own documentation of
/// `PutAccountSettingRequest$name` ("The name of the account setting, such as
/// `BASIC_SCAN_TYPE_VERSION`, `REGISTRY_POLICY_SCOPE`, or `BLOB_MOUNTING`") and the
/// settable values from `PutAccountSettingRequest$value` ("Valid value for basic scan
/// type: `AWS_NATIVE`. Valid values for registry policy scope: `V2`. Valid values for
/// blob mounting: `ENABLED` or `DISABLED`"), read with
/// `smithy_query(query="doc",shape="PutAccountSettingRequest",member="name"|"value")`.
///
/// Each default is the value of a registry nobody has configured. None of the three is
/// stated by the model, which documents only what a caller may SET, so each is an
/// inference with its reasoning below — and this constant is the single place to correct
/// one if a recorded response ever contradicts it:
///
/// * `BASIC_SCAN_TYPE_VERSION` — `AWS_NATIVE` is the only version the model lets a
///   caller set, so it is also the one in force before any call.
/// * `REGISTRY_POLICY_SCOPE` — `V2` is the only scope the model lets a caller set, so
///   the move to it is one-way and the scope a registry holds beforehand is the other
///   one, `V1`.
/// * `BLOB_MOUNTING` — settable either way and with no documented default, so this one
///   is the weakest inference of the three: a feature nobody has asked for is taken to
///   be `DISABLED`.
pub const ACCOUNT_SETTINGS: &[AccountSettingSpec] = &[
    AccountSettingSpec {
        name: "BASIC_SCAN_TYPE_VERSION",
        settable_values: &["AWS_NATIVE"],
        default_value: "AWS_NATIVE",
    },
    AccountSettingSpec {
        name: "BLOB_MOUNTING",
        settable_values: &["ENABLED", "DISABLED"],
        default_value: "DISABLED",
    },
    AccountSettingSpec {
        name: "REGISTRY_POLICY_SCOPE",
        settable_values: &["V2"],
        default_value: "V1",
    },
];

/// The setting `name` addresses, or [`Fault::InvalidParameter`] naming the member and
/// the set it must come from.
pub fn account_setting_spec(name: &str) -> Result<&'static AccountSettingSpec, Fault> {
    ACCOUNT_SETTINGS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| {
            Fault::InvalidParameter(format!(
                "Invalid parameter at 'name' failed to satisfy constraint: 'Member must \
                 satisfy enum value set: [{}]'",
                ACCOUNT_SETTINGS
                    .iter()
                    .map(|spec| spec.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// The `LayerAvailability` of a blob the repository holds. The other two values the
/// enum carries are not reachable from this store: `ARCHIVED` belongs to an image whose
/// storage class is archive (`UpdateImageStorageClass`, still a 501 stub, is the only
/// thing that archives one, so every stored image is [`ACTIVE_IMAGE_STATUS`]), and
/// `UNAVAILABLE` describes a blob the registry has a record of but cannot serve, which
/// this store never holds — a blob is either in a repository or absent from it.
pub const AVAILABLE_LAYER_AVAILABILITY: &str = "AVAILABLE";

/// The `LayerFailureCode` for a well-formed digest no repository blob answers to.
pub const MISSING_LAYER_DIGEST_FAILURE: &str = "MissingLayerDigest";

/// The `LayerFailureCode` for a requested digest that is not a content digest at all.
pub const INVALID_LAYER_DIGEST_FAILURE: &str = "InvalidLayerDigest";

/// A failure a shared state helper can report, mapped by each operation onto the
/// modelled error its own model declares.
#[derive(Debug, Clone)]
pub enum Fault {
    /// No repository of that name in this registry — `RepositoryNotFoundException`.
    RepositoryNotFound(String),
    /// The request is self-inconsistent or names another registry —
    /// `InvalidParameterException`.
    InvalidParameter(String),
    /// An `imageIds` entry matches no stored image — `ImageNotFoundException`.
    ImageNotFound(String),
    /// The repository exists but carries no lifecycle policy —
    /// `LifecyclePolicyNotFoundException`.
    LifecyclePolicyNotFound(String),
    /// A tag key or value is longer than a tag may be — `InvalidTagParameterException`.
    InvalidTagParameter(String),
    /// The resource would end up carrying more tags than one may hold —
    /// `TooManyTagsException`.
    TooManyTags(String),
}

impl Fault {
    /// The message this failure renders with.
    pub fn message(&self) -> &str {
        match self {
            Self::RepositoryNotFound(m)
            | Self::InvalidParameter(m)
            | Self::ImageNotFound(m)
            | Self::LifecyclePolicyNotFound(m)
            | Self::InvalidTagParameter(m)
            | Self::TooManyTags(m) => m,
        }
    }
}

/// One blob a repository holds: the digest it is addressed by, plus the size and media
/// type the manifest that references it declares for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerBlob {
    pub digest: String,
    pub size: Option<i64>,
    pub media_type: Option<String>,
}

/// One stored image: the manifest as pushed, its digest, and every tag pointing at it.
#[derive(Debug, Clone)]
pub struct ImageRecord {
    pub digest: String,
    pub tags: BTreeSet<String>,
    pub manifest: String,
    pub manifest_media_type: Option<String>,
    pub artifact_media_type: Option<String>,
    pub size_in_bytes: i64,
    pub pushed_at: SystemTime,
}

impl ImageRecord {
    /// The `ImageDetail` `DescribeImages` reports for this image.
    pub fn to_detail(&self, registry_id: &str, repository_name: &str) -> ImageDetail {
        ImageDetail {
            registry_id: Some(registry_id.to_string()),
            repository_name: Some(repository_name.to_string()),
            image_digest: Some(self.digest.clone()),
            // ECR omits `imageTags` for an untagged image rather than reporting [].
            image_tags: if self.tags.is_empty() {
                None
            } else {
                Some(self.tags.iter().cloned().collect())
            },
            image_size_in_bytes: Some(self.size_in_bytes),
            image_pushed_at: Some(::aws_smithy_types::DateTime::from(self.pushed_at)),
            image_manifest_media_type: self.manifest_media_type.clone(),
            artifact_media_type: self.artifact_media_type.clone(),
            image_status: Some(ACTIVE_IMAGE_STATUS.to_string()),
            ..ImageDetail::default()
        }
    }

    /// The identifier that addresses this image for a request that named `tag`.
    ///
    /// The digest is this image's own — the stored one, never the one a request guessed
    /// — and the tag is reported only when it really points at this image.
    pub fn identifier(&self, tag: Option<&str>) -> ImageIdentifier {
        ImageIdentifier {
            image_digest: Some(self.digest.clone()),
            image_tag: tag
                .filter(|tag| self.tags.contains(*tag))
                .map(ToString::to_string),
        }
    }

    /// The manifest as a JSON document, for the members a registry reads out of it
    /// (an OCI artifact's `subject`, `artifactType` and `annotations`).
    ///
    /// `None` when the stored manifest is not JSON at all, which is what a caller who
    /// pushed something else gets: no member can be read out of it.
    pub fn manifest_document(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.manifest).ok()
    }

    /// The `(digest, tag)` pairs this image is addressable by: one per tag, or a single
    /// tagless identifier when nothing points at it.
    pub fn identifiers(&self) -> Vec<ImageIdentifier> {
        if self.tags.is_empty() {
            return vec![ImageIdentifier {
                image_digest: Some(self.digest.clone()),
                image_tag: None,
            }];
        }
        self.tags
            .iter()
            .map(|tag| ImageIdentifier {
                image_digest: Some(self.digest.clone()),
                image_tag: Some(tag.clone()),
            })
            .collect()
    }
}

/// See [`MAX_REPOSITORIES`]. This one bounds `uploads`, which grows by one entry per
/// `InitiateLayerUpload` and shrinks when an upload is completed; an upload a client
/// opened and then abandoned is never reclaimed — the cap is for the life of the
/// process, and [`LayerUpload::started_at`] is a record of when the upload was opened,
/// not an expiry the store acts on — so the registry needs a ceiling on how many
/// in-flight layer uploads it holds at once. Past it the open is refused with the
/// `ServerException` `InitiateLayerUpload` models: reaching the ceiling is this
/// registry's own capacity, not a fault in the request.
pub const MAX_LAYER_UPLOADS: usize = 1_000;

/// The `partSize` `InitiateLayerUpload` reports: the size ECR tells the client to send
/// future layer parts in.
///
/// AWS publishes no single number for it — it is the registry's own answer, which is
/// why the client is told it rather than assuming one — so this is the emulator's
/// choice, bounded by the only published constraint on a part: the model caps
/// `layerPartBlob` at 20971520 bytes, and a part of `partSize` bytes must fit under
/// that cap.
pub const REPORTED_LAYER_PART_SIZE: i64 = 10 * 1024 * 1024;

/// See [`MAX_REPOSITORIES`]: the ceiling on how many bytes ONE in-flight upload
/// accumulates. `UploadLayerPart` is the only thing that grows it — by up to 20971520
/// bytes per call, the model's cap on `layerPartBlob` — and the bytes stay in memory
/// until the upload is completed or the process ends, so the registry needs a ceiling
/// on the layer a client can stream into it. Past it the part is refused with the
/// `LimitExceededException` `UploadLayerPart` models ("the operation did not succeed
/// because it would have exceeded a service limit for your account") and no byte of it
/// is kept. AWS publishes no fixed maximum layer size, so the number is this
/// emulator's own bound, set well above any layer an ordinary push sends.
pub const MAX_LAYER_UPLOAD_BYTES: usize = 128 * 1024 * 1024;

/// See [`MAX_REPOSITORIES`]. This one bounds [`Repo::layer_blobs`], which grows by one
/// entry per `CompleteLayerUpload` that registers a blob the repository did not already
/// hold, and nothing shrinks: a registered blob stays for the life of the process, so
/// the registry needs a ceiling on how many blobs one repository holds a record of.
/// Past it the completion is refused with the `ServerException`
/// `CompleteLayerUpload` models — reaching the ceiling is this registry's own capacity,
/// not a fault in the request, and that operation models no limit error. AWS publishes
/// no fixed maximum layer count per repository, so the number is this emulator's own
/// bound, set well above the layer count of any ordinary image set.
pub const MAX_LAYERS_PER_REPOSITORY: usize = 10_000;

/// One in-flight layer upload: what `InitiateLayerUpload` opened, addressed by the
/// upload id it handed the client.
///
/// `UploadLayerPart` streams the layer into it one part at a time and
/// `CompleteLayerUpload` is what closes it, registering the assembled blob on the
/// repository ([`Repo::layer_blobs`]) and dropping the upload, so what is kept here is
/// what opening the upload decided — which repository it is for, the part size the
/// client was told to send, when it was opened — plus the layer bytes received so far.
#[derive(Debug, Clone)]
pub struct LayerUpload {
    /// The repository the upload was opened against. An upload id is only valid for
    /// the repository it was issued for, so every later call names both.
    pub repository_name: String,
    /// The `partSize` reported to the client when the upload was opened.
    pub part_size: i64,
    /// When `InitiateLayerUpload` opened it. Nothing reads it: an upload is closed by
    /// `CompleteLayerUpload`, and AWS publishes no age at which an in-flight upload
    /// expires, so this registry expires none.
    pub started_at: SystemTime,
    /// The layer bytes accepted so far, in the order they arrived. Each part starts
    /// where the previous one ended, so this is the prefix of the layer the registry
    /// holds and its length is both the byte count received and the offset the next
    /// part must start at — see [`LayerUpload::receive_part`].
    pub received_bytes: Vec<u8>,
}

/// Why a layer part is not accepted into an upload. The caller-facing error each one
/// answers with belongs to the operation (`UploadLayerPart`'s
/// `InvalidLayerPartException` and `LimitExceededException`); what is decided here is
/// whether the part may join the upload at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerPartRejection {
    /// `partFirstByte` and `partLastByte` do not describe the blob sent with them: the
    /// range spans a different number of bytes, or spans none at all.
    PartSizeMismatch,
    /// The part does not begin where the upload's received bytes end, so accepting it
    /// would leave a gap in the layer or overwrite bytes already received.
    NonConsecutivePart,
    /// Accepting the part would take the upload past [`MAX_LAYER_UPLOAD_BYTES`].
    UploadTooLarge,
}

impl LayerUpload {
    /// The offset the next part must start at: the number of bytes already accepted, so
    /// the first part of an upload starts at byte 0.
    pub fn next_part_first_byte(&self) -> i64 {
        i64::try_from(self.received_bytes.len()).unwrap_or(i64::MAX)
    }

    /// The last byte of the layer this upload has accepted, or `None` while it has
    /// accepted nothing — there is no last byte before the first part arrives.
    pub fn last_byte_received(&self) -> Option<i64> {
        self.next_part_first_byte()
            .checked_sub(1)
            .filter(|last| *last >= 0)
    }

    /// Append one layer part to this upload, answering the last byte the upload has now
    /// received.
    ///
    /// A part is accepted only when its declared `[partFirstByte, partLastByte]` range
    /// spans exactly as many bytes as the blob carries AND starts where the upload's
    /// received bytes end; a rejected part changes nothing, so the upload stays exactly
    /// as the last accepted part left it.
    pub fn receive_part(
        &mut self,
        part_first_byte: i64,
        part_last_byte: i64,
        blob: &[u8],
    ) -> Result<i64, LayerPartRejection> {
        let declared_len = part_last_byte
            .checked_sub(part_first_byte)
            .and_then(|span| span.checked_add(1))
            .filter(|len| *len > 0);
        let carried_len = i64::try_from(blob.len()).unwrap_or(i64::MAX);
        if declared_len != Some(carried_len) {
            return Err(LayerPartRejection::PartSizeMismatch);
        }
        if part_first_byte != self.next_part_first_byte() {
            return Err(LayerPartRejection::NonConsecutivePart);
        }
        if self.received_bytes.len().saturating_add(blob.len()) > MAX_LAYER_UPLOAD_BYTES {
            return Err(LayerPartRejection::UploadTooLarge);
        }
        self.received_bytes.extend_from_slice(blob);
        Ok(self.last_byte_received().unwrap_or(part_last_byte))
    }
}

/// Why an `imageIds` entry resolves to no stored image.
///
/// The variants are named after the `ImageFailureCode` values ECR reports for an
/// identifier it could not resolve (`IMAGE_FAILURE_CODE_VALUES` in
/// `ops::types::shared_types`), so a batch operation reports one directly and an
/// operation that refuses the whole call maps it onto its own modelled error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageLookupFailure {
    /// The entry carries neither an `imageDigest` nor an `imageTag`, so it names
    /// nothing at all.
    MissingDigestAndTag,
    /// The entry's `imageDigest` is not a content digest at all. The model gives
    /// `ImageDigest` no pattern, so a malformed digest reaches the handler and is
    /// reported against its own entry rather than refusing the whole batch.
    InvalidImageDigest,
    /// Nothing in the repository answers to the entry.
    ImageNotFound,
    /// The entry's digest and its tag each name a stored image, but not the same one.
    ImageTagDoesNotMatchDigest,
}

impl ImageLookupFailure {
    /// The `ImageFailureCode` this failure is reported as.
    pub fn code(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => "MissingDigestAndTag",
            Self::InvalidImageDigest => "InvalidImageDigest",
            Self::ImageNotFound => "ImageNotFound",
            Self::ImageTagDoesNotMatchDigest => "ImageTagDoesNotMatchDigest",
        }
    }

    /// The `failureReason` reported beside [`Self::code`]: what about the request made
    /// the identifier unresolvable.
    pub fn reason(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => {
                "Invalid request parameters: at least one of [imageDigest, imageTag] \
                 must be specified"
            }
            Self::InvalidImageDigest => {
                "Invalid request parameters: imageDigest must be of the form \
                 <algorithm>:<hex digest>"
            }
            Self::ImageNotFound => "Requested image not found",
            Self::ImageTagDoesNotMatchDigest => {
                "Requested image tag does not match the requested image digest"
            }
        }
    }
}

/// One parsed `rules` entry of a lifecycle policy document.
///
/// ECR's lifecycle policy document is the service's own format, so it is read here
/// rather than with a crates.io parser: the JSON itself comes from `serde_json`, and
/// what this adds is the meaning of `selection` and `action`. Every modelled value it
/// checks against comes from the model's own enums (`TAG_STATUS_VALUES`,
/// `IMAGE_ACTION_TYPE_VALUES`, `LIFECYCLE_POLICY_TARGET_STORAGE_CLASS_VALUES`).
#[derive(Debug, Clone)]
pub struct LifecycleRule {
    /// `rulePriority`: unique within a policy, and the order rules are applied in.
    pub priority: i32,
    /// `selection.tagStatus`, as the modelled `TagStatus` spells it.
    pub tag_status: String,
    /// `selection.tagPrefixList`: a tag the rule selects begins with one of these.
    pub tag_prefixes: Vec<String>,
    /// `selection.tagPatternList`: a tag the rule selects matches one of these, where
    /// `*` stands for any run of characters.
    pub tag_patterns: Vec<String>,
    /// `selection.countType` and the numbers that go with it.
    pub count: LifecycleCount,
    /// `action.type`, as the modelled `ImageActionType` spells it.
    pub action_type: String,
    /// `action.targetStorageClass`, present only on a `TRANSITION` action.
    pub target_storage_class: Option<String>,
}

/// What a rule's `selection.countType` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCount {
    /// `imageCountMoreThan`: the rule acts on every image its selection matches beyond
    /// the newest `keep` of them.
    ImageCountMoreThan { keep: usize },
    /// `sinceImagePushed`: the rule acts on every image its selection matches that was
    /// pushed more than `days` days ago.
    SinceImagePushed { days: u64 },
}

impl LifecycleRule {
    /// Whether this rule's `selection` admits `image`, before its count is applied.
    fn selects(&self, image: &ImageRecord) -> bool {
        match self.tag_status.as_str() {
            "UNTAGGED" => image.tags.is_empty(),
            "TAGGED" => image.tags.iter().any(|tag| self.tag_matches(tag)),
            // ANY, and anything the model widens `TagStatus` with later.
            _ => true,
        }
    }

    /// Whether `tag` begins with one of the rule's `tagPrefixList` entries or matches one
    /// of its `tagPatternList` wildcards.
    fn tag_matches(&self, tag: &str) -> bool {
        self.tag_prefixes
            .iter()
            .any(|prefix| tag.starts_with(prefix))
            || self
                .tag_patterns
                .iter()
                .any(|pattern| wildcard_matches(pattern, tag))
    }
}

/// One image a preview acts on: the image as it stood when the preview was started, and
/// the rule that claimed it.
#[derive(Debug, Clone)]
pub struct PreviewedImage {
    pub digest: String,
    pub tags: BTreeSet<String>,
    pub pushed_at: SystemTime,
    pub applied_rule_priority: i32,
    pub action_type: String,
    pub target_storage_class: Option<String>,
}

impl PreviewedImage {
    /// The `LifecyclePolicyPreviewResult` a preview read reports for this image.
    pub fn to_result(&self) -> LifecyclePolicyPreviewResult {
        LifecyclePolicyPreviewResult {
            // As everywhere else in this service, an untagged image omits `imageTags`
            // rather than reporting [].
            image_tags: if self.tags.is_empty() {
                None
            } else {
                Some(self.tags.iter().cloned().collect())
            },
            image_digest: Some(self.digest.clone()),
            image_pushed_at: Some(::aws_smithy_types::DateTime::from(self.pushed_at)),
            action: Some(LifecyclePolicyRuleAction {
                r#type: Some(self.action_type.clone()),
                target_storage_class: self.target_storage_class.clone(),
            }),
            applied_rule_priority: Some(self.applied_rule_priority),
            storage_class: Some(STANDARD_STORAGE_CLASS.to_string()),
        }
    }
}

/// How an `imageIds` entry reads in a message: ECR renders the requested
/// `ImageIdentifier` with both members spelled out and an absent one as `null`, so a
/// caller sees the entry exactly as it sent it.
///
/// The public-registry service renders it identically
/// (`service-com-amazonaws-ecrpublic-spencerfrontendservice`'s
/// `state::identifier_description`); the two are separate functions only because
/// `ImageIdentifier` is generated per service crate. Keep them in step.
pub fn identifier_description(id: &ImageIdentifier) -> String {
    format!(
        "imageId {{imageDigest:'{}', imageTag:'{}'}}",
        id.image_digest.as_deref().unwrap_or("null"),
        id.image_tag.as_deref().unwrap_or("null")
    )
}

/// The lifecycle policy preview one `StartLifecyclePolicyPreview` call started.
///
/// The evaluation is computed once, when the preview is started, from the images the
/// repository held at that instant: a preview is a snapshot, so an image pushed after it
/// started does not appear in it, and reading the preview back computes nothing new.
#[derive(Debug, Clone)]
pub struct LifecyclePreview {
    /// The policy the preview was started with — the text the start call named, or the
    /// repository's own policy when it named none.
    pub policy_text: String,
    /// A [`PREVIEW_STATUS_IN_PROGRESS`] / [`PREVIEW_STATUS_COMPLETE`] value.
    pub status: String,
    /// When the `IN_PROGRESS` -> `COMPLETE` transition the clock owes falls due; `None`
    /// once it has been materialised.
    pub complete_due_at: Option<SystemTime>,
    /// The images the policy acts on, keyed by digest so every page — and therefore
    /// every pagination token — has one deterministic order.
    pub results: BTreeMap<String, PreviewedImage>,
}

impl LifecyclePreview {
    /// The `summary` a preview read reports: how many images the preview expires, and how
    /// many it transitions to each storage class.
    ///
    /// It summarises the whole preview rather than one page of it, so it does not change
    /// as a caller pages through the results.
    pub fn summary(&self) -> LifecyclePolicyPreviewSummary {
        let expiring = self
            .results
            .values()
            .filter(|result| result.action_type == EXPIRE_ACTION_TYPE)
            .count();
        let mut transitioning: BTreeMap<String, i32> = BTreeMap::new();
        for result in self
            .results
            .values()
            .filter(|result| result.action_type == TRANSITION_ACTION_TYPE)
        {
            let class = result.target_storage_class.clone().unwrap_or_default();
            *transitioning.entry(class).or_default() += 1;
        }
        LifecyclePolicyPreviewSummary {
            expiring_image_total_count: Some(i32::try_from(expiring).unwrap_or(i32::MAX)),
            // "This field is only present if at least one image will be transitioned in
            // the summary."
            transitioning_image_total_counts: if transitioning.is_empty() {
                None
            } else {
                Some(
                    transitioning
                        .into_iter()
                        .map(
                            |(target_storage_class, image_total_count)| {
                                TransitioningImageTotalCount {
                                    target_storage_class: Some(target_storage_class),
                                    image_total_count: Some(image_total_count),
                                }
                            },
                        )
                        .collect(),
                )
            },
        }
    }
}

/// The rules a lifecycle policy document declares, in the order they are applied, or
/// [`Fault::InvalidParameter`] naming what the document got wrong.
///
/// Every check below is a documented requirement of the policy format: a `rules` array;
/// a unique `rulePriority` of 1 or more per rule; a `selection` naming a `TagStatus`, a
/// count type and a `countNumber` of 1 or more (the format's own minimum — an
/// `imageCountMoreThan` keeps at least one image, and a `sinceImagePushed` counts at
/// least one day); exactly one of the two tag lists on a rule selecting tagged images,
/// and neither anywhere else; `countUnit` on `sinceImagePushed` and only there; and an
/// `action` naming an `ImageActionType`, with a target storage class on a transition and
/// only there.
pub fn parse_lifecycle_policy(text: &str) -> Result<Vec<LifecycleRule>, Fault> {
    fn bad(detail: impl ::std::fmt::Display) -> Fault {
        Fault::InvalidParameter(format!(
            "lifecyclePolicyText is not a valid lifecycle policy: {detail}"
        ))
    }

    let document: serde_json::Value = serde_json::from_str(text).map_err(bad)?;
    let rules = document
        .get("rules")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| bad("it carries no 'rules' array"))?;
    if rules.is_empty() {
        return Err(bad("'rules' carries no rule"));
    }

    let mut parsed: Vec<LifecycleRule> = Vec::new();
    // A rule priority is unique within a policy, so each one is recorded as it is read.
    let mut seen_priorities: BTreeSet<i32> = BTreeSet::new();
    for rule in rules {
        let priority = rule
            .get("rulePriority")
            .and_then(serde_json::Value::as_i64)
            .and_then(|priority| i32::try_from(priority).ok())
            .filter(|priority| *priority >= 1)
            .ok_or_else(|| bad("every rule needs a 'rulePriority' integer of 1 or more"))?;
        if !seen_priorities.insert(priority) {
            return Err(bad(format!(
                "two rules carry rulePriority {priority}; a rule priority is unique \
                 within a policy"
            )));
        }

        let selection = rule
            .get("selection")
            .ok_or_else(|| bad(format!("rule {priority} carries no 'selection'")))?;
        let tag_status_text = selection
            .get("tagStatus")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| bad(format!("rule {priority} carries no 'selection.tagStatus'")))?;
        // A policy document writes `tagStatus`, `countType` and `action.type` in lower
        // camel case; the modelled enums are what they mean, so they are matched against
        // those case-insensitively and stored in the modelled spelling.
        let tag_status = TAG_STATUS_VALUES
            .iter()
            .find(|value| value.eq_ignore_ascii_case(tag_status_text))
            .ok_or_else(|| {
                bad(format!(
                    "rule {priority} carries 'selection.tagStatus' {tag_status_text:?}, \
                     which is not one of {TAG_STATUS_VALUES:?}"
                ))
            })?;

        let string_list = |member: &str| -> Result<Vec<String>, Fault> {
            match selection.get(member) {
                None | Some(serde_json::Value::Null) => Ok(Vec::new()),
                Some(value) => value
                    .as_array()
                    .ok_or_else(|| {
                        bad(format!(
                            "rule {priority} carries a 'selection.{member}' that is not \
                             an array"
                        ))
                    })?
                    .iter()
                    .map(|entry| {
                        entry
                            .as_str()
                            .map(ToString::to_string)
                            .ok_or_else(|| {
                                bad(format!(
                                    "rule {priority} carries a non-string entry in \
                                     'selection.{member}'"
                                ))
                            })
                    })
                    .collect(),
            }
        };
        let tag_prefixes = string_list("tagPrefixList")?;
        let tag_patterns = string_list("tagPatternList")?;
        let tagged = *tag_status == "TAGGED";
        let names_tags = !tag_prefixes.is_empty() || !tag_patterns.is_empty();
        if tagged && !names_tags {
            return Err(bad(format!(
                "rule {priority} selects tagged images but carries neither \
                 'selection.tagPrefixList' nor 'selection.tagPatternList'"
            )));
        }
        if !tag_prefixes.is_empty() && !tag_patterns.is_empty() {
            return Err(bad(format!(
                "rule {priority} carries both 'selection.tagPrefixList' and \
                 'selection.tagPatternList'; a rule selects tagged images by one of the \
                 two"
            )));
        }
        if !tagged && names_tags {
            return Err(bad(format!(
                "rule {priority} carries a tag list with 'selection.tagStatus' \
                 {tag_status_text:?}; a tag list belongs to a rule selecting tagged images"
            )));
        }

        let count_type = selection
            .get("countType")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| bad(format!("rule {priority} carries no 'selection.countType'")))?;
        let count_number = selection
            .get("countNumber")
            .and_then(serde_json::Value::as_i64)
            .filter(|number| *number >= 1)
            .ok_or_else(|| {
                bad(format!(
                    "rule {priority} carries no 'selection.countNumber' integer of 1 or \
                     more"
                ))
            })?;
        let count_unit = selection
            .get("countUnit")
            .and_then(serde_json::Value::as_str);
        let count = if count_type.eq_ignore_ascii_case("imageCountMoreThan") {
            if count_unit.is_some() {
                return Err(bad(format!(
                    "rule {priority} carries a 'selection.countUnit' with the \
                     {count_type:?} count type, which counts images rather than time"
                )));
            }
            LifecycleCount::ImageCountMoreThan {
                keep: usize::try_from(count_number).unwrap_or(usize::MAX),
            }
        } else if count_type.eq_ignore_ascii_case("sinceImagePushed") {
            if !count_unit.is_some_and(|unit| unit.eq_ignore_ascii_case(COUNT_UNIT_DAYS)) {
                return Err(bad(format!(
                    "rule {priority} carries the {count_type:?} count type, which needs \
                     'selection.countUnit' {COUNT_UNIT_DAYS:?}, and carries \
                     {count_unit:?}"
                )));
            }
            LifecycleCount::SinceImagePushed {
                days: u64::try_from(count_number).unwrap_or(u64::MAX),
            }
        } else {
            return Err(bad(format!(
                "rule {priority} carries 'selection.countType' {count_type:?}, which is \
                 neither \"imageCountMoreThan\" nor \"sinceImagePushed\""
            )));
        };

        let action = rule
            .get("action")
            .ok_or_else(|| bad(format!("rule {priority} carries no 'action'")))?;
        let action_text = action
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| bad(format!("rule {priority} carries no 'action.type'")))?;
        let action_type: &str = IMAGE_ACTION_TYPE_VALUES
            .iter()
            .find(|value| value.eq_ignore_ascii_case(action_text))
            .ok_or_else(|| {
                bad(format!(
                    "rule {priority} carries 'action.type' {action_text:?}, which is not \
                     one of {IMAGE_ACTION_TYPE_VALUES:?}"
                ))
            })?;
        let target_text = action
            .get("targetStorageClass")
            .and_then(serde_json::Value::as_str);
        let target_storage_class = match (action_type, target_text) {
            (TRANSITION_ACTION_TYPE, Some(target)) => Some(
                LIFECYCLE_POLICY_TARGET_STORAGE_CLASS_VALUES
                    .iter()
                    .find(|value| value.eq_ignore_ascii_case(target))
                    .ok_or_else(|| {
                        bad(format!(
                            "rule {priority} carries 'action.targetStorageClass' \
                             {target:?}, which is not one of \
                             {LIFECYCLE_POLICY_TARGET_STORAGE_CLASS_VALUES:?}"
                        ))
                    })?
                    .to_string(),
            ),
            (TRANSITION_ACTION_TYPE, None) => {
                return Err(bad(format!(
                    "rule {priority} transitions images but carries no \
                     'action.targetStorageClass'"
                )))
            }
            (_, Some(target)) => {
                return Err(bad(format!(
                    "rule {priority} carries 'action.targetStorageClass' {target:?} with \
                     the {action_text:?} action, which moves no image between storage \
                     classes"
                )))
            }
            (_, None) => None,
        };

        parsed.push(LifecycleRule {
            priority,
            tag_status: (*tag_status).to_string(),
            tag_prefixes,
            tag_patterns,
            count,
            action_type: action_type.to_string(),
            target_storage_class,
        });
    }

    // Rules are applied in ascending rule priority, whatever order the document lists
    // them in.
    parsed.sort_by_key(|rule| rule.priority);

    // "A lifecycle policy rule with a tagStatus value of any must have the highest value
    // for rulePriority and be evaluated last", so a catch-all rule is refused exactly
    // when another rule sorts after it. Checked once the rules are in applied order: the
    // last one is the one with the highest priority.
    if let Some(last) = parsed.last() {
        if let Some(catch_all) = parsed
            .iter()
            .find(|rule| rule.tag_status == ANY_TAG_STATUS && rule.priority < last.priority)
        {
            return Err(bad(format!(
                "rule {} selects every image but is evaluated before rule {}; a rule \
                 selecting every image carries the highest rulePriority in the policy",
                catch_all.priority, last.priority
            )));
        }
    }

    Ok(parsed)
}

/// The images `rules` acts on, evaluated against `images` as they stand at `now` and
/// keyed by digest.
///
/// Rules are applied in ascending `rulePriority`, and an image the first matching rule
/// claims is not offered to a later one — so every previewed image carries exactly one
/// `appliedRulePriority`.
pub fn evaluate_lifecycle_policy(
    rules: &[LifecycleRule],
    images: &BTreeMap<String, ImageRecord>,
    now: SystemTime,
) -> BTreeMap<String, PreviewedImage> {
    let mut acted: BTreeMap<String, PreviewedImage> = BTreeMap::new();
    for rule in rules {
        let mut candidates: Vec<&ImageRecord> = images
            .values()
            .filter(|image| !acted.contains_key(&image.digest))
            .filter(|image| rule.selects(image))
            .collect();
        let claimed: Vec<&ImageRecord> = match rule.count {
            LifecycleCount::SinceImagePushed { days } => {
                let age = Duration::from_secs(days.saturating_mul(SECONDS_PER_DAY));
                candidates
                    .into_iter()
                    .filter(|image| {
                        now.duration_since(image.pushed_at).unwrap_or_default() > age
                    })
                    .collect()
            }
            LifecycleCount::ImageCountMoreThan { keep } => {
                // The newest `keep` images the selection matched stay; everything older
                // is what the rule acts on. The digest breaks a tie between two images
                // pushed at the same instant, so the choice is deterministic.
                candidates.sort_by(|left, right| {
                    right
                        .pushed_at
                        .cmp(&left.pushed_at)
                        .then_with(|| left.digest.cmp(&right.digest))
                });
                candidates.into_iter().skip(keep).collect()
            }
        };
        for image in claimed {
            acted.insert(
                image.digest.clone(),
                PreviewedImage {
                    digest: image.digest.clone(),
                    tags: image.tags.clone(),
                    pushed_at: image.pushed_at,
                    applied_rule_priority: rule.priority,
                    action_type: rule.action_type.clone(),
                    target_storage_class: rule.target_storage_class.clone(),
                },
            );
        }
    }
    acted
}

/// One stored repository, with the identity fields computed once at creation so every
/// later read reports exactly what the create returned.
#[derive(Debug, Clone)]
pub struct Repo {
    pub name: String,
    pub registry_id: String,
    pub arn: String,
    pub uri: String,
    pub created_at: SystemTime,
    pub image_tag_mutability: String,
    pub image_tag_mutability_exclusion_filters: Vec<ImageTagMutabilityExclusionFilter>,
    /// Always present: `ImageScanningConfiguration$scanOnPush` is modelled
    /// `@default(false)` ("If this parameter is not specified, it will default to false"),
    /// so a repository created without one scans nothing on push rather than carrying no
    /// scanning configuration at all. Every repository therefore reports the member.
    pub image_scanning_config: ImageScanningConfiguration,
    pub encryption_config: EncryptionConfiguration,
    pub tags: Vec<Tag>,
    /// The repository policy, as the last `SetRepositoryPolicy` stored it: `None`
    /// until a policy is set and again once `DeleteRepositoryPolicy` removes it, which
    /// is what `GetRepositoryPolicy` reports `RepositoryPolicyNotFoundException` for.
    /// It lives on the repository record, so deleting the repository drops its policy
    /// with it.
    pub policy_text: Option<String>,
    pub images: BTreeMap<String, ImageRecord>,
    pub tag_index: BTreeMap<String, String>,
    /// The blobs this repository holds, by digest: one entry per layer upload
    /// `CompleteLayerUpload` closed against it, carrying the digest the registry
    /// computed over the assembled bytes and how many bytes those were. A manifest
    /// cannot be pushed before every blob it references is in here, which is what
    /// `PutImage`'s `LayersNotFoundException` reports, and it is where
    /// `BatchCheckLayerAvailability` reads a layer's availability from. Blobs are
    /// per-repository: a layer uploaded to one repository is not a layer of another.
    /// The bytes themselves are not kept — nothing serves them yet — only the
    /// registry's record of the blob.
    pub layer_blobs: BTreeMap<String, LayerBlob>,
    /// The repository's lifecycle policy, which `PutLifecyclePolicy` sets: the policy
    /// `StartLifecyclePolicyPreview` evaluates when the start call names none, and the
    /// document `GetLifecyclePolicy` reports. `None` until a policy is put and again
    /// once `DeleteLifecyclePolicy` removes it, which is what the two of them report
    /// `LifecyclePolicyNotFoundException` for.
    pub lifecycle_policy: Option<String>,
    /// The one lifecycle policy preview this repository carries; a later start replaces
    /// it.
    pub lifecycle_preview: Option<LifecyclePreview>,
}

impl Repo {
    /// The `Repository` every operation that reports this repository returns.
    pub fn to_repository(&self) -> Repository {
        Repository {
            repository_arn: Some(self.arn.clone()),
            registry_id: Some(self.registry_id.clone()),
            repository_name: Some(self.name.clone()),
            repository_uri: Some(self.uri.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            image_tag_mutability: Some(self.image_tag_mutability.clone()),
            image_tag_mutability_exclusion_filters: if self
                .image_tag_mutability_exclusion_filters
                .is_empty()
            {
                None
            } else {
                Some(self.image_tag_mutability_exclusion_filters.clone())
            },
            image_scanning_configuration: Some(self.image_scanning_config.clone()),
            encryption_configuration: Some(self.encryption_config.clone()),
        }
    }

    /// The scanning configuration reported for this repository.
    ///
    /// Derived from what is stored, never from the request: `scanOnPush` is the flag the
    /// repository's own `imageScanningConfiguration` carries (modelled
    /// `@default(false)`, so a repository created without one is not scanned on push),
    /// and the frequency follows from it — a repository scanned on push has the
    /// `SCAN_ON_PUSH` frequency, and one that is not has the `MANUAL` one.
    /// `appliedScanFilters` lists the registry scanning rules' filters that match this
    /// repository; deriving that match from the registry configuration
    /// `PutRegistryScanningConfiguration` now stores belongs to this projection's own
    /// operation, so the list stays empty here.
    pub fn to_scanning_configuration(&self) -> RepositoryScanningConfiguration {
        let scan_on_push = self.image_scanning_config.scan_on_push;
        RepositoryScanningConfiguration {
            repository_arn: Some(self.arn.clone()),
            repository_name: Some(self.name.clone()),
            scan_on_push,
            scan_frequency: Some(
                if scan_on_push {
                    SCAN_ON_PUSH_FREQUENCY
                } else {
                    MANUAL_SCAN_FREQUENCY
                }
                .to_string(),
            ),
            applied_scan_filters: Some(Vec::new()),
        }
    }

    /// Whether `tag` may be moved from the image it points at to another one.
    ///
    /// The repository's `imageTagMutability` decides it, and the two
    /// `*_WITH_EXCLUSION` settings invert that decision for a tag matching any
    /// `imageTagMutabilityExclusionFilters` entry — the filters name the tags
    /// "excluded from the repository's image tag mutability setting".
    pub fn tag_is_mutable(&self, tag: &str) -> bool {
        let excluded = self
            .image_tag_mutability_exclusion_filters
            .iter()
            .any(|f| wildcard_matches(&f.filter, tag));
        match self.image_tag_mutability.as_str() {
            "IMMUTABLE" => false,
            "MUTABLE_WITH_EXCLUSION" => !excluded,
            "IMMUTABLE_WITH_EXCLUSION" => excluded,
            // MUTABLE, and anything the model widens the enum with later.
            _ => true,
        }
    }

    /// The digest `tag` currently points at.
    pub fn digest_of_tag(&self, tag: &str) -> Option<&String> {
        self.tag_index.get(tag)
    }

    /// The stored image an `imageIds` entry names, or why it names none.
    ///
    /// An entry carries a digest, a tag, or both, and it names an image only when this
    /// repository holds one that answers to every part of it: a digest addresses the
    /// manifest of that content, a tag addresses whatever the tag index points at, and
    /// an entry carrying both resolves only when they agree. A digest that is not a
    /// content digest at all addresses nothing, and is reported as that rather than as
    /// an absent image.
    ///
    /// This is the one place an `imageId` is resolved: a batch operation reports the
    /// failure per entry, and [`Self::resolve_image`] lifts it onto a [`Fault`] for an
    /// operation that refuses the whole call.
    pub fn lookup_image(&self, id: &ImageIdentifier) -> Result<&ImageRecord, ImageLookupFailure> {
        if id
            .image_digest
            .as_deref()
            // The same `<algorithm>:<hex>` form a layer digest has: a content digest is
            // one shape, whatever it addresses.
            .is_some_and(|digest| !is_layer_digest(digest))
        {
            return Err(ImageLookupFailure::InvalidImageDigest);
        }
        match (id.image_digest.as_deref(), id.image_tag.as_deref()) {
            (None, None) => Err(ImageLookupFailure::MissingDigestAndTag),
            (Some(digest), None) => self
                .images
                .get(digest)
                .ok_or(ImageLookupFailure::ImageNotFound),
            (None, Some(tag)) => self
                .digest_of_tag(tag)
                .and_then(|digest| self.images.get(digest))
                .ok_or(ImageLookupFailure::ImageNotFound),
            (Some(digest), Some(tag)) => {
                let image = self
                    .images
                    .get(digest)
                    .ok_or(ImageLookupFailure::ImageNotFound)?;
                match self.digest_of_tag(tag) {
                    // The tag is on this very image, so both parts agree.
                    Some(tagged) if tagged == digest => Ok(image),
                    // The tag names another stored image: the two parts disagree.
                    Some(_) => Err(ImageLookupFailure::ImageTagDoesNotMatchDigest),
                    // Nothing in the repository carries the tag at all.
                    None => Err(ImageLookupFailure::ImageNotFound),
                }
            }
        }
    }

    /// The stored image an [`ImageIdentifier`] names, or the [`Fault`] it resolves to.
    ///
    /// The resolution is [`Self::lookup_image`]'s; what this adds is the mapping onto
    /// the faults an operation that refuses the WHOLE call reports, so every such
    /// operation refuses the same identifier the same way. An identifier that names no
    /// stored image is an image fault; one that is malformed, or that names neither a
    /// digest nor a tag, is a parameter fault.
    pub fn resolve_image(&self, id: &ImageIdentifier) -> Result<&ImageRecord, Fault> {
        // Matched exhaustively, so a lookup failure added later is a compile error here
        // rather than silently reported as an absent image.
        match self.lookup_image(id) {
            Ok(stored) => Ok(stored),
            Err(ImageLookupFailure::MissingDigestAndTag) => Err(Fault::InvalidParameter(
                "an image identifier must carry an imageDigest, an imageTag, or both".to_string(),
            )),
            Err(ImageLookupFailure::InvalidImageDigest) => Err(Fault::InvalidParameter(format!(
                "Invalid parameter at 'imageIds' failed to satisfy constraint: imageDigest \
                 '{}' must be of the form <algorithm>:<hex digest>",
                id.image_digest.as_deref().unwrap_or_default()
            ))),
            Err(
                ImageLookupFailure::ImageNotFound
                | ImageLookupFailure::ImageTagDoesNotMatchDigest,
            ) => Err(Fault::ImageNotFound(format!(
                "The image with {} does not exist within the repository with name '{}' in \
                 the registry with id '{}'",
                identifier_description(id),
                self.name,
                self.registry_id
            ))),
        }
    }

    /// Every blob this repository holds, by digest.
    ///
    /// A blob is in a repository because a layer upload against that repository was
    /// COMPLETED — `CompleteLayerUpload` is what registers one, under the digest the
    /// registry computed over the bytes it assembled — so [`Self::layer_blobs`] is the
    /// record of what the repository holds, and the byte count it carries is the
    /// registry's own count rather than anything a manifest declares. What a manifest
    /// adds is the blob's MEDIA TYPE, which only a descriptor referencing it states (a
    /// V2 schema 1 `fsLayers` entry states not even that), so it is filled in here from
    /// whichever stored manifest knows it. Blobs are per-repository: a layer uploaded to
    /// one repository is not a layer of another, which is why this is a method on the
    /// repository.
    pub fn layers(&self) -> BTreeMap<String, LayerBlob> {
        let mut layers = self.layer_blobs.clone();
        for image in self.images.values() {
            for blob in manifest_blobs(&image.manifest) {
                // A descriptor for a blob the repository does not hold describes
                // nothing here: only a held blob is a layer of this repository.
                if let Entry::Occupied(mut held) = layers.entry(blob.digest.clone()) {
                    let record = held.get_mut();
                    if record.media_type.is_none() {
                        record.media_type = blob.media_type;
                    }
                }
            }
        }
        layers
    }

    /// The digests `manifest` references that this repository does not hold, each once
    /// and in the order [`manifest_blobs`] reads them: the image's layers, then its
    /// configuration blob.
    ///
    /// Empty means every blob the manifest references is in the repository, so the
    /// manifest may be stored; anything else is what `PutImage`'s
    /// `LayersNotFoundException` names. The same blob referenced twice is one missing
    /// blob, so it is named once. A document that is not a manifest at all references no
    /// blob ([`manifest_blobs`]), and so names no missing one.
    pub fn missing_layers(&self, manifest: &str) -> Vec<String> {
        let mut missing: Vec<String> = Vec::new();
        for blob in manifest_blobs(manifest) {
            if !self.layer_blobs.contains_key(&blob.digest) && !missing.contains(&blob.digest) {
                missing.push(blob.digest);
            }
        }
        missing
    }

    /// Apply `incoming` to this repository's metadata tags: a key the repository already
    /// carries takes the new value where it stands, a key it does not carry is appended.
    /// "Existing tags on a resource are not changed if they are not specified in the
    /// request parameters" (`TagResource`), so nothing else is touched.
    pub fn merge_tags(&mut self, incoming: Vec<Tag>) {
        for tag in incoming {
            match self.tags.iter_mut().find(|held| held.key == tag.key) {
                Some(held) => held.value = tag.value,
                None => self.tags.push(tag),
            }
        }
    }

    /// Remove every metadata tag whose key `keys` names. A key the repository does not
    /// carry removes nothing: `UntagResource` names the keys to be removed, not an
    /// assertion that they are held.
    pub fn remove_tag_keys(&mut self, keys: &[String]) {
        self.tags.retain(|held| !keys.contains(&held.key));
    }

    /// Attach `tag` to `digest`, detaching it from whatever held it before, so the tag
    /// index and the images' own tag sets never disagree.
    pub fn move_tag(&mut self, tag: &str, digest: &str) {
        if let Some(previous) = self.tag_index.get(tag).cloned() {
            if let Some(image) = self.images.get_mut(&previous) {
                image.tags.remove(tag);
            }
        }
        self.tag_index.insert(tag.to_string(), digest.to_string());
        if let Some(image) = self.images.get_mut(digest) {
            image.tags.insert(tag.to_string());
        }
    }

    /// Materialise the lifecycle policy preview completion the clock has already made
    /// due at `now`.
    ///
    /// The transition is owed by the clock, not by a call: it falls due
    /// [`LIFECYCLE_PREVIEW_DELAY`] after the start whether or not anybody asks, so every
    /// operation that looks at the preview writes down what was already due before
    /// reading it.
    pub fn settle_lifecycle_preview(&mut self, now: SystemTime) {
        let Some(preview) = self.lifecycle_preview.as_mut() else {
            return;
        };
        let Some(due) = preview.complete_due_at else {
            return;
        };
        if due <= now {
            preview.status = PREVIEW_STATUS_COMPLETE.to_string();
            preview.complete_due_at = None;
        }
    }

    /// Whether a preview started on this repository is still running.
    pub fn lifecycle_preview_in_progress(&self) -> bool {
        self.lifecycle_preview
            .as_ref()
            .is_some_and(|preview| preview.status == PREVIEW_STATUS_IN_PROGRESS)
    }
}

/// One stored repository creation template: the settings a later repository creation in
/// this namespace prefix is to be given, exactly as the create or the update left them.
#[derive(Debug, Clone)]
pub struct CreationTemplate {
    pub prefix: String,
    pub description: Option<String>,
    pub encryption_config: Option<EncryptionConfigurationForRepositoryCreationTemplate>,
    pub resource_tags: Vec<Tag>,
    pub image_tag_mutability: String,
    pub image_tag_mutability_exclusion_filters: Vec<ImageTagMutabilityExclusionFilter>,
    pub repository_policy: Option<String>,
    pub lifecycle_policy: Option<String>,
    pub applied_for: Vec<String>,
    pub custom_role_arn: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl CreationTemplate {
    /// The `RepositoryCreationTemplate` every operation that reports this template
    /// returns, derived from the stored record alone.
    pub fn to_template(&self) -> RepositoryCreationTemplate {
        RepositoryCreationTemplate {
            prefix: Some(self.prefix.clone()),
            description: self.description.clone(),
            encryption_configuration: self.encryption_config.clone(),
            // An empty list is reported as an absent member: nothing was applied.
            resource_tags: if self.resource_tags.is_empty() {
                None
            } else {
                Some(self.resource_tags.clone())
            },
            image_tag_mutability: Some(self.image_tag_mutability.clone()),
            image_tag_mutability_exclusion_filters: if self
                .image_tag_mutability_exclusion_filters
                .is_empty()
            {
                None
            } else {
                Some(self.image_tag_mutability_exclusion_filters.clone())
            },
            repository_policy: self.repository_policy.clone(),
            lifecycle_policy: self.lifecycle_policy.clone(),
            applied_for: Some(self.applied_for.clone()),
            custom_role_arn: self.custom_role_arn.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// `upstreamRepositoryPrefix`'s documented default: "When this field isn't specified,
/// Amazon ECR will use the ROOT."
pub const DEFAULT_UPSTREAM_REPOSITORY_PREFIX: &str = "ROOT";

/// The upstream registries whose URL the `upstreamRegistryUrl` documentation gives as
/// one fixed host, as `(registry, host)` pairs. The two it gives with a caller-chosen
/// label — Amazon ECR (`<accountId>.dkr.ecr.<region>.amazonaws.com`) and Microsoft Azure
/// Container Registry (`<custom>.azurecr.io`) — are matched by shape in
/// [`upstream_registry_for_url`] instead, and `chainguard`, whose URL that
/// documentation does not give, is named only by the caller.
pub const DOCUMENTED_UPSTREAM_REGISTRY_URLS: [(&str, &str); 6] = [
    ("ecr-public", "public.ecr.aws"),
    ("docker-hub", "registry-1.docker.io"),
    ("github-container-registry", "ghcr.io"),
    ("gitlab-container-registry", "registry.gitlab.com"),
    ("k8s", "registry.k8s.io"),
    ("quay", "quay.io"),
];

/// The `UpstreamRegistry` a URL names, from the per-registry URL syntax the
/// `upstreamRegistryUrl` member documents, or `None` when the URL matches none of them.
pub fn upstream_registry_for_url(url: &str) -> Option<&'static str> {
    let host = url.trim().trim_end_matches('/');
    if let Some((registry, _)) = DOCUMENTED_UPSTREAM_REGISTRY_URLS
        .iter()
        .find(|(_, documented)| host.eq_ignore_ascii_case(documented))
    {
        return Some(registry);
    }
    let lower = host.to_ascii_lowercase();
    // `<custom>.azurecr.io` and `<accountId>.dkr.ecr.<region>.amazonaws.com`: the label
    // is the caller's, the rest of the host is the registry's, so each is matched by its
    // documented label arrangement rather than by a substring.
    if lower
        .strip_suffix(".azurecr.io")
        .is_some_and(|custom| !custom.is_empty())
    {
        return Some("azure-container-registry");
    }
    if is_ecr_registry_host(&lower) {
        return Some("ecr");
    }
    None
}

/// Whether `host` is an `<accountId>.dkr.ecr.<region>.amazonaws.com` registry host, as
/// the `upstreamRegistryUrl` doc spells Amazon ECR's: an account id, the two fixed
/// labels, one region label, and nothing else. `host` is already lowercased.
fn is_ecr_registry_host(host: &str) -> bool {
    let Some(rest) = host.strip_suffix(".amazonaws.com") else {
        return false;
    };
    let mut labels = rest.split('.');
    let account_id_is_well_formed = labels.next().is_some_and(|account| {
        account.len() == ACCOUNT_ID_LEN && account.bytes().all(|byte| byte.is_ascii_digit())
    });
    account_id_is_well_formed
        && labels.next() == Some("dkr")
        && labels.next() == Some("ecr")
        && labels.next().is_some_and(|region| !region.is_empty())
        && labels.next().is_none()
}

/// The length of an Amazon Web Services account id, which the `registryId` member's own
/// pattern gives as `^[0-9]{12}$`.
const ACCOUNT_ID_LEN: usize = 12;

/// Per-service state for com-amazonaws-ecr-amazonec2containerregistry-v20150921. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// Repositories by name. A `BTreeMap` so every listing — and therefore every
    /// pagination token — has one deterministic order.
    pub repositories: BTreeMap<String, Repo>,
    /// This registry's pull through cache rules, by `ecrRepositoryPrefix` — the key AWS
    /// addresses a rule by, as [`normalize_prefix`] spells it. `CreatePullThroughCacheRule`
    /// is the only operation that adds one; `UpdatePullThroughCacheRule`,
    /// `ValidatePullThroughCacheRule` and `DeletePullThroughCacheRule` resolve a prefix
    /// here rather than each keeping their own copy.
    pub pull_through_cache_rules: BTreeMap<String, PullThroughCacheRule>,
    /// The account settings a `PutAccountSetting` has changed, by setting name. A name
    /// absent from here is still readable — it reports its spec's
    /// [`AccountSettingSpec::default_value`] — so a read never has to invent a value
    /// and never has to write one.
    pub account_settings: BTreeMap<String, String>,
    /// Repository creation templates by namespace prefix, keyed the way
    /// [`normalize_prefix`] spells that prefix. A `BTreeMap` for the same reason as
    /// `repositories`: one deterministic listing order, so a pagination token means the
    /// same thing on the next call.
    pub creation_templates: BTreeMap<String, CreationTemplate>,
    /// The registry's replication rules, exactly as the last
    /// `PutReplicationConfiguration` persisted them, in the order that call listed
    /// them. Empty until one is made: a registry nobody has configured replication for
    /// reports an empty rule list, which is what `DescribeRegistry` answers with.
    ///
    /// Registry-wide, like the single registry this store models: a put made against
    /// one Region's endpoint is the configuration every later read reports, while the
    /// "a registry cannot replicate to itself" refusal is evaluated per call, against
    /// the Region that call was made against.
    pub replication_rules: Vec<ReplicationRule>,
    /// The registry permissions policy, as `PutRegistryPolicy` stored it: the document
    /// text exactly as it arrived, so a read returns it byte for byte. `None` is "this
    /// registry has no associated registry policy", which the registry-policy read and
    /// delete answer with `RegistryPolicyNotFoundException`. One registry is served
    /// here, so there is at most one such policy.
    pub registry_policy: Option<String>,
    /// The registry's signing configuration, which `PutSigningConfiguration` creates or
    /// replaces and `GetSigningConfiguration` reports. There is at most one per registry
    /// and this emulator serves one registry, so it is a single optional record rather
    /// than a map: `None` is a registry that has none, which is what
    /// `SigningConfigurationNotFoundException` reports.
    pub signing_configuration: Option<SigningConfiguration>,
    /// The registry's pull time update exclusion list: the IAM principal ARN of every
    /// principal whose image pulls ECR does not record a pull time for, mapped to when
    /// its exclusion was registered. Keyed by the ARN — one exclusion per principal is
    /// what makes a second register of the same principal a duplicate — and a
    /// `BTreeMap` so listing and its pagination tokens have one deterministic order.
    pub pull_time_update_exclusions: BTreeMap<String, SystemTime>,
    /// In-flight layer uploads by upload id. Registry-wide, because an upload id is
    /// what `UploadLayerPart` and `CompleteLayerUpload` address an upload by; each
    /// record names the repository it belongs to, and a call naming a repository that
    /// no longer exists is refused by the repository lookup before the upload is ever
    /// reached. An upload opened against a repository later deleted is therefore inert,
    /// but still counts against [`MAX_LAYER_UPLOADS`] until the process ends.
    pub uploads: BTreeMap<String, LayerUpload>,
    /// The registry's scanning configuration, exactly as the last
    /// `PutRegistryScanningConfiguration` stored it. `None` until one is stored, which is
    /// the state the documented default describes — see
    /// [`registry_scanning_configuration`](ComAmazonawsEcrAmazonec2containerregistryV20150921State::registry_scanning_configuration).
    pub registry_scanning: Option<crate::ops::types::RegistryScanningConfiguration>,
}

impl ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// The repository `name`, or [`Fault::RepositoryNotFound`].
    pub fn repository(&self, name: &str) -> Result<&Repo, Fault> {
        self.repositories
            .get(name)
            .ok_or_else(|| Fault::RepositoryNotFound(repository_not_found_message(name)))
    }

    /// The repository `name` for mutation, or [`Fault::RepositoryNotFound`].
    pub fn repository_mut(&mut self, name: &str) -> Result<&mut Repo, Fault> {
        self.repositories
            .get_mut(name)
            .ok_or_else(|| Fault::RepositoryNotFound(repository_not_found_message(name)))
    }

    /// The in-flight upload `upload_id` addresses, for mutation, or `None` when this
    /// registry holds no such upload OR holds one that was opened against another
    /// repository: an upload id is only valid for the repository it was issued for,
    /// which is exactly what `UploadNotFoundException` reports ("the upload could not
    /// be found, or the specified upload ID is not valid for this repository").
    pub fn upload_mut(
        &mut self,
        repository_name: &str,
        upload_id: &str,
    ) -> Option<&mut LayerUpload> {
        self.uploads
            .get_mut(upload_id)
            .filter(|upload| upload.repository_name == repository_name)
    }

    /// The rule registered for `prefix`, whichever way its trailing slash is written, or
    /// `None` when the registry holds no rule for it.
    pub fn pull_through_cache_rule(&self, prefix: &str) -> Option<&PullThroughCacheRule> {
        self.pull_through_cache_rules
            .get(&normalize_prefix(prefix))
    }

    /// The value of the account setting `name`: what was last set for it, or its
    /// documented default while nothing has been. Reading one changes nothing.
    pub fn account_setting(&self, name: &str) -> Result<String, Fault> {
        let spec = account_setting_spec(name)?;
        Ok(self
            .account_settings
            .get(spec.name)
            .cloned()
            .unwrap_or_else(|| spec.default_value.to_string()))
    }

    /// Set the account setting `name` to `value`, refusing a name this registry does
    /// not serve and a value that setting does not take. Answers with the value the
    /// store now holds for that setting, read back out of the map rather than echoed,
    /// so a caller cannot report a value the store does not carry.
    pub fn set_account_setting(&mut self, name: &str, value: &str) -> Result<String, Fault> {
        let spec = account_setting_spec(name)?;
        if !spec.settable_values.contains(&value) {
            // The setting and the value come from the request, and "in this region" is
            // the reason: a value this registry's region does not offer for that
            // setting.
            return Err(Fault::InvalidParameter(format!(
                "You can't set '{}' to '{value}' in this region.",
                spec.name
            )));
        }
        self.account_settings
            .insert(spec.name.to_string(), value.to_string());
        Ok(self
            .account_settings
            .get(spec.name)
            .cloned()
            .unwrap_or_else(|| spec.default_value.to_string()))
    }

    /// The registry's replication configuration: the stored rules, which is the empty
    /// list before any `PutReplicationConfiguration` call. ECR reports the
    /// configuration either way — an unconfigured registry has no rules rather than no
    /// configuration.
    pub fn replication_configuration(&self) -> ReplicationConfiguration {
        ReplicationConfiguration {
            rules: self.replication_rules.clone(),
        }
    }
}

/// The wording ECR uses when no pull through cache rule answers to a prefix. The first
/// sentence is `PullThroughCacheRuleNotFoundException`'s own documentation.
pub fn pull_through_cache_rule_not_found_message(prefix: &str) -> String {
    format!(
        "The pull through cache rule was not found. Specify a valid pull through cache \
         rule and try again. No rule with ecrRepositoryPrefix '{prefix}' exists in the \
         registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when no repository creation template answers to a prefix. The
/// first sentence is `TemplateNotFoundException`'s own documentation.
pub fn template_not_found_message(prefix: &str) -> String {
    format!(
        "The specified repository creation template can't be found. Verify the registry \
         ID and prefix and try again. No template with prefix '{prefix}' exists in the \
         registry with id '{}'",
        registry_id()
    )
}

/// The upstream registries ECR documents as serving anonymously, each with the host it
/// is reached at: Amazon ECR Public and Kubernetes, from the ECR user guide's
/// supported-upstream table. One table, so the `UpstreamRegistry` value and the host it
/// corresponds to cannot drift apart.
pub const ANONYMOUS_UPSTREAM_REGISTRIES: [(&str, &str); 2] = [
    ("ecr-public", "public.ecr.aws"),
    ("k8s", "registry.k8s.io"),
];

/// Whether an upstream registry a pull through cache rule points at authenticates its
/// callers, and therefore needs a credential stored on the rule.
///
/// Every upstream registry ECR supports except the two in
/// [`ANONYMOUS_UPSTREAM_REGISTRIES`] — the rest of the model's `UpstreamRegistry` enum,
/// listed in `UPSTREAM_REGISTRY_VALUES` — requires credentials. A rule that names no
/// upstream registry is judged by the host it points at instead.
pub fn upstream_needs_credential(upstream_registry: Option<&str>, upstream_url: &str) -> bool {
    match upstream_registry {
        Some(registry) => !ANONYMOUS_UPSTREAM_REGISTRIES
            .iter()
            .any(|(anonymous, _)| *anonymous == registry),
        None => {
            // A registry URL's authority: what is left after an optional scheme, before
            // the path, without the userinfo or the port, compared case-insensitively —
            // a host is not case-sensitive, and `public.ecr.aws:443` is the same host as
            // `public.ecr.aws`.
            let host = upstream_url
                .split_once("://")
                .map_or(upstream_url, |(_, rest)| rest)
                .split('/')
                .next()
                .unwrap_or_default();
            let host = host.rsplit_once('@').map_or(host, |(_, after)| after);
            let host = host.split_once(':').map_or(host, |(before, _)| before);
            let host = host.to_ascii_lowercase();
            !ANONYMOUS_UPSTREAM_REGISTRIES
                .iter()
                .any(|(_, anonymous_host)| *anonymous_host == host)
        }
    }
}

/// Whether `setting` is one of the two `ImageTagMutability` values that honour
/// exclusion filters: a filter names tags "excluded from the repository's image tag
/// mutability setting", and only the `*_WITH_EXCLUSION` settings have an exclusion to
/// apply. The four modelled values are matched exactly, so a value the model widens the
/// enum with later is not read as an exclusion setting by its name alone.
pub fn honours_exclusion_filters(setting: &str) -> bool {
    matches!(setting, "IMMUTABLE_WITH_EXCLUSION" | "MUTABLE_WITH_EXCLUSION")
}

/// Refuse a tag-mutability configuration that pairs exclusion filters with a setting
/// that has no exclusion to apply: `MUTABLE` lets every tag be overwritten and
/// `IMMUTABLE` lets none, so a filter beside either could never be consulted. The model
/// cannot express the pairing, so both the operations that write this configuration
/// check it here, through one rule.
///
/// The converse is NOT refused: a `*_WITH_EXCLUSION` setting carrying no filter excludes
/// nothing, which is exactly its plain counterpart's behaviour rather than a
/// contradiction.
pub fn check_exclusion_filters(
    setting: &str,
    filters: &[ImageTagMutabilityExclusionFilter],
) -> Result<(), Fault> {
    if !filters.is_empty() && !honours_exclusion_filters(setting) {
        return Err(Fault::InvalidParameter(format!(
            "imageTagMutabilityExclusionFilters cannot be specified with the {setting} \
             image tag mutability setting"
        )));
    }
    Ok(())
}

/// The identity of a namespace prefix: a repository creation template's `prefix`, and a
/// pull through cache rule's `ecrRepositoryPrefix`, which documents the same rule.
///
/// `Prefix`'s own documentation: "There is always an assumed / applied to the end of the
/// prefix. If you specify ecr-public as the prefix, Amazon ECR treats that as
/// ecr-public/." That assumed separator is a single one, so exactly one trailing slash
/// is not part of the namespace's identity: `prod` and `prod/` name the same template —
/// or the same cache rule — and the stored record spells it one way.
pub fn normalize_prefix(prefix: &str) -> String {
    prefix.strip_suffix('/').unwrap_or(prefix).to_string()
}

/// The wording ECR uses when a repository does not exist.
pub fn repository_not_found_message(name: &str) -> String {
    format!(
        "The repository with name '{name}' does not exist in the registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses for an upload id this registry cannot resolve against the
/// repository the request named — the state `UploadNotFoundException` documents: "the
/// upload could not be found, or the specified upload ID is not valid for this
/// repository". One wording for every operation that addresses an upload.
pub fn upload_not_found_message(upload_id: &str, repository_name: &str) -> String {
    format!(
        "Upload not found with ID: '{upload_id}' for repository '{repository_name}' in \
         the registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when a repository carries no policy: the error's own
/// documentation is "the specified repository and registry combination does not have an
/// associated repository policy".
pub fn repository_policy_not_found_message(name: &str) -> String {
    format!(
        "Repository policy does not exist for the repository with name '{name}' in the \
         registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when the registry carries no signing configuration — the state
/// `SigningConfigurationNotFoundException` documents: "attempting to retrieve or delete a
/// signing configuration that does not exist".
///
/// One wording for both readers of the record (`GetSigningConfiguration` and
/// `DeleteSigningConfiguration`), shaped like [`repository_not_found_message`]'s "… does
/// not exist in the registry with id '<registry id>'", which is also the text the
/// recorded ECR refusal carries.
pub fn signing_configuration_not_found_message() -> String {
    format!(
        "The signing configuration does not exist in the registry with id '{}'",
        registry_id()
    )
}

/// This emulator's registry: its account id is the registry id of every repository.
pub fn registry_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The region this request was signed for, which the ARN and repository URI carry.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:ecr:<region>:<registry id>:repository/<name>`.
pub fn repository_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("ecr", region, &format!("repository/{name}"))
}

/// `<registry id>.dkr.ecr.<region>.amazonaws.com` — the host of this registry in `region`,
/// which every repository URI and the `docker login` endpoint are built on.
pub fn registry_host(region: &str) -> String {
    format!("{}.dkr.ecr.{region}.amazonaws.com", registry_id())
}

/// `<registry id>.dkr.ecr.<region>.amazonaws.com/<name>` — the URI a docker client pushes to.
pub fn repository_uri(region: &str, name: &str) -> String {
    format!("{}/{name}", registry_host(region))
}

/// `https://<registry id>.dkr.ecr.<region>.amazonaws.com` — the registry URL a
/// `docker login` is pointed at.
///
/// `AuthorizationData$proxyEndpoint` states the format itself: "The registry URL to use
/// for this authorization token in a `docker login` command. The Amazon ECR registry URL
/// format is `https://aws_account_id.dkr.ecr.region.amazonaws.com`."
pub fn registry_proxy_endpoint(region: &str) -> String {
    format!("https://{}", registry_host(region))
}

/// Refuse a request that names a registry other than this emulator's: there is exactly
/// one registry here, and a repository in another account's registry is not ours to serve.
pub fn check_registry(registry_id_input: Option<&String>) -> Result<(), Fault> {
    match registry_id_input {
        Some(id) if id != registry_id() => Err(Fault::InvalidParameter(format!(
            "registryId '{id}' is not this registry; the only registry served here is '{}'",
            registry_id()
        ))),
        _ => Ok(()),
    }
}

/// The policy-language versions an IAM JSON policy document may declare. The IAM JSON
/// policy reference documents exactly two `Version` values: `2012-10-17`, the current
/// one, and `2008-10-17`, the legacy one that no new policy should use.
pub const POLICY_VERSIONS: [&str; 2] = ["2012-10-17", "2008-10-17"];

/// The values an IAM statement's `Effect` may take — "the Effect element is required
/// and specifies whether the statement results in an allow or an explicit deny".
pub const POLICY_EFFECTS: [&str; 2] = ["Allow", "Deny"];

/// The wording ECR uses when the registry carries no registry policy.
pub fn registry_policy_not_found_message() -> String {
    format!(
        "Registry policy does not exist in the registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when a repository carries no lifecycle policy.
pub fn lifecycle_policy_not_found_message(name: &str) -> String {
    format!(
        "Lifecycle policy does not exist for the repository with name '{name}' in the \
         registry with id '{}'",
        registry_id()
    )
}

/// `member` of the request does not hold a usable policy document, because `detail`.
fn bad_policy(member: &str, detail: impl std::fmt::Display) -> Fault {
    Fault::InvalidParameter(format!(
        "Invalid parameter at '{member}' failed to satisfy constraint: {detail}"
    ))
}

/// Validate `text` as a resource-based IAM policy document, or report
/// [`Fault::InvalidParameter`] naming what the document gets wrong.
///
/// The grammar enforced here is the IAM JSON policy language's own: the document is a
/// JSON object; a `Version` it declares is one of [`POLICY_VERSIONS`]; `Statement` is
/// required and holds either one statement object or a non-empty array of them; and
/// every statement carries an `Effect` of [`POLICY_EFFECTS`], at least one action
/// (`Action` or `NotAction`) and — because this is a RESOURCE-based policy, which
/// states who the permission is granted to — a `Principal`.
///
/// `check_statement` is called with each statement object once it satisfies that
/// grammar, so a service that adds its own rule about what a policy may say (see
/// [`validate_registry_policy`], and the repository-policy lock-out rule in
/// `ops::set_repository_policy`) does not have to parse the document a second time; a
/// service with no such rule passes `|_| Ok(())`.
///
/// Parsing is `serde_json`'s; only the policy grammar on top of it is ours.
pub fn validate_resource_policy(
    member: &str,
    text: &str,
    mut check_statement: impl FnMut(&serde_json::Map<String, serde_json::Value>) -> Result<(), Fault>,
) -> Result<(), Fault> {
    let document: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| bad_policy(member, format!("the policy is not valid JSON: {error}")))?;
    let document = document
        .as_object()
        .ok_or_else(|| bad_policy(member, "the policy must be a JSON object"))?;

    if let Some(version) = document.get("Version") {
        let declared = version.as_str().ok_or_else(|| {
            bad_policy(member, "the policy's Version element must be a string")
        })?;
        if !POLICY_VERSIONS.contains(&declared) {
            return Err(bad_policy(
                member,
                format!(
                    "the policy declares Version '{declared}', which is not one of {}",
                    POLICY_VERSIONS.join(", ")
                ),
            ));
        }
    }

    let statement = document
        .get("Statement")
        .ok_or_else(|| bad_policy(member, "the policy must carry a Statement element"))?;
    let statements: Vec<&serde_json::Value> = match statement {
        serde_json::Value::Array(entries) => entries.iter().collect(),
        single => vec![single],
    };
    if statements.is_empty() {
        return Err(bad_policy(
            member,
            "the policy's Statement element must carry at least one statement",
        ));
    }

    for entry in statements {
        let entry = entry
            .as_object()
            .ok_or_else(|| bad_policy(member, "every policy statement must be a JSON object"))?;
        let effect = entry
            .get("Effect")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                bad_policy(member, "every policy statement must carry a string Effect")
            })?;
        if !POLICY_EFFECTS.contains(&effect) {
            return Err(bad_policy(
                member,
                format!(
                    "a policy statement's Effect is '{effect}', which is not one of {}",
                    POLICY_EFFECTS.join(", ")
                ),
            ));
        }
        if !["Action", "NotAction"]
            .iter()
            .any(|key| entry.get(*key).is_some_and(names_values))
        {
            return Err(bad_policy(
                member,
                "every policy statement must carry at least one Action or NotAction",
            ));
        }
        entry.get("Principal").filter(|p| names_values(p)).ok_or_else(|| {
            bad_policy(
                member,
                "every statement of a resource-based policy must carry a Principal",
            )
        })?;
        check_statement(entry)?;
    }
    Ok(())
}

/// Whether a policy element names at least one value: a non-empty string, a non-empty
/// array of strings, or — as `Principal` is written — an object mapping a principal
/// type to either of those.
fn names_values(element: &serde_json::Value) -> bool {
    match element {
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(entries) => !entries.is_empty() && entries.iter().all(names_values),
        serde_json::Value::Object(members) => {
            !members.is_empty() && members.values().all(names_values)
        }
        _ => false,
    }
}

/// Every value a policy element names, flattened out of the string, the array or the
/// `{"AWS": …}`-style object it may be written as. `Principal`, `Action` and `NotAction`
/// are all written in those three forms, so one flattener reads them all.
pub fn policy_values(element: &serde_json::Value) -> Vec<&str> {
    match element {
        serde_json::Value::String(value) => vec![value.as_str()],
        serde_json::Value::Array(entries) => entries.iter().flat_map(policy_values).collect(),
        serde_json::Value::Object(members) => members.values().flat_map(policy_values).collect(),
        _ => Vec::new(),
    }
}

/// Validate a registry permissions policy document: the resource-policy grammar of
/// [`validate_resource_policy`], plus the one rule a REGISTRY policy adds — it grants to
/// other accounts, so every statement must name the identities it grants to and may not
/// open the registry to every principal with the `*` wildcard.
pub fn validate_registry_policy(member: &str, text: &str) -> Result<(), Fault> {
    validate_resource_policy(member, text, |statement| {
        // The grammar above guarantees the statement carries a `Principal`.
        let grants_to_everyone = statement
            .get("Principal")
            .is_some_and(|principal| policy_values(principal).contains(&"*"));
        if grants_to_everyone {
            return Err(bad_policy(
                member,
                "a registry policy statement cannot grant to every principal ('*'); \
                 name the accounts or roles the registry is shared with",
            ));
        }
        Ok(())
    })
}

/// The resource-type prefix of an ECR repository ARN's resource field.
pub const REPOSITORY_RESOURCE_PREFIX: &str = "repository/";

/// The repository a tagging operation's `resourceArn` names.
///
/// ECR's tagging operations address their resource by ARN, and "currently, the only
/// supported resource is an Amazon ECR repository" (`resourceArn`'s own documentation),
/// so a usable value is an `arn:<partition>:ecr:<region>:<account>:repository/<name>`
/// of THIS registry: anything else — a malformed ARN, another service, another resource
/// type — is [`Fault::InvalidParameter`], and another account's registry is refused the
/// way [`check_registry`] refuses it. Whether the named repository exists is the
/// caller's lookup, so a well-formed ARN for an absent repository is reported as
/// [`Fault::RepositoryNotFound`] by the store rather than as an invalid parameter.
pub fn repository_name_from_arn(arn: &str) -> Result<String, Fault> {
    let invalid = || {
        Fault::InvalidParameter(format!(
            "resourceArn '{arn}' is not a valid Amazon ECR repository ARN; the only \
             supported resource is an Amazon ECR repository"
        ))
    };
    // `splitn(6, ..)` keeps the resource field whole: it is the only one that may carry
    // a colon of its own.
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = fields[..] else {
        return Err(invalid());
    };
    if prefix != "arn" || partition.is_empty() || service != "ecr" || region.is_empty() {
        return Err(invalid());
    }
    let name = resource
        .strip_prefix(REPOSITORY_RESOURCE_PREFIX)
        .filter(|name| !name.is_empty())
        .ok_or_else(invalid)?;
    if account != registry_id() {
        return Err(Fault::InvalidParameter(format!(
            "resourceArn '{arn}' names the registry '{account}'; the only registry \
             served here is '{}'",
            registry_id()
        )));
    }
    Ok(name.to_string())
}

/// Refuse a tag key no caller may create or remove: one in AWS's reserved namespace, or
/// one longer than a tag key may be.
fn check_tag_key(key: &str) -> Result<(), Fault> {
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Err(Fault::InvalidParameter(format!(
            "tag key '{key}' is reserved: keys beginning with \
             '{RESERVED_TAG_KEY_PREFIX}' are for Amazon Web Services use and cannot be \
             created or removed by a caller"
        )));
    }
    let key_len = key.chars().count();
    if key_len > MAX_TAG_KEY_LEN {
        return Err(Fault::InvalidTagParameter(format!(
            "tag key '{key}' is {key_len} characters; a tag key can have a maximum \
             character length of {MAX_TAG_KEY_LEN}"
        )));
    }
    Ok(())
}

/// Refuse a `tags` list a resource may not be given, whatever it already carries: an
/// unusable key (see [`check_tag_key`]) or an over-long value.
pub fn check_tags(incoming: &[Tag]) -> Result<(), Fault> {
    for tag in incoming {
        check_tag_key(&tag.key)?;
        let value_len = tag.value.chars().count();
        if value_len > MAX_TAG_VALUE_LEN {
            return Err(Fault::InvalidTagParameter(format!(
                "the value of tag key '{}' is {value_len} characters; a tag value can \
                 have a maximum length of {MAX_TAG_VALUE_LEN}",
                tag.key
            )));
        }
    }
    Ok(())
}

/// Refuse a `tagKeys` list a caller may not name (see [`check_tag_key`]).
pub fn check_tag_keys(keys: &[String]) -> Result<(), Fault> {
    for key in keys {
        check_tag_key(key)?;
    }
    Ok(())
}

/// Refuse an `incoming` list that would leave a resource already carrying `existing`
/// over the tag limit. A tag is identified by its key, so a key already held is
/// replaced rather than added and does not count twice.
pub fn check_tag_count(existing: &[Tag], incoming: &[Tag]) -> Result<(), Fault> {
    let total = existing
        .iter()
        .chain(incoming)
        .map(|tag| tag.key.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    if total > MAX_TAGS {
        return Err(Fault::TooManyTags(format!(
            "the request would leave {total} tags on the resource; the maximum number \
             of tags that can be applied to a repository is {MAX_TAGS}"
        )));
    }
    Ok(())
}

/// The content digest of an image manifest: `sha256:<hex>` over the manifest bytes,
/// which is how a registry addresses a manifest it was handed.
pub fn manifest_digest(manifest: &str) -> String {
    content_digest(manifest.as_bytes())
}

/// The content digest of `content`: `sha256:<hex>` over the bytes, the form a registry
/// addresses any content it holds by — a manifest ([`manifest_digest`]) and the layer
/// blob `CompleteLayerUpload` assembles out of an upload's parts alike.
pub fn content_digest(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(content)))
}

/// The media type a manifest declares about itself, for a push that named none.
///
/// Both the Docker image manifest V2 schema 2 and the OCI image manifest carry their own
/// `mediaType`, so the type is read out of the pushed document rather than guessed.
pub fn manifest_media_type(manifest: &str) -> Option<String> {
    let document: serde_json::Value = serde_json::from_str(manifest).ok()?;
    document
        .get("mediaType")?
        .as_str()
        .map(ToString::to_string)
}

/// Every blob an image manifest references: its filesystem layers in the order the
/// manifest lists them, and then its configuration blob.
///
/// A manifest names its blobs as descriptors, and both container-image manifest
/// formats in use spell them the same way: the Docker image manifest V2 schema 2 and
/// the OCI image manifest carry `config` (the image configuration blob, which a
/// registry stores as a blob like any other) and `layers` (the filesystem layers), each
/// descriptor carrying `digest`, `size` and `mediaType`; the older V2 schema 1 lists
/// them as `fsLayers[].blobSum`, a digest and nothing else. A manifest list or OCI
/// image index references *manifests* rather than blobs, so its `manifests` entries are
/// deliberately not read here: they are other images, not this one's layers.
///
/// The layers come before the configuration blob because that is the order a refusal
/// built from this list reports them in — `PutImage`'s `LayersNotFoundException` names
/// the image's layers first — and it is the only place the order is observable: every
/// other reader keys the blobs by digest.
pub fn manifest_blobs(manifest: &str) -> Vec<LayerBlob> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return Vec::new();
    };
    let mut blobs = Vec::new();
    if let Some(layers) = document.get("layers").and_then(serde_json::Value::as_array) {
        blobs.extend(layers.iter().filter_map(blob_descriptor));
    }
    if let Some(fs_layers) = document
        .get("fsLayers")
        .and_then(serde_json::Value::as_array)
    {
        blobs.extend(fs_layers.iter().filter_map(|entry| {
            Some(LayerBlob {
                digest: entry.get("blobSum")?.as_str()?.to_string(),
                size: None,
                media_type: None,
            })
        }));
    }
    if let Some(blob) = document.get("config").and_then(blob_descriptor) {
        blobs.push(blob);
    }
    blobs
}

/// One OCI/Docker content descriptor, or `None` when it names no blob.
fn blob_descriptor(descriptor: &serde_json::Value) -> Option<LayerBlob> {
    Some(LayerBlob {
        digest: descriptor.get("digest")?.as_str()?.to_string(),
        size: descriptor.get("size").and_then(serde_json::Value::as_i64),
        media_type: descriptor
            .get("mediaType")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    })
}

/// Whether `digest` is a content digest at all: `<algorithm>:<hex>`, the form the model
/// constrains `LayerDigest` to (`^[a-zA-Z0-9-_+.]+:[a-fA-F0-9]+$`). A batched request
/// carries `BatchedOperationLayerDigest`, which the model gives no pattern, so a
/// malformed digest reaches a handler and is reported against its own entry instead of
/// refusing the whole batch.
pub fn is_layer_digest(digest: &str) -> bool {
    let Some((algorithm, encoded)) = digest.split_once(':') else {
        return false;
    };
    !algorithm.is_empty()
        && algorithm
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.'))
        && !encoded.is_empty()
        && encoded.chars().all(|c| c.is_ascii_hexdigit())
}

/// Match `tag` against a `*`-glob pattern, where `*` stands for any run of characters.
///
/// Two callers share it. An image-tag exclusion filter: the model constrains
/// `ImageTagMutabilityExclusionFilterValue` to `^[0-9a-zA-Z._*-]{1,128}$`, so `*` is the
/// only metacharacter there is to honour — a general glob crate would match more than
/// ECR does. And an IAM policy's `Action`/`NotAction` pattern (see
/// `ops::set_repository_policy`), where `*` means the same thing; IAM's single-character
/// `?` is deliberately NOT honoured, which leaves that caller's lock-out check
/// conservative — a pattern written with `?` reads as not covering the action, so the
/// policy is accepted rather than refused. (The guardduty service carries its own
/// private matcher for its own filter alphabet; neither is reachable from the other
/// today.)
pub fn wildcard_matches(filter: &str, tag: &str) -> bool {
    let segments: Vec<&str> = filter.split('*').collect();
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return false;
    };
    if segments.len() == 1 {
        return *first == tag;
    }
    let Some(mut rest) = tag.strip_prefix(first) else {
        return false;
    };
    for middle in &segments[1..segments.len() - 1] {
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Encode a pagination token: an opaque rendering of the last key returned, resolved
/// against live state on the next call rather than against a snapshot.
pub fn encode_token(key: &str) -> String {
    hex::encode(key.as_bytes())
}

/// The key a pagination token was built from, or [`Fault::InvalidParameter`].
pub fn decode_token(token: &str) -> Result<String, Fault> {
    let bad = || Fault::InvalidParameter(format!("nextToken '{token}' is not a valid token"));
    let bytes = hex::decode(token).map_err(|_| bad())?;
    String::from_utf8(bytes).map_err(|_| bad())
}

/// One page of `items`, which must be ordered by their keys: everything after the key
/// `next_token` names, capped at `max_results` and never at more than
/// [`DEFAULT_PAGE_SIZE`], plus the token for the page after it.
pub fn paginate<T>(
    items: Vec<(String, T)>,
    next_token: Option<&String>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), Fault> {
    let after = match next_token {
        Some(token) => Some(decode_token(token)?),
        None => None,
    };
    // The modelled `@range(min: 1, max: 1000)` is refused at the boundary, so a
    // non-positive value cannot reach here; it is refused rather than silently read as
    // "an empty page". The page is then clamped to the documented maximum, so one call
    // can never ask for the whole store.
    let page_size = match max_results {
        Some(max) => usize::try_from(max)
            .ok()
            .filter(|size| *size > 0)
            .ok_or_else(|| {
                Fault::InvalidParameter(format!(
                    "maxResults '{max}' must be a positive integer"
                ))
            })?
            .min(DEFAULT_PAGE_SIZE),
        None => DEFAULT_PAGE_SIZE,
    };

    let mut page = Vec::new();
    let mut last_key: Option<String> = None;
    let mut truncated = false;
    for (key, item) in items {
        if after.as_ref().is_some_and(|a| key.as_str() <= a.as_str()) {
            continue;
        }
        if page.len() == page_size {
            truncated = true;
            break;
        }
        last_key = Some(key);
        page.push(item);
    }
    let token = if truncated {
        last_key.map(|key| encode_token(&key))
    } else {
        None
    };
    Ok((page, token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEcrAmazonec2containerregistryV20150921State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEcrAmazonec2containerregistryV20150921State>)
}

/// The registry's scanning configuration — the record `PutRegistryScanningConfiguration`
/// stores and `GetRegistryScanningConfiguration` reports — and everything derived from
/// it, in `state/scanning.rs`. Declared and re-exported here, so a handler reaches it as
/// `crate::state::…` like every other part of this store.
pub mod scanning;
pub use scanning::*;
