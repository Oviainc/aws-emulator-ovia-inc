// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecrpublic-spencerfrontendservice service.
//!
//! This module owns the public registry the handlers in `src/ops/` share — its
//! repositories and the images pushed into them, plus the in-flight layer uploads
//! `InitiateLayerUpload` opens — plus the few pieces of behaviour
//! that no single operation file can own on its own:
//!
//! * the ARN / URI forms every repository carries ([`StoredRepository`] is built once,
//!   by `CreateRepository`, and read back verbatim by every reporting operation);
//! * the `sha256:` manifest digest ([`manifest_digest`]) that `PutImage` keys an image
//!   by and `DescribeImages` / `BatchDeleteImage` resolve against;
//! * the opaque pagination token ([`encode_page_token`] / [`decode_page_token`]) that
//!   `DescribeRepositories` and `DescribeImages` must agree on.
//!
//! Every collection is a `BTreeMap`/`BTreeSet`, so each listing walks one stable,
//! key-ordered sequence and a pagination token naming the last key of a page resumes
//! exactly after it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ImageDetail, ImageIdentifier, Repository, RepositoryCatalogData, RepositoryCatalogDataInput,
    Tag,
};

/// The `ecr-public` ARN namespace, as the Smithy model's `awsService` trait declares it
/// (`OPERATIONS.md`, "Wire protocol": ARN namespace `ecr-public`).
pub const ARN_NAMESPACE: &str = "ecr-public";

/// The universal host prefix every ECR Public repository URI carries, as
/// `Registry$registryUri` documents it ("The URI of a public registry. The URI contains
/// a universal prefix and the registry alias").
pub const GALLERY_HOST: &str = "public.ecr.aws";

/// The registry alias this emulator's single public registry is assigned.
///
/// `RegistryAlias$defaultRegistryAlias` documents that "when the first public repository
/// is created, your public registry is assigned a default registry alias"; the real
/// service mints an opaque one per account. The emulator has exactly one registry, so it
/// carries one fixed alias instead — it satisfies `RegistryAliasName`'s modelled
/// `^[a-z][a-z0-9]+(?:[._-][a-z0-9]+)*$` and 2..50 length, and being stable is what lets
/// a `repositoryUri` be compared across calls.
pub const DEFAULT_REGISTRY_ALIAS: &str = "emulator";

/// The largest page either `Describe*` operation will serve, and the page size a request
/// that named none resolves to: `MaxResults` is modelled `range 1..1000`, so 1000 is the
/// most a caller can ask for.
pub const MAX_PAGE_SIZE: i32 = 1000;

/// The most tags a repository may carry.
///
/// `TooManyTagsException` documents the limit itself: "The list of tags on the repository
/// is over the limit. The maximum number of tags that can be applied to a repository is
/// 50." The modelled `TagList` length (0..200) is wider, so this bound is enforced here.
pub const MAX_TAGS_PER_REPOSITORY: usize = 50;

/// The ceiling on how many repositories this store holds, how many images one repository
/// holds, and how many image tags point into one repository: the three caps that bound
/// caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create or a push is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeRepositories` and `DescribeImages` must still answer what `CreateRepository`
/// and `PutImage` stored. The two operations that grow the store refuse past these caps
/// with the `LimitExceededException` both of them model ("the operation didn't succeed
/// because it would have exceeded a service limit for your account") and no read can
/// return. The image cap is the one that matters most: `imageManifest` is modelled
/// `@length(1..4194304)`, so each image carries up to 4 MiB of caller-supplied text.
/// The numbers are the emulator's own bound, not a published AWS default — AWS publishes
/// the repository and image quotas as adjustable, so no single value is the contract —
/// and they are set high enough that no ordinary workflow meets one.
pub const MAX_REPOSITORIES: usize = 1_000;

/// See [`MAX_REPOSITORIES`].
pub const MAX_IMAGES_PER_REPOSITORY: usize = 1_000;

/// See [`MAX_REPOSITORIES`]. This one bounds `tag_index`, which grows by one entry per
/// push that introduces an image tag the repository did not carry.
pub const MAX_IMAGE_TAGS_PER_REPOSITORY: usize = 10_000;

/// See [`MAX_REPOSITORIES`]. This one bounds [`ComAmazonawsEcrpublicSpencerfrontendserviceState::uploads`],
/// which grows by one entry per `InitiateLayerUpload`. An upload a client opened and
/// then abandoned is never reclaimed — [`LayerUpload::started_at`] records when the
/// upload was opened, it is not an expiry the store acts on — so the registry needs a
/// ceiling on how many in-flight layer uploads it holds at once. Past it the open is
/// refused with the `ServerException` `InitiateLayerUpload` models: reaching the ceiling
/// is this registry's own capacity, not a fault in the caller's request.
pub const MAX_LAYER_UPLOADS: usize = 1_000;

/// The `partSize` `InitiateLayerUpload` reports: the size, in bytes, the registry tells
/// the client to send future layer parts in.
///
/// AWS publishes no single number for it — it is the registry's own answer, which is
/// exactly why the client is told it rather than assuming one — so this is the
/// emulator's choice, bounded by the only published constraint on a part: the model caps
/// `UploadLayerPart$layerPartBlob` at 20971520 bytes, so a part of `partSize` bytes must
/// fit under that cap.
pub const REPORTED_LAYER_PART_SIZE: i64 = 10 * 1024 * 1024;

/// One in-flight layer upload: what `InitiateLayerUpload` opened, addressed by the
/// upload id it handed the client.
///
/// `UploadLayerPart` and `CompleteLayerUpload` are the operations that consume one (both
/// still 501 stubs with issues of their own), so what is kept here is what opening the
/// upload decided: which repository it is for, the part size the client was told to send
/// parts in, and when it was opened.
#[derive(Debug, Clone)]
pub struct LayerUpload {
    /// The repository the upload was opened against. An upload id is only valid for the
    /// repository it was issued for, so every later call names both.
    pub repository_name: String,
    /// The `partSize` reported to the client when the upload was opened.
    pub part_size: i64,
    /// When `InitiateLayerUpload` opened it. Nothing reads it yet: the first operation
    /// that does — `UploadLayerPart` or `CompleteLayerUpload`, each a 501 stub with its
    /// own issue — is the one to decide whether an upload also expires with age.
    pub started_at: SystemTime,
}

/// One image stored under a repository: what `PutImage` persisted, and everything
/// `DescribeImages` reports about it.
#[derive(Debug, Clone)]
pub struct StoredImage {
    /// The `sha256:<hex>` digest of [`Self::manifest`] — this image's identity.
    pub digest: String,
    /// The manifest document exactly as it was pushed.
    pub manifest: String,
    /// The manifest's own media type: the one the request declared, else the manifest
    /// document's top-level `mediaType`.
    pub manifest_media_type: Option<String>,
    /// The artifact media type: the manifest document's `config.mediaType`.
    pub artifact_media_type: Option<String>,
    /// `imageSizeInBytes`: the byte length of the stored manifest.
    pub size_in_bytes: i64,
    /// When the manifest was pushed.
    pub pushed_at: SystemTime,
    /// Every tag currently pointing at this digest, in tag order.
    pub tags: BTreeSet<String>,
}

/// One public repository: what `CreateRepository` inserted.
#[derive(Debug, Clone)]
pub struct StoredRepository {
    pub name: String,
    pub arn: String,
    pub registry_id: String,
    pub uri: String,
    pub created_at: SystemTime,
    pub catalog_data: RepositoryCatalogData,
    /// Resource tags, keyed by tag key.
    pub tags: BTreeMap<String, String>,
    /// Images keyed by `imageDigest`, so `DescribeImages` pages in digest order.
    pub images: BTreeMap<String, StoredImage>,
    /// `imageTag` -> `imageDigest`, so a tag-keyed `ImageIdentifier` resolves to a
    /// stored image in one probe and re-pushing a tag moves it between digests.
    pub tag_index: BTreeMap<String, String>,
}

impl StoredRepository {
    /// The image a caller's [`ImageIdentifier`] names, or `None` when nothing stored
    /// matches it.
    ///
    /// An identifier may carry a digest, a tag, or both; when it carries both they must
    /// agree, which is what [`Self::resolve_identifier`]'s `Err` reports.
    pub fn resolve_identifier(
        &self,
        id: &ImageIdentifier,
    ) -> Result<&StoredImage, IdentifierProblem> {
        let by_digest = id.image_digest.as_deref().filter(|d| !d.is_empty());
        let by_tag = id.image_tag.as_deref().filter(|t| !t.is_empty());
        match (by_digest, by_tag) {
            (None, None) => Err(IdentifierProblem::MissingDigestAndTag),
            (Some(digest), None) => self
                .images
                .get(digest)
                .ok_or(IdentifierProblem::ImageNotFound),
            (None, Some(tag)) => self
                .tag_index
                .get(tag)
                .and_then(|digest| self.images.get(digest))
                .ok_or(IdentifierProblem::ImageNotFound),
            (Some(digest), Some(tag)) => {
                let tagged = self
                    .tag_index
                    .get(tag)
                    .ok_or(IdentifierProblem::ImageNotFound)?;
                if tagged != digest {
                    return Err(IdentifierProblem::ImageTagDoesNotMatchDigest);
                }
                self.images
                    .get(digest)
                    .ok_or(IdentifierProblem::ImageNotFound)
            }
        }
    }

    /// This image as the modelled `ImageDetail` shape.
    pub fn image_detail(&self, image: &StoredImage) -> ImageDetail {
        ImageDetail {
            registry_id: Some(self.registry_id.clone()),
            repository_name: Some(self.name.clone()),
            image_digest: Some(image.digest.clone()),
            image_tags: Some(image.tags.iter().cloned().collect()),
            image_size_in_bytes: Some(image.size_in_bytes),
            image_pushed_at: Some(as_timestamp(image.pushed_at)),
            image_manifest_media_type: image.manifest_media_type.clone(),
            artifact_media_type: image.artifact_media_type.clone(),
        }
    }

    /// This repository as the modelled `Repository` shape.
    pub fn as_repository(&self) -> Repository {
        Repository {
            repository_arn: Some(self.arn.clone()),
            registry_id: Some(self.registry_id.clone()),
            repository_name: Some(self.name.clone()),
            repository_uri: Some(self.uri.clone()),
            created_at: Some(as_timestamp(self.created_at)),
        }
    }
}

/// Why an [`ImageIdentifier`] names no stored image. Each variant is one of the
/// `ImageFailureCode` values the model declares, which is how `BatchDeleteImage` reports
/// it; `DescribeImages` answers the same three cases with its own modelled errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierProblem {
    /// The identifier carried neither a digest nor a tag.
    MissingDigestAndTag,
    /// Nothing stored carries that digest or that tag.
    ImageNotFound,
    /// Both were given and the tag points at a different digest.
    ImageTagDoesNotMatchDigest,
}

impl IdentifierProblem {
    /// The `ImageFailureCode` value for this problem.
    pub fn failure_code(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => "MissingDigestAndTag",
            Self::ImageNotFound => "ImageNotFound",
            Self::ImageTagDoesNotMatchDigest => "ImageTagDoesNotMatchDigest",
        }
    }

    /// The reason to report beside the code.
    pub fn failure_reason(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => {
                "Invalid request parameters: at least one of imageTag or imageDigest must be \
                 specified"
            }
            Self::ImageNotFound => "Requested image not found",
            Self::ImageTagDoesNotMatchDigest => {
                "Requested image tag does not match the specified image digest"
            }
        }
    }
}

/// Per-service state for com-amazonaws-ecrpublic-spencerfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEcrpublicSpencerfrontendserviceState {
    /// The 12-digit account id of the one public registry this emulator serves. It is
    /// the `registryId` every response carries and the account segment of every ARN.
    pub registry_id: String,
    /// The registry's alias, which is the second segment of every `repositoryUri`.
    pub registry_alias: String,
    /// Repositories keyed by `repositoryName`, so `DescribeRepositories` pages in name
    /// order.
    pub repositories: BTreeMap<String, StoredRepository>,
    /// In-flight layer uploads keyed by the `uploadId` `InitiateLayerUpload` minted, the
    /// handle `UploadLayerPart` and `CompleteLayerUpload` address an upload by.
    pub uploads: BTreeMap<String, LayerUpload>,
}

impl Default for ComAmazonawsEcrpublicSpencerfrontendserviceState {
    fn default() -> Self {
        Self {
            registry_id: ::engine::EMULATOR_ACCOUNT_ID.to_string(),
            registry_alias: DEFAULT_REGISTRY_ALIAS.to_string(),
            repositories: BTreeMap::new(),
            uploads: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsEcrpublicSpencerfrontendserviceState {
    /// The ARN a repository of this registry carries.
    ///
    /// ECR Public is a global (partition-wide) service, so the region segment is empty;
    /// the account segment is the registry id.
    pub fn repository_arn(&self, repository_name: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            "",
            &format!("repository/{repository_name}"),
        )
    }

    /// The `docker pull`-able URI of a repository: the universal gallery prefix, the
    /// registry alias, then the repository name.
    pub fn repository_uri(&self, repository_name: &str) -> String {
        format!("{GALLERY_HOST}/{}/{repository_name}", self.registry_alias)
    }

    /// Whether `registry_id` — a request's `registryId`, which the model lets be either
    /// an account id or a registry alias — names this registry.
    pub fn is_this_registry(&self, registry_id: &str) -> bool {
        registry_id == self.registry_id || registry_id == self.registry_alias
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEcrpublicSpencerfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEcrpublicSpencerfrontendserviceState>)
}

/// The `sha256:<hex>` digest of a manifest, which is an image's identity in a registry.
///
/// `ImageDetail$imageDigest` documents it as "the `sha256` digest of the image
/// manifest", so it is computed over the manifest bytes exactly as pushed.
pub fn manifest_digest(manifest: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()))
}

/// A `SystemTime` as a modelled timestamp.
pub fn as_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The catalog data a repository ends up with, given the `RepositoryCatalogDataInput` a
/// request supplied.
///
/// Every text/list member passes through unchanged. `logoUrl` is documented as "the URL
/// that contains the logo that's associated with the repository", so it exists only once
/// a logo payload has been stored, and its last segment is the payload's own sha256
/// digest — derived from the bytes, so the same logo always yields the same URL.
/// `marketplaceCertified` is "whether the repository is certified by Amazon Web Services
/// Marketplace", which a repository created through this API never is.
pub fn catalog_data_of(
    repository_name: &str,
    input: Option<&RepositoryCatalogDataInput>,
) -> RepositoryCatalogData {
    let Some(input) = input else {
        return RepositoryCatalogData {
            marketplace_certified: Some(false),
            ..RepositoryCatalogData::default()
        };
    };
    RepositoryCatalogData {
        description: input.description.clone(),
        architectures: input.architectures.clone(),
        operating_systems: input.operating_systems.clone(),
        logo_url: input.logo_image_blob.as_ref().map(|blob| {
            format!(
                "https://{GALLERY_HOST}/logos/{repository_name}/{:x}",
                Sha256::digest(blob)
            )
        }),
        about_text: input.about_text.clone(),
        usage_text: input.usage_text.clone(),
        marketplace_certified: Some(false),
    }
}

/// The tags a request supplied, keyed by key.
///
/// `Err` carries the `InvalidTagParameterException` message for a tag that carries no
/// key: a tag is a key-value pair whose key "acts like a category" (`Tag$Key`), so
/// there is nothing to store without one. The modelled 1..128 / 0..256 lengths are
/// already refused before a handler runs.
pub fn stored_tags(tags: Option<&Vec<Tag>>) -> Result<BTreeMap<String, String>, String> {
    let mut stored = BTreeMap::new();
    for tag in tags.map(Vec::as_slice).unwrap_or_default() {
        let Some(key) = tag.key.as_deref().filter(|key| !key.is_empty()) else {
            return Err("Invalid tag: a tag must specify a Key".to_string());
        };
        stored.insert(
            key.to_string(),
            tag.value.clone().unwrap_or_default(),
        );
    }
    Ok(stored)
}

/// One page of a listing, and the token that resumes after it.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// The page size to serve a request that asked for `max_results`, clamped to the
/// modelled `1..1000` so a caller can neither ask for an unbounded page nor for none.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

/// Page `entries` — already in the listing's stable key order — from `resume_after`
/// onwards, taking at most `page_size` of them.
///
/// The returned token names where this page ENDED, so the next call resumes at the
/// following entry rather than restarting.
pub fn page<T>(
    entries: Vec<(String, T)>,
    resume_after: Option<&str>,
    page_size: usize,
) -> Page<T> {
    let mut remaining = entries
        .into_iter()
        .filter(|(key, _)| resume_after.is_none_or(|after| key.as_str() > after));

    let mut items = Vec::new();
    let mut last_key = None;
    for (key, item) in remaining.by_ref().take(page_size) {
        items.push(item);
        last_key = Some(key);
    }

    Page {
        next_token: match (remaining.next().is_some(), last_key) {
            (true, Some(key)) => Some(encode_page_token(&key)),
            _ => None,
        },
        items,
    }
}

/// The opaque pagination token naming the last key of a page.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key.as_bytes())
}

/// The key a `nextToken` resumes after, or the `InvalidParameterException` message for a
/// token this service did not mint.
pub fn decode_page_token(token: Option<&str>) -> Result<Option<String>, String> {
    let Some(token) = token else {
        return Ok(None);
    };
    let bytes = hex::decode(token).map_err(|_| invalid_token_message())?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| invalid_token_message())
}

/// What to tell a caller that presented a token this service did not mint.
fn invalid_token_message() -> String {
    "Invalid parameter at 'nextToken' failed to satisfy constraint: 'Member must be a \
     token returned by a previous call'"
        .to_string()
}
