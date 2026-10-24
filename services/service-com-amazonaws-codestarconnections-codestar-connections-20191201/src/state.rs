// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codestarconnections-codestar-connections-20191201 service.
//!
//! This module owns the service's persisted resources (connections, repository links,
//! hosts, sync blockers and their tags) plus the few request-derived helpers every handler needs: the ARN namespace, the
//! region the caller signed for, and the `NextToken` codec the paginated list
//! operations share. `src/lib.rs` is generated, so a helper several handlers reach
//! lives here rather than in a module of its own.

use std::collections::hash_map::RandomState;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::time::SystemTime;

use crate::ops::types::{
    Connection, Host, RepositoryLinkInfo, SyncBlocker, SyncBlockerContext, Tag, VpcConfiguration,
};

/// This service's ARN namespace (`awsService.arnNamespace` in the Smithy model), used
/// for every ARN the service mints.
pub const ARN_NAMESPACE: &str = "codestar-connections";

/// The status a connection created through the API carries.
///
/// The model's `Connection` doc: "A connection created through CloudFormation, the
/// CLI, or the SDK is in `PENDING` status by default. You can make its status
/// `AVAILABLE` by updating the connection in the console." The handshake that moves a
/// connection to `AVAILABLE` is a console action, not something the clock owes, so a
/// connection this emulator mints stays `PENDING` until something updates it.
pub const CONNECTION_STATUS_PENDING: &str = "PENDING";

/// The statuses a host carries.
///
/// From the model's `Host$Status` doc ("The status of the host, such as PENDING,
/// AVAILABLE, VPC_CONFIG_DELETING, VPC_CONFIG_INITIALIZING, and
/// VPC_CONFIG_FAILED_INITIALIZATION", `smithy_query(query="shape", shape="Host")`).
/// `HostStatus` is modeled as a free-form string (`len 1..64`) rather than an enum, so
/// these are the values the doc names rather than a closed set the wire enforces.
pub const HOST_STATUS_PENDING: &str = "PENDING";
/// The status description a host carries while it is `PENDING`.
///
/// `CreateHost`'s own doc states what a pending host is waiting for: "A host created
/// through the CLI or the SDK is in `PENDING` status by default. You can make its
/// status `AVAILABLE` by setting up the host in the console." `Host$StatusMessage` is
/// modeled "The status description for the host" — so while the host sits in that
/// non-terminal status the service has a description to report, and this is it. The
/// WORDING is the emulator's (AWS documents none); what the API contract carries is
/// that a pending host reports a description at all.
pub const HOST_STATUS_MESSAGE_PENDING: &str =
    "Host is pending. Complete the host setup in the console to make it available.";

/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_AVAILABLE: &str = "AVAILABLE";
/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_VPC_CONFIG_DELETING: &str = "VPC_CONFIG_DELETING";
/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_VPC_CONFIG_INITIALIZING: &str = "VPC_CONFIG_INITIALIZING";
/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_VPC_CONFIG_FAILED_INITIALIZATION: &str = "VPC_CONFIG_FAILED_INITIALIZATION";

/// Every status [`HOST_STATUS_PENDING`]'s doc names, in the order it names them.
pub const HOST_STATUS_VALUES: &[&str] = &[
    HOST_STATUS_PENDING,
    HOST_STATUS_AVAILABLE,
    HOST_STATUS_VPC_CONFIG_DELETING,
    HOST_STATUS_VPC_CONFIG_INITIALIZING,
    HOST_STATUS_VPC_CONFIG_FAILED_INITIALIZATION,
];

/// The statuses a host cannot be deleted from.
///
/// `DeleteHost`'s own doc states the rule: "A host cannot be deleted if it is in the
/// VPC_CONFIG_INITIALIZING or VPC_CONFIG_DELETING state." Both are transitional VPC
/// provisioning states, so the refusal is the "the host is there but is not usable
/// right now" signal (`ResourceUnavailableException`) rather than an absence.
pub const HOST_STATUS_DELETE_BLOCKED: &[&str] = &[
    HOST_STATUS_VPC_CONFIG_INITIALIZING,
    HOST_STATUS_VPC_CONFIG_DELETING,
];

/// The statuses a sync blocker carries.
///
/// `SyncBlocker$Status` targets the closed `BlockerStatus` enum
/// (`smithy_query(query="shape", shape="BlockerStatus")` — `ACTIVE`, `RESOLVED`, also
/// generated as `ops::types::shared_types::BLOCKER_STATUS_VALUES`). A blocker the
/// Git-sync side raises starts `ACTIVE`; resolving it is what `UpdateSyncBlocker` does,
/// and `RESOLVED` is terminal — nothing moves a blocker back.
pub const BLOCKER_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`BLOCKER_STATUS_ACTIVE`].
pub const BLOCKER_STATUS_RESOLVED: &str = "RESOLVED";

/// The only `BlockerType` the model declares
/// (`smithy_query(query="shape", shape="BlockerType")`): a sync blocker is raised by the
/// service's own Git-sync component, never by a caller, which is why this service models
/// no operation that creates one.
pub const BLOCKER_TYPE_AUTOMATED: &str = "AUTOMATED";

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The page size a paginated list operation applies when the caller asks for none.
///
/// `MaxResults` is modeled `range 0..100` with `default 0`, so an omitted member
/// arrives as `0` — indistinguishable from "no page size requested". The modeled
/// maximum is the largest page the operation may ever return, so it is what an
/// unconstrained list uses.
pub const MAX_PAGE_SIZE: usize = 100;

/// The largest number of connections this emulator holds at once.
///
/// The service itself caps them: the model declares `LimitExceededException` on
/// `CreateConnection` with the doc "Exceeded the maximum limit for connections.", so
/// refusing past a maximum is the modeled behaviour. The real per-account quota is
/// adjustable and the model does not state it, so this value is the EMULATOR's bound —
/// chosen high enough that no realistic client workflow meets it, and finite so a
/// long-lived process cannot grow its state without limit.
pub const MAX_CONNECTIONS: usize = 1000;

/// The largest number of repository links this emulator holds at once.
///
/// As with [`MAX_CONNECTIONS`], refusing past a maximum is the modeled behaviour: the
/// model declares `LimitExceededException` on `CreateRepositoryLink`, whose doc is
/// "Exceeded the maximum limit for connections.". The real per-account quota is
/// adjustable and the model does not state it, so this value is the EMULATOR's bound —
/// high enough that no realistic client workflow meets it, and finite so a long-lived
/// process cannot grow its state without limit.
pub const MAX_REPOSITORY_LINKS: usize = 1000;

/// The largest number of hosts this emulator holds at once.
///
/// A CHOSEN bound, exactly like [`MAX_CONNECTIONS`] and for the same reason: the store
/// lives in this process, so the number of records a caller can accumulate has to be
/// capped somewhere. `CreateHost` refuses past it with `LimitExceededException` (429),
/// the only error that operation models.
pub const MAX_HOSTS: usize = 1000;

/// The largest number of tags one resource holds.
///
/// `TagList` is modeled `len 0..200`, so 200 is the most tags the service ever accepts
/// in one request; keeping the stored map to that same maximum means a sequence of
/// merges cannot grow a resource past what a single request could set. Over the limit,
/// `TagResource` answers the `LimitExceededException` it declares.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// A connection as this service persists it.
///
/// The wire `Connection` shape is rebuilt from these fields by every reader, so a
/// create followed by a get or a list answers out of this record and nothing else.
#[derive(Debug, Clone)]
pub struct StoredConnection {
    /// The connection's generated id — the last segment of its ARN.
    pub connection_id: String,
    /// `ConnectionName` as supplied at create.
    pub connection_name: String,
    /// The ARN this connection is keyed by.
    pub connection_arn: String,
    /// `ProviderType`; the model does not require it at create, so it may be absent.
    pub provider_type: Option<String>,
    /// The 12-digit account id that owns the connection.
    pub owner_account_id: String,
    /// One of `CONNECTION_STATUS_VALUES`.
    pub connection_status: String,
    /// `HostArn`, only when one was supplied at create.
    pub host_arn: Option<String>,
    /// When the connection was created. Recorded resource metadata; the stable list
    /// order is `sequence`, which cannot be perturbed by a clock that steps back.
    pub created_at: SystemTime,
    /// Creation order, and the value `NextToken` resumes from.
    pub sequence: u64,
}

impl StoredConnection {
    /// This record as the modeled `Connection` structure every reader returns.
    ///
    /// `HostArn` stays absent when the connection was created without one, rather
    /// than being reported as an empty string.
    pub fn to_connection(&self) -> Connection {
        Connection {
            connection_name: Some(self.connection_name.clone()),
            connection_arn: Some(self.connection_arn.clone()),
            provider_type: self.provider_type.clone(),
            owner_account_id: Some(self.owner_account_id.clone()),
            connection_status: Some(self.connection_status.clone()),
            host_arn: self.host_arn.clone(),
        }
    }
}

/// A repository link as this service persists it.
///
/// One record per `RepositoryLinkId`; every member of the modeled `RepositoryLinkInfo`
/// shape is stored here, so `CreateRepositoryLink` and every later reader of the link
/// render the same row.
#[derive(Debug, Clone)]
pub struct StoredRepositoryLink {
    /// The link's generated id — the last segment of its ARN, and the key of
    /// [`ComAmazonawsCodestarconnectionsCodestarConnections20191201State::repository_links`].
    pub repository_link_id: String,
    /// The ARN minted for this link.
    pub repository_link_arn: String,
    /// The connection Git sync reaches the repository through, as supplied at create.
    pub connection_arn: String,
    /// `EncryptionKeyArn`, only when one was supplied at create.
    pub encryption_key_arn: Option<String>,
    /// The repository owner at the provider (the GitHub owner id, say).
    pub owner_id: String,
    /// DERIVED from the connection, never taken from the request: the modeled
    /// `RepositoryLinkInfo.ProviderType` is "The provider type for the connection, such
    /// as GitHub, associated with the repository link", so it is whatever the connection
    /// the link names reports.
    pub provider_type: String,
    /// The repository the link points at.
    pub repository_name: String,
    /// Creation order, taken from the store's own monotonic sequence.
    pub sequence: u64,
}

impl StoredRepositoryLink {
    /// This record as the modeled `RepositoryLinkInfo` structure every reader returns.
    pub fn to_repository_link_info(&self) -> RepositoryLinkInfo {
        RepositoryLinkInfo {
            connection_arn: self.connection_arn.clone(),
            encryption_key_arn: self.encryption_key_arn.clone(),
            owner_id: self.owner_id.clone(),
            provider_type: self.provider_type.clone(),
            repository_link_arn: self.repository_link_arn.clone(),
            repository_link_id: self.repository_link_id.clone(),
            repository_name: self.repository_name.clone(),
        }
    }
}

/// A host as this service persists it.
///
/// One record per `HostArn`, holding every member of the modeled `Host` shape the API
/// can observe, so an operation that reports or deletes a host works off this one row
/// rather than recomputing one. `CreateHost` writes the row, `GetHost` and `ListHosts`
/// render it through [`StoredHost::to_host`], and `DeleteHost` reads
/// [`StoredHost::status`] (the delete is refused while the host's VPC configuration is in
/// flight) before removing the whole record. `UpdateHost` updates this same row (still a
/// scaffold with its own open issue).
#[derive(Debug, Clone)]
pub struct StoredHost {
    /// `Name` as supplied at create.
    pub name: String,
    /// The ARN this host is keyed by.
    pub host_arn: String,
    /// One of `PROVIDER_TYPE_VALUES`; `CreateHost` models it required.
    pub provider_type: String,
    /// The endpoint of the infrastructure the host represents.
    pub provider_endpoint: String,
    /// The VPC configuration provisioned for the host, when one was supplied.
    pub vpc_configuration: Option<VpcConfiguration>,
    /// The host's `HostStatus` — one of `HOST_STATUS_VALUES`.
    pub status: String,
    /// The status description, when the service has one to report.
    pub status_message: Option<String>,
    /// Creation order, and the value `ListHosts`' `NextToken` resumes from. Drawn from
    /// the same store counter as a connection's, so no two records share one.
    pub sequence: u64,
}

impl StoredHost {
    /// This record as the modeled `Host` structure every reader renders from.
    ///
    /// `GetHost` projects this onto its own (flattened, `HostArn`-less and
    /// `StatusMessage`-less) output shape, and `ListHosts` reports it whole, so both
    /// read ONE rendering of a stored host rather than each recomputing one.
    pub fn to_host(&self) -> Host {
        Host {
            name: Some(self.name.clone()),
            host_arn: Some(self.host_arn.clone()),
            provider_type: Some(self.provider_type.clone()),
            provider_endpoint: Some(self.provider_endpoint.clone()),
            vpc_configuration: self.vpc_configuration.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
        }
    }

    /// Whether this host's status forbids deleting it — see
    /// [`HOST_STATUS_DELETE_BLOCKED`].
    pub fn delete_blocked_by_status(&self) -> bool {
        HOST_STATUS_DELETE_BLOCKED.contains(&self.status.as_str())
    }
}

/// A sync blocker as this service persists it.
///
/// One record per sync blocker `Id`. A blocker belongs to the (`ResourceName`,
/// `SyncType`) pair whose syncing it blocks — the pair every blocker operation addresses
/// it through — and carries every member of the modeled `SyncBlocker` shape, so the
/// operation that resolves one and any later reader of it render the same row.
#[derive(Debug, Clone)]
pub struct StoredSyncBlocker {
    /// The blocker's id, and the key of
    /// [`ComAmazonawsCodestarconnectionsCodestarConnections20191201State::sync_blockers`].
    pub id: String,
    /// The resource whose syncing this blocker blocks.
    pub resource_name: String,
    /// The parent of that resource, when it has one.
    pub parent_resource_name: Option<String>,
    /// The `SyncConfigurationType` the blocker was raised under.
    pub sync_type: String,
    /// `SyncBlocker$Type` — one of `BLOCKER_TYPE_VALUES`, i.e.
    /// [`BLOCKER_TYPE_AUTOMATED`].
    pub blocker_type: String,
    /// `SyncBlocker$Status` — [`BLOCKER_STATUS_ACTIVE`] or [`BLOCKER_STATUS_RESOLVED`].
    pub status: String,
    /// Why the blocker was raised.
    pub created_reason: String,
    /// When it was raised.
    pub created_at: SystemTime,
    /// The blocker's contexts, empty when it carries none.
    pub contexts: Vec<SyncBlockerContext>,
    /// The reason supplied when the blocker was resolved; absent while it is active.
    pub resolved_reason: Option<String>,
    /// When the blocker was resolved; absent while it is active.
    pub resolved_at: Option<SystemTime>,
}

impl StoredSyncBlocker {
    /// This record as the modeled `SyncBlocker` structure every reader returns.
    ///
    /// A blocker holding no contexts reports no `Contexts` member at all rather than an
    /// empty list, and an ACTIVE one reports neither `ResolvedReason` nor `ResolvedAt`:
    /// both are modeled optional and are what resolving the blocker fills in.
    pub fn to_sync_blocker(&self) -> SyncBlocker {
        SyncBlocker {
            id: self.id.clone(),
            r#type: self.blocker_type.clone(),
            status: self.status.clone(),
            created_reason: self.created_reason.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            contexts: if self.contexts.is_empty() {
                None
            } else {
                Some(self.contexts.clone())
            },
            resolved_reason: self.resolved_reason.clone(),
            resolved_at: self.resolved_at.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// Whether this blocker is still blocking syncing — the one state from which it can
    /// be resolved.
    pub fn is_active(&self) -> bool {
        self.status == BLOCKER_STATUS_ACTIVE
    }

    /// Resolve this blocker: `UpdateSyncBlocker`'s whole effect, as its doc states it
    /// ("update the status of a sync blocker, resolving the blocker and allowing syncing
    /// to continue"). The status moves to [`BLOCKER_STATUS_RESOLVED`] and the reason and
    /// the instant of the resolution are recorded; everything the blocker was raised
    /// with is left as it was.
    pub fn resolve(&mut self, resolved_reason: &str, at: SystemTime) {
        self.status = BLOCKER_STATUS_RESOLVED.to_string();
        self.resolved_reason = Some(resolved_reason.to_string());
        self.resolved_at = Some(at);
    }
}

/// Per-service state for com-amazonaws-codestarconnections-codestar-connections-20191201. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodestarconnectionsCodestarConnections20191201State {
    /// Every connection in this emulator, keyed by its ARN.
    pub connections: BTreeMap<String, StoredConnection>,
    /// Every repository link in this emulator, keyed by its `RepositoryLinkId` — the id
    /// `GetRepositoryLink`, `UpdateRepositoryLink` and `DeleteRepositoryLink` address a
    /// link by.
    pub repository_links: BTreeMap<String, StoredRepositoryLink>,
    /// Every host in this emulator, keyed by its ARN. A connection may name one
    /// through `StoredConnection::host_arn`, which is the association `DeleteHost`
    /// refuses to break.
    pub hosts: BTreeMap<String, StoredHost>,
    /// Every sync blocker in this emulator, keyed by its `Id` — the id
    /// `UpdateSyncBlocker` addresses a blocker by, alongside the resource and sync type
    /// it belongs to. The service models no operation that RAISES one (a blocker is
    /// `AUTOMATED`: the Git-sync component raises it), so this map holds whatever that
    /// side of the service has put there and an id naming no entry is the modeled
    /// `SyncBlockerDoesNotExistException`.
    pub sync_blockers: BTreeMap<String, StoredSyncBlocker>,
    /// Tags by resource ARN, shared by `TagResource` / `UntagResource` /
    /// `ListTagsForResource` and populated from `CreateConnection.Tags`.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The next creation-order sequence number to hand out.
    pub next_sequence: u64,
}

impl ComAmazonawsCodestarconnectionsCodestarConnections20191201State {
    /// Take the next creation-order sequence number.
    pub fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }

    /// Every connection in creation order — the order `ListConnections` pages over.
    ///
    /// Borrowed, not cloned: the request sorts POINTERS (an O(n log n) pass over the
    /// store's size) and clones only the page it returns.
    pub fn connections_in_creation_order(&self) -> Vec<&StoredConnection> {
        let mut connections: Vec<&StoredConnection> = self.connections.values().collect();
        connections.sort_unstable_by_key(|connection| connection.sequence);
        connections
    }

    /// Every host in creation order — the order `ListHosts` pages over.
    ///
    /// Borrowed, not cloned, for the same reason as
    /// [`Self::connections_in_creation_order`]: the request sorts POINTERS and clones
    /// only the page it returns.
    pub fn hosts_in_creation_order(&self) -> Vec<&StoredHost> {
        let mut hosts: Vec<&StoredHost> = self.hosts.values().collect();
        hosts.sort_unstable_by_key(|host| host.sequence);
        hosts
    }

    /// The repository link already registered for this connection, owner and repository,
    /// when there is one.
    ///
    /// A repository link IS that triple — the connection Git sync reaches the repository
    /// through, plus the owner and repository it names — so registering the same triple
    /// twice is the duplicate `CreateRepositoryLink` refuses with the modeled
    /// `ResourceAlreadyExistsException`.
    pub fn repository_link_for(
        &self,
        connection_arn: &str,
        owner_id: &str,
        repository_name: &str,
    ) -> Option<&StoredRepositoryLink> {
        self.repository_links.values().find(|link| {
            link.connection_arn == connection_arn
                && link.owner_id == owner_id
                && link.repository_name == repository_name
        })
    }

    /// How many connections are currently associated with `host_arn`.
    ///
    /// `DeleteHost`'s doc states the precondition this answers: "Before you delete a
    /// host, all connections associated to the host must be deleted." The association
    /// lives on the CONNECTION (`StoredConnection::host_arn`, set at create), so it is
    /// read out of the connection store rather than held a second time on the host, and
    /// counted in place: the caller needs how many still hold the host, not which.
    pub fn connections_attached_to_host_count(&self, host_arn: &str) -> usize {
        self.connections
            .values()
            .filter(|connection| connection.host_arn.as_deref() == Some(host_arn))
            .count()
    }

    /// The sync blocker the (`Id`, `ResourceName`, `SyncType`) triple of a request
    /// addresses, mutably.
    ///
    /// A blocker is addressed by its id TOGETHER with the resource and sync type it
    /// belongs to, so an id that exists under another resource or another sync type
    /// addresses nothing: it is not the blocker the request named, and the caller is
    /// answered the same absence as for an id that was never raised.
    pub fn sync_blocker_mut(
        &mut self,
        id: &str,
        resource_name: &str,
        sync_type: &str,
    ) -> Option<&mut StoredSyncBlocker> {
        self.sync_blockers
            .get_mut(id)
            .filter(|blocker| blocker.resource_name == resource_name && blocker.sync_type == sync_type)
    }

    /// Whether this service holds a resource under `resource_arn`.
    ///
    /// The tag operations (`TagResource` / `UntagResource` / `ListTagsForResource`) are
    /// modeled over a `ResourceArn` rather than over one resource kind, so each
    /// resolves the ARN through this one predicate: EVERY resource kind this service
    /// persists is tag-addressable, and an ARN naming none of them is the
    /// `ResourceNotFoundException` those operations declare. `CreateHost`,
    /// `CreateConnection` and `CreateRepositoryLink` all accept `Tags`, so all three
    /// kinds have to resolve here. A repository link is keyed by its ID, so its ARN is
    /// resolved over the link records rather than by a map lookup.
    pub fn holds_resource(&self, resource_arn: &str) -> bool {
        self.connections.contains_key(resource_arn)
            || self.hosts.contains_key(resource_arn)
            || self
                .repository_links
                .values()
                .any(|link| link.repository_link_arn == resource_arn)
    }

    /// How many distinct keys `resource_arn` would hold after merging `tags`.
    pub fn tag_count_after_merge(&self, resource_arn: &str, tags: &[Tag]) -> usize {
        match self.tags.get(resource_arn) {
            None => tags
                .iter()
                .map(|tag| tag.key.as_str())
                .collect::<::std::collections::BTreeSet<&str>>()
                .len(),
            Some(stored) => {
                let mut keys: ::std::collections::BTreeSet<&str> =
                    stored.keys().map(String::as_str).collect();
                keys.extend(tags.iter().map(|tag| tag.key.as_str()));
                keys.len()
            }
        }
    }

    /// The tags held for `resource_arn`, as the modeled `TagList`.
    pub fn tag_list(&self, resource_arn: &str) -> Vec<Tag> {
        self.tags
            .get(resource_arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| Tag {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Merge `tags` onto whatever `resource_arn` already carries; a repeated key
    /// overwrites.
    pub fn merge_tags(&mut self, resource_arn: &str, tags: &[Tag]) {
        let stored = self.tags.entry(resource_arn.to_string()).or_default();
        for tag in tags {
            stored.insert(tag.key.clone(), tag.value.clone());
        }
    }
}

/// Whether `region` is syntactically an AWS region code.
///
/// An AWS region code is lowercase alphanumerics and hyphens (`us-east-1`,
/// `ap-southeast-4`, `cn-north-1`). The check matters because the region becomes an
/// ARN field: without it a caller's credential scope could put an ARN delimiter
/// (`:` or `/`) into an ARN this service mints and keys its state by.
fn is_region_code(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The region the caller signed this request for, defaulting to [`DEFAULT_REGION`]
/// when the request carries no SigV4 credential scope — or one whose region is not a
/// region code.
///
/// Reading the scope rather than a fixed constant is what makes an ARN this service
/// mints match the endpoint the client addressed, so a client-constructed ARN
/// resolves against a stored one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_code(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Mint the id of a new resource: a hyphenated UUID, which is the shape the real
/// service's connection, host and repository-link ids carry (`connection/<uuid>`,
/// `host/<uuid>`, `repository-link/<uuid>` in the ARN, and the modeled
/// `RepositoryLinkId` pattern
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
///
/// The low 48 bits carry the store's monotonic `sequence`, so an id is unique BY
/// CONSTRUCTION — never by probability — and is never handed out twice however many
/// resources one process mints (every kind of resource draws from the one counter,
/// [`ComAmazonawsCodestarconnectionsCodestarConnections20191201State::take_sequence`],
/// so two kinds cannot collide either); the remaining bits come from the process's
/// OS-seeded [`RandomState`], so one id does not reveal the next. The version and
/// variant nibbles are set per RFC 4122 (version 4, variant `10xx`), so a client
/// that parses the id as a UUID gets a well-formed one. Built from `std` alone: the
/// uniqueness this needs comes from state, not from a random-number source, so the
/// crate carries no UUID dependency for it.
///
/// One counter feeds every resource kind ([`ComAmazonawsCodestarconnectionsCodestarConnections20191201State::take_sequence`]),
/// so no two resources of this service — of the same kind or not — ever share an id.
pub fn mint_resource_uuid(sequence: u64) -> String {
    let mut bytes = [0u8; 16];
    // Two independently seeded hashes of the sequence: `RandomState::new` takes fresh
    // keys from the process's seed, so these differ from call to call.
    bytes[..8].copy_from_slice(&RandomState::new().hash_one(sequence).to_be_bytes());
    bytes[8..].copy_from_slice(&RandomState::new().hash_one(!sequence).to_be_bytes());
    // The sequence itself occupies the last 6 bytes, which is what makes the id
    // unique rather than merely unlikely to repeat.
    bytes[10..].copy_from_slice(&sequence.to_be_bytes()[2..]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut hex = String::with_capacity(32);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing hex into a String cannot fail");
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Whether `host_arn` names a host in the account this emulator serves.
///
/// A host ARN is a caller-supplied cross-reference to another resource, and its FIFTH
/// field is the account that owns it (`service_common::arn`, this emulator's one ARN
/// parse). An operation that associates a connection with a host, or filters by one,
/// authorizes that field against the caller's own account before it resolves the ARN
/// against any store: a host in another account is not the caller's to reach, whether or
/// not it exists.
///
/// An ARN this cannot parse answers `false` too — the modeled `HostArn` pattern
/// (`^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`) is enforced by the
/// boundary validator before a handler runs, so a value that reaches here and does not
/// parse names no account at all.
pub fn host_arn_is_in_caller_account(host_arn: &str) -> bool {
    ::service_common::arn::parse(host_arn)
        .is_some_and(|arn| arn.account == ::engine::emulator_account_id())
}

/// The refusal for a host ARN outside the caller's account: it names the caller's
/// account and the ARN it was denied, so a client can tell which of the two is wrong.
pub fn host_access_denied_message(host_arn: &str) -> String {
    format!(
        "Account {} does not have access to host {host_arn}",
        ::engine::emulator_account_id()
    )
}

/// The ARN of a connection: `arn:aws:codestar-connections:<region>:<account>:connection/<id>`.
pub fn connection_arn(region: &str, connection_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("connection/{connection_id}"),
    )
}

/// The ARN of a repository link:
/// `arn:aws:codestar-connections:<region>:<account>:repository-link/<id>` — the form the
/// modeled `RepositoryLinkArn` pattern requires
/// (`^arn:aws(?:-[a-z]+)*:codestar-connections:[a-z\-0-9]+:\d{12}:repository-link\/[a-zA-Z0-9\-:/]+$`).
pub fn repository_link_arn(region: &str, repository_link_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("repository-link/{repository_link_id}"),
    )
}

/// The ARN of a host: `arn:aws:codestar-connections:<region>:<account>:host/<id>` — the
/// modeled `HostArn` form `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`.
pub fn host_arn(region: &str, host_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("host/{host_id}"))
}

/// How this service reports a `HostArn` that resolves to no host it holds.
///
/// Shared by every operation that resolves one (`DeleteHost`, `GetHost`, and the
/// `CreateConnection` / `ListConnections` host cross-references), so an absent host reads
/// the same whichever call found it absent. The message is built from the REQUEST — it
/// names the ARN that was supplied — which is the part of it that is a contract.
pub fn host_not_found_message(host_arn: &str) -> String {
    format!("No host exists with the ARN: {host_arn}")
}

/// How this service reports a sync blocker the request's (`Id`, `ResourceName`,
/// `SyncType`) triple does not address — an id never raised, or one raised for another
/// resource or sync type.
///
/// The message is built from the REQUEST: it names the id that was asked for, which is
/// the part of it a caller can act on. The template is the recorded one (corpus
/// `simparity-wire-aws-v1`, fixture
/// `codestar-connections-sync-status-and-blocker-error-paths#task#6`), down to the space
/// the live service leaves after the sentence.
pub fn sync_blocker_not_found_message(id: &str) -> String {
    format!("Sync blocker with id {id} does not exist ")
}

/// How this service refuses resolving a sync blocker that is not [`BLOCKER_STATUS_ACTIVE`].
///
/// `UpdateSyncBlocker` resolves an ACTIVE blocker; a blocker already RESOLVED has no
/// status left to move, so the request names a blocker it cannot apply to and is refused
/// with the operation's modeled `InvalidInputException` rather than reported as success.
/// The WORDING is the emulator's (AWS documents none); what is a contract is that the
/// request is refused and names the id.
pub fn sync_blocker_already_resolved_message(id: &str) -> String {
    format!("Sync blocker with id {id} is already resolved")
}

/// Encode a pagination cursor: the creation sequence the next page starts at.
///
/// Opaque to the caller (fixed-width hex), as `@paginated` tokens are — a client
/// must only hand it back unmodified.
pub fn encode_next_token(sequence: u64) -> String {
    format!("{sequence:016x}")
}

/// Decode a cursor produced by [`encode_next_token`]; `None` when it was not.
pub fn decode_next_token(token: &str) -> Option<u64> {
    if token.len() != 16 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(token, 16).ok()
}

// ---------------------------------------------------------------------------
// How this service WORDS a refusal.
//
// A refusal carries the FIXED template of the case it refuses, interpolated only from
// the request and from the resource the service already holds, so two handlers refusing
// the same case cannot word it differently.
//
// The refusals raised by the Git-sync component behind the repository-link surface carry
// one extra thing: the service framework appends its own tail to the message
// ([`with_framework_tail`]). The connection surface's refusals do not — what AWS is
// recorded to answer there is the bare sentence.
// ---------------------------------------------------------------------------

/// The service name the framework tail states.
///
/// NOT this service's own `sdkId` (`CodeStar connections`): the Git-sync component behind
/// the repository-link surface is the one CodeConnections service, and that is the name
/// the recorded wire of this endpoint carries in the tail — the recorded
/// `CreateRepositoryLink` duplicate refusal of
/// `codestar-connections-repository-link-error-paths#task#3` reads
/// `… (Service: CodeConnections; …)`.
const FRAMEWORK_TAIL_SERVICE: &str = "CodeConnections";

/// The HTTP status the Git-sync component raised a refusal with and the exception the
/// framework names for it in the tail — kept PAIRED so the two values a client parses
/// back out of the tail cannot drift from each other.
type FrameworkTailFacts = (u16, &'static str);

/// The status this service's recorded wire answers a duplicate repository link with:
/// HTTP 400, the awsJson1_0 client-error status, not the `@httpError 409` the
/// `ResourceAlreadyExistsException` shape declares (the recorded
/// `codestar-connections-repository-link-error-paths#task#3` expectation). The handler
/// passes it to `crate::scaffolded::Scaffolded::modeled_with_status`.
///
/// The status the TAIL states is a different number, and deliberately so: see
/// [`with_framework_tail`].
pub const REPOSITORY_LINK_ALREADY_EXISTS_STATUS: u16 = 400;

/// A resource the request would create that is already there, paired with the status the
/// Git-sync component raised it with — the `@httpError` the `ResourceAlreadyExistsException`
/// shape declares, which is what the tail states ([`with_framework_tail`]).
const TAIL_RESOURCE_ALREADY_EXISTS: FrameworkTailFacts = (409, "ResourceAlreadyExistsException");

/// The status this service's recorded wire answers a `RepositoryLinkId` that resolves to
/// no link with: HTTP 400, the awsJson1_0 client-error status, not the `@httpError 404`
/// the `ResourceNotFoundException` shape declares (the recorded
/// `codestar-connections-repository-link-error-paths#task#4` and `#teardown#7`
/// expectations — status 400 with `__type` `ResourceNotFoundException`). The same
/// endpoint answers every other repository-link refusal that way too (see
/// [`REPOSITORY_LINK_ALREADY_EXISTS_STATUS`]), and `GetHost` does the same for an absent
/// host. The handler passes this to `crate::scaffolded::Scaffolded::modeled_with_status`.
pub const REPOSITORY_LINK_NOT_FOUND_STATUS: u16 = 400;

/// A resource reference that resolves to nothing, paired with the status the Git-sync
/// component raised it with — the `@httpError` the `ResourceNotFoundException` shape
/// declares, which is what the tail states ([`with_framework_tail`]).
const TAIL_RESOURCE_NOT_FOUND: FrameworkTailFacts = (404, "ResourceNotFoundException");

/// `body` followed by the tail the service framework appends to the refusals its Git-sync
/// component raises:
/// ` (Service: CodeConnections; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)`.
///
/// The status the tail states is the one the Git-sync component RAISED the exception
/// with — the `@httpError` of the modelled shape — and not the status this endpoint
/// answers the call with: this surface is the legacy front for that component, so it
/// reports the inner failure verbatim while answering its own client-error status
/// ([`REPOSITORY_LINK_NOT_FOUND_STATUS`]). The recorded wire of this endpoint carries
/// exactly that pairing: HTTP 400 with `Status Code: 404` in the tail of an absent link
/// (`codestar-connections-repository-link-error-paths#teardown#7`) and `Status Code: 409`
/// in the tail of a duplicate (`#task#3`).
///
/// The request id is the only per-run field: it is minted per call by the service —
/// nothing in the request carries one — from the emulator's one request-id mint
/// (`service_common::query_error_envelope::new_request_id`, a fresh v4 UUID), so it is
/// indistinguishable from the one the framework's own renderers report.
fn with_framework_tail(body: &str, facts: FrameworkTailFacts) -> String {
    let (status_code, error_code) = facts;
    format!(
        "{body} (Service: {FRAMEWORK_TAIL_SERVICE}; Status Code: {status_code}; \
         Error Code: {error_code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// What `CreateRepositoryLink` answers when the (connection, owner, repository) triple it
/// was asked to register is already registered: the ARN of the link that holds it, so a
/// caller can adopt that link, plus the framework tail.
///
/// The ARN is the STORED one, never one rebuilt from the request.
pub fn repository_link_already_exists_message(existing_repository_link_arn: &str) -> String {
    with_framework_tail(
        &format!("RepoConfiguration already exists. Arn: {existing_repository_link_arn}"),
        TAIL_RESOURCE_ALREADY_EXISTS,
    )
}

/// What the repository-link surface answers for a `RepositoryLinkId` that resolves to no
/// link this account holds: the account the lookup was made in and the id that resolved
/// to nothing, plus the framework tail.
///
/// Stated once here so every operation that addresses a link by its id words an absent
/// link the same way, whichever call found it absent. Built from the REQUEST — it names
/// the id that was supplied — which is the part of it that is a contract; the status
/// (400) and the wire code (`ResourceNotFoundException`) are the rest of it. The recorded
/// wire of this endpoint carries the sentence in TWO casings of the word uuid for the
/// very same refusal (`…#task#4` records `UUID`, `…#teardown#7` records `uuid`), so the
/// exact wording is an example of what the service answers rather than a rule it
/// follows; this is the one of the two forms the sibling CodeConnections service — the
/// same API under its current name — also words it in.
pub fn repository_link_not_found_message(repository_link_id: &str) -> String {
    with_framework_tail(
        &format!(
            "Repository not found for accountId: {} and UUID: {repository_link_id}",
            ::engine::emulator_account_id()
        ),
        TAIL_RESOURCE_NOT_FOUND,
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodestarconnectionsCodestarConnections20191201State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodestarconnectionsCodestarConnections20191201State>)
}
