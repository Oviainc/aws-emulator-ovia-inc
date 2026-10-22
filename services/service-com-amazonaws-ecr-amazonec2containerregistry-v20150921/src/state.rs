// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecr-amazonec2containerregistry-v20150921 service.
//!
//! One registry (this emulator's account) holding repositories by name, each holding
//! its images by digest plus a tag index, plus the registry-wide settings a caller can
//! configure (today: its replication rules). Everything an operation answers with is
//! derived from here; nothing is re-synthesised from a request.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{
    EncryptionConfiguration, EncryptionConfigurationForRepositoryCreationTemplate, ImageDetail,
    ImageIdentifier, ImageScanningConfiguration, ImageTagMutabilityExclusionFilter,
    ReplicationConfiguration, ReplicationRule, Repository, RepositoryCreationTemplate,
    RepositoryScanningConfiguration, SigningConfiguration, Tag,
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

/// The tag-key namespace AWS keeps for itself: "the `aws:` prefix is reserved for AWS
/// use [...] you can't edit or delete a tag key or value with this prefix" (AWS General
/// Reference, "Tagging AWS resources"). A request that tries to create or remove a key
/// in it is refused rather than stored, so no caller-supplied tag can ever impersonate
/// one AWS set.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The region an unsigned request is served in: a request that carries no SigV4
/// credential scope names no region, and ECR's ARNs and repository URIs are regional.
pub const DEFAULT_REGION: &str = "us-east-1";

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

    /// Every blob this repository holds, by digest.
    ///
    /// A manifest cannot be pushed before every blob it references is in the
    /// repository — `PutImage` models `LayersNotFoundException` for exactly that, "the
    /// specified layers could not be found, or the specified layer is not valid for
    /// this repository" — so the blobs the stored manifests reference are the blobs the
    /// repository holds, and their descriptors are where their sizes and media types
    /// come from. Blobs are per-repository: a layer pushed to one repository is not a
    /// layer of another, which is why this is a method on the repository.
    pub fn layers(&self) -> BTreeMap<String, LayerBlob> {
        let mut layers: BTreeMap<String, LayerBlob> = BTreeMap::new();
        for image in self.images.values() {
            for blob in manifest_blobs(&image.manifest) {
                match layers.entry(blob.digest.clone()) {
                    Entry::Vacant(slot) => {
                        slot.insert(blob);
                    }
                    // One blob can be referenced by several manifests, and the formats
                    // differ in what they say about it (a V2 schema 1 `fsLayers` entry
                    // carries the digest alone), so each field is filled by whichever
                    // descriptor knows it.
                    Entry::Occupied(mut held) => {
                        let record = held.get_mut();
                        if record.size.is_none() {
                            record.size = blob.size;
                        }
                        if record.media_type.is_none() {
                            record.media_type = blob.media_type;
                        }
                    }
                }
            }
        }
        layers
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

/// Per-service state for com-amazonaws-ecr-amazonec2containerregistry-v20150921. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// Repositories by name. A `BTreeMap` so every listing — and therefore every
    /// pagination token — has one deterministic order.
    pub repositories: BTreeMap<String, Repo>,
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

/// The identity of a repository creation template's namespace prefix.
///
/// `Prefix`'s own documentation: "There is always an assumed / applied to the end of the
/// prefix. If you specify ecr-public as the prefix, Amazon ECR treats that as
/// ecr-public/." That assumed separator is a single one, so exactly one trailing slash
/// is not part of the namespace's identity: `prod` and `prod/` name the same template
/// and the stored record spells it one way.
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

/// The wording ECR uses when the registry carries no signing configuration — the state
/// `SigningConfigurationNotFoundException` documents: "attempting to retrieve or delete a
/// signing configuration that does not exist".
pub fn signing_configuration_not_found_message() -> String {
    format!(
        "The registry with id '{}' does not have an associated signing configuration",
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
/// `check_principal` is called with each statement's `Principal` element, so a service
/// that adds its own rule about who a policy may name (see
/// [`validate_registry_policy`]) does not have to parse the document a second time;
/// a service with no such rule passes `|_| Ok(())`.
///
/// Parsing is `serde_json`'s; only the policy grammar on top of it is ours.
pub fn validate_resource_policy(
    member: &str,
    text: &str,
    mut check_principal: impl FnMut(&serde_json::Value) -> Result<(), Fault>,
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
        let principal = entry.get("Principal").filter(|p| names_values(p)).ok_or_else(|| {
            bad_policy(
                member,
                "every statement of a resource-based policy must carry a Principal",
            )
        })?;
        check_principal(principal)?;
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

/// Every principal a statement's `Principal` element names, flattened out of the string,
/// the array or the `{"AWS": …}`-style object it may be written as.
fn principals(element: &serde_json::Value) -> Vec<&str> {
    match element {
        serde_json::Value::String(value) => vec![value.as_str()],
        serde_json::Value::Array(entries) => entries.iter().flat_map(principals).collect(),
        serde_json::Value::Object(members) => members.values().flat_map(principals).collect(),
        _ => Vec::new(),
    }
}

/// Validate a registry permissions policy document: the resource-policy grammar of
/// [`validate_resource_policy`], plus the one rule a REGISTRY policy adds — it grants to
/// other accounts, so every statement must name the identities it grants to and may not
/// open the registry to every principal with the `*` wildcard.
pub fn validate_registry_policy(member: &str, text: &str) -> Result<(), Fault> {
    validate_resource_policy(member, text, |principal| {
        if principals(principal).contains(&"*") {
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

/// Every blob an image manifest references, in the order the manifest lists them.
///
/// A manifest names its blobs as descriptors, and both container-image manifest
/// formats in use spell them the same way: the Docker image manifest V2 schema 2 and
/// the OCI image manifest carry `config` (the image configuration blob, which a
/// registry stores as a blob like any other) and `layers` (the filesystem layers), each
/// descriptor carrying `digest`, `size` and `mediaType`; the older V2 schema 1 lists
/// them as `fsLayers[].blobSum`, a digest and nothing else. A manifest list or OCI
/// image index references *manifests* rather than blobs, so its `manifests` entries are
/// deliberately not read here: they are other images, not this one's layers.
pub fn manifest_blobs(manifest: &str) -> Vec<LayerBlob> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return Vec::new();
    };
    let mut blobs = Vec::new();
    if let Some(blob) = document.get("config").and_then(blob_descriptor) {
        blobs.push(blob);
    }
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
