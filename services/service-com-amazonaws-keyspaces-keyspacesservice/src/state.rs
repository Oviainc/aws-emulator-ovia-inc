// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-keyspaces-keyspacesservice service.
//!
//! Amazon Keyspaces is a three-level namespace: an Amazon Web Services Region holds
//! keyspaces, and a keyspace holds tables and user-defined types. All three levels live
//! in the one [`ComAmazonawsKeyspacesKeyspacesserviceState`] below so every operation
//! reads and writes ONE copy of a resource, and the Region a request was signed for is
//! what selects the keyspaces it can see at all
//! ([`keyspaces`][ComAmazonawsKeyspacesKeyspacesserviceState::keyspaces]).
//!
//! Keyed by [`std::collections::BTreeMap`] at every level, because `ListKeyspaces`
//! and `ListTables` are `@paginated`: a page boundary is only resumable if the
//! iteration order is total and stable, which a hash map's is not.
//!
//! This module also holds what more than one operation needs — ARN construction,
//! the request's region, the modelled defaults, the [page window][page_window] every
//! `@paginated` listing reports through with the [token codec][encode_page_token] the
//! scope-qualified ones resume through, the [service-managed
//! catalog][SERVICE_MANAGED_KEYSPACES] every Region already holds (derived per request by
//! [`keyspace`][ComAmazonawsKeyspacesKeyspacesserviceState::keyspace]),
//! and [`settle`][ComAmazonawsKeyspacesKeyspacesserviceState::settle],
//! which materialises the status transitions the clock already owes — so the
//! per-operation files never carry a second copy of any of it.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AutoScalingPolicy, AutoScalingSettings, AutoScalingSpecification, CapacitySpecification,
    CdcSpecification, CdcSpecificationSummary, ClientSideTimestamps, Comment,
    EncryptionSpecification, FieldDefinition, PointInTimeRecovery, ReplicaSpecification,
    ResourceNotFoundException, SchemaDefinition, Tag,
    TargetTrackingScalingPolicyConfiguration, TimeToLive, WarmThroughputSpecification,
    WarmThroughputSpecificationSummary,
};

/// The CDC stream store, which the KeyspacesStreams service crate owns.
///
/// A table's change data capture stream is created by THIS control plane (`CreateTable`
/// and `UpdateTable`, from their `cdcSpecification`) and reported by that data plane
/// (`ListStreams`, `GetStream`, `GetShardIterator`, `GetRecords`), so there is ONE copy
/// of a stream and both sides read and write it: [`cdc_stream_state`] is that store's
/// handle in the shared engine, reached exactly the way [`service_state`] reaches this
/// service's own.
///
/// A handler that needs both locks takes THIS SERVICE'S state first and the stream store
/// second — one order everywhere, so two handlers can never wait on each other.
pub use ::service_com_amazonaws_keyspacesstreams_keyspacesstreams::state::{
    service_state as cdc_stream_state,
    ComAmazonawsKeyspacesstreamsKeyspacesstreamsState as CdcStreamStore,
    StreamEntry as CdcStreamEntry, MAX_STREAMS_PER_TABLE,
    STATUS_TRANSITION_DELAY as CDC_STATUS_TRANSITION_DELAY,
    STREAM_STATUS_DISABLED as CDC_STATUS_DISABLED,
    STREAM_STATUS_DISABLING as CDC_STATUS_DISABLING,
    STREAM_STATUS_ENABLED as CDC_STATUS_ENABLED,
    STREAM_STATUS_ENABLING as CDC_STATUS_ENABLING,
};

/// The `cassandra` ARN namespace / SigV4 signing name of Amazon Keyspaces
/// (`awsService.arnNamespace` in this service's Smithy model).
pub const ARN_NAMESPACE: &str = "cassandra";

/// The region an unsigned request is served in. A signed request carries its own
/// region in the SigV4 credential scope; see [`request_region`].
pub const DEFAULT_REGION: &str = "us-east-1";

/// `KeyspaceStatus` = `CREATING`: what `CreateKeyspace` puts a new keyspace in.
///
/// "`CreateKeyspace` is an asynchronous operation. You can monitor the creation
/// status of the new keyspace by using the `GetKeyspace` operation." Both
/// constants below are members of the model's `KeyspaceStatus` value set
/// (`crate::ops::types::KEYSPACE_STATUS_VALUES`).
pub const KEYSPACE_STATUS_CREATING: &str = "CREATING";
/// `KeyspaceStatus` = `ACTIVE`: the status a created keyspace settles into.
pub const KEYSPACE_STATUS_ACTIVE: &str = "ACTIVE";
/// `KeyspaceStatus` = `UPDATING`: what `UpdateKeyspace` puts a keyspace in until the
/// replication change it accepted completes.
///
/// `UpdateKeyspace` "adds a new Amazon Web Services Region to the keyspace. ... Amazon
/// Keyspaces is going to replicate all tables in the keyspace to the new Region", and
/// `GetKeyspace` reports the result as "the summary status of the keyspace after a new
/// Amazon Web Services Region was added" — so the addition is not instantaneous, and
/// `UPDATING` is the member of the model's `KeyspaceStatus` value set
/// (`crate::ops::types::KEYSPACE_STATUS_VALUES`) that names it.
pub const KEYSPACE_STATUS_UPDATING: &str = "UPDATING";

/// `KeyspaceStatus` = `DELETING`: what `DeleteKeyspace` puts a keyspace in until the
/// deletion completes, after which the keyspace — and every table and type it held — is
/// no longer a resource of the account.
///
/// A member of the model's `KeyspaceStatus` value set
/// (`crate::ops::types::KEYSPACE_STATUS_VALUES`), reported for a keyspace through
/// `ReplicationGroupStatus$keyspaceStatus` ("The status of the keyspace"), which
/// `GetKeyspace` answers with: the deletion the model gives a status to is one a caller
/// can still observe, so it is not instantaneous.
pub const KEYSPACE_STATUS_DELETING: &str = "DELETING";

/// `TableStatus` = `CREATING`: what `CreateTable` puts a new table in.
///
/// The three constants below are members of the model's `TableStatus` value set
/// (`crate::ops::types::TABLE_STATUS_VALUES`).
pub const TABLE_STATUS_CREATING: &str = "CREATING";
/// `TableStatus` = `ACTIVE`: the status a created table settles into.
pub const TABLE_STATUS_ACTIVE: &str = "ACTIVE";
/// `TableStatus` = `DELETING`: what `DeleteTable` puts a table in until the
/// deletion completes.
pub const TABLE_STATUS_DELETING: &str = "DELETING";
/// `TableStatus` = `UPDATING`: what `UpdateTable` puts a table in until the
/// setting change completes.
///
/// That `UpdateTable` is asynchronous and leaves the table in this status is what
/// `DeleteTable` documents from the other side: "If a table is either in the
/// `CREATING` or `UPDATING` states, then Amazon Keyspaces returns a
/// `ResourceInUseException`" — only an update can put a table in `UPDATING`.
pub const TABLE_STATUS_UPDATING: &str = "UPDATING";
/// `TableStatus` = `RESTORING`: what `RestoreTable` puts the table it is creating in
/// until the restore completes, after which it is [`TABLE_STATUS_ACTIVE`] like any
/// other table.
///
/// A member of the model's `TableStatus` value set
/// (`crate::ops::types::TABLE_STATUS_VALUES`) beside `CREATING`, and restoring is the
/// one way a table of this service comes into being that the plain create does not
/// cover — so this is the status a caller watching a restore through `GetTable$status`
/// observes while it runs, and the one that says a restore of this account is still
/// running
/// ([`restores_in_progress`][ComAmazonawsKeyspacesKeyspacesserviceState::restores_in_progress]).
pub const TABLE_STATUS_RESTORING: &str = "RESTORING";

/// `TypeStatus` = `CREATING`: what `CreateType` puts a new user-defined type in.
///
/// This and [`TYPE_STATUS_ACTIVE`] are members of the model's `TypeStatus` value
/// set (`crate::ops::types::TYPE_STATUS_VALUES`).
pub const TYPE_STATUS_CREATING: &str = "CREATING";
/// `TypeStatus` = `ACTIVE`: the status a created type settles into, and the only
/// one in which the type can be used (`ResourceNotFoundException`: "The resource
/// might not be specified correctly, or its status might not be ACTIVE").
pub const TYPE_STATUS_ACTIVE: &str = "ACTIVE";
/// `TypeStatus` = `DELETING`: what `DeleteType` puts a type in until the deletion
/// completes, after which the type is no longer a resource of its keyspace.
///
/// A member of the model's `TypeStatus` value set
/// (`crate::ops::types::TYPE_STATUS_VALUES`), which the service reports for a type
/// through `GetType`: the deletion the model gives a status to is one a caller can
/// still observe, so it is not instantaneous.
pub const TYPE_STATUS_DELETING: &str = "DELETING";

/// How long a resource stays in a transient status before the clock owes it the
/// next one: `CREATING` -> `ACTIVE` after `CreateTable`, `CreateKeyspace` or
/// `CreateType`, `RESTORING` -> `ACTIVE` after `RestoreTable`, `UPDATING` ->
/// `ACTIVE` after `UpdateTable` or `UpdateKeyspace`, and `DELETING` -> gone after
/// `DeleteTable`, `DeleteType` or `DeleteKeyspace`.
///
/// CHOSEN, not documented: Amazon Keyspaces publishes no duration for any of
/// these transitions, only that they happen (`CreateTable`: "the status of the
/// table is set to `CREATING`. You can start using a table when the status is
/// `ACTIVE`"; `CreateKeyspace`: "`CreateKeyspace` is an asynchronous operation.
/// You can monitor the creation status of the new keyspace by using the
/// `GetKeyspace` operation"; `DeleteTable`: "the specified table is in the
/// `DELETING` state until Amazon Keyspaces completes the deletion"; `TypeStatus`
/// models `CREATING` and `GetType` reports it). It is
/// deliberately NON-ZERO, so a caller that reads straight after the call still
/// observes the transient status the model documents, and short enough that a
/// test can wait it out.
pub const TRANSIENT_STATUS_SPAN: Duration = Duration::from_millis(500);

/// `ThroughputMode` = `PAY_PER_REQUEST`: the on-demand mode, which takes no capacity
/// units.
pub const PAY_PER_REQUEST_THROUGHPUT_MODE: &str = "PAY_PER_REQUEST";

/// The throughput mode a table gets when `CreateTable` carries no
/// `capacitySpecification` ("The default is `throughput_mode:PAY_PER_REQUEST`",
/// `CapacitySpecification$throughputMode`).
pub const DEFAULT_THROUGHPUT_MODE: &str = PAY_PER_REQUEST_THROUGHPUT_MODE;

/// The throughput mode that requires both capacity units as input.
pub const PROVISIONED_THROUGHPUT_MODE: &str = "PROVISIONED";

/// The encryption type a table gets when `CreateTable` carries no
/// `encryptionSpecification` ("The default is `type:AWS_OWNED_KMS_KEY`",
/// `EncryptionSpecification$type`).
pub const DEFAULT_ENCRYPTION_TYPE: &str = "AWS_OWNED_KMS_KEY";

/// The encryption type that requires `kmsKeyIdentifier` as input.
pub const CUSTOMER_MANAGED_ENCRYPTION_TYPE: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// The view type a table's CDC stream publishes when the `cdcSpecification` names none:
/// "`NEW_AND_OLD_IMAGES` - both versions of the row, before and after the change. This
/// is the default." (`CdcSpecification$viewType`).
pub const DEFAULT_CDC_VIEW_TYPE: &str = "NEW_AND_OLD_IMAGES";

/// `CdcPropagateTags` = `TABLE`: the value that makes a table's CDC stream inherit the
/// table's tags (`CdcSpecification$propagateTags`, "Specifies that the stream inherits
/// the tags from the table"). The enum's other value, `NONE`, inherits nothing, which
/// is also what an absent member asks for.
pub const CDC_PROPAGATE_TAGS_TABLE: &str = "TABLE";

/// Point-in-time recovery is off unless `CreateTable` asks for it.
pub const DEFAULT_POINT_IN_TIME_RECOVERY_STATUS: &str = "DISABLED";

/// The lowest `targetValue` a target tracking auto scaling policy may be given:
/// "You define `targetValue` as a percentage. A `double` between 20 and 90"
/// (`TargetTrackingScalingPolicyConfiguration$targetValue`).
pub const MIN_TARGET_VALUE: f64 = 20.0;

/// The highest `targetValue` of the same published range. See [`MIN_TARGET_VALUE`].
pub const MAX_TARGET_VALUE: f64 = 90.0;

/// `PointInTimeRecoveryStatus` = `ENABLED`.
pub const POINT_IN_TIME_RECOVERY_ENABLED: &str = "ENABLED";

/// How far back point-in-time recovery reaches: the restorable window `RestoreTable`
/// restores "within the `earliest_restorable_timestamp` and the current time", which
/// the Amazon Keyspaces "Time window for PITR continuous backups" that operation
/// points at publishes as the preceding 35 days.
///
/// So an instant older than this is outside every table's restorable window, whatever
/// the store still holds for it.
pub const POINT_IN_TIME_RECOVERY_WINDOW: Duration = Duration::from_secs(35 * 24 * 60 * 60);

/// How many restores may run in this account at once: "Any number of users can execute
/// up to 4 concurrent restores (any type of restore) in a given account"
/// (`RestoreTable`). A restore is running while the table it is creating is still
/// `RESTORING`; see
/// [`restores_in_progress`][ComAmazonawsKeyspacesKeyspacesserviceState::restores_in_progress].
pub const MAX_CONCURRENT_RESTORES: usize = 4;

/// How many superseded revisions of a table's point-in-time-restorable settings
/// ([`TableRevision`]) are kept.
///
/// CHOSEN, not documented: every accepted update that changes one of those settings
/// appends a revision, and only the window it falls out of removes one, so the list is
/// capped to keep a long-lived store bounded — the same ground every other `MAX_` here
/// stands on. Once the cap has dropped a revision, the oldest retained one's boundary
/// is the earliest instant whose settings are still known, and
/// [`Table::earliest_restorable_timestamp`] reports exactly that, so a restore the
/// store can no longer reproduce is refused rather than answered from the wrong
/// instant.
pub const MAX_RETAINED_TABLE_REVISIONS: usize = 64;

/// `WarmThroughputStatus` = `AVAILABLE`: "the current status of the warm throughput
/// configuration. Valid values are `AVAILABLE` when the configuration is active, and
/// `UPDATING` when changes are being applied"
/// (`WarmThroughputSpecificationSummary$status`). Both constants are members of the
/// model's `WarmThroughputStatus` value set
/// (`crate::ops::types::WARM_THROUGHPUT_STATUS_VALUES`).
pub const WARM_THROUGHPUT_STATUS_AVAILABLE: &str = "AVAILABLE";
/// `WarmThroughputStatus` = `UPDATING`: the status a warm throughput configuration holds
/// while the change that set it is still being applied. See
/// [`WARM_THROUGHPUT_STATUS_AVAILABLE`].
pub const WARM_THROUGHPUT_STATUS_UPDATING: &str = "UPDATING";

/// The read capacity a table in on-demand (`PAY_PER_REQUEST`) capacity mode can serve
/// without being pre-warmed: Amazon Keyspaces documents that such a table "can serve up
/// to 4,000 write request units per second and 12,000 read request units per second"
/// before it has to scale, which is the warm throughput every on-demand table already
/// has when nothing ever asked it to pre-warm more.
///
/// So this and [`DEFAULT_ON_DEMAND_WARM_WRITE_UNITS_PER_SECOND`] are the baseline
/// [`Table::reported_warm_throughput`] reports for an on-demand table with no
/// configuration of its own; a provisioned table's baseline is its own provisioned
/// capacity, which is what IT can serve without scaling.
pub const DEFAULT_ON_DEMAND_WARM_READ_UNITS_PER_SECOND: i64 = 12_000;

/// The write half of the same documented baseline. See
/// [`DEFAULT_ON_DEMAND_WARM_READ_UNITS_PER_SECOND`].
pub const DEFAULT_ON_DEMAND_WARM_WRITE_UNITS_PER_SECOND: i64 = 4_000;

/// `rs` (replication strategy) = `SINGLE_REGION`: a keyspace replicated only in
/// the region it was created in, and the strategy a keyspace gets when
/// `CreateKeyspace` carries no `replicationSpecification`.
pub const SINGLE_REGION_STRATEGY: &str = "SINGLE_REGION";

/// `rs` (replication strategy) = `MULTI_REGION`.
pub const MULTI_REGION_STRATEGY: &str = "MULTI_REGION";

/// The service-managed keyspaces every Amazon Keyspaces account holds in every Region
/// before any client call, each with the system tables it holds.
///
/// Amazon Keyspaces is Cassandra-compatible, and a Cassandra cluster's schema and node
/// metadata live in keyspaces the SERVICE owns: a driver resolves a keyspace, a table or
/// a type by reading them, so they exist in an account that has created nothing. No
/// `CreateKeyspace` ever makes one, so a Region's are derived from this catalog on read
/// ([`ComAmazonawsKeyspacesKeyspacesserviceState::keyspace`]) rather than created by a
/// caller, and [`is_service_managed_keyspace`] is what keeps every operation
/// that WRITES off them.
///
/// WHICH keyspaces exist is the one part of this the Smithy model does not state at all
/// (it constrains only the shape of a `keyspaceName`), so the four names are the catalog
/// a `ListKeyspaces` on an untouched account reports, and each keyspace's table list is
/// the catalog that keyspace publishes:
///
/// * `system_schema`: the Cassandra schema catalog, whose tables are the ten schema
///   views Apache Cassandra defines (`keyspaces`, `tables`, `columns`, `types`,
///   `dropped_columns`, `triggers`, `views`, `functions`, `aggregates`, `indexes`).
/// * `system_schema_mcs`: Amazon Keyspaces' own copy of that catalog — the schema views
///   it serves (`SELECT * FROM system_schema_mcs.keyspaces`, `... .tables`,
///   `... .columns` in the Amazon Keyspaces Developer Guide), the history view of the
///   same table catalog (`tables_history`), plus the two Keyspaces-specific views the
///   same guide reads a table's AWS settings out of, `tags` and `autoscaling`.
/// * `system`: the node-local and cluster-local metadata a Cassandra driver reads, which
///   is the whole Cassandra `system` keyspace, not only the `local`/`peers` pair a
///   connect handshake reads.
/// * `system_multiregion_info`: the multi-Region replication status catalog, read as
///   `SELECT * FROM system_multiregion_info.tables` in the Developer Guide's
///   multi-Region section, beside that keyspace's other per-Region views.
///
/// Ordered as a [`std::collections::BTreeMap`] would order it, so the seed is written in
/// the one total order every listing reports.
pub const SERVICE_MANAGED_KEYSPACES: &[(&str, &[&str])] = &[
    ("system", SYSTEM_TABLES),
    ("system_multiregion_info", SYSTEM_MULTIREGION_INFO_TABLES),
    ("system_schema", SYSTEM_SCHEMA_TABLES),
    ("system_schema_mcs", SYSTEM_SCHEMA_MCS_TABLES),
];

/// The tables of the `system` keyspace: the whole Cassandra system catalog Amazon
/// Keyspaces publishes, which is what a recorded `ListTables` on an untouched account
/// reports for it — the `local` and `peers` tables a driver reads on connect are two
/// entries of it, not the catalog.
///
/// The other entries are the rest of the Cassandra `system` keyspace: the index and
/// view build records (`IndexInfo`, `built_views`, `views_builds_in_progress`), the
/// write-path records (`batches`, `paxos`), the streaming and range records
/// (`available_ranges`, `range_xfers`, `transferred_ranges`), the node-event and
/// statistics records (`peer_events`, `compaction_history`, `sstable_activity`,
/// `size_estimates`) and the prepared-statement cache (`prepared_statements`). The
/// pre-3.0 write-path tables (`batchlog`, `hints`) are deliberately NOT here: the
/// recorded catalog carries neither. See [`SERVICE_MANAGED_KEYSPACES`].
pub const SYSTEM_TABLES: &[&str] = &[
    "IndexInfo",
    "available_ranges",
    "batches",
    "built_views",
    "compaction_history",
    "local",
    "paxos",
    "peer_events",
    "peers",
    "prepared_statements",
    "range_xfers",
    "size_estimates",
    "sstable_activity",
    "transferred_ranges",
    "views_builds_in_progress",
];

/// The tables of the `system_multiregion_info` keyspace: the per-Region views of the
/// multi-Region catalog — the replication status of a keyspace's `tables`, of the
/// `keyspaces` themselves and of their `types`, plus the per-Region `autoscaling`
/// settings. See [`SERVICE_MANAGED_KEYSPACES`].
pub const SYSTEM_MULTIREGION_INFO_TABLES: &[&str] =
    &["autoscaling", "keyspaces", "tables", "types"];

/// The tables of the `system_schema` keyspace: Cassandra's ten schema views, which is
/// what a recorded `ListTables` on an untouched account reports for it. See
/// [`SERVICE_MANAGED_KEYSPACES`].
pub const SYSTEM_SCHEMA_TABLES: &[&str] = &[
    "aggregates",
    "columns",
    "dropped_columns",
    "functions",
    "indexes",
    "keyspaces",
    "tables",
    "triggers",
    "types",
    "views",
];

/// The tables of the `system_schema_mcs` keyspace: the schema views Amazon Keyspaces
/// serves in its own copy of the catalog, the `tables_history` view of the table
/// catalog, plus the Keyspaces-specific `tags` and `autoscaling` views.
/// `dropped_columns` is deliberately NOT here: it is a `system_schema` view that the
/// recorded `system_schema_mcs` catalog does not carry.
/// See [`SERVICE_MANAGED_KEYSPACES`].
pub const SYSTEM_SCHEMA_MCS_TABLES: &[&str] = &[
    "aggregates",
    "autoscaling",
    "columns",
    "functions",
    "indexes",
    "keyspaces",
    "tables",
    "tables_history",
    "tags",
    "triggers",
    "types",
    "views",
];

/// Whether `keyspace_name` names one of the [`SERVICE_MANAGED_KEYSPACES`].
///
/// A service-managed keyspace is READ-ONLY to a client: Cassandra refuses DDL against
/// one as an invalid query ("system keyspace is not user-modifiable"), which is this
/// service's `ValidationException` — "The operation failed due to an invalid or
/// malformed request". Every operation that creates, updates or deletes inside a
/// keyspace asks this first.
pub fn is_service_managed_keyspace(keyspace_name: &str) -> bool {
    SERVICE_MANAGED_KEYSPACES
        .iter()
        .any(|(managed, _)| *managed == keyspace_name)
}

/// The refusal message a write against the service-managed keyspace `keyspace_name`
/// carries: ONE wording, so every operation refuses it the same way.
pub fn service_managed_rejection(keyspace_name: &str) -> String {
    format!(
        "Keyspace '{keyspace_name}' is managed by Amazon Keyspaces and cannot be \
         modified."
    )
}

/// The refusal message a request that would add a resource to — or otherwise change — a
/// keyspace whose deletion is already in progress carries: ONE wording, so every
/// operation refuses it the same way.
///
/// The caller's recourse is to wait for that deletion to finish, so the status it is
/// waiting on is in the message.
pub fn keyspace_deleting_rejection(keyspace_name: &str) -> String {
    format!(
        "Keyspace '{keyspace_name}' is in status {KEYSPACE_STATUS_DELETING} and takes no \
         further change until its deletion completes."
    )
}

/// The same rule one level down: the refusal a request that would change a TABLE whose
/// deletion is already in progress carries, worded exactly as
/// [`keyspace_deleting_rejection`] words it for a keyspace, so a caller reads one sentence
/// whichever resource is on its way out.
///
/// The table is named the way every refusal of this service names one, `<keyspace>.<table>`,
/// and the status the caller is waiting on is in the message.
pub fn table_deleting_rejection(keyspace_name: &str, table_name: &str) -> String {
    format!(
        "Table '{keyspace_name}.{table_name}' is in status {TABLE_STATUS_DELETING} and takes \
         no further change until its deletion completes."
    )
}

/// The refusal a restore whose SOURCE table is not settled carries: a table is restorable
/// only from [`TABLE_STATUS_ACTIVE`], and the status the caller is waiting on is in the
/// message, worded as the rejections above word a status that blocks a request.
///
/// ONE wording, so the check a restore makes before it reads the source table and the one
/// it makes again under the guard that creates the restored table refuse alike.
pub fn table_not_restorable_rejection(
    keyspace_name: &str,
    table_name: &str,
    status: &str,
) -> String {
    format!(
        "Table '{keyspace_name}.{table_name}' is in status {status} and cannot be restored \
         until it is {TABLE_STATUS_ACTIVE}."
    )
}

/// Whether `status` is one whose own change is still in flight: the resource is neither
/// settled nor on its way out, so the clock still owes it a transition.
///
/// `CREATING` and `UPDATING` have the same spelling in the model's `KeyspaceStatus`,
/// `TableStatus` and `TypeStatus` value sets, so one predicate answers for all three;
/// `RESTORING` is in the `TableStatus` and `TypeStatus` sets only, and no keyspace is
/// ever in it, so naming it here costs nothing and makes a table whose restore is still
/// running count as in flight exactly as one still being created does.
pub fn is_in_flight_status(status: &str) -> bool {
    status == KEYSPACE_STATUS_CREATING
        || status == KEYSPACE_STATUS_UPDATING
        // Spelled with the table constant because only a table is ever in it; the
        // comparison is still against the status STRING, as the two above are.
        || status == TABLE_STATUS_RESTORING
}

/// Whether `status` is the one a resource holds once the service has ACCEPTED its
/// deletion: the resource is on its way out, so it takes no further change, can gain no
/// new reference, and the clock owes it nothing but its removal.
///
/// The companion of [`is_in_flight_status`], which deliberately leaves this status out
/// for the same reason: a resource already on its way out does not block a change to
/// another resource. `DELETING` has the same spelling in the model's `KeyspaceStatus`,
/// `TableStatus` and `TypeStatus` value sets, so one predicate answers for all three.
pub fn is_departing_status(status: &str) -> bool {
    // Spelled with the table constant; the comparison is against the status STRING,
    // which the keyspace and type value sets spell identically.
    status == TABLE_STATUS_DELETING
}

/// The largest page a `@paginated` listing of this service returns, and the page size a
/// caller that names no `maxResults` gets: the maximum of the modelled `maxResults` range
/// (`1..1000`), which every `@paginated` operation of this service shares.
pub const MAX_PAGE_SIZE: i32 = 1000;

/// One page of a listing, and whether a further item follows it — the page window every
/// `@paginated` operation of this service reports through.
///
/// `items` is the listing's own iteration order, already positioned at the start of the
/// page the caller asked to resume at: every level of the store is a [`BTreeMap`], so that
/// order is total and stable and a page boundary is resumable. `max_results` is the page
/// size the caller named; `maxResults` is modelled `1..1000`, so the boundary validator has
/// already refused one outside that range, and an absent one means "no limit the caller
/// asked for", which is [`MAX_PAGE_SIZE`].
///
/// Takes ONE item more than the page fits, so the returned flag is true exactly when an
/// item follows the last one on the page — which is when the listing owes the caller a
/// `nextToken`, leaving the final page without one. The resume point is the last item of
/// the returned page; a scope-qualified listing turns it into a token through
/// [`encode_page_token`], so only the scope expression stays with the operation.
pub fn page_window<Item>(
    items: impl Iterator<Item = Item>,
    max_results: Option<i32>,
) -> (Vec<Item>, bool) {
    let page_size =
        usize::try_from(max_results.unwrap_or(MAX_PAGE_SIZE)).unwrap_or(MAX_PAGE_SIZE as usize);
    let mut page: Vec<Item> = items.take(page_size.saturating_add(1)).collect();
    let truncated = page.len() > page_size;
    page.truncate(page_size);
    (page, truncated)
}

/// The separator between the scope half and the item half of a scope-qualified resume
/// point, inside the token body [`encode_page_token`] hex-encodes.
const PAGE_TOKEN_SEPARATOR: char = '#';

/// Encode the resume point of a page as the opaque `nextToken` a scope-qualified listing
/// hands its caller — the ONE token codec every such listing of this service reports
/// through, so the scope expression is the only part that is a listing's own.
///
/// The token names the LAST item of the page just returned, qualified by the `scope` that
/// item was listed within, and is hex-encoded so no caller can read a resource name out of
/// it or build one by hand. Splitting at the FIRST [`PAGE_TOKEN_SEPARATOR`] separates the
/// two halves unambiguously as long as the `scope` carries none, which is each listing's
/// own obligation: a scope that could contain one hex-encodes itself first.
///
/// The `scope` is also what keeps the listings apart. Two listings of this service page
/// collections whose item names have the same shape, so a token one issued must not resume
/// the other: every scope-qualified listing builds its scope through [`listing_scope`], so
/// no two name the same scope, and [`decode_page_token`] then refuses a foreign token
/// rather than resuming at a point that would silently skip items.
pub fn encode_page_token(scope: &str, item: &str) -> String {
    ::hex::encode(format!("{scope}{PAGE_TOKEN_SEPARATOR}{item}"))
}

/// The item a `nextToken` resumes after, or `None` when the token is not one
/// [`encode_page_token`] issued for `scope` — a token minted for a different scope, or by a
/// different listing, resumes nothing and is refused rather than silently ignored.
pub fn decode_page_token(token: &str, scope: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (token_scope, item) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
    (token_scope == scope && !item.is_empty()).then(|| item.to_string())
}

/// The scope half every scope-qualified listing of this service qualifies its resume points
/// with: `listing`, which names the collection's KIND, over `collection`, which names the
/// one collection that kind was paged over.
///
/// `collection` has to name that collection completely, and completely includes the Amazon
/// Web Services Region holding it: every collection this service pages belongs to ONE
/// Region, and the same resource name names a different collection — holding different
/// items — in another Region, so a scope that left the Region out would let a token issued
/// over one Region's collection resume another Region's at a resume point that silently
/// skips the items sorting at or before it. A resource ARN of this service is such a name
/// (it carries the Region); for the listing of what a Region itself holds, the Region is.
///
/// `collection` is hex-encoded, so the scope half can carry neither
/// [`PAGE_TOKEN_SEPARATOR`] nor the separator below whatever the ARN or the Region contains,
/// and the `listing` half keeps two listings over the SAME collection apart.
pub fn listing_scope(listing: &str, collection: &str) -> String {
    format!("{listing}{LISTING_SCOPE_SEPARATOR}{}", ::hex::encode(collection))
}

/// The separator between the two halves of a [`listing_scope`]: the kind of collection, and
/// the collection itself. No `listing` of this service carries it.
const LISTING_SCOPE_SEPARATOR: char = '/';

/// The most keyspaces this emulator holds in one Amazon Web Services Region, and — with
/// [`MAX_TABLES_PER_KEYSPACE`] and [`MAX_REGIONS`] — the caps that bound caller-driven
/// growth of the store.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create is the only thing that adds one, so the store needs a maximum entry count:
/// neither a TTL nor an eviction policy is available to it, because `GetTable` must still
/// answer what `CreateTable` created. Every operation that grows the store refuses past
/// these caps with the service's own modelled `ServiceQuotaExceededException` (HTTP 402),
/// which this model declares on the two create operations, on `UpdateTable` and on no
/// read. The caps also bound the work
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`] can do while holding the state
/// lock.
///
/// It counts what a Region HOLDS, which includes the [`SERVICE_MANAGED_KEYSPACES`] every
/// Region holds before any call.
///
/// The numbers are the emulator's own bound, not a contract: Amazon Keyspaces publishes
/// these quotas as adjustable, so no single value is the published one; these are the
/// order of magnitude of the defaults and are far above any ordinary workflow.
pub const MAX_KEYSPACES: usize = 256;

/// How many Amazon Web Services Regions this emulator keeps keyspaces for.
///
/// The store is partitioned per Region and the Region comes from the caller's own SigV4
/// credential scope, so without a ceiling a caller could open a partition per made-up
/// Region. It is comfortably more than the Regions Amazon Keyspaces is offered in, so no
/// real multi-Region workflow meets it; with [`MAX_KEYSPACES`] it is what bounds the whole
/// store. `CreateKeyspace` refuses a create that would be the FIRST keyspace of a further
/// Region with the `ServiceQuotaExceededException` it models
/// ([`ComAmazonawsKeyspacesKeyspacesserviceState::region_capacity_reached`]); a read opens
/// no partition, so only Regions something was created in count against it.
pub const MAX_REGIONS: usize = 64;

/// The most tables one keyspace holds. See [`MAX_KEYSPACES`].
pub const MAX_TABLES_PER_KEYSPACE: usize = 256;

/// The most Amazon Web Services Regions one keyspace is replicated in.
///
/// `RegionList` is modelled `len 2..` with NO maximum, so the caller alone picks how long
/// a `replicationSpecification.regionList` is — and that count is both stored on the
/// keyspace and scanned by every operation that compares the list against another
/// ([`resolve_replication`], the add/drop comparison `UpdateKeyspace` runs). Capping it
/// here bounds both, so one request can neither retain nor spend an unbounded amount on a
/// Region count of its own choosing. [`resolve_replication`] refuses a longer list, so
/// `CreateKeyspace` and `UpdateKeyspace` share the one cap.
///
/// Like [`MAX_KEYSPACES`], the number is the emulator's own bound rather than a contract:
/// Amazon Keyspaces publishes multi-Region replication across the Amazon Web Services
/// Regions the service runs in, which is of this order, and no ordinary workflow replicates
/// a keyspace more widely.
pub const MAX_REPLICATION_REGIONS: usize = 32;

/// The most user-defined types one keyspace holds. See [`MAX_KEYSPACES`]: `CreateType`
/// is another operation that grows the store, and it refuses past this cap with the
/// `ServiceQuotaExceededException` the model declares for it.
pub const MAX_TYPES_PER_KEYSPACE: usize = 256;

/// The most fields one user-defined type holds, and with [`MAX_TYPES_PER_KEYSPACE`] the
/// pair that bounds everything `GetType` derives from a keyspace's types: the
/// [`nesting_depth`] walk, [`Keyspace::direct_parent_types`] and
/// [`Keyspace::direct_referring_tables`] each cost at most one pass over this many
/// fields per stored type.
///
/// `FieldList` is modelled `len 1..` with NO maximum, so without a cap here one request
/// could store a field list of any size and every later read would pay for it. Past the
/// cap `CreateType` answers the `ServiceQuotaExceededException` (HTTP 402) it models.
///
/// Like [`MAX_KEYSPACES`], the number is the emulator's own bound rather than a
/// contract: Amazon Keyspaces publishes a fields-per-UDT quota as adjustable, so no
/// single value is the published one, and this is far above any ordinary type.
pub const MAX_FIELDS_PER_TYPE: usize = 128;

/// The Cassandra native (scalar) data type names, which a field's `type` may name
/// without that naming a user-defined type.
///
/// Source: the CQL native types, as linked from `FieldDefinition$type` ("Any supported
/// Cassandra data type, including collections and other user-defined types that are
/// contained in the same keyspace" -> Cassandra data type support, Amazon Keyspaces
/// Developer Guide). The list is deliberately the WHOLE CQL set rather than only the
/// subset Keyspaces serves: its one job is to tell an identifier that names a built-in
/// type apart from one that names a UDT, in [`referenced_type_names`].
pub const CQL_NATIVE_TYPES: &[&str] = &[
    "ascii",
    "bigint",
    "blob",
    "boolean",
    "counter",
    "date",
    "decimal",
    "double",
    "duration",
    "float",
    "inet",
    "int",
    "smallint",
    "text",
    "time",
    "timestamp",
    "timeuuid",
    "tinyint",
    "uuid",
    "varchar",
    "varint",
];

/// The CQL type constructors: an identifier that names one of these introduces a
/// parameterised type (`list<int>`, `map<text, address>`, `frozen<address>`,
/// `tuple<int, text>`, `vector<float, 3>`) rather than naming a user-defined type. Same
/// source as [`CQL_NATIVE_TYPES`].
pub const CQL_TYPE_CONSTRUCTORS: &[&str] = &["frozen", "list", "map", "set", "tuple", "vector"];

/// The most columns one table's schema holds. See [`MAX_KEYSPACES`].
///
/// This is the third caller-driven growth dimension of the store, and the only one a
/// table can grow along after it exists: `addColumns` is modelled `len 1..`, with no
/// maximum, and `UpdateTable` appends to a schema that never shrinks. Both the create
/// and the update refuse past this cap, so neither one oversized request nor a run of
/// updates can grow a stored schema without bound. Both check it BEFORE they walk the
/// columns the request carries, so it also caps the per-column scans — the duplicate-name
/// scan, and the type-resolution scan each of them runs while holding the state lock.
pub const MAX_COLUMNS_PER_TABLE: usize = 1024;

/// The most tags one Amazon Keyspaces resource holds.
///
/// PUBLISHED by the service, not chosen here: the `Tag` shape's own documentation
/// (`smithy_query(query="shape",shape="Tag")`) says "A tag is a key-value pair. You can
/// add up to 50 tags to a single Amazon Keyspaces resource."
///
/// It is a different number from the modelled `TagList` bound (`len 1..60`), which caps
/// what ONE REQUEST may carry: a request well inside the modelled bound can still carry a
/// resource past this quota, and the request boundary cannot see that because it depends
/// on the tags the resource already holds. Such a call is refused as a malformed REQUEST —
/// the `ValidationException` (HTTP 400) every operation taking tags models, carrying the
/// sentence [`TagRejection::message`] renders for it — rather than as the service-quota
/// failure: what the caller has to do is supply fewer tags, not ask for more quota.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The prefix reserved for Amazon Web Services' own tag keys.
///
/// From the `Tag` shape's documentation: "Amazon Web Services-assigned tag names and
/// values are automatically assigned the `aws:` prefix, which the user cannot assign."
/// So a caller may neither apply nor remove a key carrying it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// A resource's tags, keyed by `Tag$key`.
///
/// The KEY identifies a tag, not the pair: "Each Amazon Keyspaces resource can only have
/// up to one tag with the same key. If you try to add an existing tag (same key), the
/// existing tag value will be updated to the new value" (`Tag$key`). A
/// [`std::collections::BTreeMap`] for the same reason the keyspace and table maps are
/// one: `ListTagsForResource` is `@paginated`, and a page boundary is only resumable if
/// the iteration order is total and stable.
pub type TagSet = BTreeMap<String, String>;

/// Why a tag a caller supplied was refused.
///
/// Both variants are faults of the REQUEST — one names a key the caller may not assign,
/// the other asks for more tags than a resource may hold — so both are the 400
/// `ValidationException` every operation taking tags models. This is not a bare string
/// because each variant renders its own sentence, and one of them names the key at fault.
#[derive(Debug, Clone)]
pub enum TagRejection {
    /// A key the caller may not assign ([`RESERVED_TAG_KEY_PREFIX`]).
    ReservedKeyPrefix {
        /// The key as supplied.
        key: String,
    },
    /// The call would carry the resource past [`MAX_TAGS_PER_RESOURCE`].
    ResourceTagCountExceeded,
}

impl TagRejection {
    /// What the caller has to fix, naming the member at fault and the rule it broke.
    ///
    /// The per-resource-count sentence is the service's own, FIXED text: it reports
    /// neither the resource, nor how many tags it already holds, nor the count the call
    /// would have left behind — the only value in it is the published per-resource limit
    /// ([`MAX_TAGS_PER_RESOURCE`]).
    pub fn message(&self) -> String {
        match self {
            Self::ReservedKeyPrefix { key } => format!(
                "tags contains key '{key}': the '{RESERVED_TAG_KEY_PREFIX}' prefix is reserved for Amazon Web Services and cannot be assigned."
            ),
            Self::ResourceTagCountExceeded => format!(
                "Tag count exceed the imposed limit, The resultant tag set must not have \
                 more than {MAX_TAGS_PER_RESOURCE} user tags."
            ),
        }
    }
}

/// Merge `incoming` into the tag set `existing`, as `TagResource` does and as a create
/// does onto the empty set.
///
/// A key the set already holds is an UPDATE of its value, not a second tag (`Tag$key`),
/// so it does not count towards [`MAX_TAGS_PER_RESOURCE`] — and neither does a key the
/// request repeats. The resulting size is therefore computed before anything is stored,
/// which is also what makes a refused call leave the tag set exactly as it was.
pub fn merge_tags(existing: &mut TagSet, incoming: &[Tag]) -> Result<(), TagRejection> {
    // The request-only refusal first, so merging decides it exactly as the handlers that
    // decide it before they resolve a resource do.
    reject_reserved_tag_keys(incoming)?;
    let mut added: BTreeSet<&str> = BTreeSet::new();
    for tag in incoming {
        if !existing.contains_key(&tag.key) {
            added.insert(tag.key.as_str());
        }
    }
    if existing.len() + added.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagRejection::ResourceTagCountExceeded);
    }
    for tag in incoming {
        existing.insert(tag.key.clone(), tag.value.clone());
    }
    Ok(())
}

/// The first key in `supplied` the caller may not name, as the refusal it is.
///
/// Whether a key carries [`RESERVED_TAG_KEY_PREFIX`] is a property of the REQUEST ALONE:
/// it does not depend on which tags a resource already holds, nor on whether the resource
/// the request names exists at all. So every operation taking tags decides it BEFORE any
/// refusal that depends on stored state — the resource's existence, its status, a name
/// already taken — which is what makes one and the same reserved key answer the same 400
/// `ValidationException` whichever of them was called and whatever the store holds.
///
/// The per-resource count ([`MAX_TAGS_PER_RESOURCE`]) is the other refusal [`merge_tags`]
/// raises and is NOT request-only — it counts the tags the resource already holds — so it
/// stays where the resource is known, even though both answer the same error.
pub fn reject_reserved_tag_keys(supplied: &[Tag]) -> Result<(), TagRejection> {
    match supplied
        .iter()
        .find(|tag| tag.key.starts_with(RESERVED_TAG_KEY_PREFIX))
    {
        Some(reserved) => Err(TagRejection::ReservedKeyPrefix {
            key: reserved.key.clone(),
        }),
        None => Ok(()),
    }
}

/// The tag set a create operation stores for the `tags` it was given: the same rules as
/// [`merge_tags`], applied to a resource that holds none yet.
pub fn new_tag_set(supplied: Option<&[Tag]>) -> Result<TagSet, TagRejection> {
    let mut tags = TagSet::new();
    merge_tags(&mut tags, supplied.unwrap_or_default())?;
    Ok(tags)
}

/// Which resource an Amazon Keyspaces ARN names: the resource forms this service mints
/// ([`keyspace_arn`], [`table_arn`]) and the CDC stream form it enables for a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// `arn:<partition>:cassandra:<region>:<account>:/keyspace/<name>/`.
    Keyspace {
        /// The keyspace the ARN names.
        keyspace_name: String,
    },
    /// `arn:<partition>:cassandra:<region>:<account>:/keyspace/<ks>/table/<tbl>`.
    Table {
        /// The owning keyspace.
        keyspace_name: String,
        /// The table the ARN names.
        table_name: String,
    },
    /// `arn:…:/keyspace/<ks>/table/<tbl>/stream/<label>`: the CDC stream of one table,
    /// which is a taggable Amazon Keyspaces resource of its own
    /// (`CdcSpecification$tags`, "the tags … you want to apply to the stream").
    Stream {
        /// The owning keyspace.
        keyspace_name: String,
        /// The table whose changes the stream captures.
        table_name: String,
        /// The stream's label, which is what distinguishes one of a table's streams
        /// from another.
        stream_label: String,
    },
}

/// The resource an Amazon Keyspaces ARN names, or `None` when it names none this service
/// has a resource form for.
///
/// The ARN's shape is modelled only as `^arn:(aws[a-zA-Z0-9-]*):cassandra:.+` (`ARN`), so
/// everything after the service segment is parsed here. A keyspace and a table name
/// cannot contain `/` (both are modelled `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`), and neither
/// can a stream label (a timestamp), which is what makes the three forms unambiguous:
/// only the keyspace form ends in `/`, and only a stream's resource path carries
/// `/stream/`.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceRef> {
    let mut segments = arn.splitn(6, ':');
    let scheme = segments.next()?;
    let partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let _account = segments.next()?;
    let resource = segments.next()?;
    if scheme != "arn" || !partition.starts_with("aws") || service != ARN_NAMESPACE {
        return None;
    }
    let rest = resource.strip_prefix("/keyspace/")?;
    match rest.strip_suffix('/') {
        Some(keyspace_name) => (!keyspace_name.is_empty() && !keyspace_name.contains('/')).then(
            || ResourceRef::Keyspace {
                keyspace_name: keyspace_name.to_string(),
            },
        ),
        None => {
            let (keyspace_name, below) = rest.split_once("/table/")?;
            if keyspace_name.is_empty() || keyspace_name.contains('/') {
                return None;
            }
            match below.split_once("/stream/") {
                Some((table_name, stream_label)) => (!table_name.is_empty()
                    && !table_name.contains('/')
                    && !stream_label.is_empty()
                    && !stream_label.contains('/'))
                .then(|| ResourceRef::Stream {
                    keyspace_name: keyspace_name.to_string(),
                    table_name: table_name.to_string(),
                    stream_label: stream_label.to_string(),
                }),
                None => (!below.is_empty() && !below.contains('/')).then(|| ResourceRef::Table {
                    keyspace_name: keyspace_name.to_string(),
                    table_name: below.to_string(),
                }),
            }
        }
    }
}

/// The keyspace a resource ARN belongs to: the keyspace itself for a keyspace ARN, the
/// owning keyspace for a table or CDC stream ARN, and `None` for an ARN that names no
/// Amazon Keyspaces resource.
///
/// What a caller may do to a resource is decided by its keyspace — a
/// [service-managed][SERVICE_MANAGED_KEYSPACES] one takes no change at all — so the
/// operations that address a resource by ARN resolve that keyspace through here rather
/// than each re-parsing the ARN.
pub fn arn_keyspace_name(resource_arn: &str) -> Option<String> {
    match parse_resource_arn(resource_arn)? {
        ResourceRef::Keyspace { keyspace_name }
        | ResourceRef::Table { keyspace_name, .. }
        | ResourceRef::Stream { keyspace_name, .. } => Some(keyspace_name),
    }
}

/// The `ResourceNotFoundException` payload for an ARN that names no stored resource.
///
/// The ARN the caller supplied goes in the modelled `resourceArn` member, which is what a
/// client reads to learn WHICH resource was missing.
pub fn arn_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Resource '{resource_arn}' was not found."
        )),
        resource_arn: Some(resource_arn.to_string()),
    }
}

/// Per-service state for com-amazonaws-keyspaces-keyspacesservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKeyspacesKeyspacesserviceState {
    /// The keyspaces callers have created, partitioned by the Amazon Web Services Region
    /// of the request that created each one and keyed inside a Region by `keyspaceName` —
    /// "keyspace names must be unique within each Region" (`CreateKeyspace`), so a
    /// keyspace, with the tables and types it holds, is a resource of ONE Region and the
    /// same name names a different resource in another.
    ///
    /// Only a write opens a Region's partition ([`Self::insert_keyspace`]) and
    /// [`Self::settle`] drops one once it holds nothing, so a read answers for any Region
    /// without storing a thing. Bounded by [`MAX_REGIONS`] partitions, each by
    /// [`MAX_KEYSPACES`] keyspaces, each keyspace's tables by
    /// [`MAX_TABLES_PER_KEYSPACE`] and its types by [`MAX_TYPES_PER_KEYSPACE`], and each
    /// table's schema by [`MAX_COLUMNS_PER_TABLE`].
    regions: BTreeMap<String, BTreeMap<String, Keyspace>>,
    /// The earliest instant at which any stored table or type has a status transition
    /// due, or `None` when none has. [`Self::settle`] returns without looking at a single
    /// resource until that instant arrives, so a request pays O(1) for the clock except on
    /// the calls where a transition really is due.
    next_transition_at: Option<SystemTime>,
}

/// One keyspace, with the tables it owns.
#[derive(Debug, Clone)]
pub struct Keyspace {
    /// `keyspaceName`, which is also this map entry's key.
    pub name: String,
    /// The keyspace ARN minted by `CreateKeyspace` ([`keyspace_arn`]).
    pub resource_arn: String,
    /// `replicationSpecification.replicationStrategy`. A [service-managed
    /// keyspace][SERVICE_MANAGED_KEYSPACES] is replicated in the Region it is read in, so
    /// its strategy is [`SINGLE_REGION_STRATEGY`].
    pub replication_strategy: String,
    /// `replicationSpecification.regionList`.
    pub replication_regions: Vec<String>,
    /// This keyspace's tags: what `CreateKeyspace` attached, plus what `TagResource`
    /// added and `UntagResource` removed since.
    pub tags: TagSet,
    /// A `KeyspaceStatus` value: the keyspace's own status, which `GetKeyspace`
    /// reports per replication Region.
    pub status: String,
    /// When the clock owes this keyspace its next status, or `None` once it holds
    /// a status nothing is pending on. Never slept for: [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`]
    /// compares it against the clock.
    pub status_due_at: Option<SystemTime>,
    /// The keyspace's tables, keyed by `tableName`.
    pub tables: BTreeMap<String, Table>,
    /// The keyspace's user-defined types, keyed by the FORMATTED `typeName`
    /// ([`format_type_name`]) — the name every other operation takes, and the name a
    /// field or column refers to the type by.
    pub types: BTreeMap<String, UserDefinedType>,
}

impl Keyspace {
    /// The `replicationRegions` a report of this keyspace carries: the stored region
    /// list for a `MULTI_REGION` keyspace, and nothing for any other strategy.
    ///
    /// "If the `replicationStrategy` of the keyspace is `MULTI_REGION`, a list of
    /// replication Regions is returned" (`KeyspaceSummary$replicationRegions`), and the
    /// member's modelled minimum length is 2 — which a single-Region keyspace's own
    /// region, on its own, could never satisfy.
    ///
    /// Which Regions those are is decided in ONE place,
    /// [`Self::table_replica_regions`]: this is the same list under the same rule, owned
    /// rather than borrowed, so a report of the keyspace and a report of any table of it
    /// can never name different Regions.
    pub fn reported_replication_regions(&self) -> Option<Vec<String>> {
        self.table_replica_regions().map(<[String]>::to_vec)
    }

    /// The Amazon Web Services Regions a report of ANY table of this keyspace carries
    /// Region-specific settings for: every Region a `MULTI_REGION` keyspace is replicated
    /// in, and none at all under any other strategy.
    ///
    /// A table belongs to its keyspace's replication group — `UpdateKeyspace` "adds a new
    /// Amazon Web Services Region to the keyspace ... Amazon Keyspaces is going to
    /// replicate all tables in the keyspace to the new Region" — so a table of a
    /// `MULTI_REGION` keyspace IS a multi-Region table, and is replicated in exactly the
    /// keyspace's [`replication_regions`][Self::replication_regions]. That is what
    /// `GetTableResponse$replicaSpecifications` ("the ... Region specific settings of all
    /// Regions a multi-Region table is replicated in") and
    /// `GetTableAutoScalingSettingsResponse$replicaSpecifications` ("the settings for all
    /// Regions the table is replicated in") each report one entry per; a table of a
    /// single-Region keyspace is replicated in one Region only, so it has none.
    ///
    /// This is the one place the rule lives: every report of the keyspace or of a table of
    /// it reaches the Region list through here, directly or through
    /// [`Self::reported_replication_regions`].
    pub fn table_replica_regions(&self) -> Option<&[String]> {
        (self.replication_strategy == MULTI_REGION_STRATEGY)
            .then_some(self.replication_regions.as_slice())
    }

    /// Whether this keyspace's own deletion is already in progress — the status
    /// [`KEYSPACE_STATUS_DELETING`] — so it is on its way out and takes no further
    /// change.
    pub fn is_deleting(&self) -> bool {
        is_departing_status(&self.status)
    }

    /// The first resource of this keyspace that still has a change of its OWN in flight:
    /// what kind it is (`"Table"` or `"Type"`), its name, and the status it is in.
    ///
    /// A resource in flight ([`is_in_flight_status`]) is one the clock still owes a
    /// transition to, and dropping it is exactly what `DeleteTable` refuses for a table
    /// in that state — so a keyspace holding one cannot be dropped either, since
    /// `DeleteKeyspace` "deletes a keyspace and all of its tables". A resource already
    /// `DELETING` does NOT count: it is on its way out, which is where dropping the
    /// keyspace takes it anyway.
    ///
    /// Tables first, then types, in each map's one total order, so the same keyspace
    /// always names the same blocking resource.
    pub fn resource_in_flight(&self) -> Option<(&'static str, &str, &str)> {
        self.first_resource(is_in_flight_status)
    }

    /// The first resource of this keyspace that is not settled — a table or type whose
    /// status is anything but `ACTIVE` — as the same triple
    /// [`resource_in_flight`][Self::resource_in_flight] answers with.
    ///
    /// This is the WIDER question: a resource that is already `DELETING` is not settled
    /// either, so it is named here although it is on its way out and so does not count as
    /// in flight. A keyspace-wide change that every resource of the keyspace has to take
    /// along with it — `UpdateKeyspace` "adds a new Amazon Web Services Region to the
    /// keyspace ... Amazon Keyspaces is going to replicate all tables in the keyspace to
    /// the new Region" — cannot be applied to a resource that is not settled, whichever
    /// unsettled status that resource holds, so that is the question this answers.
    ///
    /// Tables first, then types, exactly as above.
    pub fn resource_not_active(&self) -> Option<(&'static str, &str, &str)> {
        // `ACTIVE` has the same spelling in the model's `TableStatus` and `TypeStatus`
        // value sets, so one comparison answers for either kind of resource — as it does
        // in [`is_in_flight_status`] for the transient statuses.
        self.first_resource(|status| status != TABLE_STATUS_ACTIVE)
    }

    /// The first table of this keyspace whose status `blocked` answers true for, else the
    /// first such type: what kind it is (`"Table"` or `"Type"`), its name, and that
    /// status.
    ///
    /// The ONE walk over a keyspace's resources the questions above are asked through, so
    /// they differ only in the predicate and can never disagree about which resource of a
    /// keyspace they name.
    fn first_resource(
        &self,
        blocked: impl Fn(&str) -> bool,
    ) -> Option<(&'static str, &str, &str)> {
        self.tables
            .values()
            .find(|table| blocked(&table.status))
            .map(|table| ("Table", table.table_name.as_str(), table.status.as_str()))
            .or_else(|| {
                self.types
                    .values()
                    .find(|udt| blocked(&udt.status))
                    .map(|udt| ("Type", udt.type_name.as_str(), udt.status.as_str()))
            })
    }
}

/// One table: everything `CreateTable` was given, plus what the service derives.
///
/// A table of a [service-managed keyspace][SERVICE_MANAGED_KEYSPACES] is a table the
/// SERVICE owns, so no `CreateTable` ever supplied the settings a customer table carries:
/// every member below that only a create could have filled is `None` for one, and
/// `GetTable` reports exactly the members the model leaves optional rather than a
/// default nobody configured. Which tables those are is [`is_service_managed_keyspace`]
/// of the owning keyspace, not a flag stored twice.
#[derive(Debug, Clone)]
pub struct Table {
    /// The owning keyspace's name.
    pub keyspace_name: String,
    /// `tableName`, which is also this map entry's key.
    pub table_name: String,
    /// The table ARN minted by `CreateTable` ([`table_arn`]).
    pub resource_arn: String,
    /// When `CreateTable` ran, or `None` for a service-managed table, which predates
    /// every call.
    pub creation_timestamp: Option<SystemTime>,
    /// A `TableStatus` value.
    pub status: String,
    /// When the clock owes this table its next status, or `None` once it holds a
    /// status nothing is pending on. Never slept for: [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`]
    /// compares it against the clock.
    pub status_due_at: Option<SystemTime>,
    /// The schema exactly as supplied, or `None` for a service-managed table: Amazon
    /// Keyspaces publishes no column list for its system tables, and
    /// `GetTableResponse$schemaDefinition` is an optional member.
    pub schema_definition: Option<SchemaDefinition>,
    /// The supplied capacity specification, or the modelled default, as last set by
    /// `CreateTable` or `UpdateTable`; `None` for a service-managed table, whose
    /// throughput the service owns.
    pub capacity_specification: Option<CapacitySpecification>,
    /// The supplied auto scaling settings, if any — "The optional auto scaling settings
    /// for a table in provisioned capacity mode" — reported back by
    /// `GetTableAutoScalingSettings`.
    pub auto_scaling_specification: Option<AutoScalingSpecification>,
    /// When this table's throughput mode last BECAME `PAY_PER_REQUEST`, or `None` for a
    /// table that has never been in that mode
    /// (`CapacitySpecificationSummary$lastUpdateToPayPerRequestTimestamp`, "the timestamp
    /// of the last operation that changed the provisioned throughput capacity of a
    /// table").
    ///
    /// The operation that put the table in on-demand mode is the one stamped here,
    /// whether that was the create (or restore) that made the table in that mode —
    /// `PAY_PER_REQUEST` being the modelled default, that is every table no
    /// `capacitySpecification` was supplied for ([`pay_per_request_since`]) — or a later
    /// `UpdateTable` that moved it there. A call that re-sets the mode the table is
    /// already in changed no throughput capacity, so it leaves this where it is.
    pub last_update_to_pay_per_request: Option<SystemTime>,
    /// The supplied encryption specification, or the modelled default; `None` for a
    /// service-managed table.
    pub encryption_specification: Option<EncryptionSpecification>,
    /// The supplied point-in-time-recovery setting, or the modelled default; `None` for a
    /// service-managed table, which no backup setting applies to.
    pub point_in_time_recovery: Option<PointInTimeRecovery>,
    /// When the continuous backups point-in-time recovery takes of this table began — the
    /// instant the setting above was last turned ON — and `None` while it is off, which is
    /// every table no call has enabled it for and every table the service itself owns.
    ///
    /// Stored beside the setting because the setting alone does not say WHEN those backups
    /// start: a table created with point-in-time recovery off and switched on later has no
    /// backup covering the span before the switch, so
    /// [`Table::earliest_restorable_timestamp`] opens the restorable window here and not at
    /// the table's creation. Turning the setting off clears it, so a later re-enable opens
    /// a NEW window instead of reaching back across the span it was off for.
    pub continuous_backups_since: Option<SystemTime>,
    /// The supplied custom TTL setting, if any.
    pub ttl: Option<TimeToLive>,
    /// The supplied default TTL in seconds, if any.
    pub default_time_to_live: Option<i32>,
    /// The supplied comment, if any.
    pub comment: Option<Comment>,
    /// The supplied client-side-timestamps setting, if any.
    pub client_side_timestamps: Option<ClientSideTimestamps>,
    /// The Amazon Web Services Region specific settings of this table as a multi-Region
    /// table, keyed by `ReplicaSpecification$region` — "For a multi-Region table, you can
    /// configure the table's read capacity differently per Amazon Web Services Region".
    ///
    /// Keyed by Region because a Region holds ONE such setting: a later
    /// `CreateTable`/`UpdateTable` entry for a Region it already has replaces that
    /// Region's setting rather than adding a second one. A Region this map does not hold
    /// provisions the table's own read capacity and auto scaling
    /// ([`Self::read_capacity_units_in`], [`Self::replica_auto_scaling`]), and so does one
    /// whose entry carries neither override.
    pub replica_specifications: BTreeMap<String, ReplicaSpecification>,
    /// The warm throughput this table was pre-warmed for, if any was ever configured.
    pub warm_throughput_specification: Option<WarmThroughput>,
    /// This table's tags: what `CreateTable` attached, plus what `TagResource` added and
    /// `UntagResource` removed since.
    pub tags: TagSet,
    /// The tags of each CDC stream this table has had, keyed by the stream's ARN: what
    /// the `cdcSpecification` that enabled the stream applied to it
    /// ([`new_stream_tag_set`]), plus what `TagResource` added and `UntagResource`
    /// removed since.
    ///
    /// A stream is a resource of its own, addressed by its own ARN, so its tags cannot
    /// live in `tags` above — and they live HERE rather than beside the stream in the
    /// CDC stream store so that every tagging operation reaches them through the one
    /// lookup it already uses ([`ComAmazonawsKeyspacesKeyspacesserviceState::resource_tags`]).
    /// Bounded with the table, and by [`MAX_STREAMS_PER_TABLE`] entries per table, the
    /// same cap the stream store applies: a stream ARN ends in the stream's label, a
    /// timestamp, so dropping the first key drops the oldest stream's tags.
    pub cdc_stream_tags: BTreeMap<String, TagSet>,
    /// The superseded revisions of the settings a point-in-time restore reproduces "based
    /// on the selected timestamp" — the schema and the TTL settings — newest last. Empty
    /// for a table whose schema and TTL settings have never changed since it was created,
    /// which is every table no `UpdateTable` has touched.
    pub superseded: Vec<TableRevision>,
    /// Where this table came from, for a table a `RestoreTable` created, and `None` for
    /// one a `CreateTable` did.
    pub restored_from: Option<RestoreOrigin>,
    /// The earliest instant whose restorable settings are still known, once the revision
    /// cap ([`MAX_RETAINED_TABLE_REVISIONS`]) has dropped a revision this table held, and
    /// `None` while nothing has been dropped — which is every table whose revisions all
    /// still fit.
    pub earliest_known_settings: Option<SystemTime>,
}

/// One superseded revision of the table settings a point-in-time restore reproduces
/// "based on the selected timestamp" (`RestoreTable`): the schema and the TTL settings,
/// with the instant they stopped being in effect.
///
/// A revision was in effect from the PREVIOUS revision's `superseded_at` — or from the
/// table's creation, for the oldest — until its own. So the settings in effect at an
/// instant are the first revision superseded after it, and the table's current settings
/// when no revision was: see [`Table::restorable_settings_at`].
#[derive(Debug, Clone)]
pub struct TableRevision {
    /// When these settings stopped being in effect.
    pub superseded_at: SystemTime,
    /// The schema as of this revision.
    pub schema_definition: Option<SchemaDefinition>,
    /// The custom TTL setting as of this revision.
    pub ttl: Option<TimeToLive>,
    /// The default TTL in seconds as of this revision.
    pub default_time_to_live: Option<i32>,
    /// The client-side-timestamps setting as of this revision.
    pub client_side_timestamps: Option<ClientSideTimestamps>,
}

/// Where a table a `RestoreTable` created came from: the source table, and the point in
/// time whose settings it reproduces.
#[derive(Debug, Clone)]
pub struct RestoreOrigin {
    /// `sourceKeyspaceName` of the restore that created this table.
    pub source_keyspace_name: String,
    /// `sourceTableName` of that restore.
    pub source_table_name: String,
    /// The point in time the restore reproduced — `restoreTimestamp`, or the instant the
    /// call ran when the request named none.
    pub restore_timestamp: SystemTime,
}

impl Table {
    /// The settings a point-in-time restore of this table reproduces for `at`: the
    /// revision in effect then, or the current settings when nothing has superseded them
    /// since.
    ///
    /// The revisions are appended in time order, so the first one superseded after `at`
    /// is the one that was in effect at `at`.
    pub fn restorable_settings_at(&self, at: SystemTime) -> TableRevision {
        if let Some(revision) = self
            .superseded
            .iter()
            .find(|revision| revision.superseded_at > at)
        {
            return revision.clone();
        }
        TableRevision {
            superseded_at: at,
            schema_definition: self.schema_definition.clone(),
            ttl: self.ttl.clone(),
            default_time_to_live: self.default_time_to_live,
            client_side_timestamps: self.client_side_timestamps.clone(),
        }
    }

    /// Record that this table's restorable settings change at `now`, keeping the outgoing
    /// values as the revision in effect until then.
    ///
    /// Every operation that changes one of them calls this BEFORE it does — `UpdateTable`
    /// for a schema or TTL change, `UpdateKeyspace` where it enables client-side
    /// timestamps — so a restore to an earlier instant reproduces what the table held at
    /// that instant rather than what it holds now.
    pub fn supersede_restorable_settings(&mut self, now: SystemTime) {
        self.superseded.push(TableRevision {
            superseded_at: now,
            schema_definition: self.schema_definition.clone(),
            ttl: self.ttl.clone(),
            default_time_to_live: self.default_time_to_live,
            client_side_timestamps: self.client_side_timestamps.clone(),
        });
        // A revision superseded before the restorable window opened can never be the one a
        // restore targets, so it is dropped instead of kept forever.
        if let Some(window_start) = now.checked_sub(POINT_IN_TIME_RECOVERY_WINDOW) {
            self.superseded
                .retain(|revision| revision.superseded_at > window_start);
        }
        // And the newest [`MAX_RETAINED_TABLE_REVISIONS`] are all that is kept, so one
        // table's history stays bounded however many updates it takes inside one window.
        // The newest revision dropped is the boundary below which this table's settings are
        // no longer known, because the oldest revision kept describes only the span from
        // that boundary on.
        let excess = self
            .superseded
            .len()
            .saturating_sub(MAX_RETAINED_TABLE_REVISIONS);
        if excess > 0 {
            self.earliest_known_settings = self
                .superseded
                .get(excess - 1)
                .map(|dropped| dropped.superseded_at);
            self.superseded.drain(..excess);
        }
    }

    /// Whether point-in-time recovery is `ENABLED` on this table — the ONE reading of the
    /// stored setting, so every operation that asks answers the same way.
    pub fn point_in_time_recovery_enabled(&self) -> bool {
        self.point_in_time_recovery
            .as_ref()
            .is_some_and(|recovery| recovery.status == POINT_IN_TIME_RECOVERY_ENABLED)
    }

    /// Apply `recovery` as this table's point-in-time-recovery setting at `now`, moving
    /// the instant its continuous backups began with it.
    ///
    /// Turning the setting ON starts those backups at `now`; turning it OFF stops them, so
    /// nothing covers the span it stays off for and a later enable starts a new span. A
    /// request that re-sets the status the table already holds starts and stops nothing, so
    /// it leaves the instant where it is rather than reopening a window that never closed.
    pub fn set_point_in_time_recovery(&mut self, recovery: PointInTimeRecovery, now: SystemTime) {
        let was_enabled = self.point_in_time_recovery_enabled();
        let enabled = recovery.status == POINT_IN_TIME_RECOVERY_ENABLED;
        self.point_in_time_recovery = Some(recovery);
        if enabled != was_enabled {
            self.continuous_backups_since = enabled.then_some(now);
        }
    }

    /// The earliest instant a restore of this table may target —
    /// `PointInTimeRecoverySummary$earliestRestorableTimestamp`, "the earliest possible
    /// restore point of the table" — or `None` when the table has no restorable window at
    /// all, which is every table point-in-time recovery is not `ENABLED` on and every table
    /// the service itself owns.
    ///
    /// Continuous backups begin when point-in-time recovery is turned ON
    /// ([`Self::continuous_backups_since`]) — at the table's creation only for a table
    /// created with it already enabled — reach back at most
    /// [`POINT_IN_TIME_RECOVERY_WINDOW`], and can only reproduce settings the store still
    /// holds a revision for ([`MAX_RETAINED_TABLE_REVISIONS`]), so the earliest restorable
    /// instant is the latest of those three.
    pub fn earliest_restorable_timestamp(&self, now: SystemTime) -> Option<SystemTime> {
        if !self.point_in_time_recovery_enabled() {
            return None;
        }
        let mut earliest = self.continuous_backups_since?;
        if let Some(window_start) = now.checked_sub(POINT_IN_TIME_RECOVERY_WINDOW) {
            earliest = earliest.max(window_start);
        }
        if let Some(known_from) = self.earliest_known_settings {
            earliest = earliest.max(known_from);
        }
        Some(earliest)
    }

    /// Whether a CALL defined this table — a `CreateTable` or a `RestoreTable` — rather
    /// than the service owning it.
    ///
    /// A table of a [service-managed keyspace][SERVICE_MANAGED_KEYSPACES] carries none of
    /// the settings a call supplies ([`service_managed_keyspace`] builds it with every one
    /// of them unset), so a report of it carries no settings member at all — not even the
    /// empty or default value one of the always-served members below would otherwise take.
    /// A table a call defined is exactly a table whose creation instant is known.
    pub fn is_defined_by_a_call(&self) -> bool {
        self.creation_timestamp.is_some()
    }

    /// The `comment` a report of this table carries: the one it was created with, and the
    /// EMPTY comment for a table created without one.
    ///
    /// `Comment$message` is `@required` ("an optional description of the table"), so the
    /// structure a table with no description reports is the one whose message is empty —
    /// an absent `comment` would make a client reading `comment.message` read a null
    /// instead.
    pub fn reported_comment(&self) -> Option<Comment> {
        self.is_defined_by_a_call()
            .then(|| self.comment.clone().unwrap_or_default())
    }

    /// The `defaultTimeToLive` a report of this table carries: the value it was last set
    /// to, and `0` for a table no call ever set it on.
    ///
    /// The member is "the default Time to Live settings in seconds of the specified
    /// table", `@range 0..630720000`: no expiry is zero seconds of default TTL, which is
    /// the bottom of that range and the value a caller may set explicitly, so a table
    /// that has never been given one reports it rather than nothing.
    pub fn reported_default_time_to_live(&self) -> Option<i32> {
        self.is_defined_by_a_call()
            .then(|| self.default_time_to_live.unwrap_or(0))
    }

    /// The `schemaDefinition` a report of this table carries: the stored schema, with its
    /// two optional column lists filled in as EMPTY lists when the schema declares
    /// neither.
    ///
    /// `SchemaDefinition$clusteringKeys` and `$staticColumns` are lists with no modelled
    /// minimum length, and a table has a definite answer for each — the columns that are
    /// part of its clustering key, and the columns it declared `STATIC` — so a table with
    /// none of either reports two empty lists rather than two absent members, which is
    /// what lets a client iterate them without a null check.
    pub fn reported_schema_definition(&self) -> Option<SchemaDefinition> {
        self.schema_definition.as_ref().map(|schema| SchemaDefinition {
            all_columns: schema.all_columns.clone(),
            partition_keys: schema.partition_keys.clone(),
            clustering_keys: Some(schema.clustering_keys.clone().unwrap_or_default()),
            static_columns: Some(schema.static_columns.clone().unwrap_or_default()),
        })
    }

    /// The `warmThroughputSpecification` summary a report of this table carries: the
    /// configuration a call pre-warmed it with, and otherwise the baseline its capacity
    /// mode already gives it.
    ///
    /// "Contains the CURRENT warm throughput settings for a table, including the
    /// configured capacity units and the current status of the warm throughput
    /// configuration" — every table has them, because every table can serve some traffic
    /// before it has to scale: a provisioned table the capacity it provisions, and an
    /// on-demand one the documented
    /// [baseline][DEFAULT_ON_DEMAND_WARM_READ_UNITS_PER_SECOND]. A configuration a call
    /// set is reported with ITS status, which is `UPDATING` while the clock still owes
    /// that change its completion ([`WarmThroughput::status`]); a baseline nothing is
    /// changing is `AVAILABLE`.
    pub fn reported_warm_throughput(&self) -> Option<WarmThroughputSpecificationSummary> {
        if let Some(warm) = self.warm_throughput_specification.as_ref() {
            return Some(WarmThroughputSpecificationSummary {
                read_units_per_second: warm.read_units_per_second,
                write_units_per_second: warm.write_units_per_second,
                status: warm.status().to_string(),
            });
        }
        let capacity = self.capacity_specification.as_ref()?;
        let provisioned = capacity.throughput_mode == PROVISIONED_THROUGHPUT_MODE;
        Some(WarmThroughputSpecificationSummary {
            read_units_per_second: capacity
                .read_capacity_units
                .filter(|_| provisioned)
                .unwrap_or(DEFAULT_ON_DEMAND_WARM_READ_UNITS_PER_SECOND),
            write_units_per_second: capacity
                .write_capacity_units
                .filter(|_| provisioned)
                .unwrap_or(DEFAULT_ON_DEMAND_WARM_WRITE_UNITS_PER_SECOND),
            status: WARM_THROUGHPUT_STATUS_AVAILABLE.to_string(),
        })
    }
}

/// When a table CREATED or RESTORED at `now` with the capacity specification `capacity`
/// last became `PAY_PER_REQUEST`: `now` when that is the mode it is made in — the
/// modelled default, so every table no `capacitySpecification` was supplied for — and
/// `None` when it is made in any other mode, which no operation has ever moved out of
/// on-demand.
///
/// `CapacitySpecificationSummary$lastUpdateToPayPerRequestTimestamp` is "the timestamp of
/// the last operation that changed the provisioned throughput capacity of a table", and
/// for a table made in on-demand mode that operation is the call that made it. Every
/// operation that creates a table stamps [`Table::last_update_to_pay_per_request`]
/// through this, so a created and a restored table are stamped by the one rule
/// `UpdateTable` moves the stamp by afterwards.
pub fn pay_per_request_since(
    capacity: Option<&CapacitySpecification>,
    now: SystemTime,
) -> Option<SystemTime> {
    capacity
        .filter(|capacity| capacity.throughput_mode == PAY_PER_REQUEST_THROUGHPUT_MODE)
        .map(|_| now)
}

/// When the continuous backups of a table CREATED at `now` with the point-in-time-recovery
/// setting `recovery` begin: `now` while that setting is `ENABLED` — a table created with
/// point-in-time recovery on is backed up from the instant it exists — and `None`
/// otherwise, since a table it is off on has no continuous backups at all.
///
/// Every operation that creates a table stamps [`Table::continuous_backups_since`] through
/// this, so a created and a restored table open their restorable window by the one rule
/// [`Table::set_point_in_time_recovery`] moves it by afterwards.
pub fn continuous_backups_start(
    recovery: Option<&PointInTimeRecovery>,
    now: SystemTime,
) -> Option<SystemTime> {
    recovery
        .filter(|recovery| recovery.status == POINT_IN_TIME_RECOVERY_ENABLED)
        .map(|_| now)
}

/// A table's warm throughput configuration: "Pre-warming a table by specifying warm
/// throughput pre-provisions read and write capacity units to help avoid capacity
/// exceeded exceptions and reduce latency when your table starts receiving traffic"
/// (`WarmThroughputSpecification`).
///
/// Both unit counts are stored unconditionally rather than as options, because the
/// summary that reports them declares each `@required`
/// (`WarmThroughputSpecificationSummary$readUnitsPerSecond` /
/// `$writeUnitsPerSecond`); [`resolve_warm_throughput`] is what turns the request's two
/// optional members into them.
#[derive(Debug, Clone)]
pub struct WarmThroughput {
    /// `readUnitsPerSecond`: "the number of read capacity units per second currently
    /// configured for warm throughput".
    pub read_units_per_second: i64,
    /// `writeUnitsPerSecond`, the same for writes.
    pub write_units_per_second: i64,
    /// When the clock owes this configuration its [`WARM_THROUGHPUT_STATUS_AVAILABLE`]
    /// status, or `None` once it holds it. Never slept for:
    /// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`] compares it against the
    /// clock, exactly as it does a table's own status.
    pub status_due_at: Option<SystemTime>,
}

impl WarmThroughput {
    /// The `WarmThroughputStatus` this configuration reports: "`AVAILABLE` when the
    /// configuration is active, and `UPDATING` when changes are being applied"
    /// (`WarmThroughputSpecificationSummary$status`).
    pub fn status(&self) -> &'static str {
        match self.status_due_at {
            Some(_) => WARM_THROUGHPUT_STATUS_UPDATING,
            None => WARM_THROUGHPUT_STATUS_AVAILABLE,
        }
    }
}

impl Table {
    /// The read capacity units this table provisions in `region`: the Region's own
    /// `replicaSpecifications.readCapacityUnits` override when one is stored, and the
    /// table's `capacitySpecification.readCapacityUnits` otherwise.
    ///
    /// "For a multi-Region table, you can configure the table's read capacity differently
    /// per Amazon Web Services Region" (`ReplicaSpecification`) — the write capacity has
    /// no such member, because it "remains synchronized between all replicas to ensure
    /// that there's enough capacity to replicate writes across all Regions".
    pub fn read_capacity_units_in(&self, region: &str) -> Option<i64> {
        self.replica_specifications
            .get(region)
            .and_then(|replica| replica.read_capacity_units)
            .or_else(|| {
                self.capacity_specification
                    .as_ref()
                    .and_then(|capacity| capacity.read_capacity_units)
            })
    }

    /// The auto scaling settings this table reports in `region`: `base` — the settings
    /// the table is scaled by, which live in Application Auto Scaling
    /// ([`reported_auto_scaling`]) — with the Region's own
    /// `replicaSpecifications.readCapacityAutoScaling` override applied ON TOP when one is
    /// stored, or `None` when neither exists.
    pub fn replica_auto_scaling(
        &self,
        region: &str,
        base: Option<&AutoScalingSpecification>,
    ) -> Option<AutoScalingSpecification> {
        let mut specification = base.cloned();
        if let Some(read) = self
            .replica_specifications
            .get(region)
            .and_then(|replica| replica.read_capacity_auto_scaling.clone())
        {
            specification
                .get_or_insert_with(AutoScalingSpecification::default)
                .read_capacity_auto_scaling = Some(read);
        }
        specification
    }
}

/// One user-defined type: the fields `CreateType` was given, plus what the service
/// derives. Everything `GetType` reports about how the type RELATES to other
/// resources (`directParentTypes`, `directReferringTables`, `maxNestingDepth`) is
/// derived from the keyspace on read, so no copy of it can go stale.
#[derive(Debug, Clone)]
pub struct UserDefinedType {
    /// The owning keyspace's name.
    pub keyspace_name: String,
    /// The FORMATTED type name ([`format_type_name`]), which is also this map entry's
    /// key and what `CreateType` answers with.
    pub type_name: String,
    /// The field definitions exactly as supplied, in the order they were supplied.
    pub field_definitions: Vec<FieldDefinition>,
    /// When `CreateType` ran, which is the last instant the type's definition
    /// changed: nothing in this service's surface alters a type's fields.
    pub last_modified_timestamp: SystemTime,
    /// A `TypeStatus` value.
    pub status: String,
    /// When the clock owes this type its next status, or `None` once it holds a
    /// status nothing is pending on. Never slept for; see
    /// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`].
    pub status_due_at: Option<SystemTime>,
}

/// The first user-defined type a supplied column's or field's `type` refers to that
/// `usable_type_names` does not hold, or `None` when every reference in it resolves.
///
/// A column's or a field's type is "any supported Cassandra data type, including
/// collections and other user-defined types that are contained in the same keyspace"
/// (`ColumnDefinition$type` / `FieldDefinition$type`), so every identifier in it that
/// names neither a native type nor a type constructor ([`referenced_type_names`]) must
/// name a type of the SAME keyspace that is usable — one in status `ACTIVE`
/// ([`Keyspace::usable_type_names`]). Any other reference is a type "that doesn't exist.
/// The resource might not be specified correctly, or its status might not be ACTIVE"
/// ([`type_not_found`]), wherever a caller supplies a type expression: creating a table,
/// creating a type, or adding columns to a table that already exists.
pub fn unresolvable_type_reference(
    type_expression: &str,
    usable_type_names: &BTreeSet<String>,
) -> Option<String> {
    referenced_type_names(type_expression)
        .into_iter()
        .find(|referenced| !usable_type_names.contains(referenced))
}

impl Keyspace {
    /// The names of this keyspace's user-defined types a column's or field's type may
    /// refer to: the ones in status `ACTIVE`, in the FORMATTED form
    /// [`referenced_type_names`] yields.
    ///
    /// Taken as a set so one pass over the keyspace's types serves a whole request's type
    /// expressions, and so a caller that must read the types BEFORE borrowing one of the
    /// keyspace's tables (as `UpdateTable` does) resolves against the same values.
    pub fn usable_type_names(&self) -> BTreeSet<String> {
        self.types
            .values()
            .filter(|udt| udt.status == TYPE_STATUS_ACTIVE)
            .map(|udt| udt.type_name.clone())
            .collect()
    }

    /// The types of this keyspace that use `type_name` in one of their fields —
    /// `GetType`'s `directParentTypes` ("The types that use this type").
    ///
    /// Derived, never stored: a parent is any type one of whose field types refers to
    /// this one, which is exactly what [`referenced_type_names`] extracts. One pass over
    /// the keyspace's types, so at most [`MAX_TYPES_PER_KEYSPACE`] x
    /// [`MAX_FIELDS_PER_TYPE`] field types are scanned.
    ///
    /// EVERY such type, whatever its status: this is what the keyspace HOLDS, which is
    /// what a report of the type describes. The narrower question "what still uses this
    /// type" is [`Self::parent_types_still_using`].
    pub fn direct_parent_types(&self, type_name: &str) -> Vec<String> {
        self.parent_types(type_name, |_| true)
    }

    /// The types of this keyspace that still USE `type_name`: the parents
    /// [`Self::direct_parent_types`] derives, minus every one whose own deletion the
    /// service has already accepted ([`is_departing_status`]).
    ///
    /// A type on its way out takes no further change and is excluded from
    /// [`Self::usable_type_names`], so its fields can neither change nor be referred to
    /// again: the reference it holds is already dropped, and it is not a user of
    /// anything the caller can still do something about.
    pub fn parent_types_still_using(&self, type_name: &str) -> Vec<String> {
        self.parent_types(type_name, |candidate| {
            !is_departing_status(&candidate.status)
        })
    }

    /// The parents of `type_name` that `counts` keeps — the ONE derivation both readers
    /// above share, so they can never disagree about what a parent is.
    fn parent_types(
        &self,
        type_name: &str,
        counts: impl Fn(&UserDefinedType) -> bool,
    ) -> Vec<String> {
        self.types
            .values()
            .filter(|candidate| {
                candidate.type_name != type_name
                    && counts(candidate)
                    && refers_to_type(
                        candidate
                            .field_definitions
                            .iter()
                            .map(|field| field.r#type.as_str()),
                        type_name,
                    )
            })
            .map(|candidate| candidate.type_name.clone())
            .collect()
    }

    /// The tables of this keyspace that use `type_name` in one of their columns —
    /// `GetType`'s `directReferringTables` ("The tables that use this type").
    ///
    /// Derived the same way, in one pass over the keyspace's tables, and over every
    /// table the keyspace holds whatever its status.
    pub fn direct_referring_tables(&self, type_name: &str) -> Vec<String> {
        self.referring_tables(type_name, |_| true)
    }

    /// The tables of this keyspace that still USE `type_name`: the referring tables
    /// [`Self::direct_referring_tables`] derives, minus every one whose own deletion the
    /// service has already accepted ([`is_departing_status`]).
    ///
    /// The same rule as [`Self::parent_types_still_using`] one resource kind over: a
    /// table on its way out takes no further change ([`table_deleting_rejection`]), so
    /// the columns it was deleted with are the last it will ever have and the reference
    /// deleting it dropped has stopped counting.
    pub fn referring_tables_still_using(&self, type_name: &str) -> Vec<String> {
        self.referring_tables(type_name, |table| !is_departing_status(&table.status))
    }

    /// The tables referring to `type_name` that `counts` keeps — the ONE derivation both
    /// readers above share.
    fn referring_tables(&self, type_name: &str, counts: impl Fn(&Table) -> bool) -> Vec<String> {
        self.tables
            .values()
            .filter(|table| {
                counts(table)
                    && refers_to_type(
                        table
                            .schema_definition
                            .iter()
                            .flat_map(|schema| &schema.all_columns)
                            .map(|column| column.r#type.as_str()),
                        type_name,
                    )
            })
            .map(|table| table.table_name.clone())
            .collect()
    }
}

/// Whether any of `type_expressions` — the `type` of a table's columns, or of a type's
/// fields — refers to the user-defined type `type_name`.
///
/// A reference reaches a type through any type expression that names it, directly or
/// inside a collection or `frozen<...>` constructor, which is exactly what
/// [`referenced_type_names`] extracts: the derivations above differ only in WHICH
/// expressions they feed in, never in what counts as a reference.
fn refers_to_type<'a>(
    type_expressions: impl Iterator<Item = &'a str>,
    type_name: &str,
) -> bool {
    type_expressions.into_iter().any(|expression| {
        referenced_type_names(expression)
            .iter()
            .any(|referenced| referenced == type_name)
    })
}

/// `GetType`'s `maxNestingDepth` ("The level of nesting implemented for this type"):
/// `1` for a type none of whose fields refers to another user-defined type, and
/// otherwise one more than the deepest type it refers to.
///
/// The level COUNTS THE TYPE ITSELF, so an existing type is nested at least one level
/// deep and each user-defined type reached through a field adds one more. `Depth`'s
/// modelled `@default 0` is the protocol default for an unset member — the value a
/// client assumes when the service serialises none — and never the depth of a type the
/// service holds.
///
/// Derived from the keyspace's types on read, so a type whose fields are changed, or
/// whose nested types gain nesting of their own, is reported at the depth it then has.
///
/// BOUNDED, and bounded by the stored data rather than by the shape of the type graph:
/// the walk is iterative (no recursion to blow a stack) and MEMOISED in `depths`, so
/// every reachable type is expanded at most once however many times it is referred to
/// — a diamond or a repeated reference (`map<a, a>`) costs one expansion, not two. The
/// whole derivation is therefore at most [`MAX_TYPES_PER_KEYSPACE`] expansions of at
/// most [`MAX_FIELDS_PER_TYPE`] fields each: the same bounded class of work as
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`], and linear in what the
/// keyspace holds. A reference can only name a type that already exists (`CreateType`
/// refuses any other), so the relation is acyclic; `expanding` keeps the walk finite
/// even if a future operation ever changes that, by taking a reference back into the
/// chain currently being expanded as contributing no depth.
pub fn nesting_depth(types: &BTreeMap<String, UserDefinedType>, type_name: &str) -> i32 {
    /// What the walk still owes: a type whose references are not known yet, or one
    /// whose depth follows once its references have theirs.
    enum Step {
        Expand(String),
        Resolve(String, Vec<String>),
    }
    let mut depths: BTreeMap<String, i32> = BTreeMap::new();
    let mut expanding: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![Step::Expand(type_name.to_string())];
    while let Some(step) = stack.pop() {
        match step {
            Step::Expand(name) => {
                if depths.contains_key(&name) {
                    continue;
                }
                let references: Vec<String> = types
                    .get(&name)
                    .map(|udt| {
                        udt.field_definitions
                            .iter()
                            .flat_map(|field| referenced_type_names(&field.r#type))
                            .collect()
                    })
                    .unwrap_or_default();
                let pending: Vec<String> = references
                    .iter()
                    .filter(|reference| {
                        !depths.contains_key(*reference) && !expanding.contains(*reference)
                    })
                    .cloned()
                    .collect();
                expanding.insert(name.clone());
                // The resolve goes UNDER the references, so each of them has its depth
                // in `depths` by the time this type's own depth is taken.
                stack.push(Step::Resolve(name, references));
                stack.extend(pending.into_iter().map(Step::Expand));
            }
            Step::Resolve(name, references) => {
                // This type's own level, plus the deepest nesting any type it refers to
                // contributes on top of it.
                let deepest = 1 + references
                    .iter()
                    .map(|reference| depths.get(reference).copied().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                expanding.remove(&name);
                depths.insert(name, deepest);
            }
        }
    }
    depths.get(type_name).copied().unwrap_or(0)
}

/// What makes two identifiers the same one, for a comparison that is not a lookup of a
/// stored name: an unquoted identifier is case-folded (Amazon Keyspaces "converts upper
/// case characters automatically into lower case characters"), a double-quoted one
/// stands for itself. So `Id` and `id` are one field name, and `"Id"` is another.
///
/// This is what [`format_type_name`] does to a name it accepts, minus the refusal, so a
/// caller that only needs the identity of a name it is not storing (a duplicate check)
/// does not have to validate the form twice.
pub fn identifier_identity(supplied: &str) -> String {
    if supplied.len() > 1 && supplied.starts_with('"') && supplied.ends_with('"') {
        supplied.to_string()
    } else {
        supplied.to_ascii_lowercase()
    }
}

/// The formatted form of a `typeName`, or why the name cannot be one.
///
/// `CreateType$typeName`: "UDT names must contain 48 characters or less, must begin
/// with an alphabetic character, and can only contain alpha-numeric characters and
/// underscores. Amazon Keyspaces converts upper case characters automatically into
/// lower case characters. Alternatively, you can declare a UDT name in double quotes.
/// When declaring a UDT name inside double quotes, Amazon Keyspaces preserves upper
/// casing and allows special characters. You can also use double quotes as part of the
/// name when you create the UDT, but you must escape each double quote character with
/// an additional double quote character."
///
/// So there are two forms, and the formatted name is the one every other operation
/// takes (`GetType$typeName`: "if the name of the type was created without double
/// quotes, Amazon Keyspaces saved the name in lower-case characters. If the name was
/// created in double quotes, you must use double quotes to specify the type name"):
///
/// * unquoted — alphabetic first character, then alphanumerics and underscores,
///   case-folded to lower case;
/// * double-quoted — preserved verbatim, quotes included, with every interior double
///   quote doubled.
///
/// The model constrains only the length (`TypeName`, `len 1..48`), so the form is
/// checked here and a name that is neither is refused with the operation's
/// `ValidationException`.
pub fn format_type_name(supplied: &str) -> Result<String, String> {
    if let Some(rest) = supplied.strip_prefix('"') {
        let Some(inner) = rest.strip_suffix('"') else {
            return Err(format!(
                "typeName '{supplied}' opens a double-quoted name that is never closed"
            ));
        };
        if inner.is_empty() {
            return Err("typeName '\"\"' is a double-quoted name with no characters".to_string());
        }
        let mut characters = inner.chars();
        while let Some(character) = characters.next() {
            if character == '"' && characters.next() != Some('"') {
                return Err(format!(
                    "typeName '{supplied}' uses a double quote inside a double-quoted name \
                     without escaping it with an additional double quote character"
                ));
            }
        }
        return Ok(supplied.to_string());
    }
    let mut characters = supplied.chars();
    let first = characters.next().unwrap_or_default();
    if !first.is_ascii_alphabetic() || !characters.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "typeName '{supplied}' must begin with an alphabetic character and contain only \
             alpha-numeric characters and underscores, or be declared in double quotes"
        ));
    }
    Ok(supplied.to_ascii_lowercase())
}

/// The user-defined types a CQL type expression refers to, in the order they appear.
///
/// A field's or column's type is "any supported Cassandra data type, including
/// collections and other user-defined types that are contained in the same keyspace"
/// (`FieldDefinition$type`), so the UDTs it refers to are the identifiers in it that
/// name neither a native type ([`CQL_NATIVE_TYPES`]) nor a type constructor
/// ([`CQL_TYPE_CONSTRUCTORS`]): `frozen<list<address>>` refers to `address`, and
/// `map<text, int>` to nothing.
///
/// Each identifier is returned in its FORMATTED form, so it compares equal to a key of
/// [`Keyspace::types`]: an unquoted one case-folded, a double-quoted one verbatim with
/// its quotes. A token that cannot be an unquoted identifier at all (one that does not
/// begin with an alphabetic character, such as the `3` of `vector<float, 3>`) is not a
/// reference. The CQL textual type grammar has no maintained Rust crate (the Cassandra
/// crates parse the binary protocol, not this grammar), so the identifiers are scanned
/// here; this extracts references and does not claim to validate the grammar.
pub fn referenced_type_names(type_expression: &str) -> Vec<String> {
    let mut referenced = Vec::new();
    let characters: Vec<char> = type_expression.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '"' {
            // A quoted identifier runs to the next unescaped quote; `""` is one
            // escaped quote inside it, exactly as `format_type_name` accepts.
            let start = index;
            index += 1;
            while index < characters.len() {
                if characters[index] == '"' {
                    if characters.get(index + 1) == Some(&'"') {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                index += 1;
            }
            referenced.push(characters[start..index].iter().collect());
        } else if character.is_ascii_alphanumeric() || character == '_' {
            let start = index;
            while index < characters.len()
                && (characters[index].is_ascii_alphanumeric() || characters[index] == '_')
            {
                index += 1;
            }
            let identifier: String = characters[start..index]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase();
            // An unquoted identifier "must begin with an alphabetic character"
            // ([`format_type_name`]), so a token that does not — the `3` of a
            // parameterised type such as `vector<float, 3>` — names no type and is not a
            // reference to one.
            if identifier.starts_with(|first: char| first.is_ascii_alphabetic())
                && !CQL_NATIVE_TYPES.contains(&identifier.as_str())
                && !CQL_TYPE_CONSTRUCTORS.contains(&identifier.as_str())
            {
                referenced.push(identifier);
            }
        } else {
            index += 1;
        }
    }
    referenced
}

/// One [service-managed keyspace][SERVICE_MANAGED_KEYSPACES] — `keyspace_name`, holding
/// the system tables `table_names` — as the Amazon Web Services Region `region` holds it.
///
/// DERIVED per request rather than stored: these keyspaces are the SERVICE's, every Region
/// holds them before any call, no operation changes one, and their ARNs are
/// Region-qualified ([`keyspace_arn`], [`table_arn`]) — so the Region that is asking is
/// all it takes to report them, and a read of a Region nothing was ever created in stores
/// nothing to answer.
fn service_managed_keyspace(region: &str, keyspace_name: &str, table_names: &[&str]) -> Keyspace {
    let tables = table_names
        .iter()
        .map(|table_name| {
            (
                (*table_name).to_string(),
                Table {
                    keyspace_name: keyspace_name.to_string(),
                    table_name: (*table_name).to_string(),
                    resource_arn: table_arn(region, keyspace_name, table_name),
                    // Every member below is one only a `CreateTable` could have
                    // filled, and no create made this table: the service owns it.
                    creation_timestamp: None,
                    // A table the Region already held is in service: there is no
                    // create for the clock to owe a transition to.
                    status: TABLE_STATUS_ACTIVE.to_string(),
                    status_due_at: None,
                    schema_definition: None,
                    capacity_specification: None,
                    auto_scaling_specification: None,
                    last_update_to_pay_per_request: None,
                    encryption_specification: None,
                    point_in_time_recovery: None,
                    // No backup setting applies to a table the service owns, so it has no
                    // continuous backups and no restorable window.
                    continuous_backups_since: None,
                    ttl: None,
                    default_time_to_live: None,
                    comment: None,
                    client_side_timestamps: None,
                    replica_specifications: BTreeMap::new(),
                    warm_throughput_specification: None,
                    tags: TagSet::new(),
                    // Only a `cdcSpecification` creates a stream, and no call
                    // ever carried one for a table the service owns.
                    cdc_stream_tags: BTreeMap::new(),
                    superseded: Vec::new(),
                    restored_from: None,
                    earliest_known_settings: None,
                },
            )
        })
        .collect();
    Keyspace {
        name: keyspace_name.to_string(),
        resource_arn: keyspace_arn(region, keyspace_name),
        // A system keyspace is served in the Region it is read in, so it is
        // single-Region and reports no `replicationRegions`.
        replication_strategy: SINGLE_REGION_STRATEGY.to_string(),
        replication_regions: vec![region.to_string()],
        tags: TagSet::new(),
        // Nothing is being created, so the keyspace is usable now.
        status: KEYSPACE_STATUS_ACTIVE.to_string(),
        status_due_at: None,
        tables,
        // Amazon Keyspaces publishes no user-defined type in a system
        // keyspace, and no `CreateType` can put one there.
        types: BTreeMap::new(),
    }
}

impl ComAmazonawsKeyspacesKeyspacesserviceState {
    /// Every keyspace a request signed for `region` can see, in the one total order a
    /// listing pages through: the [service-managed catalog][SERVICE_MANAGED_KEYSPACES]
    /// that Region holds before any call, and the keyspaces created in that Region.
    ///
    /// A keyspace created in ANOTHER Region is not among them: a keyspace is a resource of
    /// the Region it was created in.
    pub fn keyspaces(&self, region: &str) -> Vec<Cow<'_, Keyspace>> {
        let mut visible: Vec<Cow<'_, Keyspace>> = SERVICE_MANAGED_KEYSPACES
            .iter()
            .map(|(keyspace_name, table_names)| {
                Cow::Owned(service_managed_keyspace(region, keyspace_name, table_names))
            })
            .collect();
        if let Some(created) = self.regions.get(region) {
            visible.extend(created.values().map(Cow::Borrowed));
        }
        // One total order over both sources, which is what makes a page boundary
        // resumable: a `keyspaceName` names at most one of them, so the order is total.
        visible.sort_by(|left, right| left.name.cmp(&right.name));
        visible
    }

    /// What a request signed for `region` sees of the keyspace `keyspace_name`: the one
    /// created in that Region, or the service-managed keyspace of that name, which every
    /// Region holds.
    ///
    /// `None` when that Region holds no keyspace of the name — which is what a keyspace
    /// created only in another Region is to this one.
    pub fn keyspace(&self, region: &str, keyspace_name: &str) -> Option<Cow<'_, Keyspace>> {
        if let Some(created) = self
            .regions
            .get(region)
            .and_then(|keyspaces| keyspaces.get(keyspace_name))
        {
            return Some(Cow::Borrowed(created));
        }
        SERVICE_MANAGED_KEYSPACES
            .iter()
            .find(|(managed, _)| *managed == keyspace_name)
            .map(|(managed, table_names)| {
                Cow::Owned(service_managed_keyspace(region, managed, table_names))
            })
    }

    /// Whether `region` already holds a keyspace named `keyspace_name`, which is what
    /// makes a create of that name a name already taken — in that Region alone.
    pub fn holds_keyspace(&self, region: &str, keyspace_name: &str) -> bool {
        is_service_managed_keyspace(keyspace_name)
            || self
                .regions
                .get(region)
                .is_some_and(|keyspaces| keyspaces.contains_key(keyspace_name))
    }

    /// How many keyspaces `region` holds, which [`MAX_KEYSPACES`] bounds: the ones created
    /// there and the [`SERVICE_MANAGED_KEYSPACES`] every Region holds.
    pub fn keyspace_count(&self, region: &str) -> usize {
        SERVICE_MANAGED_KEYSPACES.len()
            + self
                .regions
                .get(region)
                .map_or(0, std::collections::BTreeMap::len)
    }

    /// The keyspace `keyspace_name` of `region` as a resource to WRITE: one a caller
    /// created there, and nothing else.
    ///
    /// A service-managed keyspace is never reachable here, because it is the service's
    /// own and takes no change at all: every operation that writes refuses one before it
    /// looks ([`is_service_managed_keyspace`]). Opens no Region partition, so a write
    /// against a Region holding nothing stores nothing.
    pub fn keyspace_mut(&mut self, region: &str, keyspace_name: &str) -> Option<&mut Keyspace> {
        self.regions.get_mut(region)?.get_mut(keyspace_name)
    }

    /// Store the keyspace `CreateKeyspace` has just created in `region`, opening that
    /// Region's partition when this is the first keyspace created there.
    ///
    /// The store's only growth site in the number of Regions, which is why
    /// [`Self::region_capacity_reached`] is what the create checks before reaching it.
    pub fn insert_keyspace(&mut self, region: &str, keyspace: Keyspace) {
        self.regions
            .entry(region.to_string())
            .or_default()
            .insert(keyspace.name.clone(), keyspace);
    }

    /// Whether a create whose keyspace would be the FIRST one stored in `region` is past
    /// the [`MAX_REGIONS`] ceiling, so the operation must refuse it rather than open a
    /// further partition. A Region the store already holds keyspaces for is never past it.
    pub fn region_capacity_reached(&self, region: &str) -> bool {
        !self.regions.contains_key(region) && self.regions.len() >= MAX_REGIONS
    }

    /// Record that some table or type now has a transition due at `due`, so
    /// [`Self::settle`] knows the earliest instant at which it has to look at the store
    /// again.
    pub fn note_transition_due(&mut self, due: SystemTime) {
        self.next_transition_at = earlier_of(self.next_transition_at, due);
    }

    /// Materialise every status transition the clock already owes at `now`: a keyspace
    /// whose `CREATING` or `UPDATING` span has elapsed becomes `ACTIVE`, a table whose `CREATING`,
    /// `RESTORING` or `UPDATING` span has elapsed becomes `ACTIVE`, a table's warm throughput
    /// configuration whose span has elapsed becomes `AVAILABLE`, a type whose `CREATING` span has
    /// elapsed becomes `ACTIVE`, and a keyspace, table or type whose
    /// `DELETING` span has elapsed is gone.
    ///
    /// Every handler calls this first, including the reporting ones. That is the one write
    /// a report is allowed: the transition was due at `now` whether or not anybody called,
    /// so materialising it changes nothing a later caller would not have seen anyway.
    ///
    /// Nothing is scanned until the earliest due instant arrives, and the scan that does
    /// run is bounded by [`MAX_REGIONS`] x [`MAX_KEYSPACES`] x ([`MAX_TABLES_PER_KEYSPACE`]
    /// + [`MAX_TYPES_PER_KEYSPACE`]) and runs at most once per transition that becomes due.
    pub fn settle(&mut self, now: SystemTime) {
        if self.next_transition_at.is_none_or(|next| now < next) {
            return;
        }
        let mut earliest: Option<SystemTime> = None;
        for keyspaces in self.regions.values_mut() {
            keyspaces.retain(|_, keyspace| {
                match keyspace.status_due_at {
                    // Still pending: nothing to materialise, but `settle` has to look at
                    // this keyspace again once its span elapses.
                    Some(due) if now < due => earliest = earlier_of(earliest, due),
                    Some(_) => {
                        keyspace.status_due_at = None;
                        // The deletion `DeleteKeyspace` started has completed, so the
                        // keyspace — and every table and type it held, which live inside
                        // this record — is no longer a resource of its Region at all.
                        if keyspace.is_deleting() {
                            return false;
                        }
                        // The creation `CreateKeyspace` started, or the replication change
                        // `UpdateKeyspace` started, has completed, so the keyspace is
                        // usable again.
                        if is_in_flight_status(&keyspace.status) {
                            keyspace.status = KEYSPACE_STATUS_ACTIVE.to_string();
                        }
                    }
                    None => {}
                }
                keyspace.tables.retain(|_, table| {
                    // A warm throughput configuration settles on ITS own instant: it is
                    // `UPDATING` "when changes are being applied" and `AVAILABLE` once they
                    // are, which is a different clock from the table's status, since a
                    // later update of any other setting puts the table back in `UPDATING`
                    // without changing the warm throughput at all.
                    if let Some(warm) = table.warm_throughput_specification.as_mut() {
                        match warm.status_due_at {
                            Some(due) if now < due => earliest = earlier_of(earliest, due),
                            Some(_) => warm.status_due_at = None,
                            None => {}
                        }
                    }
                    let Some(due) = table.status_due_at else {
                        return true;
                    };
                    if now < due {
                        earliest = earlier_of(earliest, due);
                        return true;
                    }
                    table.status_due_at = None;
                    match table.status.as_str() {
                        // The creation, the restore, or the setting change, the clock
                        // owed has completed, so the table is usable again.
                        TABLE_STATUS_CREATING
                        | TABLE_STATUS_RESTORING
                        | TABLE_STATUS_UPDATING => {
                            table.status = TABLE_STATUS_ACTIVE.to_string();
                            true
                        }
                        // The deletion the clock owed has completed, so the table is
                        // no longer a resource of this keyspace at all.
                        TABLE_STATUS_DELETING => false,
                        _ => true,
                    }
                });
                // A type has the two transitions its operations start: `CREATING` ->
                // `ACTIVE` from `CreateType`, and `DELETING` -> gone from `DeleteType`.
                keyspace.types.retain(|_, udt| {
                    let Some(due) = udt.status_due_at else {
                        return true;
                    };
                    if now < due {
                        earliest = earlier_of(earliest, due);
                        return true;
                    }
                    udt.status_due_at = None;
                    match udt.status.as_str() {
                        // The creation the clock owed has completed, so the type is usable.
                        TYPE_STATUS_CREATING => {
                            udt.status = TYPE_STATUS_ACTIVE.to_string();
                            true
                        }
                        // The deletion the clock owed has completed, so the type is no
                        // longer a resource of this keyspace at all.
                        TYPE_STATUS_DELETING => false,
                        _ => true,
                    }
                });
                // Nothing is owed for this keyspace itself, so it stays.
                true
            });
        }
        // A Region whose last created keyspace is gone holds nothing a caller created, and
        // the catalog it still reports is derived — so its partition is dropped rather than
        // kept for the life of the process.
        self.regions.retain(|_, keyspaces| !keyspaces.is_empty());
        self.next_transition_at = earliest;
    }

    /// The tags of the resource `resource_arn` names to a request signed for `region`, or
    /// `None` when it names no resource that Region holds.
    ///
    /// The ARN is the key, not the name it embeds: a resource answers only to the ARN it
    /// was minted with ([`keyspace_arn`], [`table_arn`]), which is Region-qualified — so
    /// an ARN for another partition or another Region reaches nothing here even when a
    /// resource of that name exists there.
    pub fn resource_tags(&self, region: &str, resource_arn: &str) -> Option<Cow<'_, TagSet>> {
        let keyspace = self.keyspace(region, &arn_keyspace_name(resource_arn)?)?;
        match parse_resource_arn(resource_arn)? {
            ResourceRef::Keyspace { .. } => {
                if keyspace.resource_arn != resource_arn {
                    return None;
                }
                Some(match keyspace {
                    Cow::Borrowed(created) => Cow::Borrowed(&created.tags),
                    Cow::Owned(managed) => Cow::Owned(managed.tags),
                })
            }
            ResourceRef::Table { table_name, .. } => match keyspace {
                Cow::Borrowed(created) => {
                    let table = created.tables.get(&table_name)?;
                    (table.resource_arn == resource_arn).then_some(Cow::Borrowed(&table.tags))
                }
                Cow::Owned(managed) => {
                    let table = managed.tables.get(&table_name)?;
                    (table.resource_arn == resource_arn)
                        .then(|| Cow::Owned(table.tags.clone()))
                }
            },
            // A stream answers for the tags its own ARN was registered with, so a label
            // that names none of the table's streams reaches nothing here — and a system
            // table has no stream at all, because no `cdcSpecification` ever enabled one
            // on a table the service owns.
            ResourceRef::Stream { table_name, .. } => match keyspace {
                Cow::Borrowed(created) => created
                    .tables
                    .get(&table_name)?
                    .cdc_stream_tags
                    .get(resource_arn)
                    .map(Cow::Borrowed),
                Cow::Owned(_) => None,
            },
        }
    }

    /// How many restores are running in this account: "Any number of users can execute up
    /// to 4 concurrent restores (any type of restore) in a given account"
    /// ([`MAX_CONCURRENT_RESTORES`]), and a restore is running exactly while the table it
    /// is creating is still `RESTORING` — which [`Self::settle`] materialises out of that
    /// status when the clock owes it, so a finished restore stops counting without anybody
    /// having to clear a flag.
    ///
    /// The quota the sentence states is the ACCOUNT's, so every Region's tables count
    /// towards it.
    pub fn restores_in_progress(&self) -> usize {
        self.regions
            .values()
            .flat_map(|keyspaces| keyspaces.values())
            .flat_map(|keyspace| keyspace.tables.values())
            .filter(|table| {
                table.restored_from.is_some() && table.status == TABLE_STATUS_RESTORING
            })
            .count()
    }

    /// Why the resource `resource_arn` names to a request signed for `region` takes no
    /// further change — the refusal a write against it carries — or `None` when no
    /// deletion stands in that write's way.
    ///
    /// A resource whose deletion has been accepted is on its way out and takes no further
    /// change until that deletion completes ([`keyspace_deleting_rejection`],
    /// [`table_deleting_rejection`]): the record [`Self::settle`] is about to drop would
    /// throw the write away wholesale, so reporting it as stored would report a durable
    /// change that never was. `DELETING` is the only status that blocks a write here — a
    /// resource whose creation or update is still settling is still a resource that stays.
    ///
    /// Deleting a keyspace deletes everything in it ("The `DeleteKeyspace` operation
    /// deletes a keyspace and all of its tables"), so the keyspace's own deletion answers
    /// for every ARN below it; a table's own deletion answers for the table and for the CDC
    /// streams that follow its changes, which cannot outlive it.
    ///
    /// An ARN that names no resource of that Region yields `None`: that is the caller's
    /// `ResourceNotFoundException`, not a conflict, and only a resource that is there can
    /// be on its way out.
    pub fn deletion_in_progress_rejection(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<String> {
        // Resolved through the reading lookup, so the ARN is held to exactly the resource
        // it was minted for — an ARN of another partition, Region or account reaches
        // nothing here and is left to the 404.
        self.resource_tags(region, resource_arn)?;
        // Parsed ONCE: the ARN's two halves — the keyspace that owns the resource and, for
        // an ARN below a keyspace, the table the write reaches through — come out of the
        // same `ResourceRef`.
        let (keyspace_name, table_name) = match parse_resource_arn(resource_arn)? {
            ResourceRef::Keyspace { keyspace_name } => (keyspace_name, None),
            ResourceRef::Table {
                keyspace_name,
                table_name,
            }
            | ResourceRef::Stream {
                keyspace_name,
                table_name,
                ..
            } => (keyspace_name, Some(table_name)),
        };
        let keyspace = self.keyspace(region, &keyspace_name)?;
        // The keyspace's deletion takes everything in it with it, so it answers for every
        // ARN below it before the addressed resource's own status is looked at.
        if keyspace.is_deleting() {
            return Some(keyspace_deleting_rejection(&keyspace_name));
        }
        // A keyspace ARN has nothing below it, so the keyspace's own status was the whole
        // question for it.
        let table_name = table_name?;
        let table = keyspace.tables.get(&table_name)?;
        is_departing_status(&table.status)
            .then(|| table_deleting_rejection(&keyspace_name, &table_name))
    }

    /// The same lookup, for a tagging operation that writes.
    ///
    /// Only a resource a caller CREATED in `region` is reachable: the service's own
    /// resources take no change, and every tagging operation refuses one before it looks
    /// ([`is_service_managed_keyspace`]).
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&mut TagSet> {
        let keyspaces = self.regions.get_mut(region)?;
        match parse_resource_arn(resource_arn)? {
            ResourceRef::Keyspace { keyspace_name } => {
                let keyspace = keyspaces.get_mut(&keyspace_name)?;
                (keyspace.resource_arn == resource_arn).then_some(&mut keyspace.tags)
            }
            ResourceRef::Table {
                keyspace_name,
                table_name,
            } => {
                let table = keyspaces
                    .get_mut(&keyspace_name)?
                    .tables
                    .get_mut(&table_name)?;
                (table.resource_arn == resource_arn).then_some(&mut table.tags)
            }
            ResourceRef::Stream {
                keyspace_name,
                table_name,
                ..
            } => keyspaces
                .get_mut(&keyspace_name)?
                .tables
                .get_mut(&table_name)?
                .cdc_stream_tags
                .get_mut(resource_arn),
        }
    }

    /// Drop the tag sets of the CDC streams `forgotten_stream_arns` names in `region`.
    ///
    /// A tag set belongs to a stream, so it lives exactly as long as that stream: the
    /// call that forgets streams in the CDC stream store drops their tags HERE, in the
    /// same request, so the tagging operations can never answer for a stream ARN that
    /// store no longer holds — not even in the span before [`Self::settle`] removes the
    /// table that held them. A stream ARN names its own keyspace and table
    /// ([`parse_resource_arn`]), so a forgotten ARN reaches exactly the one tag set that
    /// belonged to it and nothing else.
    pub fn forget_stream_tags(&mut self, region: &str, forgotten_stream_arns: &[String]) {
        let Some(keyspaces) = self.regions.get_mut(region) else {
            return;
        };
        for stream_arn in forgotten_stream_arns {
            let Some(ResourceRef::Stream {
                keyspace_name,
                table_name,
                ..
            }) = parse_resource_arn(stream_arn)
            else {
                continue;
            };
            if let Some(table) = keyspaces
                .get_mut(&keyspace_name)
                .and_then(|keyspace| keyspace.tables.get_mut(&table_name))
            {
                table.cdc_stream_tags.remove(stream_arn.as_str());
            }
        }
    }
}

impl Table {
    /// Register the tag set of a CDC stream the control plane has just enabled for this
    /// table, under the stream's own ARN, and forget the tag set of every stream
    /// `held_stream_arns` no longer names.
    ///
    /// A tag set belongs to a stream, and the CDC stream store caps how many streams one
    /// table keeps ([`MAX_STREAMS_PER_TABLE`]) — so the pruning here is driven by the
    /// ARNs that store still holds, not by a second cap of its own. That is what keeps
    /// the two from drifting: `ListTagsForResource` can never answer for a stream
    /// `GetStream` no longer knows.
    pub fn register_stream_tags(
        &mut self,
        stream_arn: &str,
        tags: TagSet,
        held_stream_arns: &[String],
    ) {
        self.cdc_stream_tags.insert(stream_arn.to_string(), tags);
        self.cdc_stream_tags
            .retain(|arn, _| held_stream_arns.iter().any(|held| held == arn));
    }
}

/// The earlier of an instant already seen (if any) and `due`: how both
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::note_transition_due`] and
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`] keep the single earliest instant at
/// which the store has a transition to materialise.
fn earlier_of(seen: Option<SystemTime>, due: SystemTime) -> Option<SystemTime> {
    Some(seen.map_or(due, |existing| existing.min(due)))
}

/// The region this request was signed for — the third field of its SigV4
/// credential scope — or [`DEFAULT_REGION`] when it carries no usable scope.
///
/// Keyspaces ARNs are regional, so every minted ARN is built with this.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A keyspace's ARN: `arn:aws:cassandra:<region>:<account>:/keyspace/<name>/`.
///
/// The trailing `/` is part of the published Keyspaces keyspace-ARN form and is
/// what distinguishes it from a table ARN under the same keyspace.
pub fn keyspace_arn(region: &str, keyspace_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/"),
    )
}

/// A table's ARN: `arn:aws:cassandra:<region>:<account>:/keyspace/<ks>/table/<tbl>`.
pub fn table_arn(region: &str, keyspace_name: &str, table_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/table/{table_name}"),
    )
}

/// A user-defined type's ARN:
/// `arn:aws:cassandra:<region>:<account>:/keyspace/<ks>/type/<type>`.
///
/// The `type` resource segment is the published Keyspaces form for a UDT (the
/// resource an IAM policy names to grant `cassandra:Create` on a type). No operation
/// of this service returns it — `CreateType` and `GetType` both answer with the
/// KEYSPACE ARN — so its only use is naming the resource a
/// `ResourceNotFoundException` looked for.
pub fn type_arn(region: &str, keyspace_name: &str, type_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/type/{type_name}"),
    )
}

/// The `ResourceNotFoundException` payload for a keyspace that is not in state.
///
/// The message is the Cassandra-style form Amazon Keyspaces builds from the request for
/// a keyspace lookup — the bare `keyspaceName`, unquoted and with no trailing period —
/// the same shape of sentence a missing type takes ([`type_not_found`]).
///
/// The ARN of the resource that was looked for goes in the modelled
/// `resourceArn` member, which is what a client reads to learn WHICH resource
/// was missing.
pub fn keyspace_not_found(region: &str, keyspace_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Keyspace {keyspace_name} does not exist")),
        resource_arn: Some(keyspace_arn(region, keyspace_name)),
    }
}

/// The `ResourceNotFoundException` payload for a table that is not in state.
///
/// The message is the Cassandra-style form Amazon Keyspaces builds from the request for a
/// table lookup — the bare `tableName`, unqualified and unquoted — which is what
/// Cassandra-compatible tooling keys off. A keyspace lookup keeps its own form
/// ([`keyspace_not_found`]).
pub fn table_not_found(
    region: &str,
    keyspace_name: &str,
    table_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("unconfigured table {table_name}")),
        resource_arn: Some(table_arn(region, keyspace_name, table_name)),
    }
}

/// The `ResourceNotFoundException` payload for a user-defined type that is not in
/// state, or that is in state in a status other than `ACTIVE` ("The operation tried to
/// access a keyspace, table, or type that doesn't exist. The resource might not be
/// specified correctly, or its status might not be ACTIVE").
///
/// The message is the request-derived Cassandra-style form Amazon Keyspaces answers an
/// unresolvable UDT name with, `UDT <name> does not exist` — the form a recording of
/// the real service shows for a column whose type names no type of the keyspace.
pub fn type_not_found(
    region: &str,
    keyspace_name: &str,
    type_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("UDT {type_name} does not exist")),
        resource_arn: Some(type_arn(region, keyspace_name, type_name)),
    }
}

/// The capacity specification a table is created or updated with: the caller's,
/// or the modelled default (`PAY_PER_REQUEST`) when a create omits it.
///
/// The capacity units belong to `PROVISIONED` and to nothing else: that mode
/// "requires `readCapacityUnits` and `writeCapacityUnits` as input"
/// (`CapacitySpecification$throughputMode`), and no other mode takes them. The
/// model expresses neither half of that, so both are refused here, naming the
/// member the caller has to fix.
pub fn resolve_capacity(
    supplied: Option<CapacitySpecification>,
) -> Result<CapacitySpecification, String> {
    let Some(capacity) = supplied else {
        return Ok(CapacitySpecification {
            throughput_mode: DEFAULT_THROUGHPUT_MODE.to_string(),
            read_capacity_units: None,
            write_capacity_units: None,
        });
    };
    let units = [
        ("readCapacityUnits", capacity.read_capacity_units.is_some()),
        ("writeCapacityUnits", capacity.write_capacity_units.is_some()),
    ];
    if capacity.throughput_mode == PROVISIONED_THROUGHPUT_MODE {
        let missing: Vec<&str> = units
            .iter()
            .filter(|(_, present)| !*present)
            .map(|(name, _)| *name)
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "capacitySpecification with throughputMode '{PROVISIONED_THROUGHPUT_MODE}' requires {}",
                missing.join(" and ")
            ));
        }
    } else {
        let supplied_units: Vec<&str> = units
            .iter()
            .filter(|(_, present)| *present)
            .map(|(name, _)| *name)
            .collect();
        if !supplied_units.is_empty() {
            return Err(format!(
                "capacitySpecification with throughputMode '{}' does not take {}; they are input for '{PROVISIONED_THROUGHPUT_MODE}' only",
                capacity.throughput_mode,
                supplied_units.join(" or ")
            ));
        }
    }
    Ok(capacity)
}

/// The encryption specification a table is created or updated with: the
/// caller's, or the modelled default (`AWS_OWNED_KMS_KEY`) when a create omits
/// it.
///
/// The key identifier belongs to `CUSTOMER_MANAGED_KMS_KEY` and to nothing else:
/// that type "requires the `kms_key_identifier` of the KMS key in Amazon Resource
/// Name (ARN) format as input" (`EncryptionSpecification$type`), and no other type
/// takes one — the Amazon-owned key "is owned by Amazon Keyspaces", so there is no
/// key of the caller's to name. The model expresses neither half of that, so both
/// are refused here, naming the member the caller has to fix.
pub fn resolve_encryption(
    supplied: Option<EncryptionSpecification>,
) -> Result<EncryptionSpecification, String> {
    let Some(encryption) = supplied else {
        return Ok(EncryptionSpecification {
            r#type: DEFAULT_ENCRYPTION_TYPE.to_string(),
            kms_key_identifier: None,
        });
    };
    if encryption.r#type == CUSTOMER_MANAGED_ENCRYPTION_TYPE {
        if encryption.kms_key_identifier.is_none() {
            return Err(format!(
                "encryptionSpecification with type '{CUSTOMER_MANAGED_ENCRYPTION_TYPE}' requires kmsKeyIdentifier"
            ));
        }
    } else if encryption.kms_key_identifier.is_some() {
        return Err(format!(
            "encryptionSpecification with type '{}' does not take kmsKeyIdentifier; it is input for '{CUSTOMER_MANAGED_ENCRYPTION_TYPE}' only",
            encryption.r#type
        ));
    }
    Ok(encryption)
}

/// What a `cdcSpecification` asks the service to do with the table's CDC stream.
#[derive(Debug, Clone)]
pub struct ResolvedCdc {
    /// Whether the table is to have a stream capturing its changes.
    pub enabled: bool,
    /// The `ViewType` that stream publishes, defaulted when the specification omits it.
    /// It decides which images a record carries, which is fixed when the stream is
    /// created ("After you create the stream, you can't make changes to this
    /// selection"), so it says nothing about a table whose CDC is being turned off.
    pub view_type: String,
    /// "The tags (key-value pairs) that you want to apply to the stream"
    /// (`CdcSpecification$tags`), exactly as supplied.
    pub tags: Vec<Tag>,
    /// Whether the stream also inherits the TABLE's tags
    /// (`CdcSpecification$propagateTags` = [`CDC_PROPAGATE_TAGS_TABLE`]).
    pub propagate_table_tags: bool,
}

/// What a `cdcSpecification` asks for, or `None` when the request carries none.
///
/// `CdcStatus` and `ViewType` are modelled enums, so the boundary validator has already
/// refused a value outside either. What the model cannot state is which statuses ask for
/// a capturing stream — `ENABLED`, and `ENABLING`, which is the status a stream already
/// being brought up reports — and that the view type has a documented default.
pub fn resolve_cdc(supplied: Option<&CdcSpecification>) -> Option<ResolvedCdc> {
    let specification = supplied?;
    Some(ResolvedCdc {
        enabled: matches!(
            specification.status.as_str(),
            CDC_STATUS_ENABLED | CDC_STATUS_ENABLING
        ),
        view_type: specification
            .view_type
            .clone()
            .unwrap_or_else(|| DEFAULT_CDC_VIEW_TYPE.to_string()),
        tags: specification.tags.clone().unwrap_or_default(),
        propagate_table_tags: specification.propagate_tags.as_deref()
            == Some(CDC_PROPAGATE_TAGS_TABLE),
    })
}

/// The tag set a table's CDC stream is created with: the tags the specification applies
/// to the stream, onto the table's own tags when it asks to inherit them.
///
/// The per-resource tag rules are the ones every other tag set here follows
/// ([`merge_tags`]): no reserved key, one tag per key, at most
/// [`MAX_TAGS_PER_RESOURCE`] on the resource — the stream being a resource of its own,
/// addressed by its own ARN.
pub fn new_stream_tag_set(cdc: &ResolvedCdc, table_tags: &TagSet) -> Result<TagSet, TagRejection> {
    let mut tags = match cdc.propagate_table_tags {
        true => table_tags.clone(),
        false => TagSet::new(),
    };
    merge_tags(&mut tags, &cdc.tags)?;
    Ok(tags)
}

/// The `cdcSpecification` a report of a table carries, from the stream the CDC stream
/// store holds for it: its status and the view type it publishes.
///
/// Derived on read, never stored twice: a table's CDC status IS its stream's
/// `StreamStatus` (the two enums carry the same four values), so the control plane keeps
/// no copy that could disagree with what KeyspacesStreams reports.
pub fn cdc_specification_summary(stream: &CdcStreamEntry) -> CdcSpecificationSummary {
    CdcSpecificationSummary {
        status: stream.stream_status.clone(),
        view_type: Some(stream.stream_view_type.clone()),
    }
}

/// Check that the schema's roles and columns agree: every key and static column
/// is a column the schema declares, no column is named twice, and a static column
/// has a partition to share its value across.
///
/// `partitionKeys`, `clusteringKeys` and `staticColumns` each name "the columns
/// ... of the table" (`SchemaDefinition`), so a name that `allColumns` does not
/// declare is not a column of the table. And a column holds ONE role: it is part
/// of the partition key, or of the clustering key, or static, or an ordinary
/// column — the storage engine has no row shape in which a column is two of
/// those, nor one in which `allColumns` declares a name twice. The model
/// expresses only that each list is non-empty, not that the lists agree.
///
/// A static column is one whose values "are shared by all rows in the same partition"
/// (`SchemaDefinition$staticColumns`), and what divides a partition into rows is its
/// clustering columns ("the optional clustering column portion of your primary key
/// determines how the data is clustered and sorted within each partition",
/// `ClusteringKey`). A schema with no clustering column therefore holds one row per
/// partition, leaving a static column nothing to share and no storage distinct from an
/// ordinary column: the CQL storage model this service implements has no such table, so
/// such a schema is an invalid definition rather than a table with a redundant column.
///
/// A column name is an identifier, so WHICH column a name addresses is decided by
/// [`identifier_identity`] and not by the spelling: an unquoted name is case-folded, a
/// double-quoted one stands for itself. Two spellings of one identifier are therefore one
/// column wherever this check compares names — a second `allColumns` entry for it is the
/// same name declared twice, and a key or static column spelled in another casing names
/// the column `allColumns` declares rather than one it does not.
pub fn validate_schema(schema: &SchemaDefinition) -> Result<(), String> {
    let mut declared: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for column in &schema.all_columns {
        if !declared.insert(identifier_identity(&column.name)) {
            return Err(format!(
                "schemaDefinition.allColumns declares column '{}' more than once",
                column.name
            ));
        }
    }
    let roles = schema
        .partition_keys
        .iter()
        .map(|key| ("partitionKeys", key.name.as_str()))
        .chain(
            schema
                .clustering_keys
                .iter()
                .flatten()
                .map(|key| ("clusteringKeys", key.name.as_str())),
        )
        .chain(
            schema
                .static_columns
                .iter()
                .flatten()
                .map(|column| ("staticColumns", column.name.as_str())),
        );
    let mut assigned: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for (member, name) in roles {
        let identity = identifier_identity(name);
        if !declared.contains(&identity) {
            return Err(format!(
                "schemaDefinition.{member} names column '{name}', which schemaDefinition.allColumns does not declare"
            ));
        }
        if !assigned.insert(identity) {
            return Err(format!(
                "schemaDefinition.{member} names column '{name}', which the schema already gives another role"
            ));
        }
    }
    if schema.clustering_keys.iter().flatten().next().is_none() {
        if let Some(first_static) = schema.static_columns.iter().flatten().next() {
            return Err(format!(
                "schemaDefinition.staticColumns names column '{}', but static columns are only \
                 useful (and thus allowed) if the table has at least one clustering column",
                first_static.name
            ));
        }
    }
    Ok(())
}

/// One `AutoScalingSettings`, checked against what the service documents for it; `member`
/// names the request member it came from, so a refusal tells the caller what to fix.
///
/// For a capacity auto scaling is turned on for ("This optional parameter enables auto
/// scaling for the table if set to `false`"), "you must specify the following parameters":
/// `minimumUnits`, `maximumUnits` and `scalingPolicy`, whose target tracking configuration
/// defines `targetValue` as "a percentage. A `double` between 20 and 90". The model states
/// none of that, only that the unit members are `long`s with a minimum of 1 — so it holds
/// wherever the shape appears: `autoScalingSpecification`'s two capacities and a
/// `replicaSpecifications` entry's `readCapacityAutoScaling`.
pub fn validate_auto_scaling_settings(
    member: &str,
    settings: &AutoScalingSettings,
) -> Result<(), String> {
    if settings.auto_scaling_disabled {
        return Ok(());
    }
    let target_value = settings
        .scaling_policy
        .as_ref()
        .and_then(|policy| policy.target_tracking_scaling_policy_configuration.as_ref())
        .map(|configuration| configuration.target_value);
    let (Some(minimum), Some(maximum), Some(target_value)) =
        (settings.minimum_units, settings.maximum_units, target_value)
    else {
        return Err(format!(
            "{member} with autoScalingDisabled false requires minimumUnits, maximumUnits and \
             scalingPolicy.targetTrackingScalingPolicyConfiguration.targetValue"
        ));
    };
    if minimum > maximum {
        return Err(format!(
            "{member}.minimumUnits ({minimum}) must not exceed maximumUnits ({maximum})"
        ));
    }
    if !(MIN_TARGET_VALUE..=MAX_TARGET_VALUE).contains(&target_value) {
        return Err(format!(
            "{member}.scalingPolicy.targetTrackingScalingPolicyConfiguration.targetValue \
             ({target_value}) must be between {MIN_TARGET_VALUE} and {MAX_TARGET_VALUE}"
        ));
    }
    Ok(())
}

/// The auto scaling settings a table is created or updated with: the caller's, checked
/// against what the service documents for them, or `None`.
///
/// Auto scaling is "The optional auto scaling settings for a table in provisioned capacity
/// mode" (`CreateTableRequest$autoScalingSpecification`), so it belongs to
/// `throughputMode:PROVISIONED` and to nothing else; each capacity's settings are then
/// what [`validate_auto_scaling_settings`] holds them to. `UpdateTable` is THE operation
/// that changes the setting afterwards — "If auto scaling is already enabled for the
/// table, you can use UpdateTable to update the minimum and maximum values or the auto
/// scaling policy settings independently" — so it resolves the caller's specification
/// against the capacity mode the table is in now.
pub fn resolve_auto_scaling(
    supplied: Option<AutoScalingSpecification>,
    capacity: &CapacitySpecification,
) -> Result<Option<AutoScalingSpecification>, String> {
    let Some(specification) = supplied else {
        return Ok(None);
    };
    if capacity.throughput_mode != PROVISIONED_THROUGHPUT_MODE {
        return Err(format!(
            "autoScalingSpecification is the auto scaling setting of a table in capacity mode '{PROVISIONED_THROUGHPUT_MODE}'; \
             capacitySpecification.throughputMode is '{}'",
            capacity.throughput_mode
        ));
    }
    for (member, settings) in [
        (
            "autoScalingSpecification.readCapacityAutoScaling",
            specification.read_capacity_auto_scaling.as_ref(),
        ),
        (
            "autoScalingSpecification.writeCapacityAutoScaling",
            specification.write_capacity_auto_scaling.as_ref(),
        ),
    ] {
        let Some(settings) = settings else {
            continue;
        };
        validate_auto_scaling_settings(member, settings)?;
    }
    Ok(Some(specification))
}

/// The per-Region settings a table is created or updated with, keyed by Region and
/// merged onto the ones it already holds.
///
/// `replicaSpecifications` is "The Amazon Web Services Region specific settings of a
/// multi-Region table": for one "you can configure the table's read capacity differently
/// per Amazon Web Services Region" through `readCapacityUnits` and
/// `readCapacityAutoScaling` (`ReplicaSpecification`). So the rules the model cannot
/// state are:
///
/// * the Regions are the ones the table IS replicated in — `replica_regions` is
///   [`Keyspace::table_replica_regions`], and a table of a single-Region keyspace has
///   none, so it takes no per-Region setting at all;
/// * a Region appears once, since a Region holds one such setting;
/// * a read capacity override belongs to `throughputMode:PROVISIONED`, the only mode
///   whose read capacity is provisioned at all; and
/// * a `readCapacityAutoScaling` is held to the same rules as the table's own
///   ([`validate_auto_scaling_settings`]).
///
/// An entry carrying neither override states that its Region uses the table's own read
/// capacity and auto scaling, which is what a Region with no entry does
/// ([`Table::read_capacity_units_in`], [`Table::replica_auto_scaling`]) — so it is accepted
/// and resets that Region.
pub fn resolve_replica_specifications(
    supplied: Vec<ReplicaSpecification>,
    existing: &BTreeMap<String, ReplicaSpecification>,
    capacity: Option<&CapacitySpecification>,
    replica_regions: Option<&[String]>,
) -> Result<BTreeMap<String, ReplicaSpecification>, String> {
    // The one cap on a caller-chosen entry count (`ReplicaSpecificationList` is modelled
    // `len 1..` with no maximum), applied before the list is walked or cloned: a table is
    // replicated in at most [`MAX_REPLICATION_REGIONS`] Regions, so a longer list cannot
    // name a Region per entry anyway.
    if supplied.len() > MAX_REPLICATION_REGIONS {
        return Err(format!(
            "replicaSpecifications carries {} entries, which exceeds the limit of \
             {MAX_REPLICATION_REGIONS} replication Regions per keyspace",
            supplied.len()
        ));
    }
    let Some(replica_regions) = replica_regions else {
        return Err(
            "replicaSpecifications is the Amazon Web Services Region specific setting of a \
             multi-Region table, and this table's keyspace is replicated in one Region only"
                .to_string(),
        );
    };
    let mut resolved = existing.clone();
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for specification in &supplied {
        if !replica_regions.contains(&specification.region) {
            return Err(format!(
                "replicaSpecifications names Region '{}', which the table is not replicated \
                 in: its Regions are {}",
                specification.region,
                replica_regions.join(", ")
            ));
        }
        if !named.insert(specification.region.as_str()) {
            return Err(format!(
                "replicaSpecifications names Region '{}' more than once, and a Region holds \
                 one set of per-Region settings",
                specification.region
            ));
        }
        let overrides_read_capacity = specification.read_capacity_units.is_some()
            || specification.read_capacity_auto_scaling.is_some();
        let throughput_mode = capacity.map(|capacity| capacity.throughput_mode.as_str());
        if overrides_read_capacity && throughput_mode != Some(PROVISIONED_THROUGHPUT_MODE) {
            return Err(format!(
                "replicaSpecifications entry for Region '{}' overrides the read capacity of a \
                 table in capacity mode '{PROVISIONED_THROUGHPUT_MODE}'; \
                 capacitySpecification.throughputMode is '{}'",
                specification.region,
                throughput_mode.unwrap_or("unset")
            ));
        }
        if let Some(settings) = specification.read_capacity_auto_scaling.as_ref() {
            validate_auto_scaling_settings(
                &format!(
                    "replicaSpecifications entry for Region '{}' readCapacityAutoScaling",
                    specification.region
                ),
                settings,
            )?;
        }
        resolved.insert(specification.region.clone(), specification.clone());
    }
    Ok(resolved)
}

/// The warm throughput a table is created or updated with: the caller's two optional unit
/// counts, merged onto the configuration the table already holds, and owed its
/// [`WARM_THROUGHPUT_STATUS_AVAILABLE`] status at `settles_at`.
///
/// Both members of `WarmThroughputSpecification` are optional on the request, but the
/// summary that reports the result declares each `@required`
/// (`WarmThroughputSpecificationSummary`), so a unit the request omits is the one the
/// table was already pre-warmed for — "you can update the read and write capacity units
/// to adjust the pre-provisioned throughput"
/// (`UpdateTableRequest$warmThroughputSpecification`). A unit that has never been
/// configured has no value to keep, and Amazon Keyspaces publishes no default for it, so
/// the first configuration must carry both rather than have one invented for it.
pub fn resolve_warm_throughput(
    supplied: &WarmThroughputSpecification,
    existing: Option<&WarmThroughput>,
    settles_at: SystemTime,
) -> Result<WarmThroughput, String> {
    let read = supplied
        .read_units_per_second
        .or_else(|| existing.map(|warm| warm.read_units_per_second));
    let write = supplied
        .write_units_per_second
        .or_else(|| existing.map(|warm| warm.write_units_per_second));
    let (Some(read_units_per_second), Some(write_units_per_second)) = (read, write) else {
        return Err(
            "warmThroughputSpecification requires readUnitsPerSecond and writeUnitsPerSecond \
             for a table that has no warm throughput configured yet; afterwards either one \
             may be updated on its own"
                .to_string(),
        );
    };
    Ok(WarmThroughput {
        read_units_per_second,
        write_units_per_second,
        // "UPDATING when changes are being applied": the pre-warming the caller asked for
        // is owed at the same instant as the table's own status, and nothing sleeps for it.
        status_due_at: Some(settles_at),
    })
}

/// Whether `candidate` names an Amazon Web Services Region.
///
/// `region` is modelled as a plain string (`len 2..25`, with NO pattern and no value
/// set — smithy_query shape="region"), so the set of values a Region member may name is
/// not on the wire and cannot be checked at the boundary. The set is the shared catalog
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may carry,
/// sorted for binary search", maintained centrally and already a dependency of this
/// crate) rather than a list copied into this service, so the Regions this service
/// accepts cannot drift from the ones the rest of the emulator knows. The lookup is the
/// binary search that ordering is maintained for.
pub fn is_aws_region(candidate: &str) -> bool {
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .binary_search(&candidate)
        .is_ok()
}

/// The replication strategy and region list a keyspace is created with.
///
/// A `MULTI_REGION` keyspace's "`regionList` requires the current Region and at
/// least one additional Amazon Web Services Region"
/// (`CreateKeyspaceRequest$replicationSpecification`); the model expresses only
/// the list's minimum length, and nothing about the current region or about the
/// list being required at all, so both are checked here. A `SINGLE_REGION`
/// keyspace is replicated in the region it was created in.
///
/// The list names "the Amazon Web Services Regions where the keyspace is replicated in"
/// (`ReplicationSpecification$regionList`), so it denotes a SET: `RegionList` carries no
/// `uniqueItems` (smithy_query shape="RegionList"), and a Region a request names twice is
/// still one Region the keyspace is replicated in. So the list is collapsed to the Regions
/// it denotes, in the order the request first named each, and the "at least one additional
/// Amazon Web Services Region" half of the sentence holds of that set — the modelled
/// minimum length of 2 is on the list, not on the set, so a list that only repeats one
/// Region satisfies the model and still names no additional Region. What this returns is
/// therefore what a keyspace is replicated in, with every Region in it once, whichever
/// operation resolved it.
///
/// `regionList` holds "the Amazon Web Services Regions where the keyspace is replicated
/// in" (`ReplicationSpecification$regionList`), and a keyspace can only be replicated in
/// a Region that exists — a value naming none is the "invalid or malformed request" the
/// `ValidationException` covers — so every entry is held to [`is_aws_region`] whatever
/// the strategy is.
pub fn resolve_replication(
    supplied: Option<&crate::ops::types::ReplicationSpecification>,
    region: &str,
) -> Result<(String, Vec<String>), String> {
    let Some(specification) = supplied else {
        return Ok((
            SINGLE_REGION_STRATEGY.to_string(),
            vec![region.to_string()],
        ));
    };
    // The ONE cap on a caller-chosen Region count, applied before the list is cloned,
    // stored or compared against another: see [`MAX_REPLICATION_REGIONS`] for why the
    // model's `len 2..` leaves it to be stated here.
    if let Some(regions) = specification.region_list.as_ref() {
        if regions.len() > MAX_REPLICATION_REGIONS {
            return Err(format!(
                "replicationSpecification.regionList names {} Amazon Web Services Regions, \
                 which exceeds the limit of {MAX_REPLICATION_REGIONS} replication Regions \
                 per keyspace",
                regions.len()
            ));
        }
        // Every entry names a Region the keyspace is replicated in, so a value that names
        // no Region is refused here: the model carries the member's length only.
        for (index, listed) in regions.iter().enumerate() {
            if !is_aws_region(listed) {
                return Err(format!(
                    "replicationSpecification.regionList[{index}] is '{listed}', which is \
                     not an Amazon Web Services Region"
                ));
            }
        }
    }
    // "`regionList` - if the `replicationStrategy` is `MULTI_REGION`, the `regionList`
    // requires the current Region and at least one additional Amazon Web Services Region"
    // (`CreateKeyspaceRequest$replicationSpecification`): the list names the Regions a
    // keyspace is replicated in only under that strategy. Under any other strategy the
    // keyspace is replicated in exactly the Region of the call, so a supplied list widens
    // nothing and is not what gets stored — storing it would record Regions the keyspace
    // is not replicated in, which no reader reports and which `UpdateKeyspace` would then
    // refuse to let a widening omit.
    if specification.replication_strategy != MULTI_REGION_STRATEGY {
        return Ok((
            specification.replication_strategy.clone(),
            vec![region.to_string()],
        ));
    }
    let Some(regions) = specification.region_list.clone() else {
        return Err(format!(
            "replicationSpecification with replicationStrategy '{MULTI_REGION_STRATEGY}' requires regionList"
        ));
    };
    if !regions.iter().any(|listed| listed == region) {
        return Err(format!(
            "replicationSpecification.regionList must contain the current Region '{region}'"
        ));
    }
    // The Regions the list denotes: a repeated entry is kept once, in the order the
    // request first named it — the set beside the list is what keeps that one pass linear,
    // as this crate's other duplicate-name checks do ([`resolve_replica_specifications`]).
    // The count is already capped above.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut denoted: Vec<String> = Vec::with_capacity(regions.len());
    for listed in &regions {
        if seen.insert(listed.as_str()) {
            denoted.push(listed.clone());
        }
    }
    if denoted.len() < 2 {
        return Err(format!(
            "replicationSpecification.regionList names only Region '{}': a \
             {MULTI_REGION_STRATEGY} keyspace requires the current Region and at least one \
             additional Amazon Web Services Region",
            denoted.join(", ")
        ));
    }
    Ok((specification.replication_strategy.clone(), denoted))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKeyspacesKeyspacesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKeyspacesKeyspacesserviceState>)
}

// ---------------------------------------------------------------------------
// A table's auto scaling settings, as the Application Auto Scaling resources they are
// ---------------------------------------------------------------------------

// Amazon Keyspaces auto scaling is implemented ON TOP of Application Auto Scaling: the
// settings a caller configures through `autoScalingSpecification` are a scalable target
// per capacity dimension, plus the target-tracking policy that steers it, in THAT
// service — which is why `GetTableAutoScalingSettings` documents
// `application-autoscaling:DescribeScalableTargets` and
// `application-autoscaling:DescribeScalingPolicies` as the two permissions needed to read
// them. So this service writes them there rather than keeping a disconnected copy, and
// reports back what that service holds.
use ::service_com_amazonaws_applicationautoscaling_anyscalefrontendservice as application_auto_scaling;

/// The Application Auto Scaling `ServiceNamespace` Amazon Keyspaces' scalable targets are
/// registered in (`cassandra`, the same namespace as this service's ARNs).
pub const AUTO_SCALING_NAMESPACE: &str = ARN_NAMESPACE;

/// Which capacity of a table a scalable target scales.
///
/// A table in provisioned capacity mode has two capacities, provisioned and scaled
/// independently of each other: `AutoScalingSpecification` carries one
/// `AutoScalingSettings` per capacity, and Application Auto Scaling declares one
/// `ScalableDimension` per capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableCapacity {
    /// `readCapacityUnits`.
    Read,
    /// `writeCapacityUnits`.
    Write,
}

impl TableCapacity {
    /// Both capacities, so a caller walks them rather than spelling each out twice.
    pub const ALL: [Self; 2] = [Self::Read, Self::Write];

    /// The `ScalableDimension` this capacity is scaled on.
    pub fn scalable_dimension(self) -> &'static str {
        match self {
            Self::Read => "cassandra:table:ReadCapacityUnits",
            Self::Write => "cassandra:table:WriteCapacityUnits",
        }
    }

    /// The `PredefinedMetricType` a target-tracking policy of this capacity steers on:
    /// the Amazon Keyspaces utilisation metric Application Auto Scaling publishes FOR
    /// this dimension.
    pub fn predefined_metric(self) -> &'static str {
        match self {
            Self::Read => "CassandraReadCapacityUtilization",
            Self::Write => "CassandraWriteCapacityUtilization",
        }
    }

    /// This capacity's settings inside a specification.
    pub fn settings_of(self, specification: &AutoScalingSpecification) -> Option<&AutoScalingSettings> {
        match self {
            Self::Read => specification.read_capacity_auto_scaling.as_ref(),
            Self::Write => specification.write_capacity_auto_scaling.as_ref(),
        }
    }

    /// Put this capacity's settings into a specification being built for a report.
    fn set_in(self, specification: &mut AutoScalingSpecification, settings: Option<AutoScalingSettings>) {
        match self {
            Self::Read => specification.read_capacity_auto_scaling = settings,
            Self::Write => specification.write_capacity_auto_scaling = settings,
        }
    }
}

/// The `ResourceId` a table's scalable targets are registered under:
/// `keyspace/<keyspaceName>/table/<tableName>`, the form Application Auto Scaling
/// documents for the `cassandra` namespace's table resource type.
pub fn auto_scaling_resource_id(keyspace_name: &str, table_name: &str) -> String {
    format!("keyspace/{keyspace_name}/table/{table_name}")
}

/// The identity of one capacity's scalable target: the Region the table lives in, the
/// `cassandra` namespace, the table's resource id and that capacity's dimension.
///
/// A scalable target is a regional resource, so the table's Region is part of the identity
/// its settings are written under and read back from.
fn scalable_target_key(
    region: &str,
    keyspace_name: &str,
    table_name: &str,
    capacity: TableCapacity,
) -> application_auto_scaling::state::TargetKey {
    application_auto_scaling::state::TargetKey {
        region: region.to_string(),
        service_namespace: AUTO_SCALING_NAMESPACE.to_string(),
        resource_id: auto_scaling_resource_id(keyspace_name, table_name),
        scalable_dimension: capacity.scalable_dimension().to_string(),
    }
}

/// The `PolicyName` of the target-tracking policy this service manages for one capacity.
///
/// A policy's name is its identity under its target, so a service-managed policy needs a
/// name derived from the resource it was created FOR rather than chosen per call: that is
/// what makes a second configuration of the same capacity an UPDATE of the one policy
/// instead of a second policy on one metric, and what lets a caller address it through
/// Application Auto Scaling's own API without discovering it first.
///
/// This service names it after the table whose capacity it steers — the keyspace, the
/// table and the `-scaling-policy` suffix — which is one name for BOTH of the table's
/// capacities: the policy's `ScalableDimension`, not its name, says which capacity it
/// steers, and a name is unique only under one target.
fn managed_policy_name(keyspace_name: &str, table_name: &str) -> String {
    format!("{keyspace_name}-{table_name}-scaling-policy")
}

/// What one capacity's settings are to be materialised as in Application Auto Scaling, or
/// `None` when they turn auto scaling OFF for that capacity — "this optional parameter
/// enables auto scaling for the table if set to `false`" — which is a capacity with no
/// scalable target at all.
fn managed_target(
    keyspace_name: &str,
    table_name: &str,
    capacity: TableCapacity,
    settings: &AutoScalingSettings,
) -> Option<application_auto_scaling::state::ManagedTarget> {
    if settings.auto_scaling_disabled {
        return None;
    }
    let target_value = settings
        .scaling_policy
        .as_ref()
        .and_then(|policy| policy.target_tracking_scaling_policy_configuration.as_ref());
    // Settings that turn auto scaling on carry all three ([`validate_auto_scaling_settings`]),
    // so this is the same rule read from the other side rather than a second check.
    let (Some(minimum), Some(maximum), Some(configuration)) =
        (settings.minimum_units, settings.maximum_units, target_value)
    else {
        return None;
    };
    Some(application_auto_scaling::state::ManagedTarget {
        // A capacity window in Application Auto Scaling is an `int`, so a unit count
        // beyond its range is registered at the largest capacity that service can hold.
        min_capacity: i32::try_from(minimum).unwrap_or(i32::MAX),
        max_capacity: i32::try_from(maximum).unwrap_or(i32::MAX),
        policy_name: managed_policy_name(keyspace_name, table_name),
        target_tracking:
            application_auto_scaling::ops::types::TargetTrackingScalingPolicyConfiguration {
                target_value: configuration.target_value,
                predefined_metric_specification: Some(
                    application_auto_scaling::ops::types::PredefinedMetricSpecification {
                        predefined_metric_type: capacity.predefined_metric().to_string(),
                        resource_label: None,
                    },
                ),
                customized_metric_specification: None,
                scale_out_cooldown: Some(configuration.scale_out_cooldown),
                scale_in_cooldown: Some(configuration.scale_in_cooldown),
                disable_scale_in: Some(configuration.disable_scale_in),
            },
    })
}

/// Materialise in Application Auto Scaling the auto scaling settings a request just
/// configured for a table: for each capacity the request named, that capacity's scalable
/// target and target-tracking policy when auto scaling is on for it, and neither when it
/// is off.
///
/// A capacity the request did NOT name keeps whatever Application Auto Scaling holds for
/// it — the two capacities are configured independently of each other, and a request that
/// says nothing about one changes nothing about it.
///
/// Call it with NO store lock held. Only the table's OWN settings travel: a per-Region
/// `replicaSpecifications.readCapacityAutoScaling` override belongs to another Region's
/// Application Auto Scaling, which this emulator holds no store for, so it stays with the
/// table record that reports it.
pub fn publish_auto_scaling<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    keyspace_name: &str,
    table_name: &str,
    configured: &AutoScalingSpecification,
) {
    for capacity in TableCapacity::ALL {
        let Some(settings) = capacity.settings_of(configured) else {
            continue;
        };
        let wanted = managed_target(keyspace_name, table_name, capacity, settings);
        application_auto_scaling::state::apply_managed_target(
            engine,
            &scalable_target_key(region, keyspace_name, table_name, capacity),
            wanted.as_ref(),
        );
    }
}

/// Retire both of a table's scalable targets, with their policies and the alarms those
/// minted: the table is going away, so the capacities it was scaled on no longer exist.
///
/// Call it with NO store lock held.
pub fn retire_auto_scaling<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    keyspace_name: &str,
    table_name: &str,
) {
    for capacity in TableCapacity::ALL {
        application_auto_scaling::state::apply_managed_target(
            engine,
            &scalable_target_key(region, keyspace_name, table_name, capacity),
            None,
        );
    }
}

/// The auto scaling settings of a table as they stand NOW: what Application Auto Scaling
/// holds for each of the table's two capacities, which is where the settings live.
///
/// Per capacity, the settings are the registered scalable target's capacity window and
/// the target-tracking policy that steers it — including one a caller registered through
/// Application Auto Scaling itself, since that is the same resource this service's own
/// writes produce. A capacity with NO scalable target is a capacity auto scaling is off
/// for, which is what a stored `autoScalingDisabled: true` states; one that was never
/// configured, or whose target has since been deregistered, has no settings at all.
///
/// `None` means the table has no auto scaling settings to report, which
/// `GetTableAutoScalingSettings` reports by leaving its OPTIONAL `autoScalingSpecification`
/// out of an otherwise ordinary report of the table. `stored` is the
/// specification this service last wrote, which is also all that is known when the
/// Application Auto Scaling store is not reachable from `engine`.
///
/// Call it with NO store lock held.
pub fn reported_auto_scaling<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    keyspace_name: &str,
    table_name: &str,
    stored: Option<&AutoScalingSpecification>,
) -> Option<AutoScalingSpecification> {
    let Some(store) = application_auto_scaling::state::service_state(engine) else {
        return stored.cloned();
    };
    let store = store.lock();
    let mut reported = AutoScalingSpecification::default();
    let mut configured = false;
    for capacity in TableCapacity::ALL {
        let key = scalable_target_key(region, keyspace_name, table_name, capacity);
        let policy_name = managed_policy_name(keyspace_name, table_name);
        let settings = match application_auto_scaling::state::managed_target_settings(
            &store,
            &key,
            &policy_name,
        ) {
            Some((minimum, maximum, tracking)) => Some(AutoScalingSettings {
                auto_scaling_disabled: false,
                minimum_units: Some(i64::from(minimum)),
                maximum_units: Some(i64::from(maximum)),
                scaling_policy: tracking.map(|configuration| AutoScalingPolicy {
                    target_tracking_scaling_policy_configuration: Some(
                        TargetTrackingScalingPolicyConfiguration {
                            disable_scale_in: configuration.disable_scale_in.unwrap_or_default(),
                            scale_in_cooldown: configuration.scale_in_cooldown.unwrap_or_default(),
                            scale_out_cooldown: configuration
                                .scale_out_cooldown
                                .unwrap_or_default(),
                            target_value: configuration.target_value,
                        },
                    ),
                }),
            }),
            None => stored
                .and_then(|specification| capacity.settings_of(specification))
                .filter(|settings| settings.auto_scaling_disabled)
                .cloned(),
        };
        configured |= settings.is_some();
        capacity.set_in(&mut reported, settings);
    }
    configured.then_some(reported)
}
