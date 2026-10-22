// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecr-amazonec2containerregistry-v20150921 service.
//!
//! One registry (this emulator's account) holding repositories by name, each holding
//! its images by digest plus a tag index. Everything an operation answers with is
//! derived from here; nothing is re-synthesised from a request.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{
    EncryptionConfiguration, ImageDetail, ImageIdentifier, ImageScanningConfiguration,
    ImageTagMutabilityExclusionFilter, Repository, RepositoryScanningConfiguration, Tag,
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

/// The `ImageStatus` of an image in standard storage. Every image this emulator
/// stores is in standard storage (moving one to archive is `UpdateImageStorageClass`,
/// still a 501 stub), so every stored image reports `ACTIVE`.
pub const ACTIVE_IMAGE_STATUS: &str = "ACTIVE";

/// The `ScanFrequency` of a repository whose scan on push is switched on: a push is what
/// triggers its scan. (`ScanFrequency`'s modelled values are `SCAN_ON_PUSH`,
/// `CONTINUOUS_SCAN` and `MANUAL`; `CONTINUOUS_SCAN` belongs to `ENHANCED` registry
/// scanning, which no operation configures here yet.)
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

/// The region an unsigned request is served in: a request that carries no SigV4
/// credential scope names no region, and ECR's ARNs and repository URIs are regional.
pub const DEFAULT_REGION: &str = "us-east-1";

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
}

impl Fault {
    /// The message this failure renders with.
    pub fn message(&self) -> &str {
        match self {
            Self::RepositoryNotFound(m) | Self::InvalidParameter(m) | Self::ImageNotFound(m) => m,
        }
    }
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
    pub image_scanning_config: Option<ImageScanningConfiguration>,
    pub encryption_config: EncryptionConfiguration,
    pub tags: Vec<Tag>,
    pub images: BTreeMap<String, ImageRecord>,
    pub tag_index: BTreeMap<String, String>,
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
            image_scanning_configuration: self.image_scanning_config.clone(),
            encryption_configuration: Some(self.encryption_config.clone()),
        }
    }

    /// The scanning configuration reported for this repository.
    ///
    /// Derived from what is stored, never from the request: `scanOnPush` is the flag the
    /// repository's own `imageScanningConfiguration` carries (absent means not
    /// configured, so `false`, the member's modelled default), and the frequency follows
    /// from it — a repository scanned on push has the `SCAN_ON_PUSH` frequency, and one
    /// that is not has the `MANUAL` one. `appliedScanFilters` lists the registry
    /// scanning rules' filters that match this repository; a registry nobody has
    /// configured carries no rules (`PutRegistryScanningConfiguration` is still a 501
    /// stub), so no filter applies and the list is empty.
    pub fn to_scanning_configuration(&self) -> RepositoryScanningConfiguration {
        let scan_on_push = self
            .image_scanning_config
            .as_ref()
            .is_some_and(|config| config.scan_on_push);
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
}

/// Per-service state for com-amazonaws-ecr-amazonec2containerregistry-v20150921. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// Repositories by name. A `BTreeMap` so every listing — and therefore every
    /// pagination token — has one deterministic order.
    pub repositories: BTreeMap<String, Repo>,
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
}

/// The wording ECR uses when a repository does not exist.
pub fn repository_not_found_message(name: &str) -> String {
    format!(
        "The repository with name '{name}' does not exist in the registry with id '{}'",
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

/// `<registry id>.dkr.ecr.<region>.amazonaws.com/<name>` — the URI a docker client pushes to.
pub fn repository_uri(region: &str, name: &str) -> String {
    format!(
        "{}.dkr.ecr.{region}.amazonaws.com/{name}",
        registry_id()
    )
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

/// The content digest of an image manifest: `sha256:<hex>` over the manifest bytes,
/// which is how a registry addresses a manifest it was handed.
pub fn manifest_digest(manifest: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(manifest.as_bytes())))
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

/// Match `tag` against an exclusion filter. `*` is the only metacharacter a filter can
/// carry: the model constrains `ImageTagMutabilityExclusionFilterValue` to
/// `^[0-9a-zA-Z._*-]{1,128}$`, so there is no other wildcard to honour — a general glob
/// crate would match more than ECR does. (The guardduty service carries its own private
/// matcher for its own filter alphabet; neither is reachable from the other today.)
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
