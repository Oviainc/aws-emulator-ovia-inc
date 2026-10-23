// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeconnections-codeconnections-20231201 service.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Connection, Host, RepositoryLinkInfo, RepositorySyncDefinition, ResourceSyncAttempt, Revision,
    SyncBlocker, SyncBlockerContext, SyncBlockerSummary, SyncConfiguration, Tag, VpcConfiguration,
};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. `emulator_arn`'s own documentation gives `us-east-1` as the
/// emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The status a connection minted through the API, CLI or an SDK carries.
///
/// From the `Connection` shape's own documentation: "A connection created through
/// CloudFormation, the CLI, or the SDK is in `PENDING` status by default. You can
/// make its status `AVAILABLE` by updating the connection in the console." The
/// handshake that moves it is a console action, not something the clock owes, so a
/// connection this service mints stays `PENDING` until something updates it.
pub const CONNECTION_STATUS_ON_CREATE: &str = "PENDING";

/// The status a host minted through the API, CLI or an SDK carries.
///
/// From `CreateHost`'s own documentation: "A host created through the CLI or the SDK
/// is in `PENDING` status by default. You can make its status `AVAILABLE` by setting
/// up the host in the console." Setting the host up is a console action, not
/// something the clock owes, so a host this service mints stays `PENDING` until
/// something updates it.
pub const HOST_STATUS_ON_CREATE: &str = "PENDING";

/// The largest page a paginated report of this service renders, whatever the caller
/// asked for.
///
/// This is the published maximum of the modeled `MaxResults` member (`range 0..100`,
/// from `smithy_query(query="operation", operation="ListConnections")` and
/// `operation="ListSyncConfigurations"`, which agree), applied server-side: a caller
/// that asks for more, or that omits `MaxResults` entirely (the member is modeled
/// `default: 0`), still gets at most this many items in one response and a `NextToken`
/// for the rest. Without the clamp a one-line request would render the whole store in a
/// single response.
pub const MAX_PAGE_SIZE: usize = 100;

/// The largest number of connections this emulator stores for the account.
///
/// A CHOSEN bound, not a quota read off any AWS document: the store lives in the
/// emulator's process, so the number of records a caller can accumulate has to be
/// capped somewhere. `CreateConnection` refuses past it with the modeled
/// `LimitExceededException` (429, "Exceeded the maximum limit for connections."),
/// which is the error the operation already declares for exactly this.
pub const MAX_CONNECTIONS: usize = 1000;

/// The largest number of hosts this emulator stores for the account.
///
/// A CHOSEN bound, exactly like [`MAX_CONNECTIONS`] and for the same reason: the store
/// lives in the emulator's process. `CreateHost` refuses past it with the modeled
/// `LimitExceededException` (429), the only error that operation declares.
pub const MAX_HOSTS: usize = 1000;

/// The largest number of repository links this emulator stores for the account.
///
/// A CHOSEN bound, exactly like [`MAX_CONNECTIONS`] and for the same reason: the store
/// lives in the emulator's process. `CreateRepositoryLink` refuses past it with the
/// modeled `LimitExceededException` (429), which it declares for exactly this.
pub const MAX_REPOSITORY_LINKS: usize = 1000;

/// The largest number of sync configurations this emulator stores for the account.
///
/// A CHOSEN bound, exactly like [`MAX_CONNECTIONS`]. `CreateSyncConfiguration` refuses
/// past it with the modeled `LimitExceededException` (429), which it declares.
pub const MAX_SYNC_CONFIGURATIONS: usize = 1000;

/// A connection as this service persists it.
///
/// One record per `ConnectionArn`; every member of the modeled `Connection` shape
/// that the API can observe is stored here, so `GetConnection` and
/// `ListConnections` both render from the same row rather than recomputing one.
#[derive(Debug, Clone)]
pub struct StoredConnection {
    /// Unique within the account — see [`ComAmazonawsCodeconnectionsCodeconnections20231201State::connection_name_taken`].
    pub connection_name: String,
    pub connection_arn: String,
    /// `None` when the create call named no provider: the member is optional.
    pub provider_type: Option<String>,
    /// The 12-digit account that owns the connection.
    pub owner_account_id: String,
    /// One of the modeled `ConnectionStatus` values (`PENDING`|`AVAILABLE`|`ERROR`).
    pub connection_status: String,
    /// The host the connection is associated with, when it has one.
    pub host_arn: Option<String>,
    /// The tags supplied at create time, in the order they were supplied.
    pub tags: Vec<Tag>,
}

impl StoredConnection {
    /// Render the stored row as the modeled `Connection` structure.
    ///
    /// The single projection from state onto the wire shape, so every reader
    /// (`GetConnection`, `ListConnections`) reports the same connection the same way.
    pub fn to_modeled(&self) -> Connection {
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

/// A host as this service persists it.
///
/// One record per `HostArn`; every member of the modeled `Host` shape that the API
/// can observe is stored here, so `GetHost` (and any later reader) renders from the
/// same row rather than recomputing one.
#[derive(Debug, Clone)]
pub struct StoredHost {
    pub name: String,
    pub host_arn: String,
    /// One of the modeled `ProviderType` values; `CreateHost` models it required.
    pub provider_type: String,
    /// The endpoint of the infrastructure the host represents.
    pub provider_endpoint: String,
    /// The VPC configuration provisioned for the host, when the create supplied one.
    pub vpc_configuration: Option<VpcConfiguration>,
    /// The modeled `HostStatus` (free-form 1..64; the shape's doc lists PENDING,
    /// AVAILABLE, VPC_CONFIG_DELETING, VPC_CONFIG_INITIALIZING,
    /// VPC_CONFIG_FAILED_INITIALIZATION).
    pub status: String,
    /// The status description, when the service has one to report.
    pub status_message: Option<String>,
    /// The tags supplied at create time, in the order they were supplied.
    pub tags: Vec<Tag>,
}

impl StoredHost {
    /// Render the stored row as the modeled `Host` structure: the single projection
    /// from state onto the wire shape, so every reader reports the same host the
    /// same way.
    pub fn to_modeled(&self) -> Host {
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
}

/// A repository link as this service persists it.
///
/// One record per `RepositoryLinkId`; every member of the modeled `RepositoryLinkInfo`
/// shape is stored here, so `GetRepositoryLink`, `CreateRepositoryLink` and
/// `UpdateRepositoryLink` all render from the same row.
#[derive(Debug, Clone)]
pub struct StoredRepositoryLink {
    /// The link's id: the 8-4-4-4-12 hex label the modeled `RepositoryLinkId` pattern
    /// requires, and the key of [`ComAmazonawsCodeconnectionsCodeconnections20231201State::repository_links`].
    pub repository_link_id: String,
    pub repository_link_arn: String,
    /// The connection Git sync reaches the repository through.
    pub connection_arn: String,
    pub encryption_key_arn: Option<String>,
    /// The repository owner at the provider (the GitHub owner, say).
    pub owner_id: String,
    /// DERIVED from the connection, never taken from the request: the modeled
    /// `RepositoryLinkInfo.ProviderType` is "The provider type for the connection […]
    /// associated with the repository link", so it is whatever the connection names and
    /// it follows the connection when `UpdateRepositoryLink` replaces it.
    pub provider_type: String,
    pub repository_name: String,
    /// The tags supplied at create time, in the order they were supplied.
    pub tags: Vec<Tag>,
}

impl StoredRepositoryLink {
    /// Render the stored row as the modeled `RepositoryLinkInfo` structure: the single
    /// projection from state onto the wire shape, so every reader reports the same link
    /// the same way.
    pub fn to_modeled(&self) -> RepositoryLinkInfo {
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

/// A sync configuration as this service persists it.
///
/// One record per `(SyncType, ResourceName)` — the pair every sync-configuration
/// operation addresses it by (`GetSyncConfiguration`, `UpdateSyncConfiguration`,
/// `DeleteSyncConfiguration` all take exactly those two members).
#[derive(Debug, Clone)]
pub struct StoredSyncConfiguration {
    /// One of the modeled `SyncConfigurationType` values (`CFN_STACK_SYNC`).
    pub sync_type: String,
    /// The Amazon Web Services resource the configuration syncs.
    pub resource_name: String,
    pub branch: String,
    pub config_file: Option<String>,
    /// DERIVED from the repository link, like `repository_name` and `provider_type`:
    /// the modeled members describe the repository the link names, so they are
    /// re-derived whenever the configuration's `RepositoryLinkId` changes.
    pub owner_id: String,
    pub provider_type: String,
    pub repository_link_id: String,
    pub repository_name: String,
    pub role_arn: String,
    pub publish_deployment_status: Option<String>,
    pub trigger_resource_update_on: Option<String>,
    pub pull_request_comment: Option<String>,
}

impl StoredSyncConfiguration {
    /// Render the stored row as the modeled `SyncConfiguration` structure.
    pub fn to_modeled(&self) -> SyncConfiguration {
        SyncConfiguration {
            branch: self.branch.clone(),
            config_file: self.config_file.clone(),
            owner_id: self.owner_id.clone(),
            provider_type: self.provider_type.clone(),
            repository_link_id: self.repository_link_id.clone(),
            repository_name: self.repository_name.clone(),
            resource_name: self.resource_name.clone(),
            role_arn: self.role_arn.clone(),
            sync_type: self.sync_type.clone(),
            publish_deployment_status: self.publish_deployment_status.clone(),
            trigger_resource_update_on: self.trigger_resource_update_on.clone(),
            pull_request_comment: self.pull_request_comment.clone(),
        }
    }

    /// Render the stored row as the modeled `RepositorySyncDefinition` structure: the
    /// definition of the repository branch this configuration syncs, which is what
    /// `ListRepositorySyncDefinitions` reports.
    ///
    /// Every member is read off the SAME stored row `to_modeled` renders, so the two
    /// reports of one configuration can never disagree:
    ///
    /// * `Branch` is the configuration's branch — the `RepositorySyncDefinition` shape's
    ///   own documentation calls it "the branch specified for a repository sync
    ///   definition".
    /// * `Directory` is the configuration's config file: "This value comes from creating
    ///   or updating the `config-file` field of a `sync-configuration`". The member is
    ///   modeled required while `SyncConfiguration.ConfigFile` is optional, so a
    ///   configuration created without one reports the empty string rather than a value
    ///   nothing supplied.
    /// * `Parent` and `Target` are the synced resource, and they are EQUAL: the `Target`
    ///   member's documentation says "In some cases, such as CFN_STACK_SYNC, the parent
    ///   and target resource are the same", and `CFN_STACK_SYNC` is the only value the
    ///   modeled `SyncConfigurationType` enum admits.
    pub fn to_sync_definition(&self) -> RepositorySyncDefinition {
        RepositorySyncDefinition {
            branch: self.branch.clone(),
            directory: self.config_file.clone().unwrap_or_default(),
            parent: self.resource_name.clone(),
            target: self.resource_name.clone(),
        }
    }
}

/// A sync blocker as this service persists it.
///
/// A blocker is minted by the SERVICE when a sync it performed is blocked (the
/// `SyncBlocker` shape carries the reason and the instant it was created, and
/// `BlockerType` has the single value `AUTOMATED`), never by a caller: this API has no
/// create-blocker operation. `UpdateSyncBlocker` resolves one by id, and
/// `GetSyncBlockerSummary` reports the ones a resource holds.
#[derive(Debug, Clone)]
pub struct StoredSyncBlocker {
    /// The blocker's id, and the key of
    /// [`ComAmazonawsCodeconnectionsCodeconnections20231201State::sync_blockers`].
    pub id: String,
    /// The sync type and resource the blocker belongs to: `UpdateSyncBlocker` names
    /// both beside the id, so a blocker found by id is checked against them.
    pub sync_type: String,
    pub resource_name: String,
    pub parent_resource_name: Option<String>,
    /// One of the modeled `BlockerType` values; see `types::BLOCKER_TYPE_VALUES`.
    pub blocker_type: String,
    /// One of the modeled `BlockerStatus` values; see `types::BLOCKER_STATUS_VALUES`.
    pub status: String,
    pub created_reason: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub contexts: Vec<SyncBlockerContext>,
    pub resolved_reason: Option<String>,
    pub resolved_at: Option<::aws_smithy_types::DateTime>,
}

impl StoredSyncBlocker {
    /// Render the stored row as the modeled `SyncBlocker` structure.
    pub fn to_modeled(&self) -> SyncBlocker {
        SyncBlocker {
            id: self.id.clone(),
            r#type: self.blocker_type.clone(),
            status: self.status.clone(),
            created_reason: self.created_reason.clone(),
            created_at: self.created_at,
            // The modeled member is a list the service omits when it has no context to
            // report, exactly like a connection's absent `HostArn`.
            contexts: if self.contexts.is_empty() {
                None
            } else {
                Some(self.contexts.clone())
            },
            resolved_reason: self.resolved_reason.clone(),
            resolved_at: self.resolved_at,
        }
    }
}

/// The sync state this service has recorded for one `(SyncType, ResourceName)` pair:
/// what `GetResourceSyncStatus` reports.
///
/// Only the SERVICE records a sync attempt, when it drives a resource from the Git
/// repository the pair is configured against — never a caller: this API has no
/// report-a-sync operation. Driving a resource means fetching that remote
/// repository's commits, which this emulator does not do (nothing here performs a Git
/// sync), so nothing puts a row here and every lookup misses — exactly like
/// [`ComAmazonawsCodeconnectionsCodeconnections20231201State::sync_blockers`]. That is
/// why `GetResourceSyncStatus` reports no sync state rather than inventing a revision,
/// a commit sha and an event list for work nothing ran.
#[derive(Debug, Clone, Default)]
pub struct StoredResourceSyncState {
    /// The revision the resource is being driven TO, when a sync has set one.
    pub desired_state: Option<Revision>,
    /// The latest attempt, successful or not.
    pub latest_sync: Option<ResourceSyncAttempt>,
    /// The latest attempt that succeeded.
    pub latest_successful_sync: Option<ResourceSyncAttempt>,
}

/// The key a sync configuration is stored under: the `(SyncType, ResourceName)` pair
/// every sync-configuration operation addresses.
///
/// One function so a writer and a reader can never disagree about the key's field
/// order.
pub fn sync_configuration_key(sync_type: &str, resource_name: &str) -> (String, String) {
    (sync_type.to_string(), resource_name.to_string())
}

/// Per-service state for com-amazonaws-codeconnections-codeconnections-20231201. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodeconnectionsCodeconnections20231201State {
    /// Every connection this account holds, keyed by its `ConnectionArn`.
    ///
    /// A `BTreeMap`, so `ListConnections` walks a total, stable order and a page
    /// token can name a position in it.
    pub connections: BTreeMap<String, StoredConnection>,
    /// How many connections this store has ever minted: the sequence number behind
    /// the next connection's resource id (see [`resource_id`]).
    ///
    /// It counts mints rather than live connections, so it never rewinds when one is
    /// deleted — which is what keeps a deleted connection's ARN from coming back.
    pub connections_minted: u64,
    /// Every host this account holds, keyed by its `HostArn`.
    ///
    /// A `BTreeMap` for the same reason as `connections`: a total, stable order a
    /// page token can name a position in.
    pub hosts: BTreeMap<String, StoredHost>,
    /// How many hosts this store has ever minted: the sequence number behind the next
    /// host's resource id (see [`resource_id`]). It counts mints, so it never rewinds.
    pub hosts_minted: u64,
    /// Every repository link this account holds, keyed by its `RepositoryLinkId`.
    ///
    /// A `BTreeMap` for the same reason as `connections`: a total, stable order every
    /// reader walks the same way.
    pub repository_links: BTreeMap<String, StoredRepositoryLink>,
    /// How many repository links this store has ever minted: the sequence number behind
    /// the next link's id (see [`resource_id`]). It counts mints, so it never rewinds
    /// and a deleted link's id and ARN never come back.
    pub repository_links_minted: u64,
    /// Every sync configuration this account holds, keyed by
    /// [`sync_configuration_key`] — the `(SyncType, ResourceName)` pair the API
    /// addresses one by.
    pub sync_configurations: BTreeMap<(String, String), StoredSyncConfiguration>,
    /// Every sync blocker this account holds, keyed by its blocker `Id`.
    ///
    /// Keyed by id ALONE because `UpdateSyncBlocker` resolves the id before the
    /// `(SyncType, ResourceName)` pair it is also given. Only the service mints a
    /// blocker, when a sync it performed is blocked; this emulator performs no Git sync
    /// (nothing fetches from or pushes to a remote repository), so nothing puts a row
    /// here and every lookup misses — which is why `UpdateSyncBlocker` answers the
    /// modeled `SyncBlockerDoesNotExistException` rather than inventing a blocker.
    pub sync_blockers: BTreeMap<String, StoredSyncBlocker>,
    /// The sync state recorded against each `(SyncType, ResourceName)` pair, keyed by
    /// [`sync_configuration_key`] — the same pair `GetResourceSyncStatus` addresses.
    ///
    /// Only a sync the service performed writes here; see [`StoredResourceSyncState`].
    pub resource_sync_states: BTreeMap<(String, String), StoredResourceSyncState>,
}

impl ComAmazonawsCodeconnectionsCodeconnections20231201State {
    /// Whether a connection with this name already exists in the account.
    ///
    /// The `Connection.ConnectionName` member documents "Connection names must be
    /// unique in an Amazon Web Services account", so this is the uniqueness check
    /// `CreateConnection` applies. It scans `connections` rather than keeping a
    /// second name index: one map is one source of truth, and a secondary index
    /// can only ever disagree with it.
    pub fn connection_name_taken(&self, name: &str) -> bool {
        self.connections
            .values()
            .any(|connection| connection.connection_name == name)
    }

    /// The blocker summary this service reports for `resource_name` under `sync_type`.
    ///
    /// The ONE projection from the blocker store onto the modeled `SyncBlockerSummary`,
    /// so every reader of a resource's blockers reports them the same way.
    /// `GetSyncBlockerSummary` "Returns a list of the most recent sync blockers" and the
    /// `LatestBlockers` member is "The latest events for a sync blocker summary", so the
    /// pair's blockers are reported MOST RECENT FIRST — by `CreatedAt`, with the blocker
    /// id breaking a tie, which makes the order total and stable instead of dependent on
    /// how the rows sit in the store. The list is always rendered, empty when the pair
    /// holds no blocker: this reports blocker EVENTS, not whether the resource exists,
    /// so "no blockers" is an answer rather than a missing member or a refusal.
    /// `ParentResourceName` comes from the most recently raised of those blockers, the
    /// rows that carry it; a pair with no blocker reports none.
    pub fn blocker_summary(&self, sync_type: &str, resource_name: &str) -> SyncBlockerSummary {
        let mut latest: Vec<&StoredSyncBlocker> = self
            .sync_blockers
            .values()
            .filter(|blocker| {
                blocker.sync_type == sync_type && blocker.resource_name == resource_name
            })
            .collect();
        // `DateTime` carries no total order of its own, so recency is compared on the
        // seconds-and-nanoseconds pair it exposes.
        let raised_at =
            |blocker: &StoredSyncBlocker| (blocker.created_at.secs(), blocker.created_at.subsec_nanos());
        latest.sort_by(|left, right| {
            raised_at(right)
                .cmp(&raised_at(left))
                .then_with(|| left.id.cmp(&right.id))
        });
        SyncBlockerSummary {
            resource_name: resource_name.to_string(),
            parent_resource_name: latest
                .first()
                .and_then(|blocker| blocker.parent_resource_name.clone()),
            latest_blockers: Some(
                latest
                    .into_iter()
                    .map(StoredSyncBlocker::to_modeled)
                    .collect(),
            ),
        }
    }

    /// The tags stored against the resource this ARN names, or `None` when the
    /// service holds no such resource.
    ///
    /// Every resource this service persists is taggable (`CreateConnection`,
    /// `CreateHost` and `CreateRepositoryLink` all take a `TagList`), so tag reporting
    /// resolves an ARN against each store rather than against connections alone. An ARN
    /// names at most one resource: the modeled patterns put the resource type in the ARN
    /// (`connection/…` vs `host/…` vs `repository-link/…`).
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&[Tag]> {
        self.connections
            .get(resource_arn)
            .map(|connection| connection.tags.as_slice())
            .or_else(|| self.hosts.get(resource_arn).map(|host| host.tags.as_slice()))
            .or_else(|| {
                // Keyed by id rather than by ARN, so this store is scanned: one map is
                // one source of truth, and a second ARN index could only disagree
                // with it.
                self.repository_links
                    .values()
                    .find(|link| link.repository_link_arn == resource_arn)
                    .map(|link| link.tags.as_slice())
            })
    }

    /// The repository link already registered for this connection, owner and
    /// repository, when there is one.
    ///
    /// A repository link IS that triple — the connection Git sync reaches the
    /// repository through, plus the repository it names — so registering the same
    /// triple twice is the duplicate `CreateRepositoryLink` refuses with the modeled
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
}

/// A fresh resource id for one of this service's resources: 32 hex characters grouped
/// the way a connection or host id is grouped (8-4-4-4-12), to go after
/// `<resource_kind>/` in its ARN.
///
/// UNIQUENESS comes from `sequence`, a per-kind counter this store never rewinds
/// ([`ComAmazonawsCodeconnectionsCodeconnections20231201State::connections_minted`],
/// [`ComAmazonawsCodeconnectionsCodeconnections20231201State::hosts_minted`]), so the
/// ARN built from it is never reused — the `ConnectionArn` member documents "The ARN is
/// never reused if the connection is deleted" — even after the deleted resource's name
/// is freed and taken again. `resource_kind` is mixed in so two kinds minted at the same
/// sequence cannot collide. The digest is what makes the id OPAQUE, the way this
/// project's other services derive their opaque labels (see
/// `service-com-amazonaws-mediapackagev2-mediapackagev2`'s `state::egress_domain`), so
/// the id carries neither the account's name nor a count of its resources; mixing in
/// the minting instant keeps two emulator runs from minting the same first id.
pub fn resource_id(resource_kind: &str, account_id: &str, region: &str, sequence: u64) -> String {
    let minted_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    for field in [
        resource_kind.as_bytes(),
        account_id.as_bytes(),
        region.as_bytes(),
        &sequence.to_le_bytes(),
        &minted_at.to_le_bytes(),
    ] {
        // Length-prefixed, so two different field lists cannot hash alike.
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field);
    }
    let digest = hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// Encode a page token naming the store position the page just rendered ended at: the
/// last connection's `ConnectionArn` for `ListConnections`, the last configuration's
/// `ResourceName` for `ListSyncConfigurations`.
///
/// The token is the hex of that key: opaque to a caller, and a POSITION in the store's
/// own key order rather than an offset into a snapshot, so resuming skips exactly the
/// keys at or before it however the map changed in between. The modeled next-token
/// members are 1..1024 (`NextToken`) and 1..2048 (`SharpNextToken`) characters matching
/// `^.*$`; an ARN is at most 256 characters and a `ResourceName` at most 100, so the hex
/// of either fits.
pub fn encode_page_token(last_returned_key: &str) -> String {
    hex::encode(last_returned_key.as_bytes())
}

/// Split an ordered, already-filtered selection into the page one response renders and
/// the token naming its end, given the `MaxResults` the caller asked for and the member
/// each item's store position is read from.
///
/// The ONE page bound and token rule every paginated report of this service applies, so
/// two list operations cannot drift in how they clamp a page or decide to stop:
/// `MaxResults` is modelled `range 0..100, default: 0`, so a caller that omits it
/// arrives with zero; neither zero nor an over-large value is read as "render the whole
/// store" — [`MAX_PAGE_SIZE`] is the service's own page size either way, and the
/// remainder is reported through the token. A token comes back only when items are
/// really left: the contract is that the caller stops when none does.
pub fn page_of<Item>(
    items: &[Item],
    max_results: i32,
    position: impl Fn(&Item) -> &str,
) -> (&[Item], Option<String>) {
    let page_size = if max_results > 0 {
        (max_results as usize).min(MAX_PAGE_SIZE)
    } else {
        MAX_PAGE_SIZE
    }
    .min(items.len());
    let (page, remaining) = items.split_at(page_size);
    let next_token = match (remaining.is_empty(), page.last()) {
        (false, Some(last)) => Some(encode_page_token(position(last))),
        _ => None,
    };
    (page, next_token)
}

/// Decode a token [`encode_page_token`] produced, or `None` when it is not one.
///
/// A token only names a position if it decodes to an ARN of the kind this service
/// mints, so anything else — a hand-written string, or a hex blob that happens to
/// decode to text — is rejected rather than silently read as a position.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    is_connection_arn(&decoded).then_some(decoded)
}

/// Decode a page token [`encode_page_token`] produced for a report positioned by
/// `HostArn` (`ListHosts`), or `None` when it is not one.
///
/// The same rule as [`decode_page_token`], applied to the other ARN-positioned report:
/// a token names a position only if it decodes to a host ARN of the kind this service
/// mints, so a connection ARN's token names no position in the host store either.
pub fn decode_host_arn_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    is_service_arn(&decoded, "host").then_some(decoded)
}

/// Decode a page token [`encode_page_token`] produced for a report positioned by
/// `ResourceName` (`ListSyncConfigurations`), or `None` when it is not one.
///
/// A token only names a position if it decodes to a value the API could have stored as
/// a `ResourceName`, so anything else — a hand-written string, or a hex blob that
/// decodes to text no resource name could be — is rejected rather than silently read
/// as a position.
pub fn decode_resource_name_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    is_resource_name(&decoded).then_some(decoded)
}

/// The published maximum length of the modeled `ResourceName` (`length 1..100`, from
/// `smithy_query(query="shape", shape="ResourceName")`).
const MAX_RESOURCE_NAME_LEN: usize = 100;

/// Whether `candidate` satisfies the modeled `ResourceName` constraints: length
/// 1..100 and pattern `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`, from
/// `smithy_query(query="shape", shape="ResourceName")` — the same constraints the
/// request walk applies to a `ResourceName` member, so this reader can never refuse a
/// token this service issued itself. The admitted alphabet is ASCII only, so the byte
/// length is the character length.
fn is_resource_name(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= MAX_RESOURCE_NAME_LEN
        && candidate.starts_with(|first: char| first.is_ascii_alphanumeric())
        && candidate
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

/// The partitions the modeled `ConnectionArn` pattern `^arn:aws(-[\w]+)*:…` admits,
/// as AWS publishes them: the commercial partition and the two isolated ones.
const ARN_PARTITIONS: &[&str] = &["aws", "aws-cn", "aws-us-gov"];

/// The longest region token this service will put in an ARN. AWS region codes are
/// far shorter; the cap only keeps a caller-supplied scope from growing an ARN past
/// the modeled `ConnectionArn` length of 256.
const MAX_REGION_LEN: usize = 64;

/// Whether `candidate` is shaped like an AWS region code: non-empty lowercase ASCII,
/// digits and hyphens (`us-east-1`, `eu-west-1`, `cn-north-1`), within
/// [`MAX_REGION_LEN`].
///
/// This is the ONE rule both the ARN minter ([`request_region`]) and the page-token
/// reader ([`is_connection_arn`]) apply, so this service can never refuse a token it
/// issued itself.
fn is_region_token(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= MAX_REGION_LEN
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Whether `candidate` has the shape of an ARN this service mints for `resource_type`:
/// `arn:<partition>:codeconnections:<region>:<12 digits>:<resource_type>/<id>`.
///
/// ONE rule for every resource type, so the ARN of one resource can never be read as
/// the ARN of another: the modeled patterns put the resource type in the ARN
/// (`connection/…` vs `host/…` vs `repository-link/…`), which is exactly the field
/// compared here.
fn is_service_arn(candidate: &str, resource_type: &str) -> bool {
    let fields: Vec<&str> = candidate.splitn(6, ':').collect();
    let prefix = format!("{resource_type}/");
    fields.len() == 6
        && fields[0] == "arn"
        && ARN_PARTITIONS.contains(&fields[1])
        && fields[2] == "codeconnections"
        && is_region_token(fields[3])
        && fields[4].len() == 12
        && fields[4].bytes().all(|byte| byte.is_ascii_digit())
        && fields[5].starts_with(&prefix)
        && fields[5].len() > prefix.len()
}

/// Whether `candidate` has the shape of a connection ARN this service mints:
/// `arn:<partition>:codeconnections:<region>:<12 digits>:connection/<id>`.
fn is_connection_arn(candidate: &str) -> bool {
    is_service_arn(candidate, "connection")
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it is unsigned
/// or the scope does not carry a region code.
///
/// The SigV4 credential scope is the only place a request states its region, so an
/// ARN this service mints is regional in exactly the region the caller addressed —
/// but the scope is caller-supplied text, and the value is load-bearing (it becomes
/// a field of every minted ARN, and the ARN is the pagination position). Only a value
/// shaped like a region code ([`is_region_token`]) is used; anything else falls back
/// to the default rather than being spliced into an ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodeconnectionsCodeconnections20231201State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodeconnectionsCodeconnections20231201State>)
}
