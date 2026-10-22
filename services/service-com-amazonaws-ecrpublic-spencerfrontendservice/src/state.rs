// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecrpublic-spencerfrontendservice service.
//!
//! This module owns the public registry the handlers in `src/ops/` share — its
//! repositories, the images pushed into them, the in-flight layer uploads
//! `InitiateLayerUpload` opens and the one catalog display name the registry itself
//! carries — plus the few pieces of behaviour
//! that no single operation file can own on its own:
//!
//! * the ARN / URI forms every repository carries ([`StoredRepository`] is built once,
//!   by `CreateRepository`, and read back verbatim by every reporting operation);
//! * the repository policy document ([`StoredRepository::policy_text`]) that
//!   `SetRepositoryPolicy` writes and `GetRepositoryPolicy` / `DeleteRepositoryPolicy`
//!   read back;
//! * the `sha256:` manifest digest ([`manifest_digest`]) that `PutImage` keys an image
//!   by and `DescribeImages` / `BatchDeleteImage` resolve against;
//! * what a manifest references ([`manifest_content`]), which is what an image's
//!   reported size is computed over ([`StoredRepository::refresh_image_sizes`]);
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
    ImageDetail, ImageIdentifier, RegistryCatalogData, Repository, RepositoryCatalogData,
    RepositoryCatalogDataInput, Tag,
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

/// The status every alias of this registry carries.
///
/// `RegistryAliasStatus` is modelled as an enum of `ACTIVE` / `PENDING` / `REJECTED`
/// (`REGISTRY_ALIAS_STATUS_VALUES` in `ops/types/shared_types.rs`), and `PENDING` /
/// `REJECTED` describe a *requested* custom alias: this service exposes no operation that
/// requests one, so the alias the registry was assigned is the only one it has and it is
/// usable — `ACTIVE`.
pub const REGISTRY_ALIAS_STATUS_ACTIVE: &str = "ACTIVE";

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

/// How many levels of manifest list [`StoredRepository::refresh_image_sizes`] resolves
/// when it recomputes the size of every stored image.
///
/// An index entry may name another index, so a list's size can rest on a chain of lists;
/// the documents and the chain are both caller-supplied, so the resolution is bounded.
/// Four is well past anything a registry client builds (an index over image manifests is
/// one level).
pub const MAX_MANIFEST_LIST_DEPTH: usize = 4;

/// The most `manifests[*]` entries this service reads from one manifest list.
///
/// A manifest list's entries are caller-supplied and `imageManifest` is modelled
/// `@length(1..4194304)`, so one document can name tens of thousands of children; this
/// is the bound that, together with [`MAX_MANIFEST_LIST_DEPTH`] and
/// [`MAX_IMAGES_PER_REPOSITORY`], caps the work of recomputing image sizes. AWS
/// publishes no limit on the entries of an index, and real indexes carry one entry per
/// platform (a handful), so a document that names more children than this states no size
/// this service reports rather than being read in part.
pub const MAX_MANIFEST_LIST_ENTRIES: usize = 100;

/// One image stored under a repository: what `PutImage` persisted, and everything
/// `DescribeImages` reports about it.
#[derive(Debug, Clone)]
pub struct StoredImage {
    /// The `sha256:<hex>` digest of [`Self::manifest`] — this image's identity.
    pub digest: String,
    /// The manifest document exactly as it was pushed.
    pub manifest: String,
    /// What [`Self::manifest`] references, read ONCE when the image was stored (see
    /// [`manifest_content`]): the layer sizes of an image manifest, or the digests a
    /// manifest list names. Sizing an image never re-parses a manifest document.
    pub references: ManifestContent,
    /// `imageSizeInBytes`: the size this image reports, derived from [`Self::references`]
    /// and, for a manifest list, from the images stored beside it. Maintained by
    /// [`StoredRepository::refresh_image_sizes`] whenever the repository's image set
    /// changes, so a read projects it in constant time; `None` when nothing stored
    /// states a size for this image.
    pub size_in_bytes: Option<i64>,
    /// The manifest's own media type: the one the request declared, else the manifest
    /// document's top-level `mediaType`.
    pub manifest_media_type: Option<String>,
    /// The artifact media type: the manifest document's `config.mediaType`.
    pub artifact_media_type: Option<String>,
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
    /// The repository policy document, exactly as `SetRepositoryPolicy` supplied it, or
    /// `None` while this repository carries no policy.
    ///
    /// `RepositoryPolicyText` is modelled as an opaque string, so the document is stored
    /// and reported byte for byte: `GetRepositoryPolicy` documents that "the policy text
    /// will be in JSON format", not that the service reformats it.
    pub policy_text: Option<String>,
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
            image_size_in_bytes: image.size_in_bytes,
            image_pushed_at: Some(as_timestamp(image.pushed_at)),
            image_manifest_media_type: image.manifest_media_type.clone(),
            artifact_media_type: image.artifact_media_type.clone(),
        }
    }

    /// Recompute the `imageSizeInBytes` every image in this repository reports.
    ///
    /// `ImageDetail$imageSizeInBytes` is "the size, in bytes, of the image in the
    /// repository", which the same doc's note pins to the layer content the client
    /// pushed ("the Docker client compresses image layers before pushing them"): it is
    /// the total size of the layer blobs the manifest references, never the size of the
    /// manifest document or of the config blob. "If the image is a manifest list, this
    /// is the max size of all manifests in the list", so a list reports the largest of
    /// the images it references, each child's size being its own layer total.
    ///
    /// A list's size is a statement about other images in the repository, so it is
    /// recomputed here whenever the image set changes — a push or a delete — rather than
    /// frozen when the list itself arrived, and every read then projects
    /// [`StoredImage::size_in_bytes`] in constant time. The recomputation reads no
    /// manifest document (each one was parsed once, into [`StoredImage::references`]) and
    /// its work is the product of three documented bounds:
    /// [`MAX_IMAGES_PER_REPOSITORY`] entries, each resolving at most
    /// [`MAX_MANIFEST_LIST_ENTRIES`] children, over at most [`MAX_MANIFEST_LIST_DEPTH`]
    /// levels of nesting.
    ///
    /// A size is `None` for an image nothing stored states one for: the member is
    /// neither required nor defaulted, so an unknown size is absent rather than zero.
    /// That covers a document that references no layer or manifest array at all, a layer
    /// descriptor that states no usable size (none at all, or one no byte count can be),
    /// a total no `long` can represent — the addends are caller-supplied, so they are
    /// added with checked arithmetic and never wrapped — and a list none of whose
    /// children the repository holds. An image manifest with an EMPTY `layers` array is
    /// a statement, not a gap: it references no layer content, so its total is `0`.
    pub fn refresh_image_sizes(&mut self) {
        // An image manifest's total rests on nothing but its own descriptors.
        let mut sizes: BTreeMap<String, Option<i64>> = self
            .images
            .iter()
            .map(|(digest, image)| {
                let own = match &image.references {
                    ManifestContent::Layers(layer_sizes) => layer_sizes
                        .iter()
                        .try_fold(0i64, |total, size| total.checked_add((*size)?)),
                    ManifestContent::List(_) | ManifestContent::Opaque => None,
                };
                (digest.clone(), own)
            })
            .collect();

        // A manifest list is the max over the images it names, so a list of lists
        // settles one level per pass; a pass that changes nothing has settled.
        for _ in 0..MAX_MANIFEST_LIST_DEPTH {
            let mut settled = true;
            let mut next = sizes.clone();
            for (digest, image) in &self.images {
                if let ManifestContent::List(children) = &image.references {
                    let resolved = children
                        .iter()
                        .filter_map(|child| sizes.get(child).copied().flatten())
                        .max();
                    if next.insert(digest.clone(), resolved) != Some(resolved) {
                        settled = false;
                    }
                }
            }
            sizes = next;
            if settled {
                break;
            }
        }

        for (digest, size) in sizes {
            if let Some(image) = self.images.get_mut(&digest) {
                image.size_in_bytes = size;
            }
        }
    }

    /// This repository's resource tags as the modelled `TagList`.
    ///
    /// The stored map is keyed by tag key, so the list comes out in key order: a stable
    /// sequence, which is what lets two reads of one repository be compared.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: Some(key.clone()),
                value: Some(value.clone()),
            })
            .collect()
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

/// How a requested `imageIds` entry reads inside an error message.
///
/// ECR names the whole requested [`ImageIdentifier`] back to the caller, both members
/// spelled out and an absent one rendered as `null`, so the caller sees the entry
/// exactly as it sent it rather than only the half that was matched.
///
/// The private-registry service renders it identically
/// (`service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`'s
/// `state::identifier_description`); the two are separate functions only because
/// `ImageIdentifier` is generated per service crate. Keep them in step.
pub fn identifier_description(id: &ImageIdentifier) -> String {
    format!(
        "imageId {{imageDigest:'{}', imageTag:'{}'}}",
        id.image_digest.as_deref().unwrap_or("null"),
        id.image_tag.as_deref().unwrap_or("null")
    )
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
    /// The registry's catalog display name — the ONE `RegistryCatalogData` this registry
    /// has (`RegistryCatalogData` is "the metadata for a public registry" and carries
    /// `displayName` alone), so it lives here beside the registry id rather than per
    /// repository. `None` until a `PutRegistryCatalogData` call sets one, which is what
    /// lets `GetRegistryCatalogData` report an unset display name as absent rather than
    /// as an invented string.
    pub registry_display_name: Option<String>,
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
            registry_display_name: None,
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

    /// The ARN of the registry itself.
    ///
    /// `Registry$registryArn` is "the Amazon Resource Name (ARN) of the public registry",
    /// and ECR Public is a partition-wide service, so — exactly as for a repository ARN —
    /// the region segment is empty and the account segment is the registry id, which is
    /// also the registry resource's own name.
    pub fn registry_arn(&self) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, "", &format!("registry/{}", self.registry_id))
    }

    /// The URI of the registry: `Registry$registryUri` documents it as "a universal
    /// prefix and the registry alias", which is the `repositoryUri` form without the
    /// trailing repository segment.
    pub fn registry_uri(&self) -> String {
        format!("{GALLERY_HOST}/{}", self.registry_alias)
    }

    /// The `docker pull`-able URI of a repository: the universal gallery prefix, the
    /// registry alias, then the repository name.
    pub fn repository_uri(&self, repository_name: &str) -> String {
        format!("{}/{repository_name}", self.registry_uri())
    }

    /// This registry's catalog metadata as the modelled `RegistryCatalogData` shape: what
    /// `GetRegistryCatalogData` reports and `PutRegistryCatalogData` answers with after a
    /// write. There is exactly one per registry, so both operations project this one
    /// stored value.
    pub fn registry_catalog_data(&self) -> RegistryCatalogData {
        RegistryCatalogData {
            display_name: self.registry_display_name.clone(),
        }
    }

    /// Whether `registry_id` — a request's `registryId`, which the model lets be either
    /// an account id or a registry alias — names this registry.
    ///
    /// This leniency belongs to the `registryId` REQUEST MEMBER only: an ARN's account
    /// segment is an account id, so [`Self::repository_by_arn`] compares that segment
    /// against this registry's id and not against its alias.
    pub fn is_this_registry(&self, registry_id: &str) -> bool {
        registry_id == self.registry_id || registry_id == self.registry_alias
    }

    /// The repository a `resourceArn` names, or why nothing stored answers to it.
    ///
    /// The ARN is parsed by [`parse_repository_arn`] and then resolved against this
    /// registry: it must agree with the ARN this registry mints for that repository
    /// ([`Self::repository_arn`]) on partition, account and name, so an ARN naming
    /// another partition or another registry names no repository here — the same
    /// not-found answer as a name this registry never held ("ensure that you're
    /// performing operations on the correct registry",
    /// `RepositoryNotFoundException$message`). The region segment is not compared:
    /// ECR Public is partition-wide, so the ARNs it mints carry none and a caller that
    /// supplies one still names the same resource.
    pub fn repository_by_arn(
        &self,
        resource_arn: &str,
    ) -> Result<&StoredRepository, ResourceArnProblem> {
        let arn = parse_repository_arn(resource_arn)?;
        let not_found = || {
            ResourceArnProblem::RepositoryNotFound(format!(
                "The repository with name '{}' does not exist in the registry with id '{}'",
                arn.name, arn.registry_id
            ))
        };
        // The ARN this registry mints for the same name, so neither the partition nor
        // the account id is written out here: both come from `engine::emulator_arn`.
        let minted = parse_repository_arn(&self.repository_arn(&arn.name))
            .map_err(|_| not_found())?;
        if arn.partition != minted.partition || arn.registry_id != minted.registry_id {
            return Err(not_found());
        }
        self.repositories.get(&arn.name).ok_or_else(not_found)
    }
}

/// Why a `resourceArn` names no repository of this registry.
///
/// The two variants are the two modelled refusals every tagging operation declares:
/// a string that is not a well-formed ECR Public repository ARN is an
/// `InvalidParameterException`, while a well-formed one naming a repository (or a
/// registry) this emulator does not hold is a `RepositoryNotFoundException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceArnProblem {
    /// Not a well-formed ECR Public repository ARN; carries the message that names
    /// the offending member and the constraint it failed.
    Malformed(String),
    /// Well-formed, but nothing stored answers to it.
    RepositoryNotFound(String),
}

/// The registry and repository a well-formed ECR Public repository ARN names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryArn {
    /// The ARN's partition segment (`aws` and its siblings).
    pub partition: String,
    /// The ARN's account segment, which for this service is the registry id.
    pub registry_id: String,
    /// The repository name, which may itself carry `/` separators (a namespaced
    /// repository such as `team/app`), as `RepositoryName`'s modelled pattern
    /// `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$` admits.
    pub name: String,
}

/// What the service tells a caller whose `resourceArn` is not a well-formed ECR Public
/// repository ARN: the parameter-constraint form the real service answers with, naming
/// the member and the constraint it failed (`InvalidParameterException`: "the specified
/// parameter is invalid; review the available parameters for the API request").
pub const INVALID_ARN_MESSAGE: &str =
    "Invalid parameter at 'resourceArn' failed to satisfy constraint: 'Invalid ARN'";

/// Parse `resource_arn` as the ARN of an ECR Public repository.
///
/// `ListTagsForResource$resourceArn` documents that "currently, the supported resource
/// is an Amazon ECR Public repository", so the only string this accepts is the six-field
/// ARN form `arn:<partition>:ecr-public:<region>:<account>:repository/<name>` — the
/// general ARN syntax with this service's namespace and the `repository` resource type.
/// Every other string fails the one `resourceArn` constraint, so they all share
/// [`INVALID_ARN_MESSAGE`].
///
/// The region segment is left free: ECR Public is partition-wide, so the ARNs it mints
/// carry none, but a caller that supplies one still names the same resource.
pub fn parse_repository_arn(resource_arn: &str) -> Result<RepositoryArn, ResourceArnProblem> {
    // The resource segment may itself contain `:`, so only the first five fields are
    // split off; everything after the fifth colon is the resource.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    let malformed = || {
        Err(ResourceArnProblem::Malformed(
            INVALID_ARN_MESSAGE.to_string(),
        ))
    };
    let [arn, partition, service, _region, account, resource] = fields[..] else {
        return malformed();
    };
    if arn != "arn" || partition.is_empty() || service != ARN_NAMESPACE || account.is_empty() {
        return malformed();
    }
    // A repository name may itself carry `/` separators (`team/app`), so everything
    // after the resource type is the name.
    let Some(name) = resource.strip_prefix("repository/").filter(|n| !n.is_empty()) else {
        return malformed();
    };
    Ok(RepositoryArn {
        partition: partition.to_string(),
        registry_id: account.to_string(),
        name: name.to_string(),
    })
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

/// What content a stored manifest document references.
///
/// A registry manifest is one of two things: an image manifest, which lists the layer
/// blobs of one image, or a manifest list / OCI index, which lists other manifests by
/// digest. Anything else states the size of nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestContent {
    /// An image manifest: what each of its layer descriptors states the size of its blob
    /// to be, in manifest order — `None` for a descriptor that states no usable size.
    Layers(Vec<Option<i64>>),
    /// A manifest list or OCI index: the distinct digests of the manifests it
    /// references, in digest order.
    List(BTreeSet<String>),
    /// A document that references neither layers nor manifests.
    Opaque,
}

/// Read a PARSED manifest document for the content it references.
///
/// The push path has already parsed the document (it reads the media types from it), so
/// this takes the `Value` rather than the text: one request parses one manifest once.
///
/// Both forms carry OCI content descriptors — a `mediaType`, a `digest` and a `size` —
/// under `layers` for an image manifest and under `manifests` for a list, so this reads
/// those two arrays and nothing else. A document that carries neither array is
/// [`ManifestContent::Opaque`], and so is one that could not be parsed at all (the
/// caller has no `Value` to pass for it).
///
/// A descriptor's `size` is the byte count of the blob it points at, so a value that is
/// absent, not an integer, or below zero states no size at all — the document is
/// caller-supplied, and nothing it says may be read as a byte count it did not give.
///
/// A list's digests are collected as a SET, so naming one child a thousand times costs
/// what naming it once costs, and a document naming more than
/// [`MAX_MANIFEST_LIST_ENTRIES`] children is [`ManifestContent::Opaque`] rather than
/// read in part.
pub fn manifest_content_of(document: &serde_json::Value) -> ManifestContent {
    if let Some(manifests) = document.get("manifests").and_then(|m| m.as_array()) {
        if manifests.len() > MAX_MANIFEST_LIST_ENTRIES {
            return ManifestContent::Opaque;
        }
        return ManifestContent::List(
            manifests
                .iter()
                .filter_map(|descriptor| descriptor.get("digest"))
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect(),
        );
    }
    if let Some(layers) = document.get("layers").and_then(|l| l.as_array()) {
        return ManifestContent::Layers(
            layers
                .iter()
                .map(|descriptor| {
                    descriptor
                        .get("size")
                        .and_then(serde_json::Value::as_i64)
                        .filter(|size| *size >= 0)
                })
                .collect(),
        );
    }
    ManifestContent::Opaque
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
/// Marketplace": it is neither required nor defaulted, and nothing a caller of this API
/// can do makes a certification statement about a repository, so it stays unset and is
/// absent from every response — `false` would claim the service had decided the
/// repository is not certified.
pub fn catalog_data_of(
    repository_name: &str,
    input: Option<&RepositoryCatalogDataInput>,
) -> RepositoryCatalogData {
    let Some(input) = input else {
        return RepositoryCatalogData::default();
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
        marketplace_certified: None,
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
pub fn invalid_token_message() -> String {
    "Invalid parameter at 'nextToken' failed to satisfy constraint: 'Member must be a \
     token returned by a previous call'"
        .to_string()
}
