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
//! `DomainSummary`, `RepositoryDescription`, `RepositorySummary` and
//! `PackageGroupDescription` are all projections of the records below
//! ([`Domain::description`] and friends), so a value that is derived —
//! `repositoryCount`, a package group's `parent` and each origin restriction's
//! `effectiveMode` above all — can never go stale against the records it summarises.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    DomainDescription, DomainSummary, PackageGroupDescription, PackageGroupOriginConfiguration,
    PackageGroupOriginRestriction, PackageGroupReference, RepositoryDescription,
    RepositoryExternalConnectionInfo, RepositorySummary, ResourcePolicy, Tag,
    UpstreamRepositoryInfo, PACKAGE_GROUP_ORIGIN_RESTRICTION_TYPE_VALUES,
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

/// Maximum external connections one repository may hold. `AssociateExternalConnection`'s
/// own documentation states the limit — "One external connection is allowed per
/// repository" — and it is why that operation models
/// `ServiceQuotaExceededException` (402).
pub const MAX_EXTERNAL_CONNECTIONS_PER_REPOSITORY: usize = 1;

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
    pub asset_size_bytes: i64,
    pub s3_bucket_arn: String,
    pub tags: Vec<Tag>,
    pub policy: Option<ResourcePolicy>,
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
            asset_size_bytes: self.asset_size_bytes,
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

/// A stored repository inside a domain.
///
/// Later slices hang the package maps off this record for the package-version
/// operations; the fields here are the ones the repository lifecycle itself needs.
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
}

impl Repository {
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
