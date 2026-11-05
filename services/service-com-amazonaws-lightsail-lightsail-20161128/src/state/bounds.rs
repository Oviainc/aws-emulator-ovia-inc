// SPDX-License-Identifier: CC0-1.0

//! What this store will hold, and how much of it one page reports: the growth
//! caps every caller-created record is kept under, and the page size each
//! paginated read serves.

/// The most instances this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota (Lightsail's own default quota is far
/// tighter, and deliberately not enforced here because `GetInstances`' pagination
/// needs more instances than that to exercise). A create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_INSTANCES: usize = 500;

/// The most instance names ONE request may carry — `CreateInstances`'
/// `instanceNames`, or `AttachInstancesToLoadBalancer`'s.
///
/// Emulator resource guard: neither the modelled `StringList` nor the modelled
/// `ResourceNameList` declares a length, so this is where the bound on a
/// caller-sized list of instance names lives.
pub const MAX_INSTANCE_NAMES_PER_REQUEST: usize = 50;

/// The most domain recordsets this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DOMAINS: usize = 500;

/// The most block storage disks this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DISKS: usize = 500;

/// The most managed relational databases this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_RELATIONAL_DATABASES: usize = 500;

/// The most disk snapshots this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DISK_SNAPSHOTS: usize = 500;

/// The most instance snapshots this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_INSTANCE_SNAPSHOTS: usize = 500;

/// The most managed relational database snapshots this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_RELATIONAL_DATABASE_SNAPSHOTS: usize = 500;

/// The most automatic snapshots this emulator holds of ONE resource.
///
/// Emulator resource guard, not a refusal: nobody asks for an automatic
/// snapshot, so there is no request to refuse — the `AutoSnapshot` add-on takes
/// one a day for as long as it is enabled, and the oldest copy drops out once a
/// new one would take the resource past this. It is also what bounds the walk
/// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] and
/// `GetAutoSnapshots` make over one resource's copies.
pub const MAX_AUTO_SNAPSHOTS_PER_RESOURCE: usize = 7;

/// The most resources this emulator holds KEPT automatic snapshots of — copies
/// a delete left in the account because it was not asked to delete the
/// resource's add-ons, so the resource they were taken of is gone and the copies
/// are not.
///
/// Emulator resource guard, not a refusal: nobody asks for an automatic
/// snapshot, so there is no request to refuse — the copies kept longest ago drop
/// out once a further delete would take the store past this, exactly as the
/// oldest entry drops out of the operation log ([`MAX_OPERATION_HISTORY`]). It
/// is also what keeps the walk
/// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] makes over the
/// schedules bounded once a resource no longer has to be held for its copies to
/// be.
pub const MAX_KEPT_AUTO_SNAPSHOT_SOURCES: usize = 500;

/// The most export snapshot records this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: every `ExportSnapshot` call adds a
/// record, so an export that would exceed it is refused with the modelled
/// `InvalidInputException`.
pub const MAX_EXPORT_SNAPSHOT_RECORDS: usize = 500;

/// The most static IP addresses this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota (Lightsail's own static-IP quota is
/// far tighter, and deliberately not enforced here so a suite can allocate as
/// many as a listing needs): an allocation that would exceed it is refused with
/// the modelled `InvalidInputException`.
pub const MAX_STATIC_IPS: usize = 500;

/// The most recordset entries this emulator will hold in one domain.
///
/// Emulator resource guard, NOT an AWS quota: an entry is caller-sized and the
/// zone's entry list grows with every create, so a create that would exceed this
/// is refused with the modelled `InvalidInputException`.
pub const MAX_DOMAIN_ENTRIES: usize = 1_000;

/// The most tags one instance may carry.
///
/// From the Lightsail tagging documentation the modelled `tags` member links to:
/// "You can add up to 50 tags to each Lightsail resource."
pub const MAX_TAGS: usize = 50;

/// The most add-on requests one `CreateInstances` call may carry. The modelled
/// `AddOnRequestList` declares no length, and the `AddOnType` enum has only two
/// members, so no useful request needs more than this.
pub const MAX_ADD_ONS: usize = 8;

/// The most alarms this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a `PutAlarm` that would create one
/// past it is refused with the modelled `InvalidInputException`.
pub const MAX_ALARMS: usize = 500;

/// The most operations the log keeps, oldest dropped first.
///
/// Emulator resource guard: the log is append-only, so without a cap a long-lived
/// emulator would grow without bound.
pub const MAX_OPERATION_HISTORY: usize = 2_000;

/// How many instances one `GetInstances` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more instances remain.
pub const INSTANCE_PAGE_SIZE: usize = 10;

/// How many managed relational databases one `GetRelationalDatabases` page
/// carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more databases remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const RELATIONAL_DATABASE_PAGE_SIZE: usize = 10;

/// How many database snapshots one `GetRelationalDatabaseSnapshots` page
/// carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more snapshots remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const RELATIONAL_DATABASE_SNAPSHOT_PAGE_SIZE: usize = 10;

/// How many database events one `GetRelationalDatabaseEvents` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more events remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every paged listing in this slice pages the same
/// way.
pub const RELATIONAL_DATABASE_EVENT_PAGE_SIZE: usize = 10;

/// How many log lines one `GetRelationalDatabaseLogEvents` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that "if
/// your results are paginated, the response will return a next forward token
/// and/or next backward token that you can specify as the page token in a
/// subsequent request" (the modelled `pageToken` documentation). Matches
/// [`INSTANCE_PAGE_SIZE`], so every paged read in this slice pages the same way.
pub const RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE: usize = 10;

/// How many instance snapshots one `GetInstanceSnapshots` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more snapshots remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const INSTANCE_SNAPSHOT_PAGE_SIZE: usize = 10;

/// How many block storage disks one `GetDisks` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more disks remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const DISK_PAGE_SIZE: usize = 10;

/// How many disk snapshots one `GetDiskSnapshots` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more snapshots remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const DISK_SNAPSHOT_PAGE_SIZE: usize = 10;

/// How many static IPs one `GetStaticIps` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more static IPs remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const STATIC_IP_PAGE_SIZE: usize = 10;

/// How many SSH key pairs one `GetKeyPairs` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more key pairs remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const KEY_PAIR_PAGE_SIZE: usize = 10;

/// How many export snapshot records one `GetExportSnapshotRecords` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation either, only that
/// the response carries a `nextPageToken` when more records remain — "A next page
/// token is not returned if there are no more results to display."
pub const EXPORT_SNAPSHOT_RECORD_PAGE_SIZE: usize = 10;

/// How many domain recordsets one `GetDomains` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more recordsets remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const DOMAIN_PAGE_SIZE: usize = 10;

/// How many resource names one `GetActiveNames` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more names remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every paged listing in this slice pages the same
/// way.
pub const ACTIVE_NAME_PAGE_SIZE: usize = 10;

/// How many log entries one `GetOperationsForResource` page carries.
///
/// CHOSEN: Lightsail publishes a page size for `GetOperations` only ("Results
/// are returned from oldest to newest, up to a maximum of 200") and none for
/// `GetOperationsForResource`, which documents only that "a next page token is
/// not returned if there are no more results to display". Matches
/// [`INSTANCE_PAGE_SIZE`], so every listing in this slice pages the same way.
pub const OPERATIONS_FOR_RESOURCE_PAGE_SIZE: usize = 10;

/// How many log entries one `GetOperations` page carries.
///
/// PUBLISHED, from that operation's own documentation: "Returns information
/// about all operations. Results are returned from oldest to newest, up to a
/// maximum of 200." It is therefore NOT the chosen size the rest of this
/// slice's listings share.
pub const OPERATIONS_PAGE_SIZE: usize = 200;
