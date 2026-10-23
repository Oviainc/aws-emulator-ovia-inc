// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeartifact-codeartifactcontrolplaneservice service.
//!
//! CodeArtifact's primary resource pair is the domain and the repositories nested
//! inside it, so the store is a single sorted map of domains, each owning a sorted map
//! of its repositories. Sorted (`BTreeMap`) rather than hashed because every `List*`
//! operation on this service is `@paginated` over a name: a stable key order is what
//! makes an opaque `nextToken` resolve to the same place on the next call.
//!
//! The modelled response shapes are NOT stored. `DomainDescription`,
//! `DomainSummary`, `RepositoryDescription`, `RepositorySummary`, `PackageSummary` and
//! `PackageGroupDescription` are all projections of the records below
//! ([`Domain::description`] and friends), so a value that is derived —
//! `repositoryCount`, a package group's `parent` and each origin restriction's
//! `effectiveMode` above all — can never go stale against the records it summarises.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    AssetSummary, DomainDescription, DomainEntryPoint, DomainSummary, PackageGroupDescription,
    PackageGroupOriginConfiguration, PackageGroupOriginRestriction, PackageGroupReference,
    PackageOriginConfiguration, PackageOriginRestrictions, PackageSummary,
    PackageVersionDescription, PackageVersionOrigin, RepositoryDescription,
    RepositoryExternalConnectionInfo, RepositorySummary, ResourcePolicy, Tag,
    UpstreamRepositoryInfo, ValidationException, PACKAGE_GROUP_ORIGIN_RESTRICTION_TYPE_VALUES,
};

/// This service's SigV4 signing name, which is also the service segment of every ARN
/// it mints (`arn:aws:codeartifact:...`).
pub const SERVICE_SIGNING_NAME: &str = "codeartifact";

/// The region attributed to a request that carries no SigV4 credential scope to read a
/// region out of (an anonymous or unsigned call). Matches the region the emulator's own
/// generated client config uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The only status a domain this emulator holds can be in: CodeArtifact creates a
/// domain synchronously and `DescribeDomain` reports it `Active` from the first read, so
/// there is no transient creating state to move through. (`DomainStatus` models exactly
/// `Active` and `Deleted`; `Deleted` is never observable, because a deleted domain is
/// gone from the store.)
pub const DOMAIN_STATUS_ACTIVE: &str = "Active";

/// The only status an external connection this emulator holds can be in.
/// `ExternalConnectionStatus` models exactly one value, `Available`, and the
/// `RepositoryExternalConnectionInfo.status` doc says so ("There is one valid value,
/// `Available`"), so an associated connection is reported available from the first read.
pub const EXTERNAL_CONNECTION_STATUS_AVAILABLE: &str = "Available";

/// The public registries CodeArtifact can connect a repository to, each paired with the
/// `PackageFormat` its packages carry.
///
/// Source: the `externalConnection` member doc of `AssociateExternalConnectionRequest`
/// ("The following values are supported: public:npmjs - for the npm public repository
/// …"), read with
/// `smithy_query(query="doc",shape="AssociateExternalConnectionRequest",member="externalConnection")`;
/// the format of each is the registry's own package format from `PackageFormat`. The
/// member is modelled as a free `ExternalConnectionName` string rather than an enum, so
/// this catalog is what distinguishes a supported connection from an arbitrary one.
pub const EXTERNAL_CONNECTIONS: &[(&str, &str)] = &[
    ("public:npmjs", "npm"),
    ("public:nuget-org", "nuget"),
    ("public:pypi", "pypi"),
    ("public:maven-central", "maven"),
    ("public:maven-googleandroid", "maven"),
    ("public:maven-gradleplugins", "maven"),
    ("public:maven-commonsware", "maven"),
    ("public:maven-clojars", "maven"),
    ("public:ruby-gems-org", "ruby"),
    ("public:crates-io", "cargo"),
];

/// The `PackageFormat` of the packages a named external connection serves, or `None` when
/// the name is not a connection CodeArtifact supports — which the caller answers as an
/// invalid parameter value rather than storing a connection to a registry that does not
/// exist.
pub fn external_connection_package_format(name: &str) -> Option<&'static str> {
    EXTERNAL_CONNECTIONS
        .iter()
        .find(|(connection, _)| *connection == name)
        .map(|(_, format)| *format)
}

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller request and freed only by an explicit
// delete, so each growth site needs a declared ceiling: without one, a local caller can
// grow the shared in-process state without limit. These are CodeArtifact's own published
// default quotas (AWS CodeArtifact "Quotas" page: Domains per account 10, Repositories
// per domain 1,000, Upstream repositories per repository 10), so a caller that hits one
// gets the SAME answer the real service gives — `ServiceQuotaExceededException` (402),
// which both create operations already model.
// ---------------------------------------------------------------------------

/// Maximum domains one account holds ("Domains per account" quota).
pub const MAX_DOMAINS: usize = 10;

/// Maximum repositories one domain holds ("Repositories per domain" quota).
pub const MAX_REPOSITORIES_PER_DOMAIN: usize = 1_000;

/// Maximum upstream repositories one repository may list ("Upstream repositories per
/// repository" quota). `UpstreamRepositoryList` carries NO modelled length, so the
/// boundary validator cannot cap it and `CreateRepository` must.
pub const MAX_UPSTREAMS_PER_REPOSITORY: usize = 10;

/// The `associationType` of a package whose associated group's pattern names the
/// package's own path components (an "exact" match).
pub const ASSOCIATION_TYPE_STRONG: &str = "STRONG";

/// The `associationType` of a package that matched part of a component of its group's
/// pattern (a "relative" match) rather than whole components of it.
pub const ASSOCIATION_TYPE_WEAK: &str = "WEAK";

/// Maximum packages one repository holds.
///
/// Unlike the three bounds above this one is NOT a published CodeArtifact quota — AWS
/// publishes none for packages per repository — so it is a bound this emulator CHOOSES to
/// keep its in-process store finite: a package record is freed only by an explicit delete,
/// so without a ceiling a local caller could grow the shared state without limit. It is far
/// above anything a fixture creates, and a caller that reaches it is refused (the operations
/// that create a package model no quota error, so the refusal is their `ValidationException`
/// naming this limit) rather than served a truncated store.
pub const MAX_PACKAGES_PER_REPOSITORY: usize = 10_000;

/// Maximum external connections one repository may hold. `AssociateExternalConnection`'s
/// own documentation states the limit — "One external connection is allowed per
/// repository" — and it is why that operation models
/// `ServiceQuotaExceededException` (402).
pub const MAX_EXTERNAL_CONNECTIONS_PER_REPOSITORY: usize = 1;

/// Maximum bytes one published asset may carry.
///
/// NOT an AWS quota — CodeArtifact's published asset ceiling is measured in gigabytes,
/// which this in-memory store could not hold — but the emulator's OWN bound: asset bytes
/// live in the process until an explicit delete, so without a ceiling one local caller
/// can grow the shared store until the process dies. An asset over it is refused with the
/// `ServiceQuotaExceededException` (402) every publish path already models.
///
/// It bounds what is STORED, not the peak allocation of one request: the request body is
/// already buffered by the time a handler sees it, so the only bound on THAT is the
/// server's body cap.
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;

/// Maximum assets one package version may hold (the emulator's own bound, for the same
/// reason as [`MAX_ASSET_BYTES`]: an unfinished version accepts assets until it closes).
pub const MAX_ASSETS_PER_PACKAGE_VERSION: usize = 100;

/// Maximum versions one package may hold (the emulator's own bound).
pub const MAX_PACKAGE_VERSIONS_PER_PACKAGE: usize = 1_000;

/// Maximum total bytes of asset content this service retains, across every domain.
///
/// [`MAX_ASSET_BYTES`] bounds ONE asset; the record-count ceilings bound only their
/// product, which is astronomically larger than the host's memory. This is the bound that
/// actually holds the process: real CodeArtifact keeps assets in S3, this emulator keeps
/// them in RAM, so the bytes it retains need a ceiling of their own. Peer services that
/// retain caller bytes declare the same thing (S3's `MAX_OBJECT_BYTES_TOTAL`,
/// mediastoredata's `MAX_STORED_BYTES`); this sits in the same band and is far above
/// anything a fixture publishes. A publish that would cross it is refused with
/// `ServiceQuotaExceededException` (402) BEFORE the asset is stored.
///
/// An `i64` rather than a `usize` because it is compared against the derived total of
/// [`ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::stored_asset_bytes`],
/// which is an `i64` to match the modelled `assetSizeBytes`.
pub const MAX_STORED_ASSET_BYTES: i64 = 256 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Package-version vocabulary
//
// Each constant below is one value of a modelled enum (`PackageFormat`,
// `PackageVersionStatus`, `AllowPublish`/`AllowUpstream`, `PackageVersionOriginType`,
// `HashAlgorithm`), named here so the handlers compare against one spelling.
// ---------------------------------------------------------------------------

/// The only `PackageFormat` `PublishPackageVersion` accepts: "Only generic packages can
/// be published using this API" (the operation's own documentation).
pub const PACKAGE_FORMAT_GENERIC: &str = "generic";

/// `PackageVersionStatus` of a version whose assets are all uploaded.
pub const PACKAGE_VERSION_STATUS_PUBLISHED: &str = "Published";

/// `PackageVersionStatus` of a version that is still accepting assets, which is what the
/// `unfinished` flag keeps a publish in.
pub const PACKAGE_VERSION_STATUS_UNFINISHED: &str = "Unfinished";

/// The `AllowPublish`/`AllowUpstream` value that permits the action.
pub const ORIGIN_RESTRICTION_ALLOW: &str = "ALLOW";

/// The `AllowPublish`/`AllowUpstream` value that refuses the action.
pub const ORIGIN_RESTRICTION_BLOCK: &str = "BLOCK";

/// `PackageVersionOriginType` of a version published straight into a repository (as
/// opposed to one pulled in from an upstream or an external connection).
pub const PACKAGE_VERSION_ORIGIN_INTERNAL: &str = "INTERNAL";

/// Maximum package groups one domain holds, counting the root group every domain is
/// created with.
///
/// Unlike the ceilings above this one does NOT quote a published figure: AWS caps
/// package groups per domain, but the number is not cited here, so treat this as the
/// emulator's own declared bound on a growth site a caller controls. What matters for
/// fidelity is the REFUSAL — `ServiceQuotaExceededException` (402), which
/// `CreatePackageGroup` models — not the exact threshold.
pub const MAX_PACKAGE_GROUPS_PER_DOMAIN: usize = 100;

// ---------------------------------------------------------------------------
// Package groups
// ---------------------------------------------------------------------------

/// The pattern of the root package group. Every domain has it from the moment it is
/// created: it matches every package in the domain, it is the ancestor of every other
/// package group, and it is what the origin restrictions of a group that inherits
/// ultimately resolve against.
pub const ROOT_PACKAGE_GROUP_PATTERN: &str = "/*";

/// `PackageGroupOriginRestrictionMode$ALLOW`.
pub const ORIGIN_MODE_ALLOW: &str = "ALLOW";

/// `PackageGroupOriginRestrictionMode$ALLOW_SPECIFIC_REPOSITORIES` — the only mode that
/// carries an allowed-repository list, and therefore the only one with a
/// `repositoriesCount`.
pub const ORIGIN_MODE_ALLOW_SPECIFIC_REPOSITORIES: &str = "ALLOW_SPECIFIC_REPOSITORIES";

/// `PackageGroupOriginRestrictionMode$INHERIT` — the mode a newly created group carries
/// for every restriction type, which resolves to an ancestor's mode.
pub const ORIGIN_MODE_INHERIT: &str = "INHERIT";

/// Per-service state for com-amazonaws-codeartifact-codeartifactcontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState {
    /// Every domain this emulator holds, keyed by domain name.
    ///
    /// Keyed by NAME alone, not by region: this emulator serves one logical account and
    /// its state is not partitioned per region, so a domain created through a request
    /// signed for one region is visible to a request signed for another (and keeps the
    /// ARNs minted from the creating request's region). Real CodeArtifact namespaces
    /// domains per region; a later slice that needs that distinction should widen this
    /// key to `(region, name)` rather than add a second map.
    pub domains: BTreeMap<String, Domain>,
}

impl ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState {
    /// Resolve a domain the way every domain-scoped operation addresses it: by `name`,
    /// qualified by the optional `domainOwner` the caller may pin it to.
    ///
    /// `None` means "no such domain for this caller" — either the name is not held, or
    /// it is held by an account other than the one the caller named — and each operation
    /// answers that with its own modelled not-found error. Keeping the rule here means
    /// `domainOwner` cannot be honoured by one operation and ignored by the next.
    pub fn domain(&self, name: &str, owner: Option<&str>) -> Option<&Domain> {
        self.domains
            .get(name)
            .filter(|domain| owner.is_none_or(|owner| owner == domain.owner))
    }

    /// [`Self::domain`], for an operation that mutates the domain it resolves.
    pub fn domain_mut(&mut self, name: &str, owner: Option<&str>) -> Option<&mut Domain> {
        self.domains
            .get_mut(name)
            .filter(|domain| owner.is_none_or(|owner| owner == domain.owner))
    }

    /// The total asset bytes this service currently retains, across every domain — what
    /// [`MAX_STORED_ASSET_BYTES`] bounds.
    ///
    /// Summed from the assets themselves rather than kept as a running counter, for the
    /// same reason as every other derived value here: a counter is one more place that
    /// can forget to move when a version or a repository is deleted. The sum costs the
    /// NUMBER of stored assets (bounded by the record ceilings above), never their bytes.
    pub fn stored_asset_bytes(&self) -> i64 {
        self.domains.values().fold(0i64, |total, domain| {
            total.saturating_add(domain.asset_size_bytes())
        })
    }
}

// ---------------------------------------------------------------------------
// Resource tagging
//
// Everything the ARN-addressed tag surface needs, in one place: the bound, the records'
// one tag list, the ARN resolver and the keyed merge. `TagResource` and
// `ListTagsForResource` reach the SAME list through the accessors below, so a writer and
// a reader cannot hold their own copies of a resource's tags.
// ---------------------------------------------------------------------------

/// Maximum tags one resource holds, from CodeArtifact's own published tag restrictions
/// ("Tagging resources in CodeArtifact": "Maximum number of tags per resource – 50";
/// the same block's "Maximum key length – 128" and "Maximum value length – 256" are
/// what `TagKey` and `TagValue` model).
///
/// This is an ACCUMULATED bound on the resource, not on one request: `TagList` is
/// modelled `@length(0..200)`, so the boundary validator refuses an oversized single
/// batch, and nothing but this constant stops a sequence of small batches from growing
/// one resource's tag set without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag accessors, a second `impl` block beside the store's own so this whole surface
/// reads in one place.
impl ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState {
    /// The tags of the resource an ARN named, resolved through [`ResourceRef`].
    ///
    /// `None` means this store holds no such resource — the same rule as
    /// [`Self::domain`], extended through the domain to its repositories and package
    /// groups — which each ARN-addressed operation answers with its own modelled
    /// not-found error.
    pub fn resource_tags(&self, reference: &ResourceRef) -> Option<&[Tag]> {
        match reference {
            ResourceRef::Domain { owner, name } => {
                Some(self.domain(name, owner.as_deref())?.tags.as_slice())
            }
            ResourceRef::Repository {
                owner,
                domain,
                repository,
            } => Some(
                self.domain(domain, owner.as_deref())?
                    .repositories
                    .get(repository)?
                    .tags
                    .as_slice(),
            ),
            ResourceRef::PackageGroup {
                owner,
                domain,
                pattern,
            } => Some(
                self.domain(domain, owner.as_deref())?
                    .package_groups
                    .get(pattern)?
                    .tags
                    .as_slice(),
            ),
        }
    }

    /// [`Self::resource_tags`], for an operation that rewrites the tag set it resolves.
    pub fn resource_tags_mut(&mut self, reference: &ResourceRef) -> Option<&mut Vec<Tag>> {
        match reference {
            ResourceRef::Domain { owner, name } => {
                Some(&mut self.domain_mut(name, owner.as_deref())?.tags)
            }
            ResourceRef::Repository {
                owner,
                domain,
                repository,
            } => Some(
                &mut self
                    .domain_mut(domain, owner.as_deref())?
                    .repositories
                    .get_mut(repository)?
                    .tags,
            ),
            ResourceRef::PackageGroup {
                owner,
                domain,
                pattern,
            } => Some(
                &mut self
                    .domain_mut(domain, owner.as_deref())?
                    .package_groups
                    .get_mut(pattern)?
                    .tags,
            ),
        }
    }
}

/// A resource of THIS service that a caller addressed by ARN, as the identifiers the
/// store is keyed by.
///
/// Only the kinds this store actually holds are represented — the three that carry a tag
/// set. A CodeArtifact ARN naming anything else (a package, a package version, an asset)
/// resolves to no record here, which [`taggable_resource`] reports by answering `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// `arn:<partition>:codeartifact:<region>:<owner>:domain/<name>`.
    Domain {
        /// The account the ARN pins the resource to, or `None` when the ARN's account
        /// field is empty — the same optional qualifier the `domainOwner` member is.
        owner: Option<String>,
        name: String,
    },
    /// `arn:<partition>:codeartifact:<region>:<owner>:repository/<domain>/<name>`.
    Repository {
        owner: Option<String>,
        domain: String,
        repository: String,
    },
    /// `arn:<partition>:codeartifact:<region>:<owner>:package-group/<domain><pattern>`,
    /// the form [`package_group_arn`] mints: the pattern is a path from the domain root,
    /// so it carries its own `/` separators and its leading `/` is the one between the
    /// domain segment and the path.
    PackageGroup {
        owner: Option<String>,
        domain: String,
        /// The stored pattern, leading `/` included — the key `Domain::package_groups`
        /// holds a group under.
        pattern: String,
    },
}

/// The resource a PARSED ARN of this service's namespace names, read from its ACCOUNT
/// and RESOURCE fields.
///
/// Takes the `Arn` that `service_common::arn::parse_of(value, SERVICE_SIGNING_NAME)`
/// yields — the one parse of a caller-supplied ARN — so a value that is not a
/// well-formed ARN of this namespace never reaches here: that is a bad REQUEST, a
/// different answer from a resource that is not held. `None` here means the ARN is ours
/// and well formed, but its resource field names no kind this store holds.
///
/// The REGION field is deliberately not read: this store is not partitioned per region
/// (see [`ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::domains`]), so
/// an ARN resolves to the one record of that name whatever region minted it.
#[must_use]
pub fn taggable_resource(arn: &::service_common::arn::Arn<'_>) -> Option<ResourceRef> {
    // An empty account field pins the ARN to no account, so it qualifies nothing.
    let owner = (!arn.account.is_empty()).then(|| arn.account.to_string());

    // The shape of the RESOURCE field is this service's own: `<type>/<id…>`, where a
    // package group's id is a path carrying further `/` separators.
    let mut segments = arn.resource.splitn(3, '/');
    match (segments.next()?, segments.next(), segments.next()) {
        ("domain", Some(name), None) if !name.is_empty() => Some(ResourceRef::Domain {
            owner,
            name: name.to_string(),
        }),
        ("repository", Some(domain), Some(repository))
            if !domain.is_empty() && !repository.is_empty() && !repository.contains('/') =>
        {
            Some(ResourceRef::Repository {
                owner,
                domain: domain.to_string(),
                repository: repository.to_string(),
            })
        }
        // A package group's identifier is a PATH, so everything after the domain segment
        // belongs to it, `/` separators included — the leading one is what makes `/*` the
        // ancestor of `/npm/*`.
        ("package-group", Some(domain), Some(path)) if !domain.is_empty() && !path.is_empty() => {
            Some(ResourceRef::PackageGroup {
                owner,
                domain: domain.to_string(),
                pattern: format!("/{path}"),
            })
        }
        _ => None,
    }
}

/// Why a caller-supplied `resourceArn` names no resource of this service.
///
/// Both tag operations model the same two refusals for the same two reasons, so the
/// reason is decided ONCE (in [`resolve_taggable`]) and each handler lifts it into its
/// own modelled error enum — the two cannot drift into disagreeing about which ARN is a
/// bad request and which is a missing resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagArnRefusal {
    /// Not a well-formed ARN of this service's namespace: it addresses nothing this API
    /// can reach, so it is a bad REQUEST (`ValidationException`, HTTP 400).
    NotThisServicesArn,
    /// A well-formed ARN of this namespace whose resource field names no kind this store
    /// holds (`ResourceNotFoundException`, HTTP 404).
    NoSuchResource,
}

/// The resource a caller-supplied `resourceArn` names, or why it names none.
///
/// The ONE place a tag operation parses an ARN: [`service_common::arn::parse_of`] decides
/// whether the value is this service's ARN at all, and [`taggable_resource`] which record
/// kind it addresses.
pub fn resolve_taggable(resource_arn: &str) -> Result<ResourceRef, TagArnRefusal> {
    let parsed = ::service_common::arn::parse_of(resource_arn, SERVICE_SIGNING_NAME)
        .ok_or(TagArnRefusal::NotThisServicesArn)?;
    taggable_resource(&parsed).ok_or(TagArnRefusal::NoSuchResource)
}

/// The `repository` value of the modelled `ResourceType` enum — the counterpart of
/// [`DOMAIN_RESOURCE_TYPE`]. (`ResourceType` models exactly `asset`, `domain`, `package`,
/// `package-version` and `repository`; see `RESOURCE_TYPE_VALUES` in `ops::types`.)
pub const REPOSITORY_RESOURCE_TYPE: &str = "repository";

/// The `resourceType` a modelled error reports for a resolved resource.
///
/// `None` for a package group: `ResourceType` declares no value for one, so the member is
/// left unset rather than filled with a spelling the model does not define.
#[must_use]
pub fn reported_resource_type(reference: &ResourceRef) -> Option<String> {
    match reference {
        ResourceRef::Domain { .. } => Some(DOMAIN_RESOURCE_TYPE.to_string()),
        ResourceRef::Repository { .. } => Some(REPOSITORY_RESOURCE_TYPE.to_string()),
        ResourceRef::PackageGroup { .. } => None,
    }
}

/// `held` with `incoming` applied, the way CodeArtifact's `TagResource` documents it
/// ("Adds or updates tags for a resource"): a key the resource already holds keeps its
/// position and takes the new value, and a key it does not hold is appended.
///
/// Tag KEYS are the identity of a tag, so the result holds one entry per distinct key —
/// including when one request repeats a key, where the last value in the request wins.
/// Values are copied verbatim: a tag value is an opaque Unicode string, never
/// normalised, trimmed or re-encoded.
#[must_use]
pub fn merged_tags(held: &[Tag], incoming: &[Tag]) -> Vec<Tag> {
    let mut merged = held.to_vec();
    for tag in incoming {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(existing) => existing.value = tag.value.clone(),
            None => merged.push(tag.clone()),
        }
    }
    merged
}

/// A stored CodeArtifact domain.
#[derive(Debug, Clone)]
pub struct Domain {
    pub name: String,
    /// The 12-digit account that owns the domain.
    pub owner: String,
    pub arn: String,
    pub status: String,
    pub created_time: SystemTime,
    pub encryption_key: Option<String>,
    pub s3_bucket_arn: String,
    pub tags: Vec<Tag>,
    pub policy: Option<ResourcePolicy>,
    /// How many resource policies this domain has ever had attached, counting the one
    /// attached now. Never reset — not by replacing the policy and not by deleting it —
    /// because it is what the revision in [`attached_policy`] is derived from: a revision
    /// a caller held before a delete must not start matching again when the same document
    /// is re-attached.
    pub policy_attachments: u64,
    /// The domain's repositories, keyed by repository name. `repositoryCount` is
    /// derived from this map's length and never stored beside it.
    pub repositories: BTreeMap<String, Repository>,
    /// The domain's package groups, keyed by their pattern — which the model calls out
    /// as the group's identifier (`CreatePackageGroupRequest$packageGroup`: "The pattern
    /// is also the identifier of the package group").
    ///
    /// A domain always holds at least [`ROOT_PACKAGE_GROUP_PATTERN`], inserted when the
    /// domain is created. The parent/child hierarchy is NOT stored here: it is derived
    /// from the patterns themselves on every read ([`Domain::package_group_parent`]), so
    /// inserting an intermediate group reparents the groups below it with no rewrite.
    pub package_groups: BTreeMap<String, PackageGroup>,
}

impl Domain {
    /// Attach `document` as this domain's resource policy, replacing any policy it already
    /// holds, and answer with the record that was stored.
    ///
    /// The counter advances first, so the minted revision is one this domain has never
    /// reported — see [`attached_policy`].
    pub fn attach_policy(&mut self, document: String) -> ResourcePolicy {
        self.policy_attachments = self.policy_attachments.saturating_add(1);
        let policy = attached_policy(&self.arn, document, self.policy_attachments);
        self.policy = Some(policy.clone());
        policy
    }

    /// The modelled `DomainDescription` for this record.
    pub fn description(&self) -> DomainDescription {
        DomainDescription {
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            created_time: Some(self.created_time.into()),
            encryption_key: self.encryption_key.clone(),
            repository_count: self.repository_count(),
            asset_size_bytes: self.asset_size_bytes(),
            s3_bucket_arn: Some(self.s3_bucket_arn.clone()),
        }
    }

    /// The modelled `DomainSummary` for this record — the subset of
    /// [`Domain::description`] that `ListDomains` returns.
    pub fn summary(&self) -> DomainSummary {
        DomainSummary {
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            created_time: Some(self.created_time.into()),
            encryption_key: self.encryption_key.clone(),
        }
    }

    /// `repositoryCount`, derived from the repositories actually held.
    ///
    /// Saturating rather than wrapping: the modelled member is an `i32`, and a count
    /// that overflowed it would otherwise report a negative number of repositories.
    fn repository_count(&self) -> i32 {
        i32::try_from(self.repositories.len()).unwrap_or(i32::MAX)
    }

    /// The package group most closely associated with the package `path`
    /// ([`package_path`]): of the groups whose pattern matches that path, the MOST
    /// SPECIFIC one — the one matching on the longest literal, with an exact pattern
    /// beating a relative pattern that matches the same literal.
    ///
    /// This is the ASSOCIATION question (`GetAssociatedPackageGroup`), which compares a
    /// pattern against a package path character by character, so a pattern naming part
    /// of a component still matches; [`Domain::package_group_parent`] answers the
    /// HIERARCHY question between two patterns, which compares whole components.
    ///
    /// Never `None` for a domain that holds its root group, whose pattern matches every
    /// package.
    pub fn associated_package_group(&self, path: &str) -> Option<&PackageGroup> {
        self.package_groups
            .values()
            .filter(|group| pattern_matches(&group.pattern, path))
            .max_by_key(|group| pattern_specificity(&group.pattern))
    }

    /// `assetSizeBytes`: the total size of every asset the domain's repositories hold.
    ///
    /// Derived, never stored, for the same reason as [`Domain::repository_count`]: a
    /// publish or a delete changes it, and a stored copy would be the one place that
    /// forgot to move. Saturating, so a total past `i64::MAX` reports the ceiling rather
    /// than wrapping negative.
    pub fn asset_size_bytes(&self) -> i64 {
        self.repositories.values().fold(0i64, |total, repository| {
            total.saturating_add(repository.asset_size_bytes())
        })
    }

    /// The modelled `PackageGroupDescription` of the group this domain holds under
    /// `pattern`, or `None` when it holds no such group.
    ///
    /// Everything derived — the parent reference and each restriction's effective mode —
    /// is computed from the groups the domain holds AT THIS CALL, so a description can
    /// never disagree with the hierarchy it is part of.
    pub fn package_group_description(&self, pattern: &str) -> Option<PackageGroupDescription> {
        let group = self.package_groups.get(pattern)?;
        Some(PackageGroupDescription {
            arn: Some(group.arn.clone()),
            pattern: Some(group.pattern.clone()),
            domain_name: Some(group.domain_name.clone()),
            domain_owner: Some(group.domain_owner.clone()),
            created_time: Some(group.created_time.into()),
            contact_info: group.contact_info.clone(),
            description: group.description.clone(),
            origin_configuration: Some(PackageGroupOriginConfiguration {
                restrictions: Some(self.origin_restrictions(group)),
            }),
            parent: self.package_group_parent(pattern).map(PackageGroup::reference),
        })
    }

    /// The DIRECT parent of `pattern`: the most specific group this domain holds whose
    /// pattern covers a strict prefix of the path `pattern` covers.
    ///
    /// `None` only for the root group, which covers the whole domain and so has nothing
    /// above it.
    pub fn package_group_parent(&self, pattern: &str) -> Option<&PackageGroup> {
        let path = covered_path(pattern);
        self.package_groups
            .values()
            .filter(|candidate| covers_strictly(&candidate.pattern, &path))
            .max_by_key(|candidate| covered_path(&candidate.pattern).len())
    }

    /// One modelled restriction per `PackageGroupOriginRestrictionType`, which is the
    /// whole `originConfiguration` of a group: the type list is the model's own enum, so
    /// a type added to the model shows up here without another edit.
    fn origin_restrictions(
        &self,
        group: &PackageGroup,
    ) -> HashMap<String, PackageGroupOriginRestriction> {
        PACKAGE_GROUP_ORIGIN_RESTRICTION_TYPE_VALUES
            .iter()
            .map(|restriction_type| {
                (
                    (*restriction_type).to_string(),
                    self.origin_restriction(group, restriction_type),
                )
            })
            .collect()
    }

    /// One origin restriction of `group`, resolved.
    ///
    /// `mode` is what the group itself carries. `effectiveMode` is the same value unless
    /// the group inherits, in which case it is "the value of `mode` of the first parent
    /// group which does not have a value of `INHERIT`"
    /// (`PackageGroupOriginRestriction$effectiveMode`), and `inheritedFrom` names that
    /// group.
    fn origin_restriction(
        &self,
        group: &PackageGroup,
        restriction_type: &str,
    ) -> PackageGroupOriginRestriction {
        // Walk up while the mode inherits. Each step moves to a group covering a
        // STRICTLY shorter path, so the walk cannot cycle and ends at the root.
        //
        // The walk breaks at the root, whose modes never inherit as long as the root is
        // created by `PackageGroup::root` — so `effective_mode` below is a resolved
        // mode. An `UpdatePackageGroupOriginConfiguration` that ever let the ROOT group
        // carry `INHERIT` would make this report `INHERIT` as the effective mode, and
        // must refuse that instead: the root has nothing to inherit from.
        let mut source = group;
        let mut inherited_from = None;
        while source.restriction_mode(restriction_type) == ORIGIN_MODE_INHERIT {
            match self.package_group_parent(&source.pattern) {
                Some(parent) => {
                    source = parent;
                    inherited_from = Some(parent.reference());
                }
                None => break,
            }
        }

        let effective_mode = source.restriction_mode(restriction_type).to_string();
        PackageGroupOriginRestriction {
            mode: Some(group.restriction_mode(restriction_type).to_string()),
            effective_mode: Some(effective_mode.clone()),
            inherited_from,
            // "The number of repositories in the allowed repository list"
            // (`PackageGroupOriginRestriction$repositoriesCount`) — a list only an
            // ALLOW_SPECIFIC_REPOSITORIES restriction has, counted on the group the
            // effective mode came from.
            repositories_count: (effective_mode == ORIGIN_MODE_ALLOW_SPECIFIC_REPOSITORIES).then(
                || {
                    i64::try_from(source.allowed_repositories(restriction_type).len())
                        .unwrap_or(i64::MAX)
                },
            ),
        }
    }
}

/// A stored package group.
///
/// The group's place in the hierarchy is not a field: it follows from [`Self::pattern`]
/// and the other patterns the domain holds, so it is derived where it is reported.
#[derive(Debug, Clone)]
pub struct PackageGroup {
    /// The pattern, which is also the group's identifier within the domain.
    pub pattern: String,
    pub domain_name: String,
    pub domain_owner: String,
    pub arn: String,
    pub created_time: SystemTime,
    pub contact_info: Option<String>,
    pub description: Option<String>,
    /// The group's own origin restriction per `PackageGroupOriginRestrictionType`. A
    /// type missing from the map reads as [`ORIGIN_MODE_INHERIT`].
    pub restrictions: BTreeMap<String, OriginRestriction>,
    pub tags: Vec<Tag>,
}

impl PackageGroup {
    /// The root group of a domain: it has no parent, so none of its restrictions can
    /// inherit — they are the domain's effective defaults, and CodeArtifact's root group
    /// allows publishing and both upstream kinds.
    pub fn root(region: &str, domain: &str, owner: &str, created_time: SystemTime) -> Self {
        Self {
            pattern: ROOT_PACKAGE_GROUP_PATTERN.to_string(),
            domain_name: domain.to_string(),
            domain_owner: owner.to_string(),
            arn: package_group_arn(region, domain, ROOT_PACKAGE_GROUP_PATTERN),
            created_time,
            contact_info: None,
            description: None,
            restrictions: origin_restrictions_in_mode(ORIGIN_MODE_ALLOW),
            tags: Vec::new(),
        }
    }

    /// How this group is referred to from another group's description — as a `parent`,
    /// or as the `inheritedFrom` of a restriction.
    pub fn reference(&self) -> PackageGroupReference {
        PackageGroupReference {
            arn: Some(self.arn.clone()),
            pattern: Some(self.pattern.clone()),
        }
    }

    /// This group's OWN mode for `restriction_type` — not the effective one.
    pub fn restriction_mode(&self, restriction_type: &str) -> &str {
        self.restrictions
            .get(restriction_type)
            .map_or(ORIGIN_MODE_INHERIT, |restriction| {
                restriction.mode.as_str()
            })
    }

    /// The repositories this group's `restriction_type` allows, which is meaningful only
    /// in [`ORIGIN_MODE_ALLOW_SPECIFIC_REPOSITORIES`].
    pub fn allowed_repositories(&self, restriction_type: &str) -> &BTreeSet<String> {
        /// Borrowed when the group carries no restriction of that type at all, so the
        /// caller never has to handle a missing list.
        static NONE: BTreeSet<String> = BTreeSet::new();

        self.restrictions
            .get(restriction_type)
            .map_or(&NONE, |restriction| &restriction.allowed_repositories)
    }
}

/// One stored origin restriction of a package group.
#[derive(Debug, Clone)]
pub struct OriginRestriction {
    /// A `PackageGroupOriginRestrictionMode` value.
    pub mode: String,
    /// The repositories named by `ALLOW_SPECIFIC_REPOSITORIES`, which
    /// `UpdatePackageGroupOriginConfiguration` adds to and removes from. A set, because
    /// a repository is either allowed or not and the allowed list is reported as a
    /// count.
    pub allowed_repositories: BTreeSet<String>,
}

/// Every `PackageGroupOriginRestrictionType` in `mode`, with no allowed repositories:
/// the restriction map a group is created with — [`ORIGIN_MODE_ALLOW`] for the root
/// group, [`ORIGIN_MODE_INHERIT`] for every group created under it.
pub fn origin_restrictions_in_mode(mode: &str) -> BTreeMap<String, OriginRestriction> {
    PACKAGE_GROUP_ORIGIN_RESTRICTION_TYPE_VALUES
        .iter()
        .map(|restriction_type| {
            (
                (*restriction_type).to_string(),
                OriginRestriction {
                    mode: mode.to_string(),
                    allowed_repositories: BTreeSet::new(),
                },
            )
        })
        .collect()
}

/// The package groups a freshly created domain holds: just the root group.
pub fn initial_package_groups(
    region: &str,
    domain: &str,
    owner: &str,
    created_time: SystemTime,
) -> BTreeMap<String, PackageGroup> {
    BTreeMap::from([(
        ROOT_PACKAGE_GROUP_PATTERN.to_string(),
        PackageGroup::root(region, domain, owner, created_time),
    )])
}

/// The path a package group pattern covers, as segments, with the pattern's terminator
/// removed: `/*` covers nothing (the whole domain), `/npm/*` covers `npm`, and
/// `/npm/scope/pkg$` covers `npm`, `scope`, `pkg`.
///
/// A pattern is a `/`-separated path whose final element either is the wildcard `*` (the
/// group covers everything below the path before it) or names one package, optionally
/// marked exact with a trailing `$`. Only that terminator is interpreted; the segments
/// themselves are compared literally, which is all the hierarchy needs.
fn covered_path(pattern: &str) -> Vec<&str> {
    let mut segments: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    if segments.last().is_some_and(|last| *last == "*") {
        segments.pop();
    } else if let Some(last) = segments.last_mut() {
        let named = *last;
        *last = named.strip_suffix('$').unwrap_or(named);
    }
    segments
}

/// Whether `pattern` covers a STRICT prefix of `path` — the relation that makes the
/// group with that pattern an ANCESTOR of the group covering `path`.
///
/// Only a wildcard pattern covers anything below the path it names, so a pattern that
/// names a single package is nobody's ancestor.
fn covers_strictly(pattern: &str, path: &[&str]) -> bool {
    if !pattern.ends_with('*') {
        return false;
    }
    let ancestor = covered_path(pattern);
    ancestor.len() < path.len() && path.starts_with(&ancestor)
}

/// Split a package group pattern into the literal it matches a PACKAGE PATH on, and
/// whether that match is EXACT.
///
/// This is the association reading of a pattern (`GetAssociatedPackageGroup`), beside
/// [`covered_path`]'s hierarchy reading of the same string: a package path is matched
/// character by character, so a pattern naming part of a component still matches it.
///
/// A pattern ends in the terminator that decides how it matches (see "Package group
/// definition syntax and matching behavior" in the CodeArtifact User Guide): `$` matches
/// that one package path and nothing under it — an exact match — while `*` and `~` match
/// every package path beginning with the literal before them. A pattern carrying no
/// terminator names a single path, so it is read as exact.
fn pattern_body(pattern: &str) -> (&str, bool) {
    if let Some(body) = pattern.strip_suffix('$') {
        (body, true)
    } else if let Some(body) = pattern
        .strip_suffix('*')
        .or_else(|| pattern.strip_suffix('~'))
    {
        (body, false)
    } else {
        (pattern, true)
    }
}

/// Does `pattern` match the package `path` ([`package_path`])?
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (body, exact) = pattern_body(pattern);
    if exact {
        path == body
    } else {
        path.starts_with(body)
    }
}

/// Is `pattern`'s match of `path` an EXACT match — the one reported as
/// [`ASSOCIATION_TYPE_STRONG`] — rather than a RELATIVE one
/// ([`ASSOCIATION_TYPE_WEAK`])?
///
/// A package path is a sequence of `/`-separated components (the format, the optional
/// namespace, the name). The match is exact when the pattern's literal covers whole
/// components of the path — it is the entire path, or it stops where a component ends —
/// because the group then names the package's own format, namespace and name. It is
/// relative when the literal stops INSIDE a component, which is a prefix match on a
/// namespace or a name (what a `~`-terminated pattern does for a sub-namespace): the
/// group names something the package is only relative to.
pub fn match_is_exact(pattern: &str, path: &str) -> bool {
    let (body, exact) = pattern_body(pattern);
    if exact {
        return path == body;
    }
    match path.strip_prefix(body) {
        Some(rest) => rest.is_empty() || rest.starts_with('/') || body.ends_with('/'),
        None => false,
    }
}

/// How specific `pattern` is, for picking the most closely associated group: the length
/// of the literal it matches on, and — between two patterns matching the same literal —
/// the exact one, which matches strictly fewer packages.
///
/// Two patterns can still tie: `/npm/*` and `/npm/~` match the same literal the same
/// way, so they are EQUIVALENT as matchers and which one is reported is decided by the
/// group map's key order, not by a rule of the service. Nothing below depends on which
/// of two equivalent patterns wins; a caller that wants one to win creates the more
/// specific pattern.
fn pattern_specificity(pattern: &str) -> (usize, bool) {
    let (body, exact) = pattern_body(pattern);
    (body.len(), exact)
}

/// The path a package is matched against a package group pattern on:
/// `/<format>/<namespace>/<name>`, with the namespace segment omitted for a package that
/// has none (the formats whose versions carry no namespace component).
pub fn package_path(format: &str, namespace: Option<&str>, package: &str) -> String {
    match namespace {
        Some(namespace) => format!("/{format}/{namespace}/{package}"),
        None => format!("/{format}/{package}"),
    }
}

/// The package formats whose packages are addressed WITH a namespace, so an operation
/// that resolves a package by format must be given one: a Maven package's namespace is
/// its `groupId`, a Swift package's is its scope, and a generic package's is its
/// namespace (`GetAssociatedPackageGroup`'s own `namespace` doc). The remaining formats'
/// versions carry no corresponding component.
pub const NAMESPACE_REQUIRED_FORMATS: &[&str] = &["generic", "maven", "swift"];

/// A stored repository inside a domain.
///
/// Later slices hang the package-VERSION maps off [`Package`]; the fields here are the
/// ones the repository lifecycle and the package listing need.
#[derive(Debug, Clone)]
pub struct Repository {
    pub name: String,
    pub domain_name: String,
    pub domain_owner: String,
    pub administrator_account: String,
    pub arn: String,
    pub description: Option<String>,
    /// Upstream repositories in the caller's priority order, which CodeArtifact
    /// preserves — so this is an ordered `Vec`, not a set.
    pub upstreams: Vec<UpstreamRepositoryInfo>,
    pub external_connections: Vec<RepositoryExternalConnectionInfo>,
    pub created_time: SystemTime,
    pub tags: Vec<Tag>,
    pub policy: Option<ResourcePolicy>,
    /// How many resource policies this repository has ever had attached, counting the one
    /// attached now — see [`Domain::policy_attachments`] for why it is never reset.
    pub policy_attachments: u64,
    /// The packages held in this repository, keyed by [`package_key`]. Sorted for the
    /// same reason as the maps above: `ListPackages` is `@paginated`, and a stable key
    /// order is what makes its opaque `nextToken` resolve to the same place next call.
    pub packages: BTreeMap<String, Package>,
}

impl Repository {
    /// Attach `document` as this repository's resource policy, replacing any policy it
    /// already holds, and answer with the record that was stored.
    ///
    /// The counter advances first, so the minted revision is one this repository has
    /// never reported. Both policy-bearing records attach a policy through this one rule
    /// ([`Domain::attach_policy`]), so a revision minted on a domain and one minted on a
    /// repository cannot follow different schemes.
    pub fn attach_policy(&mut self, document: String) -> ResourcePolicy {
        self.policy_attachments = self.policy_attachments.saturating_add(1);
        let policy = attached_policy(&self.arn, document, self.policy_attachments);
        self.policy = Some(policy.clone());
        policy
    }
    /// The modelled `RepositoryDescription` for this record.
    pub fn description(&self) -> RepositoryDescription {
        RepositoryDescription {
            name: Some(self.name.clone()),
            administrator_account: Some(self.administrator_account.clone()),
            domain_name: Some(self.domain_name.clone()),
            domain_owner: Some(self.domain_owner.clone()),
            arn: Some(self.arn.clone()),
            description: self.description.clone(),
            upstreams: Some(self.upstreams.clone()),
            external_connections: Some(self.external_connections.clone()),
            created_time: Some(self.created_time.into()),
        }
    }

    /// The modelled `RepositorySummary` for this record — the subset of
    /// [`Repository::description`] that the repository `List*` operations return.
    pub fn summary(&self) -> RepositorySummary {
        RepositorySummary {
            name: Some(self.name.clone()),
            administrator_account: Some(self.administrator_account.clone()),
            domain_name: Some(self.domain_name.clone()),
            domain_owner: Some(self.domain_owner.clone()),
            arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_time: Some(self.created_time.into()),
        }
    }

    /// Resolve a package the way every package-scoped operation addresses it: by the
    /// `(format, namespace, package)` triple, never by name alone.
    pub fn package(&self, format: &str, namespace: Option<&str>, package: &str) -> Option<&Package> {
        self.packages.get(&package_key(format, namespace, package))
    }

    /// The total size of every asset this repository holds, summed from the assets
    /// themselves (see [`Domain::asset_size_bytes`] on why it is derived).
    pub fn asset_size_bytes(&self) -> i64 {
        self.packages.values().fold(0i64, |total, package| {
            total.saturating_add(package.asset_size_bytes())
        })
    }
}

/// The key a package is held under inside its repository.
///
/// A package's identity is the `(format, namespace, package)` triple — the same name in
/// two formats, or under two namespaces, is two different packages — so the key carries
/// all three. `/` is the separator because no component can contain one: `PackageFormat`
/// is an enum of lowercase words, and both `PackageNamespace` and `PackageName` are
/// modelled `^[^#/\s]+$`. A package with no namespace keys as `<format>//<package>`,
/// which no namespaced package can collide with for that same reason.
pub fn package_key(format: &str, namespace: Option<&str>, package: &str) -> String {
    format!("{format}/{}/{package}", namespace.unwrap_or_default())
}

/// A stored package inside a repository.
///
/// A package is identified by the triple (`format`, `namespace`, `name`) — the same triple
/// every package-scoped operation addresses it with — and carries the origin restrictions
/// that decide whether new versions may be published into, or ingested from an upstream
/// into, this repository, plus the versions published into it. A package can legitimately
/// hold no versions at all: `PutPackageOriginConfiguration` is documented to create
/// exactly that when called for a package the repository does not yet hold.
#[derive(Debug, Clone)]
pub struct Package {
    pub format: String,
    /// `None` for a format whose versions carry no namespace component (the model's
    /// `PackageSummary$namespace` doc lists Python, NuGet, Ruby and Cargo).
    pub namespace: Option<String>,
    pub name: String,
    /// `publish`/`upstream` origin control, as `PutPackageOriginConfiguration` sets it.
    pub origin_restrictions: PackageOriginRestrictions,
    /// The package's versions, keyed by version string.
    pub versions: BTreeMap<String, PackageVersion>,
}

impl Package {
    /// A package created by a direct publish: CodeArtifact gives a package that was
    /// published into a repository the default origin configuration, which allows both
    /// further publishes and upstream fetches until an explicit
    /// `PutPackageOriginConfiguration` narrows it.
    pub fn new(format: &str, namespace: Option<&str>, name: &str) -> Self {
        Self {
            format: format.to_string(),
            namespace: namespace.map(str::to_string),
            name: name.to_string(),
            origin_restrictions: PackageOriginRestrictions {
                publish: ORIGIN_RESTRICTION_ALLOW.to_string(),
                upstream: ORIGIN_RESTRICTION_ALLOW.to_string(),
            },
            versions: BTreeMap::new(),
        }
    }

    /// The modelled `PackageSummary` for this record — what the package `List*`
    /// operations return.
    pub fn summary(&self) -> PackageSummary {
        PackageSummary {
            format: Some(self.format.clone()),
            namespace: self.namespace.clone(),
            package: Some(self.name.clone()),
            origin_configuration: Some(self.origin_configuration()),
        }
    }

    /// The modelled `PackageOriginConfiguration` for this record.
    pub fn origin_configuration(&self) -> PackageOriginConfiguration {
        PackageOriginConfiguration {
            restrictions: Some(self.origin_restrictions.clone()),
        }
    }

    /// Whether origin control permits publishing a version into this package.
    pub fn publish_allowed(&self) -> bool {
        self.origin_restrictions.publish != ORIGIN_RESTRICTION_BLOCK
    }

    /// The total size of every asset this package's versions hold.
    pub fn asset_size_bytes(&self) -> i64 {
        self.versions.values().fold(0i64, |total, version| {
            total.saturating_add(version.asset_size_bytes())
        })
    }
}

/// A stored package version: its status, when it was published, where it entered the
/// domain, and the assets published into it.
#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub version: String,
    /// One of `PackageVersionStatus`; a version is only ever stored in a status it can be
    /// observed in, so `Deleted` never appears here (a deleted version is gone).
    pub status: String,
    pub published_time: SystemTime,
    /// One of `PackageVersionOriginType`.
    pub origin_type: String,
    /// The repository the version entered the domain through, reported as the origin's
    /// `domainEntryPoint`.
    pub entry_point_repository: String,
    /// The version's assets, keyed by asset name.
    pub assets: BTreeMap<String, Asset>,
}

impl PackageVersion {
    /// `versionRevision`: an opaque token that identifies this version's CONTENT.
    ///
    /// Derived from everything a revision has to move with — the version, its status and
    /// each asset's name and content digest — so two reads of an unchanged version report
    /// the same revision, adding an asset or closing an unfinished version reports a new
    /// one, and a version restored to identical content reports the identical token.
    /// Base64 of a SHA-256 digest, which is 44 characters: inside the modelled
    /// `PackageVersionRevision` length (1..50) and `^\S+$` pattern.
    ///
    /// Costs the NUMBER of assets, not their bytes: each asset's digest was computed once
    /// when the asset was accepted (see [`Asset::new`]), so no read ever hashes stored
    /// content again.
    pub fn revision(&self) -> String {
        use base64::Engine as _;

        let mut fields: Vec<&str> = vec![self.version.as_str(), self.status.as_str()];
        for (name, asset) in &self.assets {
            fields.push(name.as_str());
            fields.push(asset.sha256_hex());
        }
        base64::engine::general_purpose::STANDARD.encode(identity_digest(&fields))
    }

    /// The modelled `PackageVersionDescription` for this record, inside `package`.
    ///
    /// `displayName`, `summary`, `homePage`, `sourceCodeRepository` and `licenses` are
    /// read by CodeArtifact out of a package's own metadata file (a `pom.xml`, a
    /// `package.json`): a generic package has none, so they have no source here and are
    /// omitted rather than invented.
    pub fn description(&self, package: &Package) -> PackageVersionDescription {
        PackageVersionDescription {
            format: Some(package.format.clone()),
            namespace: package.namespace.clone(),
            package_name: Some(package.name.clone()),
            display_name: None,
            version: Some(self.version.clone()),
            summary: None,
            home_page: None,
            source_code_repository: None,
            published_time: Some(self.published_time.into()),
            licenses: None,
            revision: Some(self.revision()),
            status: Some(self.status.clone()),
            origin: Some(PackageVersionOrigin {
                domain_entry_point: Some(DomainEntryPoint {
                    repository_name: Some(self.entry_point_repository.clone()),
                    // Set only for a version pulled through an external connection.
                    external_connection_name: None,
                }),
                origin_type: Some(self.origin_type.clone()),
            }),
        }
    }

    /// The total size of this version's assets.
    pub fn asset_size_bytes(&self) -> i64 {
        self.assets.values().fold(0i64, |total, asset| {
            total.saturating_add(asset.size_bytes())
        })
    }
}

/// A stored asset: its name, the exact bytes that were published, and their hashes.
///
/// The bytes are kept verbatim — `GetPackageVersionAsset` has to answer with what
/// `PublishPackageVersion` was given, byte for byte. Their hashes are computed ONCE, when
/// the asset is accepted, and stored beside them:
///
/// * a hash can never disagree with the content it describes, because the fields are
///   private and [`Asset::new`] is the only way to make one — an asset is replaced
///   wholesale, never edited in place, so a stored digest cannot go stale; and
/// * no request pays for the bytes already in the store. Hashing on every read would
///   make a few-hundred-byte `DescribePackageVersion` cost a pass over every byte the
///   version holds (its revision digests each asset), so the work one caller can ask for
///   would be the PRODUCT of the asset ceiling and the asset size rather than the size of
///   the request they sent.
#[derive(Debug, Clone)]
pub struct Asset {
    name: String,
    content: Vec<u8>,
    /// Keyed by the model's `HashAlgorithm` wire values, computed from `content`.
    hashes: HashMap<String, String>,
}

impl Asset {
    /// Accept `content` as the asset named `name`, computing its hashes now — one pass
    /// per algorithm over the bytes of THIS request, and never again for this asset.
    pub fn new(name: &str, content: Vec<u8>) -> Self {
        use md5::Digest as _;

        // One entry per value of the model's `HashAlgorithm` enum, lowercase hex, so a
        // client can look up the algorithm it verifies with.
        let hashes = HashMap::from([
            (
                HASH_ALGORITHM_MD5.to_string(),
                hex::encode(md5::Md5::digest(&content)),
            ),
            (
                HASH_ALGORITHM_SHA1.to_string(),
                hex::encode(sha1::Sha1::digest(&content)),
            ),
            (HASH_ALGORITHM_SHA256.to_string(), sha256_hex(&content)),
            (
                HASH_ALGORITHM_SHA512.to_string(),
                hex::encode(sha2::Sha512::digest(&content)),
            ),
        ]);
        Self {
            name: name.to_string(),
            content,
            hashes,
        }
    }

    /// The modelled `AssetSummary` for this record.
    pub fn summary(&self) -> AssetSummary {
        AssetSummary {
            name: self.name.clone(),
            size: Some(self.size_bytes()),
            hashes: Some(self.hashes.clone()),
        }
    }

    /// The asset's name.
    ///
    /// Read path for `GetPackageVersionAsset`, which is still a stub with its own issue:
    /// this and [`Asset::content`] are how that operation will reach a stored asset
    /// without the fields being public.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The asset's bytes, exactly as they were published.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// The asset's size, saturating at `i64::MAX` because the modelled member is an
    /// `i64` while a length is a `usize`.
    pub fn size_bytes(&self) -> i64 {
        i64::try_from(self.content.len()).unwrap_or(i64::MAX)
    }

    /// The asset's stored SHA-256, lowercase hex — the digest a caller declares in
    /// `assetSHA256`, the one two assets are compared by, and the one a version's
    /// revision is derived from. Always present: [`Asset::new`] computes it.
    pub fn sha256_hex(&self) -> &str {
        self.hashes
            .get(HASH_ALGORITHM_SHA256)
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// The `HashAlgorithm` value the `assetSHA256` integrity check and the stored revision
/// both read.
const HASH_ALGORITHM_SHA256: &str = "SHA-256";

/// The model's other three `HashAlgorithm` values, each named once so the keys of an
/// asset's `hashes` map cannot drift from the enum.
const HASH_ALGORITHM_MD5: &str = "MD5";
const HASH_ALGORITHM_SHA1: &str = "SHA-1";
const HASH_ALGORITHM_SHA512: &str = "SHA-512";

/// The lowercase-hex SHA-256 of `content`, in the spelling the modelled `SHA256` shape
/// accepts (`^[0-9a-f]+$`, exactly 64 characters). Private: a handler reads an asset's
/// digest through [`Asset::sha256_hex`], which is the stored one.
fn sha256_hex(content: &[u8]) -> String {
    use sha2::Digest as _;

    hex::encode(sha2::Sha256::digest(content))
}

/// The cursor scope of `ListPackages` for one repository. Scoped PER REPOSITORY, so a
/// cursor minted while listing one repository's packages cannot resolve as a key in
/// another repository's listing (or in a domain or repository listing).
pub fn package_cursor_scope(domain: &str, repository: &str) -> String {
    format!("packages/{domain}/{repository}")
}

// ---------------------------------------------------------------------------
// Resource policies
//
// A domain and a repository each hold at most one resource policy ([`Domain::policy`] /
// [`Repository::policy`]), and the whole `Put*PermissionsPolicy` /
// `Get*PermissionsPolicy` / `Delete*PermissionsPolicy` family reads and writes exactly
// that field. The revision rules below therefore live here rather than once per handler,
// so a revision minted by one operation is the revision the next one accepts.
// ---------------------------------------------------------------------------

/// The `ResourceType` enum member naming a domain, reported as `resourceType` beside
/// `resourceId` on a domain-scoped policy failure and named in the messages below.
/// `ResourceType` models exactly `domain`, `repository`, `package`, `package-version` and
/// `asset` (`smithy_query(query="shape",shape="ResourceType")`).
pub const DOMAIN_RESOURCE_TYPE: &str = "domain";

/// The not-found message a domain-scoped policy operation reports when the domain it
/// resolved holds no policy: the type of resource whose policy was asked for, and the name
/// the request addressed it by. Shared by the read and the delete, so the two cannot report
/// one domain's unset policy differently.
pub fn no_policy_message(resource_type: &str, resource: &str) -> String {
    format!("No policy found for ({resource_type}): ({resource})")
}

/// The conflict message a domain-scoped policy write or delete reports when the
/// `policyRevision` the caller supplied is not the revision currently attached — the
/// optimistic-locking refusal [`policy_revision_matches`] decides.
pub fn policy_revision_conflict_message(resource_type: &str, resource: &str) -> String {
    format!(
        "The policy revision supplied for ({resource_type}): ({resource}) is not the current revision of its resource policy."
    )
}

/// The refusal a resource-policy write owes a `policyDocument` that is not a policy, or
/// `None` when the document is one.
///
/// `policyDocument` is documented as "A valid displayable JSON Aspen policy string" and
/// the modelled pattern (`^[\P{C}\s]+$`) only forbids control characters, so JSON
/// well-formedness is the service's own rule — checked here, once, for every resource a
/// policy can hang off, so the modelled `reason` cannot differ between the domain and the
/// repository families. The document is parsed ONLY to check it: what the caller sent is
/// what gets stored ([`attached_policy`]).
pub fn policy_document_refusal(document: &str) -> Option<ValidationException> {
    match serde_json::from_str::<serde_json::Value>(document) {
        Ok(serde_json::Value::Object(_)) => None,
        Ok(other) => Some(ValidationException {
            message: format!(
                "policyDocument must be a JSON policy object; got a JSON {}.",
                json_type_name(&other)
            ),
            reason: Some("FIELD_VALIDATION_FAILED".to_string()),
        }),
        Err(error) => Some(ValidationException {
            message: format!("policyDocument is not valid JSON: {error}."),
            reason: Some("CANNOT_PARSE".to_string()),
        }),
    }
}

/// The JSON type name of a `policyDocument` that parsed but is not a policy object, so
/// the refusal says what the caller actually sent.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Whether a caller-supplied `policyRevision` authorises writing over the policy a
/// resource currently holds (`current`).
///
/// `policyRevision` is modelled as optional and documented as optimistic locking ("This
/// revision is used for optimistic locking, which prevents others from overwriting your
/// changes"), so: a caller that supplies one is asserting which revision it read, and the
/// write proceeds only while that is still the revision in the store; a caller that
/// supplies none is locking nothing and the write proceeds unconditionally. A resource
/// holding no policy carries no revision, so a supplied revision can never match it.
pub fn policy_revision_matches(current: Option<&ResourcePolicy>, supplied: Option<&str>) -> bool {
    match supplied {
        None => true,
        Some(supplied) => current.and_then(|policy| policy.revision.as_deref()) == Some(supplied),
    }
}

/// The policy to store on the resource `resource_arn` names: `document` as the caller
/// sent it, under the revision minted for this resource's `attachment`-th policy.
///
/// The document is stored VERBATIM — a resource policy is reported back as the string
/// that was submitted, never as a re-serialised or normalised rendering of it.
pub fn attached_policy(resource_arn: &str, document: String, attachment: u64) -> ResourcePolicy {
    ResourcePolicy {
        revision: Some(policy_revision(resource_arn, &document, attachment)),
        resource_arn: Some(resource_arn.to_string()),
        document: Some(document),
    }
}

/// The revision a resource policy carries once it is written.
///
/// Derived from the resource the policy hangs off, the document being written and
/// `attachment` — how many policies that resource has ever had attached — so it is stable
/// across every read of the same policy (nothing per-call goes into it) and distinct
/// between resources. The counter is what makes the revision a usable optimistic lock: it
/// only ever grows, so every write mints a revision this resource has never reported,
/// even one that rewrites a byte-identical document or re-attaches, after a delete, the
/// document that was attached before it — a revision a caller still holds can therefore
/// never start matching again.
fn policy_revision(resource_arn: &str, document: &str, attachment: u64) -> String {
    let digest = identity_digest(&[
        "policy-revision",
        resource_arn,
        document,
        &attachment.to_string(),
    ]);
    // 16 bytes rendered as 32 hex characters: inside `PolicyRevision`'s modelled 1..100
    // length and matching its `^\S+$` pattern.
    hex::encode(&digest[..16])
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState>)
}

/// The region this request is addressed to: the region field of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when the request carries none.
///
/// Every ARN this service mints is regional, and the region is a property of the
/// REQUEST (the endpoint the client signed for), not of the stored record — so it is
/// read here rather than guessed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a domain: `arn:aws:codeartifact:<region>:<owner>:domain/<name>`.
pub fn domain_arn(region: &str, domain: &str) -> String {
    ::engine::emulator_arn(SERVICE_SIGNING_NAME, region, &format!("domain/{domain}"))
}

/// The ARN of a repository:
/// `arn:aws:codeartifact:<region>:<owner>:repository/<domain>/<repository>`.
pub fn repository_arn(region: &str, domain: &str, repository: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_SIGNING_NAME,
        region,
        &format!("repository/{domain}/{repository}"),
    )
}

// ---------------------------------------------------------------------------
// Refusal messages
//
// A CodeArtifact refusal that names a resource IDENTIFIES it: the resource kind, its
// name quoted, the parent domain when the resource is nested in one, and the 12-digit
// account that owns the domain. The account is the point — CodeArtifact domains are
// shared across accounts through `domainOwner`, so a caller has to be able to tell
// "missing in my account" from "missing in the owner's account" from the message alone.
//
// They live here, beside the store the handlers resolve against, so every operation that
// refuses the same way words it the same way: without this, each handler spells its own
// sentence and the implemented ones drift from each other.
// ---------------------------------------------------------------------------

/// The account a request addressed a domain to: the `domainOwner` it pinned the domain
/// to, or — when the request named none — the account this emulator serves, which owns
/// every domain it holds.
///
/// This is the account a refusal reports, including a refusal for a domain that is NOT
/// held: it is the account the caller asked about, which is what makes a cross-account
/// miss distinguishable from a miss in the caller's own account.
pub fn addressed_domain_owner(domain_owner: Option<&str>) -> &str {
    match domain_owner {
        Some(owner) => owner,
        None => ::engine::emulator_account_id(),
    }
}

/// The `ResourceNotFoundException` message for a domain the store does not hold for this
/// caller.
pub fn domain_not_found_message(domain: &str, domain_owner: Option<&str>) -> String {
    format!(
        "Domain not found. Domain '{domain}' owned by account '{}' does not exist.",
        addressed_domain_owner(domain_owner)
    )
}

/// The `ResourceNotFoundException` message for a repository the addressed domain does not
/// hold — whether the caller named it directly or as another repository's upstream.
pub fn repository_not_found_message(
    domain: &str,
    domain_owner: Option<&str>,
    repository: &str,
) -> String {
    format!(
        "Repository not found. Repository '{repository}' in domain '{domain}' owned by \
         account '{}' does not exist.",
        addressed_domain_owner(domain_owner)
    )
}

/// The `ConflictException` message for a create whose domain name is already held.
///
/// `owner` is the account that owns the COLLIDING domain — read off the stored record,
/// not off the request, because that is the fact the caller cannot otherwise see.
pub fn domain_already_exists_message(domain: &str, owner: &str) -> String {
    format!("Domain '{domain}' owned by '{owner}' already exists.")
}

/// The `ConflictException` message for a create whose repository name is already held in
/// the addressed domain.
pub fn repository_already_exists_message(domain: &str, owner: &str, repository: &str) -> String {
    format!(
        "Repository '{repository}' in domain '{domain}' owned by '{owner}' already exists."
    )
}

/// The `ConflictException` message for `DeleteDomain` on a domain that still holds
/// repositories.
///
/// This refusal states the PRECONDITION and interpolates nothing: it is the same sentence
/// whatever domain was addressed and however many repositories remain, so it reports no
/// count of a domain the caller may not be able to list.
pub const DOMAIN_NOT_EMPTY_MESSAGE: &str = "Domain must contain 0 repositories before deletion";

/// The `ResourceNotFoundException` message for a package group the addressed domain does
/// not hold. Same composition as the two above, for the third resource kind a domain
/// contains: the pattern is the group's identifier, so it is what the sentence quotes.
pub fn package_group_not_found_message(
    domain: &str,
    domain_owner: Option<&str>,
    package_group: &str,
) -> String {
    format!(
        "Package group not found. Package group '{package_group}' in domain '{domain}' \
         owned by account '{}' does not exist.",
        addressed_domain_owner(domain_owner)
    )
}

/// The `ConflictException` message for a create whose package-group pattern is already
/// held in the addressed domain.
pub fn package_group_already_exists_message(
    domain: &str,
    owner: &str,
    package_group: &str,
) -> String {
    format!("Package group '{package_group}' in domain '{domain}' owned by '{owner}' already exists.")
}

/// The ARN of a package group:
/// `arn:aws:codeartifact:<region>:<owner>:package-group/<domain><pattern>`.
///
/// The pattern is a path that already begins with `/`, so it joins the domain segment
/// directly — the form CodeArtifact's `package-group` resource ARN takes. A pattern
/// handed in without the leading `/` gets one here, so the ARN always has exactly one
/// separator between the domain and the pattern.
pub fn package_group_arn(region: &str, domain: &str, pattern: &str) -> String {
    let path = pattern.strip_prefix('/').unwrap_or(pattern);
    ::engine::emulator_arn(
        SERVICE_SIGNING_NAME,
        region,
        &format!("package-group/{domain}/{path}"),
    )
}

/// The `EndpointType` a request that names none is served: AWS documents the IPv4
/// endpoint as a repository's default, with `dualstack` the opt-in alternative
/// (`EndpointType` models exactly `dualstack | ipv4`).
pub const DEFAULT_ENDPOINT_TYPE: &str = "ipv4";

/// The `EndpointType` value that selects the dual-stack (IPv4/IPv6) host.
pub const ENDPOINT_TYPE_DUALSTACK: &str = "dualstack";

/// The endpoint a repository serves one package format on.
///
/// CodeArtifact gives a repository ONE endpoint per package format, and its URL is built
/// entirely from the repository's identity — the domain and its owning account name the
/// host, the region names the regional endpoint the request was addressed to, and the
/// format and repository name are the two path segments (AWS CodeArtifact user guide,
/// e.g. the npm endpoint
/// `https://my_domain-111122223333.d.codeartifact.us-west-2.amazonaws.com/npm/my_repo/`).
/// A dual-stack endpoint is that same URL on the `on.aws` suffix
/// (`…d.codeartifact.us-west-2.on.aws/npm/my_repo/`), which is what `endpointType`
/// selects. The trailing slash is part of the documented endpoint.
pub fn repository_endpoint(
    region: &str,
    domain: &str,
    domain_owner: &str,
    format: &str,
    repository: &str,
    endpoint_type: &str,
) -> String {
    let suffix = if endpoint_type == ENDPOINT_TYPE_DUALSTACK {
        "on.aws"
    } else {
        "amazonaws.com"
    };
    format!(
        "https://{domain}-{domain_owner}.d.codeartifact.{region}.{suffix}/{format}/{repository}/"
    )
}

/// The ARN of the S3 bucket a domain stores its assets in.
///
/// CodeArtifact names this bucket `assets-<account>-<region>-<suffix>`. An S3 ARN carries
/// NO region or account segment of its own, so this one cannot go through
/// `engine::emulator_arn` (which always writes both) and is formatted here instead; the
/// partition is the same `aws` that helper uses, so every ARN this service emits agrees
/// on it. The suffix is derived from the domain's identity (account, region, name) so it
/// is stable across every read of the same domain and distinct between domains — never a
/// per-call random value.
pub fn asset_bucket_arn(region: &str, domain: &str) -> String {
    let suffix = hex::encode(&identity_digest(&["s3-assets", region, domain])[..6]);
    format!(
        "arn:aws:s3:::assets-{}-{region}-{suffix}",
        ::engine::emulator_account_id()
    )
}

/// The KMS key ARN a domain created without an explicit `encryptionKey` is encrypted
/// with.
///
/// CodeArtifact creates (or reuses) an AWS-managed key for the domain when the caller
/// names none, and reports its ARN. The key id is derived from the domain's identity for
/// the same reason as [`asset_bucket_arn`]'s suffix: the same domain must report the
/// same key on every read.
pub fn default_encryption_key_arn(region: &str, domain: &str) -> String {
    let digest = identity_digest(&["kms-key", region, domain]);
    let hex = hex::encode(&digest[..16]);
    let key_id = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    ::engine::emulator_arn("kms", region, &format!("key/{key_id}"))
}

/// A stable 32-byte digest over a resource's identifying fields.
///
/// Each field is terminated with a NUL so two different field splits (`["ab", "c"]` vs
/// `["a", "bc"]`) cannot digest to the same value.
fn identity_digest(fields: &[&str]) -> [u8; 32] {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for field in fields {
        hasher.update(field.as_bytes());
        hasher.update([0u8]);
    }
    hasher.finalize().into()
}

/// The marker every pagination cursor this service mints carries, so a token the service
/// did not issue (or one minted by an older cursor format) is REFUSED rather than read as
/// a key: without it, any hex string that happens to decode to UTF-8 would be accepted.
/// The trailing digit is the cursor format version, to be bumped if the payload changes.
const TOKEN_MARKER: &str = "ca1";

/// The cursor scope of `ListDomains`.
pub const DOMAIN_CURSOR_SCOPE: &str = "domains";

/// The cursor scope of `ListRepositoriesInDomain` for one domain. Scoped PER DOMAIN, so a
/// cursor minted while listing one domain's repositories cannot resolve as a key in
/// another domain's listing (or in the domain listing).
pub fn repository_cursor_scope(domain: &str) -> String {
    format!("repositories/{domain}")
}

/// Encode a pagination cursor: the key of the LAST item returned on this page, inside the
/// listing `scope` it names a place in.
///
/// The token is opaque to the client (hex of a marked, scoped payload, so it satisfies
/// the modelled `^\S+$` pattern and needs no escaping in a query string) and carries no
/// offset — it is resolved against the live sorted key order by [`page_of`], so a token
/// stays valid across calls that insert or remove other keys.
pub fn encode_token(scope: &str, last_key: &str) -> String {
    hex::encode(format!("{TOKEN_MARKER}:{scope}:{last_key}").as_bytes())
}

/// Decode a cursor [`encode_token`] produced FOR THIS `scope`. `None` when the token was
/// not produced by this service, or was minted by a different listing — which a handler
/// answers as an invalid `nextToken` rather than silently resolving it as a key here.
pub fn decode_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("{TOKEN_MARKER}:{scope}:"))
        .map(str::to_string)
}

/// One page of the entries of `items` that `matches` accepts, in key order: everything
/// strictly after the `after` cursor (when one was supplied), capped at `max_results` and
/// projected through `project`.
///
/// The cursor is resolved with `BTreeMap::range`, so a page costs the keys it actually
/// returns rather than a projection of the whole store, and `project` runs only on the
/// entries that end up on the page.
///
/// Returns the page and the `nextToken` to report — `None` on the last page, which is
/// what tells a paginating client to stop.
pub fn page_of<V, T, M, P>(
    items: &BTreeMap<String, V>,
    scope: &str,
    after: Option<&str>,
    max_results: Option<i32>,
    matches: M,
    project: P,
) -> (Vec<T>, Option<String>)
where
    M: Fn(&str, &V) -> bool,
    P: Fn(&V) -> T,
{
    use std::ops::Bound;

    // CodeArtifact's `maxResults` is modelled 1..=1000 on every paginated operation
    // here, so the boundary validator has already refused anything outside that range;
    // an absent value means "the service's own page size", which is the maximum.
    const DEFAULT_PAGE_SIZE: usize = 1000;

    let limit = max_results
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .max(1);
    // The clamp above is a floor, not a policy: a `maxResults` under the modelled minimum
    // is refused at the boundary and must never reach here, and a page size of zero would
    // return an empty page with no cursor forever. Assert it in debug builds so a model
    // or validator change shows up as a test failure rather than as silent coercion.
    debug_assert!(
        max_results.is_none_or(|n| n >= 1),
        "maxResults below the modelled minimum reached page_of: {max_results:?}"
    );

    let start = match after {
        Some(cursor) => Bound::Excluded(cursor.to_string()),
        None => Bound::Unbounded,
    };
    let mut matching = items
        .range((start, Bound::Unbounded))
        .filter(|(key, value)| matches(key.as_str(), value))
        .peekable();

    let mut page = Vec::new();
    let mut last_key = None;
    while page.len() < limit {
        match matching.next() {
            Some((key, value)) => {
                last_key = Some(key.clone());
                page.push(project(value));
            }
            None => break,
        }
    }

    // A `nextToken` is reported only when something is actually left, so the final page
    // omits it instead of handing back a token that resolves to an empty page.
    let next_token = match (matching.peek().is_some(), last_key) {
        (true, Some(key)) => Some(encode_token(scope, &key)),
        _ => None,
    };
    (page, next_token)
}
