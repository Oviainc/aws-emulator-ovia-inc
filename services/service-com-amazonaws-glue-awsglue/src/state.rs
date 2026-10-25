// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-glue-awsglue service.
//!
//! The Data Catalog is the service's primary resource tree: a catalog (keyed by
//! `CatalogId`, which defaults to the request's account id) holds databases, and a
//! database holds tables. Every handler reads and writes THIS store, so a
//! `CreateTable` in one request is visible to a `GetTable` in the next.
//!
//! Beside the catalog the store holds the two account-level resources an ETL client
//! registers: JOB DEFINITIONS (`CreateJob` writes one, `GetJob` reads it back) and, inside
//! each catalog, CONNECTION definitions (`CreateConnection` / `GetConnection`). A job is
//! named per account ("The name you assign to this job definition. It must be unique in
//! your account" — `CreateJobRequest$Name`) and a connection per catalog, which is where
//! each lives here. A job definition also holds the RUNS `StartJobRun` started of it
//! ([`JobRunEntry`]), because every operation that addresses a run names its job too.
//!
//! Crawlers sit BESIDE that tree too, in one per-account namespace keyed by crawler name:
//! every operation that addresses a crawler does so by name alone, so a crawler's
//! `CatalogId` says where its output goes, not where the crawler itself lives.
//!
//! CUSTOM ENTITY TYPES — the custom sensitive-data patterns `CreateCustomEntityType`
//! registers — sit in the same kind of per-account namespace, keyed by pattern name:
//! "This name must be unique per Amazon Web Services account"
//! (`CustomEntityType$Name`), and every operation that addresses one names it by name
//! alone, with no `CatalogId` member.
//!
//! Database and table names are folded to lowercase on the way in — the model says
//! so for both `DatabaseInput$Name` ("For Hive compatibility, this is folded to
//! lowercase when it is stored") and `TableInput$Name` — and every lookup folds the
//! requested name the same way, so the fold cannot make a resource unreachable. A JOB,
//! CONNECTION or CRAWLER name is NOT folded: the model documents the fold for the two
//! Hive-compatible catalog resources only.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CodeGenConfigurationNode, Column, Connection, ConnectionInput, ConnectionsList, Crawler,
    CrawlerTargets, CreateCrawlerInput, CreateCustomEntityTypeInput, CreateJobInput,
    DataLakePrincipal, Database, DatabaseIdentifier, DatabaseInput, ErrorDetail, ExecutionProperty,
    FederatedDatabase, FederatedTable, Job, JobCommand, JobRun, LakeFormationConfiguration,
    LineageConfiguration, NotificationProperty, PrincipalPermissions, RLSProperties, RecrawlPolicy,
    Schedule, SchemaChangePolicy, SourceControlDetails, StartJobRunInput, StorageDescriptor, Table,
    TableIdentifier, TableInput,
};

/// Ceiling on the databases one catalog may hold.
///
/// This emulator's own capacity, NOT an AWS quota: the store is resident memory and a
/// create is reachable by any caller, so the number of records one process will hold
/// is bounded here. A create past the ceiling is refused with the operation's modelled
/// `ResourceNumberLimitExceededException` rather than inserted.
pub const MAX_DATABASES_PER_CATALOG: usize = 1_000;

/// Ceiling on the tables one database may hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`].
pub const MAX_TABLES_PER_DATABASE: usize = 1_000;

/// Ceiling on the tables the whole store may hold, across every catalog and database.
/// Bounds the product of the two per-container ceilings, which on its own would still
/// let a caller grow the store far past what one process should hold.
pub const MAX_STORED_TABLES: usize = 10_000;

/// Ceiling on the connection definitions one catalog may hold, for the same reason as
/// [`MAX_JOBS`].
pub const MAX_CONNECTIONS_PER_CATALOG: usize = 1_000;

/// Ceiling on the CATALOGS the whole store may hold.
///
/// The ceilings above bound what one container holds; this one bounds how many
/// containers there are, which is what makes the store as a whole bounded. The map is
/// keyed by `CatalogId`, a member a request may name, so without this a create that
/// reached a catalog the store does not hold yet could add containers without end — each
/// able to hold its own [`MAX_DATABASES_PER_CATALOG`] and
/// [`MAX_CONNECTIONS_PER_CATALOG`] records. It is also what keeps the store-wide walks
/// below ([`ComAmazonawsGlueAwsglueState::stored_tables`] and its siblings) a bounded
/// cost per create. A create that would add one past the ceiling is refused with the
/// operation's modelled `ResourceNumberLimitExceededException`.
pub const MAX_CATALOGS: usize = 16;

/// Ceiling on the databases the whole store may hold, across every catalog, for the same
/// reason as [`MAX_STORED_TABLES`]: the per-catalog ceiling alone, multiplied by
/// [`MAX_CATALOGS`], would still let a caller grow the store past what one process
/// should hold.
pub const MAX_STORED_DATABASES: usize = 10_000;

/// Ceiling on the connection definitions the whole store may hold, across every catalog,
/// for the same reason as [`MAX_STORED_DATABASES`].
pub const MAX_STORED_CONNECTIONS: usize = 10_000;

/// The `ConnectionProperties` key holding a connection's password: "`PASSWORD` - A
/// password, if one is used, for the user name." (`Connection$ConnectionProperties`.) It is
/// what `GetConnection`'s `HidePassword` withholds.
pub const PASSWORD_PROPERTY: &str = "PASSWORD";

/// The `ConnectionProperties` key holding the encrypted form of that password: "
/// `ENCRYPTED_PASSWORD` - When you enable connection password protection … this field
/// stores the encrypted password." Withheld with [`PASSWORD_PROPERTY`].
pub const ENCRYPTED_PASSWORD_PROPERTY: &str = "ENCRYPTED_PASSWORD";

/// `Connection$ConnectionSchemaVersion` for a connection whose properties are the version 1
/// set, and for one carrying the per-compute-environment properties: "The version of the
/// connection schema for this connection. Version 2 supports properties for specific
/// compute environments."
pub const CONNECTION_SCHEMA_VERSION_V1: i32 = 1;
/// See [`CONNECTION_SCHEMA_VERSION_V1`].
pub const CONNECTION_SCHEMA_VERSION_V2: i32 = 2;

/// The three values the model's `ComputeEnvironment` enum carries, which
/// `GetConnection$ApplyOverrideForComputeEnvironment` names one of.
pub const COMPUTE_ENVIRONMENT_SPARK: &str = "SPARK";
/// See [`COMPUTE_ENVIRONMENT_SPARK`].
pub const COMPUTE_ENVIRONMENT_ATHENA: &str = "ATHENA";
/// See [`COMPUTE_ENVIRONMENT_SPARK`].
pub const COMPUTE_ENVIRONMENT_PYTHON: &str = "PYTHON";

/// Ceiling on the crawlers this emulator will hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`]: resident memory bounded against a create any caller
/// can reach. A create past it is refused with `CreateCrawler`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_CRAWLERS: usize = 1_000;

/// Ceiling on the job definitions this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateJob`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_JOBS: usize = 1_000;

/// Ceiling on the custom entity types this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateCustomEntityType`'s own
/// modelled `ResourceNumberLimitExceededException`.
pub const MAX_CUSTOM_ENTITY_TYPES: usize = 1_000;

/// The `CrawlerState` of a crawler with no run in progress — the state a crawler is in
/// the moment `CreateCrawler` returns, and the only one this emulator stores until
/// `StartCrawler`/`StopCrawler` are implemented. One of the modelled `CrawlerState`
/// values (`READY`, `RUNNING`, `STOPPING`).
pub const CRAWLER_STATE_READY: &str = "READY";

/// The `Schedule$State` reported for a crawler that carries a cron expression. One of
/// the modelled `ScheduleState` values (`SCHEDULED`, `NOT_SCHEDULED`,
/// `TRANSITIONING`); a crawler created WITHOUT a `Schedule` has no schedule object at
/// all, so `NOT_SCHEDULED` is not reachable from a create alone.
pub const SCHEDULE_STATE_SCHEDULED: &str = "SCHEDULED";

/// The `Version` a crawler carries straight after `CreateCrawler`: the first version of
/// a crawler is 1, and an update moves it on. The model's `@default` of 0 on
/// `Crawler$Version` is what an ABSENT long deserializes to, not the version of a stored
/// crawler. (A table's first version is numbered differently — see
/// [`FIRST_TABLE_VERSION_ID`] — because the catalog's table version history starts at
/// 0.)
pub const FIRST_CRAWLER_VERSION: i64 = 1;

/// The Lake Formation principal that stands for "whoever IAM already allows": the
/// `DataLakePrincipalIdentifier` of the grant a Data Catalog database carries while
/// nothing has narrowed it to Lake Formation permissions. Spelled exactly like this on
/// the wire, and the same principal
/// `services/service-com-amazonaws-lakeformation-awslakeformation/src/state.rs` holds the
/// matching database grant under.
pub const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The permission that default grant carries.
///
/// From the documented Lake Formation default, not chosen: Lake Formation's
/// `DataLakeSettings$CreateDatabaseDefaultPermissions` documents that "A value that
/// assigns ALL to IAM_ALLOWED_PRINCIPALS indicates access control by IAM permissions.
/// This is referred to as the setting \"Use only IAM access control\", and is for
/// backward compatibility with the Glue permission model", and that "The only permitted
/// values are an empty array or an array that contains a single JSON object that grants
/// ALL to IAM_ALLOWED_PRINCIPALS". `ALL` is also one of the modelled `Permission` enum's
/// values (`ALL`, `SELECT`, `ALTER`, `DROP`, `DELETE`, `INSERT`, `CREATE_DATABASE`,
/// `CREATE_TABLE`, `DATA_LOCATION_ACCESS`).
pub const PERMISSION_ALL: &str = "ALL";

/// The `CreateTableDefaultPermissions` a database gets when its create named none.
///
/// `Database$CreateTableDefaultPermissions` "creates a set of default permissions on the
/// table for principals. Used by Lake Formation": a database the caller did not govern
/// with Lake Formation is open to every IAM-allowed principal, which the catalog records
/// as the single `ALL` grant to [`IAM_ALLOWED_PRINCIPALS`] rather than as no grant at
/// all. The distinction matters to the reader: an EMPTY grant list is the opposite
/// governance decision ("nobody, until Lake Formation grants it"), so it is stored only
/// when the client sends one.
pub fn default_create_table_default_permissions() -> Vec<PrincipalPermissions> {
    vec![PrincipalPermissions {
        principal: Some(DataLakePrincipal {
            data_lake_principal_identifier: Some(IAM_ALLOWED_PRINCIPALS.to_string()),
        }),
        permissions: Some(vec![PERMISSION_ALL.to_string()]),
        // Stamped by the read projection, not stored: see
        // [`tagged_principal_permissions`].
        __type: None,
    }]
}

/// The `__type` tag the Data Catalog writes onto a nested `PrincipalPermissions`.
///
/// The Data Catalog's awsJson1.1 serializer tags the nested structures it returns with a
/// type discriminator whose value is the structure's own Smithy shape name, unqualified,
/// so a client that dispatches on the tag can identify the entry. The two structures the
/// recorded wire traffic shows tagged are `PrincipalPermissions` (inside
/// `Database$CreateTableDefaultPermissions`) and [`ERROR_DETAIL_TYPE_TAG`]'s
/// `ErrorDetail`; the tag is therefore keyed on the SHAPE, which is why each constant is
/// spelled exactly like the shape it tags.
pub const PRINCIPAL_PERMISSIONS_TYPE_TAG: &str = "PrincipalPermissions";

/// The `__type` tag the Data Catalog writes onto a nested `ErrorDetail`. See
/// [`PRINCIPAL_PERMISSIONS_TYPE_TAG`] for the rule.
pub const ERROR_DETAIL_TYPE_TAG: &str = "ErrorDetail";

/// A stored grant as the catalog REPORTS it: the stored members plus the `__type` tag.
///
/// The tag belongs to the projection, not to the record: a client may send
/// `CreateTableDefaultPermissions` with or without a tag of its own, and the catalog
/// answers with its own either way — so the tag is stamped here on the way out rather
/// than carried over from whatever the request happened to contain.
pub fn tagged_principal_permissions(stored: &PrincipalPermissions) -> PrincipalPermissions {
    PrincipalPermissions {
        __type: Some(PRINCIPAL_PERMISSIONS_TYPE_TAG.to_string()),
        ..stored.clone()
    }
}

/// A per-item `ErrorDetail` the way the Data Catalog reports one: the error's code and
/// message, plus the `__type` tag ([`ERROR_DETAIL_TYPE_TAG`]).
///
/// Every batch operation that reports a per-item failure builds its `ErrorDetail` here,
/// so the tag cannot be present on one operation's entries and absent from another's.
pub fn catalog_error_detail(error_code: &str, error_message: String) -> ErrorDetail {
    ErrorDetail {
        error_code: Some(error_code.to_string()),
        error_message: Some(error_message),
        __type: Some(ERROR_DETAIL_TYPE_TAG.to_string()),
    }
}

/// How many database ids this process has minted.
///
/// Not catalog state — the id itself is what gets stored, on the [`DatabaseEntry`]. This
/// is only a nonce source for [`new_database_id`], so that two creates landing in the
/// same nanosecond are still given different input to mint from.
static MINTED_DATABASE_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `DatabaseId`: the opaque id the catalog assigns a database when it is
/// created.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is the shape a
/// catalog-assigned Data Catalog id has on the wire. The bits come from three sources:
/// the OS-seeded randomness a `RandomState` draws, the system clock in nanoseconds, and
/// [`MINTED_DATABASE_IDS`] — so two creates in the same nanosecond cannot be minted from
/// the same input. It is minted once, at create, and stored on the [`DatabaseEntry`], so
/// every later read of that database — and every table in it — reports the same id, and
/// two databases never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_database_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let sequence = MINTED_DATABASE_IDS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let high = RandomState::new().hash_one((sequence, nanos));
    let low = RandomState::new().hash_one((nanos, sequence));
    format!("{high:016x}{low:016x}")
}

/// Per-service state for com-amazonaws-glue-awsglue. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGlueAwsglueState {
    /// Every Data Catalog this emulator holds, keyed by `CatalogId`.
    pub catalogs: HashMap<String, Catalog>,
    /// Every crawler this emulator holds, keyed by its name.
    ///
    /// Crawlers live BESIDE the catalog tree rather than inside it: `GetCrawler`,
    /// `DeleteCrawler` and `StartCrawler` address a crawler by name alone, with no
    /// `CatalogId` member, so the namespace is per account and a crawler's `CatalogId`
    /// says only where its output goes. The key is the name exactly as sent — unlike
    /// `DatabaseInput$Name`/`TableInput$Name`, the model documents no case fold for
    /// `Crawler$Name`.
    pub crawlers: HashMap<String, CrawlerEntry>,
    /// Every job definition this emulator holds, keyed by its name.
    ///
    /// Jobs live BESIDE the catalog tree, like crawlers: `GetJob`, `UpdateJob`,
    /// `DeleteJob` and `StartJobRun` address a job by name alone, with no `CatalogId`
    /// member, and `CreateJob$Name` is documented as unique per account ("It must be
    /// unique in your account"). The key is the name exactly as sent — the model
    /// documents no case fold for `Job$Name`.
    pub jobs: HashMap<String, JobEntry>,
    /// Every custom entity type (custom sensitive-data pattern) this emulator holds,
    /// keyed by its name.
    ///
    /// Per account, like crawlers and job definitions: `GetCustomEntityType`,
    /// `DeleteCustomEntityType` and `BatchGetCustomEntityTypes` address a pattern by name
    /// alone, and `CustomEntityType$Name` is documented as unique per account. Ordered,
    /// so `ListCustomEntityTypes` has one deterministic order to page over. The key is
    /// the name exactly as sent — the model documents no case fold for it.
    pub custom_entity_types: BTreeMap<String, CustomEntityTypeEntry>,
}

impl ComAmazonawsGlueAwsglueState {
    /// The stored database, or `None` when either the catalog or the database is absent.
    pub fn database(&self, catalog_id: &str, name: &str) -> Option<&DatabaseEntry> {
        self.catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.databases.get(&fold_name(name)))
    }

    /// The stored database, mutably.
    pub fn database_mut(&mut self, catalog_id: &str, name: &str) -> Option<&mut DatabaseEntry> {
        self.catalogs
            .get_mut(catalog_id)
            .and_then(|catalog| catalog.databases.get_mut(&fold_name(name)))
    }

    /// The stored connection definition `name` of `catalog_id`, or `None` when either the
    /// catalog or the connection is absent.
    pub fn connection(&self, catalog_id: &str, name: &str) -> Option<&ConnectionEntry> {
        self.catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.connections.get(name))
    }

    /// How many records the whole store holds of one catalog-scoped kind: `count` says
    /// how many ONE catalog holds, and this sums it over every catalog.
    ///
    /// The one walk the three store-wide counts below share, so a count and the ceiling
    /// checked against it can never disagree about which records they cover. Bounded by
    /// [`MAX_CATALOGS`] catalogs, so the walk itself is a bounded cost.
    fn stored_in_catalogs(&self, count: impl Fn(&Catalog) -> usize) -> usize {
        self.catalogs.values().map(count).sum()
    }

    /// How many tables the whole store holds. Bounded by the ceilings above, so this
    /// walk is over at most [`MAX_CATALOGS`] catalogs of
    /// [`MAX_DATABASES_PER_CATALOG`] entries each.
    pub fn stored_tables(&self) -> usize {
        self.stored_in_catalogs(|catalog| {
            catalog
                .databases
                .values()
                .map(|database| database.tables.len())
                .sum()
        })
    }

    /// Whether the store as a whole can hold another table, i.e. is below
    /// [`MAX_STORED_TABLES`].
    pub fn can_store_table(&self) -> bool {
        self.stored_tables() < MAX_STORED_TABLES
    }

    /// How many databases the whole store holds, across every catalog. Bounded by
    /// [`MAX_CATALOGS`] times [`MAX_DATABASES_PER_CATALOG`], so this walk is bounded.
    pub fn stored_databases(&self) -> usize {
        self.stored_in_catalogs(|catalog| catalog.databases.len())
    }

    /// Whether the store as a whole can hold another database, i.e. is below
    /// [`MAX_STORED_DATABASES`].
    pub fn can_store_database(&self) -> bool {
        self.stored_databases() < MAX_STORED_DATABASES
    }

    /// How many connection definitions the whole store holds, across every catalog.
    pub fn stored_connections(&self) -> usize {
        self.stored_in_catalogs(|catalog| catalog.connections.len())
    }

    /// Whether the store as a whole can hold another connection definition, i.e. is
    /// below [`MAX_STORED_CONNECTIONS`].
    pub fn can_store_connection(&self) -> bool {
        self.stored_connections() < MAX_STORED_CONNECTIONS
    }

    /// The catalog `catalog_id`, for a create that is about to write a record into it:
    /// the one this store already holds, or a fresh empty one when there is room for
    /// another container ([`MAX_CATALOGS`]). `None` means the store holds no such catalog
    /// and cannot take one, so the create must be refused rather than served.
    ///
    /// The ONE place a catalog is brought into existence, so no create can grow the
    /// container map past the ceiling, and a create that is refused earlier leaves no
    /// empty catalog behind: a handler calls this only once every other check has passed.
    pub fn catalog_mut_for_create(&mut self, catalog_id: &str) -> Option<&mut Catalog> {
        if !self.catalogs.contains_key(catalog_id) && self.catalogs.len() >= MAX_CATALOGS {
            return None;
        }
        Some(self.catalogs.entry(catalog_id.to_string()).or_default())
    }

    /// Whether the store can hold another crawler, i.e. is below [`MAX_CRAWLERS`].
    pub fn can_store_crawler(&self) -> bool {
        self.crawlers.len() < MAX_CRAWLERS
    }

    /// Whether the store can hold another job definition, i.e. is below [`MAX_JOBS`].
    pub fn can_store_job(&self) -> bool {
        self.jobs.len() < MAX_JOBS
    }

    /// Whether the store can hold another custom entity type, i.e. is below
    /// [`MAX_CUSTOM_ENTITY_TYPES`].
    pub fn can_store_custom_entity_type(&self) -> bool {
        self.custom_entity_types.len() < MAX_CUSTOM_ENTITY_TYPES
    }

    /// The tag set of the resource `resource` names, or `None` when this store holds no
    /// such resource.
    ///
    /// The ONE resolution every tag READ goes through, so an ARN names the same record
    /// whichever operation — this service's own `GetTags`, or the account-wide tagging
    /// API that projects this store — resolved it.
    pub fn resource_tags(&self, resource: &TaggedResource) -> Option<&HashMap<String, String>> {
        match resource.kind {
            // A database and a connection live INSIDE a catalog, so the ARN's account
            // segment is the `CatalogId` they are looked for under.
            TaggedResourceKind::Database => self
                .catalogs
                .get(&resource.account_id)
                .and_then(|catalog| catalog.databases.get(&resource.name))
                .map(|database| &database.tags),
            TaggedResourceKind::Connection => self
                .catalogs
                .get(&resource.account_id)
                .and_then(|catalog| catalog.connections.get(&resource.name))
                .map(|connection| &connection.tags),
            // A crawler and a job definition live BESIDE the catalog tree in one
            // per-account namespace, so the account segment is what decides whether this
            // emulator could hold them at all.
            TaggedResourceKind::Crawler => is_served_account(&resource.account_id)
                .then(|| self.crawlers.get(&resource.name).map(|crawler| &crawler.tags))
                .flatten(),
            TaggedResourceKind::Job => is_served_account(&resource.account_id)
                .then(|| self.jobs.get(&resource.name).map(|job| &job.tags))
                .flatten(),
        }
    }

    /// [`Self::resource_tags`] mutably: the ONE record every tag WRITE lands in, so a tag
    /// written through this service's own `TagResource` and one written through the
    /// account-wide tagging API are the same tag.
    pub fn resource_tags_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut HashMap<String, String>> {
        match resource.kind {
            TaggedResourceKind::Database => self
                .catalogs
                .get_mut(&resource.account_id)
                .and_then(|catalog| catalog.databases.get_mut(&resource.name))
                .map(|database| &mut database.tags),
            TaggedResourceKind::Connection => self
                .catalogs
                .get_mut(&resource.account_id)
                .and_then(|catalog| catalog.connections.get_mut(&resource.name))
                .map(|connection| &mut connection.tags),
            TaggedResourceKind::Crawler => {
                if !is_served_account(&resource.account_id) {
                    return None;
                }
                self.crawlers
                    .get_mut(&resource.name)
                    .map(|crawler| &mut crawler.tags)
            }
            TaggedResourceKind::Job => {
                if !is_served_account(&resource.account_id) {
                    return None;
                }
                self.jobs.get_mut(&resource.name).map(|job| &mut job.tags)
            }
        }
    }

    /// Every resource this store holds a tag set for, as `(ARN, tags)`.
    ///
    /// This is the account's tag-store rows for this service: the tags a create recorded
    /// are the resource's own, so the account-wide tagging API projects THESE records
    /// rather than keeping a second copy that a create would have to be taught to write.
    /// Resources carrying NO tag are included — the caller decides what to do with an
    /// untagged resource — and `region` is the Region the ARNs are minted for, for the
    /// reason [`request_region`] gives.
    pub fn tagged_resources(&self, region: &str) -> Vec<(String, HashMap<String, String>)> {
        let mut found: Vec<(String, HashMap<String, String>)> = Vec::new();
        for catalog in self.catalogs.values() {
            for database in catalog.databases.values() {
                found.push((
                    TaggedResourceKind::Database.arn(region, &database.name),
                    database.tags.clone(),
                ));
            }
            for connection in catalog.connections.values() {
                found.push((
                    TaggedResourceKind::Connection.arn(region, &connection.name),
                    connection.tags.clone(),
                ));
            }
        }
        for crawler in self.crawlers.values() {
            found.push((
                TaggedResourceKind::Crawler.arn(region, &crawler.name),
                crawler.tags.clone(),
            ));
        }
        for job in self.jobs.values() {
            found.push((
                TaggedResourceKind::Job.arn(region, &job.name),
                job.tags.clone(),
            ));
        }
        found
    }
}

/// One Data Catalog: the databases stored under a single `CatalogId`.
#[derive(Debug, Default)]
pub struct Catalog {
    /// Databases keyed by their folded (lowercased) name.
    pub databases: HashMap<String, DatabaseEntry>,
    /// Connection definitions keyed by `ConnectionInput$Name`, in name order.
    pub connections: BTreeMap<String, ConnectionEntry>,
}

impl Catalog {
    /// Whether this catalog can hold another database, i.e. is below
    /// [`MAX_DATABASES_PER_CATALOG`].
    pub fn can_store_database(&self) -> bool {
        self.databases.len() < MAX_DATABASES_PER_CATALOG
    }

    /// Whether this catalog can hold another connection, i.e. is below
    /// [`MAX_CONNECTIONS_PER_CATALOG`].
    pub fn can_store_connection(&self) -> bool {
        self.connections.len() < MAX_CONNECTIONS_PER_CATALOG
    }
}

/// A stored database, plus the tables it contains.
#[derive(Debug, Clone)]
pub struct DatabaseEntry {
    /// The folded name, which is also this entry's key in [`Catalog::databases`].
    pub name: String,
    pub description: Option<String>,
    pub location_uri: Option<String>,
    pub parameters: Option<HashMap<String, String>>,
    /// The Lake Formation default grants for tables created in this database. Always a
    /// definite list: the create either sent one (an empty list included) or the
    /// catalog's default from [`default_create_table_default_permissions`] was recorded.
    pub create_table_default_permissions: Vec<PrincipalPermissions>,
    pub target_database: Option<DatabaseIdentifier>,
    pub federated_database: Option<FederatedDatabase>,
    pub catalog_id: String,
    /// The opaque id the catalog assigned this database at create
    /// ([`new_database_id`]). Server-set and written once, so every later read of this
    /// database reports the same id and the tables it holds report it too — an update
    /// that renames the database does not change it, because it identifies the
    /// database rather than naming it.
    pub database_id: String,
    /// Server-set, written once at create so every later read reports the same instant.
    pub create_time: SystemTime,
    pub tags: HashMap<String, String>,
    /// Tables keyed by their folded (lowercased) name.
    pub tables: HashMap<String, TableEntry>,
}

impl DatabaseEntry {
    /// Whether this database can hold another table, i.e. is below
    /// [`MAX_TABLES_PER_DATABASE`].
    pub fn can_store_table(&self) -> bool {
        self.tables.len() < MAX_TABLES_PER_DATABASE
    }

    /// Build a stored database from the client's `DatabaseInput`.
    pub fn from_input(
        input: &DatabaseInput,
        catalog_id: &str,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: fold_name(&input.name),
            description: input.description.clone(),
            location_uri: input.location_uri.clone(),
            parameters: input.parameters.clone(),
            // Recorded at create, not filled in per read, so every later read of this
            // database reports the same grants.
            create_table_default_permissions: input
                .create_table_default_permissions
                .clone()
                .unwrap_or_else(default_create_table_default_permissions),
            target_database: input.target_database.clone(),
            federated_database: input.federated_database.clone(),
            catalog_id: catalog_id.to_string(),
            database_id: new_database_id(),
            create_time: now,
            tags: tags.cloned().unwrap_or_default(),
            tables: HashMap::new(),
        }
    }

    /// Rebuild the modelled `Database` output shape from what is stored.
    pub fn to_database(&self) -> Database {
        Database {
            name: self.name.clone(),
            description: self.description.clone(),
            location_uri: self.location_uri.clone(),
            parameters: self.parameters.clone(),
            create_time: Some(to_date_time(self.create_time)),
            create_table_default_permissions: Some(
                self.create_table_default_permissions
                    .iter()
                    .map(tagged_principal_permissions)
                    .collect(),
            ),
            target_database: self.target_database.clone(),
            catalog_id: Some(self.catalog_id.clone()),
            federated_database: self.federated_database.clone(),
            // The catalog attributes every database it returns to the catalog that owns
            // it and to the id it assigned the database, both reported DEFINITELY: a
            // client builds an ARN or correlates the database across Lake Formation and
            // Athena from these, so an absent member would leave it with nothing to key
            // on. Both come from what is stored, so repeated reads agree.
            catalog_identifier: Some(self.catalog_id.clone()),
            database_id: Some(self.database_id.clone()),
        }
    }

    /// Overwrite this database's definition with the one the client submitted.
    ///
    /// `DatabaseInput` is the WHOLE client-settable definition of a database — the
    /// same shape a create submits — so an update writes every member it carries and
    /// clears the ones it omits; what the client sent is exactly what the next read
    /// reports. The members no `DatabaseInput` carries are left alone, because the
    /// client cannot set them: `CatalogId` and `CreateTime` belong to the server, the
    /// tags are written through the tagging operations, and the tables this database
    /// owns are their own resources.
    ///
    /// The submitted `Name` is folded and written like any other member, so a request
    /// that submits a different one has MOVED the database: its tables' reported
    /// `DatabaseName` moves with it. Relocating the entry under its new key is the
    /// caller's job, since the key lives in [`Catalog::databases`].
    pub fn apply_input(&mut self, input: &DatabaseInput) {
        let folded = fold_name(&input.name);
        for table in self.tables.values_mut() {
            table.database_name = folded.clone();
        }
        self.name = folded;
        self.description = input.description.clone();
        self.location_uri = input.location_uri.clone();
        self.parameters = input.parameters.clone();
        self.create_table_default_permissions = input
            .create_table_default_permissions
            .clone()
            // An update that names no grants records the catalog's default, exactly as
            // a create does: the stored list is always definite, and an EMPTY list is
            // the opposite governance decision, so it is stored only when sent.
            .unwrap_or_else(default_create_table_default_permissions);
        self.target_database = input.target_database.clone();
        self.federated_database = input.federated_database.clone();
    }

    /// The stored tables ordered by folded name — the deterministic order
    /// `GetTables` pages over.
    pub fn tables_ordered(&self) -> Vec<&TableEntry> {
        let mut tables: Vec<&TableEntry> = self.tables.values().collect();
        tables.sort_by(|left, right| left.name.cmp(&right.name));
        tables
    }
}

/// A stored table inside its parent database.
#[derive(Debug, Clone)]
pub struct TableEntry {
    /// The folded name, which is also this entry's key in [`DatabaseEntry::tables`].
    pub name: String,
    pub database_name: String,
    pub catalog_id: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub table_type: Option<String>,
    pub retention: i32,
    pub parameters: Option<HashMap<String, String>>,
    pub storage_descriptor: Option<StorageDescriptor>,
    pub partition_keys: Option<Vec<Column>>,
    pub view_original_text: Option<String>,
    pub view_expanded_text: Option<String>,
    pub target_table: Option<TableIdentifier>,
    pub federated_table: Option<FederatedTable>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_access_time: Option<::aws_smithy_types::DateTime>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_analyzed_time: Option<::aws_smithy_types::DateTime>,
    /// Server-set, written once at create.
    pub create_time: SystemTime,
    /// Server-set; moves with every write to this table.
    pub update_time: SystemTime,
    /// Server-set version counter, [`FIRST_TABLE_VERSION_ID`] for a freshly created
    /// table.
    pub version_id: String,
    /// Server-set: the principal that issued the create.
    pub created_by: Option<String>,
}

impl TableEntry {
    /// Build a stored table from the client's `TableInput` plus the server-derived
    /// members (`DatabaseName`, `CatalogId`, `CreateTime`, `UpdateTime`, `VersionId`,
    /// `CreatedBy`), none of which exist on `TableInput`.
    pub fn from_input(
        input: &TableInput,
        database_name: &str,
        catalog_id: &str,
        created_by: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: fold_name(&input.name),
            database_name: database_name.to_string(),
            catalog_id: catalog_id.to_string(),
            description: input.description.clone(),
            owner: input.owner.clone(),
            table_type: input.table_type.clone(),
            retention: input.retention,
            parameters: input.parameters.clone(),
            storage_descriptor: input.storage_descriptor.clone(),
            partition_keys: input.partition_keys.clone(),
            view_original_text: input.view_original_text.clone(),
            view_expanded_text: input.view_expanded_text.clone(),
            target_table: input.target_table.clone(),
            federated_table: input.federated_table.clone(),
            last_access_time: input.last_access_time,
            last_analyzed_time: input.last_analyzed_time,
            create_time: now,
            update_time: now,
            version_id: FIRST_TABLE_VERSION_ID.to_string(),
            created_by,
        }
    }

    /// Overwrite this table's definition with the one the client submitted.
    ///
    /// `TableInput` is the WHOLE client-settable definition of a table — the same
    /// shape a create submits — so an update writes every member it carries and clears
    /// the ones it omits; what the client sent is exactly what the next read reports.
    /// The server-set members keep their own history: `CreateTime`, `CreatedBy`,
    /// `CatalogId` and `DatabaseName` stay as the create left them, `UpdateTime` moves
    /// to this write's instant, and `VersionId` advances to the next version, because
    /// the update produced a new version of the table.
    ///
    /// `TableInput$ViewDefinition` is NOT carried here: this slice stores no view
    /// representations, so an update that submits one is refused by the handler rather
    /// than accepted and dropped.
    ///
    /// The submitted `Name` is folded and written like any other member, but it can
    /// only ever be the name this table already has: `UpdateTable` refuses a request
    /// whose `TableInput$Name` is not the table it addresses, so the entry's key in
    /// [`DatabaseEntry::tables`] stays correct. A caller that ever applies an input
    /// under a different name must relocate the entry under its new key, the way
    /// `UpdateDatabase` does for [`DatabaseEntry::apply_input`].
    pub fn apply_input(&mut self, input: &TableInput, now: SystemTime) {
        self.name = fold_name(&input.name);
        self.description = input.description.clone();
        self.owner = input.owner.clone();
        self.table_type = input.table_type.clone();
        self.retention = input.retention;
        self.parameters = input.parameters.clone();
        self.storage_descriptor = input.storage_descriptor.clone();
        self.partition_keys = input.partition_keys.clone();
        self.view_original_text = input.view_original_text.clone();
        self.view_expanded_text = input.view_expanded_text.clone();
        self.target_table = input.target_table.clone();
        self.federated_table = input.federated_table.clone();
        self.last_access_time = input.last_access_time;
        self.last_analyzed_time = input.last_analyzed_time;
        self.update_time = now;
        self.version_id = next_version_id(&self.version_id);
    }

    /// Rebuild the modelled `Table` output shape from what is stored.
    ///
    /// `database_id` is the catalog-assigned id of the database this table belongs to,
    /// read from that [`DatabaseEntry`] by the caller rather than copied onto the table:
    /// the id belongs to the database, so there is one place it is stored and the table
    /// read cannot report an id that has drifted from the database read's.
    pub fn to_table(&self, database_id: &str) -> Table {
        // `Table$RLSProperties` is "the default row-level security policy of the table".
        // No stored table carries one, because no `TableInput` can set one — so the
        // catalog holds no row filter for any table it serves. Bound once here, because
        // `IsRowFilteringEnabled` below is derived from it: the flag says exactly whether
        // such a policy is in force, so the two can never report different things.
        let rls_properties: Option<RLSProperties> = None;
        Table {
            name: self.name.clone(),
            database_name: Some(self.database_name.clone()),
            description: self.description.clone(),
            owner: self.owner.clone(),
            create_time: Some(to_date_time(self.create_time)),
            update_time: Some(to_date_time(self.update_time)),
            last_access_time: self.last_access_time,
            last_analyzed_time: self.last_analyzed_time,
            retention: self.retention,
            storage_descriptor: self.storage_descriptor.as_ref().map(to_storage_descriptor),
            partition_keys: self.partition_keys.clone(),
            view_original_text: self.view_original_text.clone(),
            view_expanded_text: self.view_expanded_text.clone(),
            table_type: self.table_type.clone(),
            parameters: self.parameters.clone(),
            created_by: self.created_by.clone(),
            is_registered_with_lake_formation: false,
            target_table: self.target_table.clone(),
            catalog_id: Some(self.catalog_id.clone()),
            version_id: Some(self.version_id.clone()),
            federated_table: self.federated_table.clone(),
            // `Table$ViewDefinition` is "all the information that defines the view": no
            // stored table has one, because `CreateTable` REFUSES a `ViewDefinition`
            // rather than dropping it (see `ops/create_table.rs`).
            view_definition: None,
            // `IsMultiDialectView` and `IsMaterializedView` are three-state
            // `NullableBoolean` members the catalog answers DEFINITELY on every read: a
            // table is one of them exactly when the catalog holds the matching view
            // definition for it. No stored table does (see `view_definition` above), so
            // both are `false` — reported rather than omitted, so a client branching on
            // three states never reads "unknown" for a table whose kind the catalog
            // knows.
            is_multi_dialect_view: Some(false),
            is_materialized_view: Some(false),
            iceberg_table_metadata: None,
            status: None,
            // The catalog attributes every table it returns to the catalog that owns it
            // and to the database that holds it, by the ids it assigned them — the same
            // `DatabaseId` the database's own read reports, so the two responses agree.
            catalog_identifier: Some(self.catalog_id.clone()),
            database_id: Some(database_id.to_string()),
            // Row filtering is on exactly when the catalog holds a row-level security
            // policy for the table, so the flag is derived from `rls_properties` rather
            // than restated: `false` for every stored table today, and DEFINITELY false
            // rather than omitted, so a client that distinguishes "no filter" from
            // "unknown" reads the first — which is what the catalog knows.
            is_row_filtering_enabled: Some(rls_properties.is_some()),
            rls_properties,
        }
    }
}

/// Project a stored `StorageDescriptor` the way the Data Catalog reports one.
///
/// `StorageDescriptor$SortColumns` is "a list specifying the sort order of each bucket in
/// the table" — part of the table definition the catalog holds, so a table that defines
/// no sort order has an EMPTY sort order rather than an unknown one, and the catalog
/// reports `[]`. A client that iterates it then loops zero times instead of having to
/// handle an absent member. Every other member is reported exactly as it was stored: a
/// member the client never sent is one the catalog does not determine, so it stays
/// absent.
pub fn to_storage_descriptor(stored: &StorageDescriptor) -> StorageDescriptor {
    StorageDescriptor {
        sort_columns: Some(stored.sort_columns.clone().unwrap_or_default()),
        ..stored.clone()
    }
}

/// A stored crawler.
///
/// Every member of the modelled `Crawler` shape that a create can fill is held here, so
/// a later `GetCrawler` reports what the create stored and nothing else: the members
/// only a crawl produces (`LastCrawl`, `CrawlElapsedTime`) are not stored, because no
/// crawl has run.
#[derive(Debug, Clone)]
pub struct CrawlerEntry {
    /// The crawler's name, which is also its key in [`ComAmazonawsGlueAwsglueState::crawlers`].
    pub name: String,
    pub role: String,
    pub targets: CrawlerTargets,
    pub database_name: Option<String>,
    pub description: Option<String>,
    /// The custom classifiers associated with the crawler; empty when the create named
    /// none, which is the accurate report for a crawler that has no custom classifier.
    pub classifiers: Vec<String>,
    pub table_prefix: Option<String>,
    pub schema_change_policy: Option<SchemaChangePolicy>,
    pub recrawl_policy: Option<RecrawlPolicy>,
    pub lineage_configuration: Option<LineageConfiguration>,
    pub lake_formation_configuration: Option<LakeFormationConfiguration>,
    pub configuration: Option<String>,
    pub crawler_security_configuration: Option<String>,
    /// The cron expression the create supplied, if any. Reported inside a `Schedule`.
    pub schedule_expression: Option<String>,
    /// The catalog the crawler writes its output to.
    pub catalog_id: String,
    /// One of the modelled `CrawlerState` values; [`CRAWLER_STATE_READY`] at create.
    pub state: String,
    /// Server-set version counter, [`FIRST_CRAWLER_VERSION`] for a fresh crawler.
    pub version: i64,
    /// Server-set, written once at create so every later read reports the same instant.
    pub creation_time: SystemTime,
    /// Server-set; moves with every write to this crawler.
    pub last_updated: SystemTime,
    /// The request's `Tags`, which are not part of the `Crawler` shape — they are read
    /// back through the tagging operations.
    pub tags: HashMap<String, String>,
}

impl CrawlerEntry {
    /// Build a stored crawler from a `CreateCrawler` request plus the server-derived
    /// members (`State`, `Version`, `CreationTime`, `LastUpdated`), none of which the
    /// client may set.
    pub fn from_create_input(
        input: &CreateCrawlerInput,
        catalog_id: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            name: input.name.clone(),
            role: input.role.clone(),
            targets: input.targets.clone(),
            database_name: input.database_name.clone(),
            description: input.description.clone(),
            classifiers: input.classifiers.clone().unwrap_or_default(),
            table_prefix: input.table_prefix.clone(),
            schema_change_policy: input.schema_change_policy.clone(),
            recrawl_policy: input.recrawl_policy.clone(),
            lineage_configuration: input.lineage_configuration.clone(),
            lake_formation_configuration: input.lake_formation_configuration.clone(),
            configuration: input.configuration.clone(),
            crawler_security_configuration: input.crawler_security_configuration.clone(),
            schedule_expression: input.schedule.clone(),
            catalog_id: catalog_id.to_string(),
            state: CRAWLER_STATE_READY.to_string(),
            version: FIRST_CRAWLER_VERSION,
            creation_time: now,
            last_updated: now,
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `Crawler` output shape from what is stored.
    pub fn to_crawler(&self) -> Crawler {
        Crawler {
            name: Some(self.name.clone()),
            role: Some(self.role.clone()),
            targets: Some(self.targets.clone()),
            database_name: self.database_name.clone(),
            description: self.description.clone(),
            classifiers: Some(self.classifiers.clone()),
            recrawl_policy: self.recrawl_policy.clone(),
            schema_change_policy: self.schema_change_policy.clone(),
            lineage_configuration: self.lineage_configuration.clone(),
            state: Some(self.state.clone()),
            table_prefix: self.table_prefix.clone(),
            schedule: self.schedule_expression.as_ref().map(|expression| Schedule {
                schedule_expression: Some(expression.clone()),
                state: Some(SCHEDULE_STATE_SCHEDULED.to_string()),
            }),
            // "If the crawler is running, contains the total time elapsed since the
            // last crawl began" — nothing has run, so no time has elapsed.
            crawl_elapsed_time: 0,
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated: Some(to_date_time(self.last_updated)),
            // "Status and error information about the most recent crawl": absent until
            // a crawl has produced one.
            last_crawl: None,
            version: self.version,
            configuration: self.configuration.clone(),
            crawler_security_configuration: self.crawler_security_configuration.clone(),
            lake_formation_configuration: self.lake_formation_configuration.clone(),
            catalog_id: Some(self.catalog_id.clone()),
        }
    }
}

/// A stored custom entity type: the custom pattern `CreateCustomEntityType` registers
/// for detecting sensitive data.
///
/// Every member of the modelled `CustomEntityType` shape is held here, plus the create's
/// `Tags`, which that shape does not carry — so a later read reports the pattern the
/// create stored and nothing the server invented. There is no server-set member: the
/// model gives a custom entity type no id, status or timestamp.
#[derive(Debug, Clone)]
pub struct CustomEntityTypeEntry {
    /// The pattern's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::custom_entity_types`].
    pub name: String,
    /// "A regular expression string that is used for detecting sensitive data in a custom
    /// pattern", stored exactly as sent.
    pub regex_string: String,
    /// The context words the create supplied, if any. "If no context words are passed only
    /// a regular expression is checked", so a create that named none is stored with NO
    /// list rather than with an empty one: the two are different patterns, and the
    /// modelled list is 1..20 long, so an empty list is not even a value a create can send.
    pub context_words: Option<Vec<String>>,
    /// The create's `Tags`, which are not part of the `CustomEntityType` shape.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations: [`TaggedResourceKind`] carries no custom entity
    /// type variant, because the model states no ARN resource-type segment for this kind
    /// (nothing in it spells one), and the segment is what every tag lookup resolves an
    /// ARN against. So a tagging call naming a custom entity type still resolves to
    /// nothing, and these tags are stored rather than served.
    pub tags: HashMap<String, String>,
}

impl CustomEntityTypeEntry {
    /// Build a stored custom entity type from a `CreateCustomEntityType` request. Every
    /// member comes from the request: the client supplies the whole definition.
    pub fn from_create_input(input: &CreateCustomEntityTypeInput) -> Self {
        Self {
            name: input.name.clone(),
            regex_string: input.regex_string.clone(),
            context_words: input.context_words.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }
}

/// `JobCommand$Name` values the model documents: "For an Apache Spark ETL job, this
/// must be `glueetl`. For a Python shell job, it must be `pythonshell`. For an Apache
/// Spark streaming ETL job, this must be `gluestreaming`. For a Ray job, this must be
/// `glueray`." A command naming anything else is not a job this service can run.
pub const JOB_COMMAND_NAMES: [&str; 4] = ["glueetl", "pythonshell", "gluestreaming", "glueray"];

/// The `JobMode` stored for a create that named none: "When the JobMode field is
/// missing or null, SCRIPT is assigned as the default value" (`CreateJob$JobMode`).
pub const DEFAULT_JOB_MODE: &str = "SCRIPT";

/// The `GlueVersion` stored for a create that named none: "Jobs that are created
/// without specifying a Glue version default to Glue 5.1" (`CreateJob$GlueVersion`).
pub const DEFAULT_GLUE_VERSION: &str = "5.1";

/// `ExecutionProperty$MaxConcurrentRuns` for a job whose create named no execution
/// property: "The maximum number of concurrent runs allowed for the job. The default
/// is 1." The model's `@default` of 0 is what an ABSENT integer deserializes to, not a
/// concurrency a job can run at.
pub const DEFAULT_MAX_CONCURRENT_RUNS: i32 = 1;

/// `Job$Timeout` when the create left it blank, for Glue 4.0 and earlier: "When the
/// value is left blank, the timeout is defaulted to 2,880 minutes for Glue version 4.0
/// and earlier, or 480 minutes for Glue version 5.0 and later."
pub const DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4: i32 = 2_880;

/// `Job$Timeout` when the create left it blank, for Glue 5.0 and later — the second
/// half of the sentence quoted on [`DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4`].
pub const DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5: i32 = 480;

/// The largest `Timeout` a job may carry: "Jobs must have timeout values less than 7
/// days or 10080 minutes. Otherwise, the jobs will throw an exception." (`Job$Timeout`)
pub const MAX_TIMEOUT_MINUTES: i32 = 10_080;

/// The `JobCommand$Name` of an Apache Spark ETL job, which is the only command
/// `ExecutionClass=FLEX` may be set on, and the `ExecutionClass` that restriction is
/// about: "Only jobs with Glue version 3.0 and above and command type `glueetl` will be
/// allowed to set `ExecutionClass` to `FLEX`." (`CreateJob$ExecutionClass`.)
pub const SPARK_ETL_COMMAND: &str = "glueetl";
/// See [`SPARK_ETL_COMMAND`].
pub const EXECUTION_CLASS_FLEX: &str = "FLEX";
/// The lowest `GlueVersion` that sentence admits `FLEX` on.
pub const MIN_FLEX_GLUE_VERSION: f64 = 3.0;

/// Whether a job's command and Glue version are the pair `ExecutionClass=FLEX` is allowed
/// on, by the sentence quoted on [`SPARK_ETL_COMMAND`].
///
/// A `GlueVersion` this cannot read as a number (the model's pattern `^(\w+\.)+\w+$` is
/// wider than a decimal) admits nothing: the restriction is stated over a version
/// comparison, so a version that cannot be compared does not satisfy it.
pub fn admits_flex_execution_class(command: &JobCommand, glue_version: &str) -> bool {
    let spark_etl = command.name.as_deref() == Some(SPARK_ETL_COMMAND);
    let version_admits = glue_version
        .parse::<f64>()
        .is_ok_and(|version| version >= MIN_FLEX_GLUE_VERSION);
    spark_etl && version_admits
}

/// The first Glue major version that takes [`DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5`].
const FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT: u32 = 5;

/// The DPUs one worker of `worker_type` maps to, as `CreateJob$WorkerType` documents
/// each predefined worker ("For the `G.1X` worker type, each worker maps to 1 DPU", "…
/// `G.2X` … 2 DPU", "… `G.4X` … 4 DPU", "… `G.8X` … 8 DPU", "… `G.025X` … 0.25 DPU",
/// "… `Z.2X` … 2 M-DPU").
///
/// `None` for anything the model gives no per-worker DPU figure for — the `Standard`
/// worker type among them — so no capacity is derived where the service documents none.
pub fn dpu_per_worker(worker_type: &str) -> Option<f64> {
    match worker_type {
        "G.025X" => Some(0.25),
        "G.1X" => Some(1.0),
        "G.2X" | "Z.2X" => Some(2.0),
        "G.4X" => Some(4.0),
        "G.8X" => Some(8.0),
        _ => None,
    }
}

/// The `Timeout` a job created without one carries, which depends on its Glue version
/// (see [`DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4`]). The major version is the leading
/// integer of the `GlueVersion` string (`^(\w+\.)+\w+$`); a version whose major part is
/// not a number is treated as the current default, which is what an absent
/// `GlueVersion` resolves to.
pub fn default_timeout_minutes(glue_version: &str) -> i32 {
    let major = glue_version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT);
    if major >= FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT {
        DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5
    } else {
        DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4
    }
}

/// A stored job definition, plus the runs `StartJobRun` has started of it.
///
/// Every member of the modelled `Job` shape a create can fill is held here, so a later
/// `GetJob` reports what the create stored and nothing else — the members only a RUN
/// produces live on [`JobRunEntry`], in [`JobEntry::runs`], and `ProfileName` is not a
/// `CreateJob` member, so a created job carries none.
#[derive(Debug, Clone)]
pub struct JobEntry {
    /// The job's name, which is also its key in [`ComAmazonawsGlueAwsglueState::jobs`].
    pub name: String,
    /// One of the modelled `JobMode` values; [`DEFAULT_JOB_MODE`] when the create
    /// named none.
    pub job_mode: String,
    pub job_run_queuing_enabled: Option<bool>,
    pub description: Option<String>,
    pub log_uri: Option<String>,
    pub role: String,
    /// Always stored, because the service documents a default concurrency
    /// ([`DEFAULT_MAX_CONCURRENT_RUNS`]) rather than "no execution property".
    pub execution_property: ExecutionProperty,
    pub command: JobCommand,
    pub default_arguments: Option<HashMap<String, String>>,
    pub non_overridable_arguments: Option<HashMap<String, String>>,
    pub connections: Option<ConnectionsList>,
    pub max_retries: i32,
    /// The job's capacity in DPUs: what the create named, else what its worker
    /// configuration maps to (see [`JobEntry::from_create_input`]). `None` when the
    /// create named neither, because this service documents no figure for that job.
    pub max_capacity: Option<f64>,
    /// The job timeout in minutes: what the create named, else the documented default
    /// for its Glue version.
    pub timeout: i32,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub security_configuration: Option<String>,
    pub notification_property: Option<NotificationProperty>,
    /// [`DEFAULT_GLUE_VERSION`] when the create named none.
    pub glue_version: String,
    pub code_gen_configuration_nodes: Option<HashMap<String, CodeGenConfigurationNode>>,
    pub execution_class: Option<String>,
    pub source_control_details: Option<SourceControlDetails>,
    pub maintenance_window: Option<String>,
    /// Server-set, written once at create so every later read reports the same instant.
    pub created_on: SystemTime,
    /// Server-set; moves with every write to this job.
    pub last_modified_on: SystemTime,
    /// The request's `Tags`, which are not part of the `Job` shape — they are read back
    /// through the tagging operations.
    pub tags: HashMap<String, String>,
    /// The runs of this job definition, oldest first, as `StartJobRun` appended them.
    ///
    /// They live on the job rather than in a store-wide map because every operation that
    /// addresses a run names its job as well (`GetJobRun`/`GetJobRuns`/`BatchStopJobRun`
    /// all take `JobName`), so a run is reclaimed with the definition it belongs to.
    pub runs: Vec<JobRunEntry>,
}

impl JobEntry {
    /// Build a stored job from a `CreateJob` request plus the members the SERVICE
    /// fills: the documented defaults for an absent `JobMode`, `GlueVersion`,
    /// `ExecutionProperty` and `Timeout`, the capacity its worker configuration maps
    /// to, and the create instant.
    pub fn from_create_input(input: &CreateJobInput, now: SystemTime) -> Self {
        let glue_version = input
            .glue_version
            .clone()
            .unwrap_or_else(|| DEFAULT_GLUE_VERSION.to_string());
        let max_concurrent_runs = input
            .execution_property
            .as_ref()
            .map(|property| property.max_concurrent_runs)
            .filter(|runs| *runs > 0)
            .unwrap_or(DEFAULT_MAX_CONCURRENT_RUNS);
        Self {
            name: input.name.clone(),
            job_mode: input
                .job_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_JOB_MODE.to_string()),
            job_run_queuing_enabled: input.job_run_queuing_enabled,
            description: input.description.clone(),
            log_uri: input.log_uri.clone(),
            role: input.role.clone(),
            execution_property: ExecutionProperty {
                max_concurrent_runs,
            },
            command: input.command.clone(),
            default_arguments: input.default_arguments.clone(),
            non_overridable_arguments: input.non_overridable_arguments.clone(),
            connections: input.connections.clone(),
            max_retries: input.max_retries,
            max_capacity: capacity_of(input),
            timeout: input
                .timeout
                .unwrap_or_else(|| default_timeout_minutes(&glue_version)),
            worker_type: input.worker_type.clone(),
            number_of_workers: input.number_of_workers,
            security_configuration: input.security_configuration.clone(),
            notification_property: input.notification_property.clone(),
            glue_version,
            code_gen_configuration_nodes: input.code_gen_configuration_nodes.clone(),
            execution_class: input.execution_class.clone(),
            source_control_details: input.source_control_details.clone(),
            maintenance_window: input.maintenance_window.clone(),
            created_on: now,
            last_modified_on: now,
            tags: input.tags.clone().unwrap_or_default(),
            // A job definition has no runs until `StartJobRun` starts one.
            runs: Vec::new(),
        }
    }

    /// The maximum number of this job's runs that may be in flight at once:
    /// `ExecutionProperty$MaxConcurrentRuns`, held at the documented minimum of one
    /// ([`DEFAULT_MAX_CONCURRENT_RUNS`]) so a stored zero cannot make the job unrunnable.
    pub fn max_concurrent_runs(&self) -> i32 {
        self.execution_property
            .max_concurrent_runs
            .max(DEFAULT_MAX_CONCURRENT_RUNS)
    }

    /// Give every run of this job the state the CLOCK already owed it at `now`.
    ///
    /// This is the one write a reporting operation performs: a run's next state was due
    /// at [`JobRunEntry::settles_at`] whether or not anybody called, so materialising it
    /// on read reports the state the clock had already reached. It never moves a run
    /// because something was read.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            run.settle_if_due(now);
        }
    }

    /// The stored run with this `Id`, or `None` when this job has no such run.
    pub fn run(&self, run_id: &str) -> Option<&JobRunEntry> {
        self.runs.iter().find(|run| run.id == run_id)
    }

    /// How many of this job's runs have not finished.
    ///
    /// Call [`JobEntry::settle_due_runs`] first, or a run the clock has already finished
    /// still counts as in flight.
    pub fn runs_in_flight(&self) -> usize {
        self.runs.iter().filter(|run| !run.is_terminal()).count()
    }

    /// Append a run, keeping at most [`MAX_RUNS_PER_JOB`] of them.
    pub fn record_run(&mut self, run: JobRunEntry) {
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_JOB {
            let overflow = self.runs.len() - MAX_RUNS_PER_JOB;
            self.runs.drain(..overflow);
        }
    }

    /// Rebuild the modelled `Job` output shape from what is stored.
    pub fn to_job(&self) -> Job {
        Job {
            name: Some(self.name.clone()),
            job_mode: Some(self.job_mode.clone()),
            job_run_queuing_enabled: self.job_run_queuing_enabled,
            description: self.description.clone(),
            log_uri: self.log_uri.clone(),
            role: Some(self.role.clone()),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            execution_property: Some(self.execution_property.clone()),
            command: Some(self.command.clone()),
            default_arguments: self.default_arguments.clone(),
            non_overridable_arguments: self.non_overridable_arguments.clone(),
            connections: self.connections.clone(),
            max_retries: self.max_retries,
            // `AllocatedCapacity` is the same quantity as `MaxCapacity` — "This field
            // is deprecated. Use MaxCapacity instead" — so it reports the same number
            // of whole DPUs rather than a second, disagreeing figure. A job with no
            // capacity figure reports the model's own default of 0.
            allocated_capacity: self.max_capacity.map_or(0, |capacity| capacity as i32),
            timeout: Some(self.timeout),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            security_configuration: self.security_configuration.clone(),
            notification_property: self.notification_property.clone(),
            glue_version: Some(self.glue_version.clone()),
            code_gen_configuration_nodes: self.code_gen_configuration_nodes.clone(),
            execution_class: self.execution_class.clone(),
            source_control_details: self.source_control_details.clone(),
            maintenance_window: self.maintenance_window.clone(),
            // `ProfileName` names a usage profile, which `CreateJob` cannot set.
            profile_name: None,
        }
    }
}

/// The capacity, in DPUs, a `CreateJob` request asks for: see [`capacity_from`].
fn capacity_of(input: &CreateJobInput) -> Option<f64> {
    capacity_from(
        input.max_capacity,
        input.allocated_capacity,
        input.worker_type.as_deref(),
        input.number_of_workers,
    )
}

/// The capacity, in DPUs, a request asks for: `MaxCapacity` when it names one, else the
/// deprecated `AllocatedCapacity` ("This parameter is deprecated. Use MaxCapacity
/// instead", so the two are one quantity), else what the worker configuration maps to —
/// `NumberOfWorkers` workers of `WorkerType`, at the DPUs per worker [`dpu_per_worker`]
/// quotes from the model. `None` when nothing names a capacity and no worker
/// configuration the model gives a DPU figure for is named.
///
/// `CreateJob` and `StartJobRun` carry the same four members and the same documentation
/// for each, so both resolve capacity here.
fn capacity_from(
    max_capacity: Option<f64>,
    allocated_capacity: i32,
    worker_type: Option<&str>,
    number_of_workers: Option<i32>,
) -> Option<f64> {
    if let Some(max_capacity) = max_capacity {
        return Some(max_capacity);
    }
    if allocated_capacity > 0 {
        return Some(f64::from(allocated_capacity));
    }
    let workers = number_of_workers.filter(|count| *count > 0)?;
    let per_worker = dpu_per_worker(worker_type?)?;
    Some(per_worker * f64::from(workers))
}

/// The `JobRunState` a run is in the instant `StartJobRun` returns, and the one the
/// clock then owes it. Both are values of the modelled `JobRunState` enum (`STARTING`,
/// `RUNNING`, `STOPPING`, `STOPPED`, `SUCCEEDED`, `FAILED`, `TIMEOUT`, `ERROR`,
/// `WAITING`, `EXPIRED`).
pub const JOB_RUN_STATE_STARTING: &str = "STARTING";
/// See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_FAILED: &str = "FAILED";

/// The `JobRunState` values a run never moves out of again: of the modelled enum quoted
/// on [`JOB_RUN_STATE_STARTING`], the ones that name a finished run. The other four
/// (`STARTING`, `RUNNING`, `STOPPING`, `WAITING`) each still owe a further state.
pub const TERMINAL_JOB_RUN_STATES: [&str; 6] = [
    "STOPPED",
    "SUCCEEDED",
    "FAILED",
    "TIMEOUT",
    "ERROR",
    "EXPIRED",
];

/// How long a run stays in [`JOB_RUN_STATE_STARTING`] before the clock owes it its
/// terminal state.
///
/// CHOSEN, not documented: Glue publishes no duration for starting a job run, so this is
/// the emulator's own span — deliberately not zero, so a caller that polls `GetJobRun`
/// straight after `StartJobRun` still observes the run in flight, and the concurrency
/// limit that goes with it. The due instant is stored on the run and materialised by
/// whichever later call reads it; nothing sleeps.
pub const JOB_RUN_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// `JobRun$ErrorMessage` for the terminal state the clock owes every run.
///
/// This emulator is the Glue control plane: it records a run faithfully, but it runs no
/// job script, so a run can never reach `SUCCEEDED` — it ends in the modelled failure
/// state carrying a reason that says exactly that, rather than reporting a result nothing
/// computed.
pub const JOB_RUN_NOT_EXECUTED_MESSAGE: &str =
    "This emulator records job runs but executes no job script, so the job run ended \
     without running.";

/// `JobRun$Attempt` of a run that is not a retry of an earlier one: "The number of the
/// attempt to run this job", which for a first attempt is the model's own default of 0.
pub const FIRST_JOB_RUN_ATTEMPT: i32 = 0;

/// The prefix a Glue job run id carries, which is what makes an `IdString` recognisable
/// as a job run id rather than any other id the service mints.
pub const JOB_RUN_ID_PREFIX: &str = "jr_";

/// How many runs one job definition keeps.
///
/// CHOSEN: Glue publishes no retention for job-run records, so this is the emulator's own
/// window. It bounds [`JobEntry::runs`], and with it every per-job walk, so a job that
/// has been started many times still costs a bounded amount of work and memory. The
/// OLDEST record is dropped first, so the runs a caller is most likely to read are kept.
pub const MAX_RUNS_PER_JOB: usize = 100;

/// How many job run ids this process has minted. A nonce source for
/// [`new_job_run_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
static MINTED_JOB_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `JobRunId`: the id the service assigns a job run when it starts.
///
/// [`JOB_RUN_ID_PREFIX`] followed by 64 lowercase hexadecimal characters, i.e. 256 bits,
/// which is the shape a Glue job run id has on the wire and is inside `IdString`'s
/// `length 1..255`. The bits are minted the way [`new_database_id`] mints a database id,
/// so two runs started in the same nanosecond cannot be given the same id. It is stored
/// on the run, so every later read of that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_job_run_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let sequence = MINTED_JOB_RUN_IDS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let words = [
        RandomState::new().hash_one((sequence, nanos)),
        RandomState::new().hash_one((nanos, sequence)),
        RandomState::new().hash_one((sequence, sequence, nanos)),
        RandomState::new().hash_one((nanos, nanos, sequence)),
    ];
    let mut id = String::from(JOB_RUN_ID_PREFIX);
    for word in words {
        id.push_str(&format!("{word:016x}"));
    }
    id
}

/// One run of a job definition, as `StartJobRun` recorded it.
///
/// A run carries the job's own configuration with the members the `StartJobRun` request
/// overrode applied on top — "This value overrides the timeout value set in the parent
/// job" (`StartJobRun$Timeout`) is the rule for each of them — plus the members only a
/// run has: its id, its attempt, its state and the instants it moved between them.
#[derive(Debug, Clone)]
pub struct JobRunEntry {
    /// `JobRun$Id`, minted by [`new_job_run_id`].
    pub id: String,
    /// The name of the job definition this is a run of.
    pub job_name: String,
    /// `JobRun$Attempt`; one past the retried run's attempt when this run retries one.
    pub attempt: i32,
    /// `JobRun$PreviousRunId`: "the `JobRunId` specified in the `StartJobRun` action",
    /// i.e. the run this one retries, when the request named one.
    pub previous_run_id: Option<String>,
    pub job_mode: String,
    pub job_run_queuing_enabled: Option<bool>,
    /// Server-set: the instant `StartJobRun` recorded this run.
    pub started_on: SystemTime,
    /// Server-set: moves with every state change of this run.
    pub last_modified_on: SystemTime,
    /// Server-set when the run reaches a terminal state, and absent until then.
    pub completed_on: Option<SystemTime>,
    /// One of the modelled `JobRunState` values.
    pub state: String,
    /// The instant the clock owes this run its next state; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// The job's `DefaultArguments` with this run's `Arguments` applied over them, then
    /// the job's `NonOverridableArguments`: "For this job run, they replace the default
    /// arguments set in the job definition itself" (`StartJobRun$Arguments`), except for
    /// the "arguments for this job that are not overridden when providing job arguments
    /// in a job run" (`CreateJob$NonOverridableArguments`).
    pub arguments: HashMap<String, String>,
    pub error_message: Option<String>,
    pub max_capacity: Option<f64>,
    pub timeout: i32,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub security_configuration: Option<String>,
    pub notification_property: Option<NotificationProperty>,
    pub glue_version: String,
    pub execution_class: Option<String>,
    pub maintenance_window: Option<String>,
    pub execution_role_session_policy: Option<String>,
}

impl JobRunEntry {
    /// Record a run of `job` as the `StartJobRun` request asks for it, in
    /// [`JOB_RUN_STATE_STARTING`] with its next state due [`JOB_RUN_SETTLES_AFTER`] out.
    ///
    /// `retries` is the `(id, attempt)` of the run this one retries, when the request's
    /// `JobRunId` ("The ID of a previous JobRun to retry") named one.
    pub fn start(
        job: &JobEntry,
        input: &StartJobRunInput,
        id: String,
        now: SystemTime,
        retries: Option<(String, i32)>,
    ) -> Self {
        let (previous_run_id, attempt) = match retries {
            Some((previous_id, previous_attempt)) => {
                (Some(previous_id), previous_attempt.saturating_add(1))
            }
            None => (None, FIRST_JOB_RUN_ATTEMPT),
        };
        // Each capacity member of the run is the one the request named, else the job's,
        // and the DPU figure is then derived from that pair exactly as a job's is.
        let worker_type = input.worker_type.clone().or_else(|| job.worker_type.clone());
        let number_of_workers = input.number_of_workers.or(job.number_of_workers);
        let max_capacity = capacity_from(
            input.max_capacity,
            input.allocated_capacity,
            worker_type.as_deref(),
            number_of_workers,
        )
        .or(job.max_capacity);

        let mut arguments = job.default_arguments.clone().unwrap_or_default();
        if let Some(requested) = input.arguments.as_ref() {
            arguments.extend(
                requested
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }
        if let Some(locked) = job.non_overridable_arguments.as_ref() {
            arguments.extend(
                locked
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }

        Self {
            id,
            job_name: job.name.clone(),
            attempt,
            previous_run_id,
            job_mode: job.job_mode.clone(),
            // "If this field does not match the value set in the job run, then the value
            // from the job run field will be used."
            job_run_queuing_enabled: input.job_run_queuing_enabled.or(job.job_run_queuing_enabled),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            state: JOB_RUN_STATE_STARTING.to_string(),
            settles_at: Some(now.checked_add(JOB_RUN_SETTLES_AFTER).unwrap_or(now)),
            arguments,
            // Nothing has failed yet: a run carries an error message only once it has.
            error_message: None,
            max_capacity,
            timeout: input.timeout.unwrap_or(job.timeout),
            worker_type,
            number_of_workers,
            security_configuration: input
                .security_configuration
                .clone()
                .or_else(|| job.security_configuration.clone()),
            notification_property: input
                .notification_property
                .clone()
                .or_else(|| job.notification_property.clone()),
            // `GlueVersion` is not a `StartJobRun` member: a run uses its job's.
            glue_version: job.glue_version.clone(),
            execution_class: input
                .execution_class
                .clone()
                .or_else(|| job.execution_class.clone()),
            // Nor is `MaintenanceWindow`.
            maintenance_window: job.maintenance_window.clone(),
            execution_role_session_policy: input.execution_role_session_policy.clone(),
        }
    }

    /// Whether this run has finished, by [`TERMINAL_JOB_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_JOB_RUN_STATES.contains(&self.state.as_str())
    }

    /// Move this run into the state the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave [`JOB_RUN_STATE_STARTING`] at [`JobRunEntry::settles_at`]
    /// whether or not anybody called, and the instants recorded are that DUE instant, not
    /// the instant somebody read it — so two reads of a settled run report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.state = JOB_RUN_STATE_FAILED.to_string();
        self.error_message = Some(JOB_RUN_NOT_EXECUTED_MESSAGE.to_string());
        self.completed_on = Some(due);
        self.last_modified_on = due;
    }

    /// `JobRun$ExecutionTime`: "The amount of time (in seconds) that the job run consumed
    /// resources" — the whole seconds between the run starting and finishing, and the
    /// model's own default of 0 while it has not finished.
    fn execution_time_seconds(&self) -> i32 {
        let Some(completed_on) = self.completed_on else {
            return 0;
        };
        let elapsed = completed_on
            .duration_since(self.started_on)
            .unwrap_or_default();
        i32::try_from(elapsed.as_secs()).unwrap_or(i32::MAX)
    }

    /// Rebuild the modelled `JobRun` output shape from what is stored.
    pub fn to_job_run(&self) -> JobRun {
        JobRun {
            id: Some(self.id.clone()),
            attempt: self.attempt,
            previous_run_id: self.previous_run_id.clone(),
            // "The name of the trigger that started this job run": this run was started
            // through `StartJobRun`, so no trigger started it.
            trigger_name: None,
            job_name: Some(self.job_name.clone()),
            job_mode: Some(self.job_mode.clone()),
            job_run_queuing_enabled: self.job_run_queuing_enabled,
            started_on: Some(to_date_time(self.started_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            completed_on: self.completed_on.map(to_date_time),
            job_run_state: Some(self.state.clone()),
            arguments: Some(self.arguments.clone()),
            error_message: self.error_message.clone(),
            // "A job run that was used in the predicate of a conditional trigger that
            // triggered this job run" (`Predecessor`): a run started through
            // `StartJobRun` has none, and a run this one retries is not one of them —
            // that is `PreviousRunId`.
            predecessor_runs: None,
            // `AllocatedCapacity` is the same quantity as `MaxCapacity` — "This field is
            // deprecated. Use MaxCapacity instead" — so it reports the same number of
            // whole DPUs, and the model's own default of 0 for a run with no capacity.
            allocated_capacity: self.max_capacity.map_or(0, |capacity| capacity as i32),
            execution_time: self.execution_time_seconds(),
            timeout: Some(self.timeout),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            security_configuration: self.security_configuration.clone(),
            // "The name of the log group for secure logging": no job script runs here, so
            // nothing writes this run's output anywhere and there is no log group holding
            // it to name.
            log_group_name: None,
            notification_property: self.notification_property.clone(),
            glue_version: Some(self.glue_version.clone()),
            // "The total time each executor ran during the lifecycle of a job run in
            // seconds, multiplied by a DPU factor": no executor ran.
            dpu_seconds: None,
            execution_class: self.execution_class.clone(),
            maintenance_window: self.maintenance_window.clone(),
            // `ProfileName` names a usage profile, which neither `CreateJob` nor
            // `StartJobRun` can set.
            profile_name: None,
            // "For example, when a job run is in a WAITING state as a result of job run
            // queuing, the field has the reason why": this run was never queued, and the
            // reason it failed is its `ErrorMessage`.
            state_detail: None,
            execution_role_session_policy: self.execution_role_session_policy.clone(),
        }
    }
}

/// Whether `targets` names at least one data store to crawl, across every target list
/// `CrawlerTargets` carries. `CreateCrawler` documents that "at least one crawl target
/// must be specified", and an empty list names none.
pub fn has_crawl_target(targets: &CrawlerTargets) -> bool {
    fn any<T>(list: &Option<Vec<T>>) -> bool {
        list.as_ref().is_some_and(|entries| !entries.is_empty())
    }

    any(&targets.s3_targets)
        || any(&targets.jdbc_targets)
        || any(&targets.mongo_db_targets)
        || any(&targets.dynamo_db_targets)
        || any(&targets.catalog_targets)
        || any(&targets.delta_targets)
        || any(&targets.iceberg_targets)
        || any(&targets.hudi_targets)
}

/// Whether `expression` has the form the service documents for a crawler schedule: a
/// `cron(..)` expression of six fields ("to run something every day at 12:15 UTC, you
/// would specify: `cron(15 12 * * ? *)`" — `Schedule$ScheduleExpression`). Only the
/// form is checked here; the field values are the scheduler's business.
pub fn is_cron_expression(expression: &str) -> bool {
    let trimmed = expression.trim();
    let Some(fields) = trimmed
        .strip_prefix("cron(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    fields.split_whitespace().count() == 6
}

/// The `VersionId` a table carries straight after `CreateTable`.
///
/// The catalog's table version history starts at 0: the version a create makes is
/// version `"0"`, and each update appends the next one. So a table that has never been
/// updated reports `"0"`, and that is the id `GetTableVersion`/`DeleteTableVersion`
/// address its only version by — numbering the first version `"1"` left version `"0"`
/// unfetchable. `VersionString` is a string, so the id is formatted as one.
pub const FIRST_TABLE_VERSION_ID: &str = "0";

/// A stored connection definition: the `Connection` every read projects, plus the tags the
/// create carried.
#[derive(Debug, Clone)]
pub struct ConnectionEntry {
    /// `Name`, which is also this entry's key in [`Catalog::connections`].
    pub name: String,
    /// What a read of this connection reports, before `HidePassword` is applied.
    pub definition: Connection,
    /// `CreateConnectionRequest$Tags`.
    pub tags: HashMap<String, String>,
}

impl ConnectionEntry {
    /// Build a stored connection definition from the client's `ConnectionInput`.
    ///
    /// `CreationTime` and `LastUpdatedTime` are both the instant of the create (nothing has
    /// updated the definition yet), `LastUpdatedBy` is the principal that issued it, and
    /// `ConnectionSchemaVersion` is 2 exactly when the input carried the
    /// per-compute-environment property maps version 2 adds ("Version 2 supports properties
    /// for specific compute environments"), else 1.
    pub fn from_input(
        input: &ConnectionInput,
        last_updated_by: Option<String>,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        let compute_environment_properties = [
            input.spark_properties.as_ref(),
            input.athena_properties.as_ref(),
            input.python_properties.as_ref(),
        ]
        .iter()
        .any(|properties| properties.is_some_and(|map| !map.is_empty()));
        let at = to_date_time(now);
        Self {
            name: input.name.clone(),
            definition: Connection {
                name: Some(input.name.clone()),
                description: input.description.clone(),
                connection_type: Some(input.connection_type.clone()),
                match_criteria: input.match_criteria.clone(),
                connection_properties: Some(input.connection_properties.clone()),
                spark_properties: input.spark_properties.clone(),
                athena_properties: input.athena_properties.clone(),
                python_properties: input.python_properties.clone(),
                physical_connection_requirements: input.physical_connection_requirements.clone(),
                creation_time: Some(at),
                last_updated_time: Some(at),
                last_updated_by,
                // `Status`/`StatusReason` report the outcome of VALIDATING the connection
                // ("Can be one of: READY, IN_PROGRESS, or FAILED"), and this emulator
                // connects to nothing, so it reports no status and no validation instant
                // rather than inventing one.
                status: None,
                status_reason: None,
                last_connection_validation_time: None,
                // The authentication configuration a read reports is the one the input
                // declared, with its credentials left where they were: an
                // `AuthenticationConfigurationInput` carries secrets
                // (`BasicAuthenticationCredentials`, OAuth2 credentials) that the response
                // shape has no member for, and nothing here copies them into one.
                authentication_configuration: input.authentication_configuration.as_ref().map(
                    |configuration| crate::ops::types::AuthenticationConfiguration {
                        authentication_type: configuration.authentication_type.clone(),
                        secret_arn: configuration.secret_arn.clone(),
                        kms_key_arn: configuration.kms_key_arn.clone(),
                        o_auth2_properties: None,
                    },
                ),
                connection_schema_version: Some(if compute_environment_properties {
                    CONNECTION_SCHEMA_VERSION_V2
                } else {
                    CONNECTION_SCHEMA_VERSION_V1
                }),
                // "A list of compute environments compatible with the connection" is a
                // property of the connection TYPE, which this emulator does not model.
                compatible_compute_environments: None,
            },
            tags: tags.cloned().unwrap_or_default(),
        }
    }

    /// What a read of this connection reports.
    ///
    /// `hide_password` is `GetConnectionRequest$HidePassword`: "Allows you to retrieve the
    /// connection metadata without returning the password", and
    /// `compute_environment` is `GetConnectionRequest$ApplyOverrideForComputeEnvironment`:
    /// "For connections that may be used in multiple services, specifies returning
    /// properties for the specified compute environment" — that environment's own property
    /// map overlays the connection's, key by key, so a property only that environment sets
    /// is reported and one it does not set keeps the connection's value.
    ///
    /// The overlay is applied FIRST and the password properties are withheld from EVERY
    /// property map the answer carries — the connection's own and each
    /// per-compute-environment one — because a per-environment map may itself carry a
    /// `PASSWORD`, and "without returning the password" is about the password, not about
    /// one member.
    pub fn to_connection(
        &self,
        hide_password: bool,
        compute_environment: Option<&str>,
    ) -> Connection {
        let mut connection = self.definition.clone();
        let overrides = match compute_environment {
            Some(COMPUTE_ENVIRONMENT_SPARK) => connection.spark_properties.clone(),
            Some(COMPUTE_ENVIRONMENT_ATHENA) => connection.athena_properties.clone(),
            Some(COMPUTE_ENVIRONMENT_PYTHON) => connection.python_properties.clone(),
            // The model's `ComputeEnvironment` enum carries exactly those three, and the
            // request walk refuses any other value before a handler runs.
            _ => None,
        };
        if let Some(overrides) = overrides {
            connection
                .connection_properties
                .get_or_insert_with(Default::default)
                .extend(overrides);
        }
        if hide_password {
            for properties in [
                connection.connection_properties.as_mut(),
                connection.spark_properties.as_mut(),
                connection.athena_properties.as_mut(),
                connection.python_properties.as_mut(),
            ]
            .into_iter()
            .flatten()
            {
                properties.remove(PASSWORD_PROPERTY);
                properties.remove(ENCRYPTED_PASSWORD_PROPERTY);
            }
        }
        connection
    }
}

/// The `VersionId` that follows `current`: the stored counter plus one.
///
/// Only the server ever writes a `VersionId`, and it writes a decimal counter, so a
/// value that does not parse is treated as the version before the first one
/// ([`FIRST_TABLE_VERSION_ID`] minus one, i.e. the id a fresh table advances FROM)
/// rather than carried forward unchanged — a version must always move when the table
/// does.
pub fn next_version_id(current: &str) -> String {
    current
        .parse::<u64>()
        // Explicitly the version before the first one, not a general-purpose default.
        .unwrap_or(0)
        .saturating_add(1)
        .to_string()
}

/// The text the service puts in an error body's `Message` member.
///
/// Error wording here is PER OPERATION AND PER RESOURCE, never one template this
/// emulator invents and reuses: the service answers a fixed constant where its message
/// names no resource, and otherwise a template carrying only the request member the
/// refusal is about — never the name of a container the caller also sent, and never
/// emulator-side context such as the resolved `CatalogId`.
///
/// Four of these are pinned by the recorded wire corpus: `GetCrawler`'s crawler miss
/// ("Crawler entry with name `<Name>` does not exist"), `DeleteTable`'s table miss
/// ("Table `<Name>` not found."), `GetTable`'s table miss (the constant
/// "Entity Not Found", which names neither table nor database) and `CreateDatabase`'s
/// duplicate (the constant "Database already exists.", which withholds the requested
/// name). The rest are the same per-resource wording for the same refusal on the
/// sibling operations. Every handler refuses through these constructors, so one
/// operation's wording is never silently borrowed for another's refusal.
///
/// This is the TOP-LEVEL error body only. A per-item failure a batch reports inside its
/// output (`BatchDeleteTable`'s `TableError$ErrorDetail`) has its own recorded wording
/// and is built by [`super::catalog_error_detail`]. The job and connection slices are
/// not here either: no recording or doc states their wording, so their messages are not
/// requirements and are left to the issues that own them rather than guessed at.
pub mod messages {
    /// A crawler lookup (`GetCrawler`) for a name no crawler in this account has.
    /// Built from the request's `Name`, which is stored and matched exactly as sent.
    pub fn crawler_not_found(name: &str) -> String {
        format!("Crawler entry with name {name} does not exist")
    }

    /// `CreateCrawler` for a name already taken. NOT recorded: inferred from the
    /// recorded `CreateDatabase` duplicate, whose already-exists wording states the
    /// resource kind and withholds the name. What is pinned here is the behaviour — 400,
    /// `AlreadyExistsException`, and no disclosure of the requested name.
    pub const CRAWLER_ALREADY_EXISTS: &str = "Crawler already exists.";

    /// Any Data Catalog operation whose `DatabaseName`/`Name` names no database
    /// (`GetDatabase`, `DeleteDatabase`, `UpdateDatabase`, and the parent-database miss
    /// of every table operation: `GetTable`, `GetTables`, `CreateTable`, `UpdateTable`,
    /// `DeleteTable`, `BatchDeleteTable`). Built from the requested name alone — the
    /// catalog the emulator resolved is not part of the refusal.
    pub fn database_not_found(name: &str) -> String {
        format!("Database {name} not found.")
    }

    /// `CreateDatabase` (and `UpdateDatabase`, whose rename collides the same way) for
    /// a `DatabaseInput$Name` already taken. Recorded constant: the requested name is
    /// deliberately withheld.
    pub const DATABASE_ALREADY_EXISTS: &str = "Database already exists.";

    /// `GetTable` for a table the database does not hold. Recorded constant: this
    /// read's refusal names neither the table nor the database.
    pub const TABLE_ENTITY_NOT_FOUND: &str = "Entity Not Found";

    /// `DeleteTable` (and `UpdateTable`, the other write that addresses one table) for a
    /// table the database does not hold. Recorded template, built from the request's
    /// `Name` and naming no database — a different refusal from `GetTable`'s, which is
    /// why the two are not one shared string.
    pub fn table_not_found(name: &str) -> String {
        format!("Table {name} not found.")
    }

    /// `CreateTable` for a table name the database already holds. NOT recorded either:
    /// inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`].
    pub const TABLE_ALREADY_EXISTS: &str = "Table already exists.";
}

/// Fold a database or table name the way the catalog stores it: ASCII-lowercased.
/// Applied on both write and lookup, so a name is reachable by any casing the client
/// sends. The fold is ASCII because Hive compatibility is an ASCII rule, and because
/// ASCII folding is length-stable: full Unicode lowercasing can change a name's length
/// and so its modelled `length` constraint.
pub fn fold_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The IAM user name of the identity this emulator's long-term credentials speak for.
///
/// The emulator publishes exactly ONE long-term identity, and STS is what defines it:
/// `GetCallerIdentity` answers `arn:aws:iam::<account>:user/emulator` for every request
/// whose access key id names no session (`AMBIENT_USER_NAME` in
/// `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs`).
const AMBIENT_USER_NAME: &str = "emulator";

/// The principal that made this request, as the ARN the catalog records in
/// `Table$CreatedBy` ("The person or entity who created the table").
///
/// Server-derived from the emulator's own identity contract, never from the request: it
/// is the ARN of the identity a request signed with this emulator's credentials speaks
/// for, which is the one [`AMBIENT_USER_NAME`] names — so Glue and STS agree on who the
/// caller is. An access key id is a credential id, not a user name, so it is never
/// rendered into an IAM ARN: that is a principal no AWS rule produces. The value is
/// therefore the same on every call and across restarts, exactly as the IAM identity
/// behind one set of credentials is.
pub fn request_principal() -> String {
    ::engine::emulator_arn("iam", "", &format!("user/{AMBIENT_USER_NAME}"))
}

/// The `CatalogId` a request addresses: the one it names, else this emulator's
/// account id, which is what the model documents as the default
/// ("If none is provided, the Amazon Web Services account ID is used by default").
pub fn resolve_catalog_id(catalog_id: Option<&String>) -> String {
    match catalog_id.map(String::as_str).filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None => ::engine::emulator_account_id().to_string(),
    }
}

/// Whether `catalog_id` names a Data Catalog this emulator holds.
///
/// It holds exactly one: the one for its own account, which is the catalog the model
/// documents as the default. A catalog under any other id belongs to another account
/// (reachable on the real service only through a Data Catalog resource policy) or
/// would have to be created with `CreateCatalog`, which is not implemented — so no
/// other id can name a catalog here, and a request that names one is answered as a
/// miss rather than quietly given a new, empty catalog.
pub fn is_known_catalog(catalog_id: &str) -> bool {
    catalog_id == ::engine::emulator_account_id()
}

/// Whether `account_id` — field 5 of an ARN — is the account this emulator serves, and
/// so the only account whose Glue resources this store can hold. An ARN naming any other
/// account names a resource of another account's estate, which this process does not
/// hold.
pub fn is_served_account(account_id: &str) -> bool {
    account_id == ::engine::emulator_account_id()
}

/// The Region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] for a request that carries no scope (an unsigned call).
///
/// A Glue record carries no Region of its own — every operation that addresses a resource
/// names it by name — so the Region a resource's ARN is minted for, and compared against,
/// is the one its request is served in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a request with no SigV4 credential scope is scoped to. Matches the Region
/// the emulator's own generated client config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace every Glue resource lives in, as the modelled
/// `GlueResourceArn` pattern spells it
/// (`^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:`).
pub const ARN_NAMESPACE: &str = "glue";

/// One kind of Glue resource that carries its OWN tag set, and so can be named by a
/// tagging call's `ResourceArn`.
///
/// The kinds are the ones whose create carries a `Tags` map and whose record in this
/// store therefore holds a tag set: a Data Catalog database (`CreateDatabase$Tags`), a
/// connection (`CreateConnection$Tags`), a crawler (`CreateCrawler$Tags`) and a job
/// definition (`CreateJob$Tags`). A table is NOT one of them — `CreateTable` models no
/// `Tags` member — and a Glue resource this emulator does not store yet has no tag set
/// to name either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// `connection/<name>`.
    Connection,
    /// `crawler/<name>`.
    Crawler,
    /// `database/<name>`.
    Database,
    /// `job/<name>`.
    Job,
}

/// The ARN resource-type segment each taggable kind carries: the catalog
/// [`parse_tagged_resource`] resolves an ARN against and [`TaggedResourceKind::arn`]
/// mints one from.
///
/// Source: the resource portion of each resource's documented Glue ARN
/// (`database/<name>`, `connection/<name>`, `crawler/<name>`, `job/<name>`). The model
/// states only the prefix every Glue ARN shares (`GlueResourceArn`'s pattern, see
/// [`ARN_NAMESPACE`]), so the segments are listed here once and every mint and every
/// lookup goes through this one table.
pub const TAGGED_RESOURCE_TYPES: [(&str, TaggedResourceKind); 4] = [
    ("connection", TaggedResourceKind::Connection),
    ("crawler", TaggedResourceKind::Crawler),
    ("database", TaggedResourceKind::Database),
    ("job", TaggedResourceKind::Job),
];

impl TaggedResourceKind {
    /// This kind's ARN resource-type segment, from [`TAGGED_RESOURCE_TYPES`].
    pub fn arn_segment(self) -> &'static str {
        TAGGED_RESOURCE_TYPES
            .iter()
            .find(|(_, kind)| *kind == self)
            .map(|(segment, _)| *segment)
            // Every variant has a row above, so this is unreachable; answering the
            // service namespace rather than panicking keeps a request path alive.
            .unwrap_or(ARN_NAMESPACE)
    }

    /// The ARN of the resource `name` of this kind, as served in `region`.
    ///
    /// The account segment is this emulator's own ([`::engine::emulator_arn`]), which is
    /// also the only `CatalogId` a catalog resource can live under here (see
    /// [`is_known_catalog`]), so a minted ARN and a stored record always agree on whose
    /// resource it is.
    pub fn arn(self, region: &str, name: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("{}/{name}", self.arn_segment()),
        )
    }
}

/// A Glue resource a tagging call named by ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    /// Which kind of resource the ARN's resource-type segment names.
    pub kind: TaggedResourceKind,
    /// Field 4 of the ARN: the Region whose estate the resource belongs to.
    pub region: String,
    /// Field 5 of the ARN: the account that owns the resource, which for a Data Catalog
    /// resource is also the `CatalogId` it lives under ("If none is provided, the Amazon
    /// Web Services account ID is used by default").
    pub account_id: String,
    /// The resource's name, folded exactly as the store keys it: lowercased for a
    /// database ([`fold_name`]), as written for the kinds the model documents no fold
    /// for.
    pub name: String,
}

/// The Glue resource `resource_arn` names, or `None` when it is not the ARN of a taggable
/// Glue resource at all — a value in another service's namespace, a malformed ARN, or a
/// Glue resource kind this store holds no tag set for.
///
/// Only the KIND and the identity fields are read here; whether this store holds the
/// resource is [`ComAmazonawsGlueAwsglueState::resource_tags`]'s answer, so an ARN of a
/// known kind that names nothing earns that kind's not-found refusal rather than being
/// called malformed.
pub fn parse_tagged_resource(resource_arn: &str) -> Option<TaggedResource> {
    // `arn:<partition>:glue:<region>:<account>:<resource>`. The resource is the LAST
    // field, so it keeps any colon of its own.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    if fields.len() != 6 || fields[0] != "arn" || fields[2] != ARN_NAMESPACE {
        return None;
    }
    let (segment, name) = fields[5].split_once('/')?;
    if name.is_empty() {
        return None;
    }
    let (_, kind) = TAGGED_RESOURCE_TYPES
        .iter()
        .find(|(candidate, _)| *candidate == segment)?;
    Some(TaggedResource {
        kind: *kind,
        region: fields[3].to_string(),
        account_id: fields[4].to_string(),
        name: match kind {
            // The two Hive-compatible catalog resources are the ones the catalog folds;
            // a crawler, job or connection name is keyed as written.
            TaggedResourceKind::Database => fold_name(name),
            TaggedResourceKind::Connection
            | TaggedResourceKind::Crawler
            | TaggedResourceKind::Job => name.to_string(),
        },
    })
}

/// The message a refusal carries for a `ResourceArn` of a known kind that names no
/// resource this store holds.
///
/// Each sentence is the one this crate already answers wherever that kind fails to
/// resolve ([`messages`]), so "this ARN names no database" reads the same from a tagging
/// operation as from `GetDatabase`. The two kinds whose refusal wording no recording or
/// doc states take a sentence DERIVED from the request instead: it names the ARN that
/// resolved to nothing, and what is pinned is the behaviour — 400 with
/// `EntityNotFoundException`.
pub fn tagged_resource_not_found_message(resource: &TaggedResource, resource_arn: &str) -> String {
    match resource.kind {
        TaggedResourceKind::Database => messages::database_not_found(&resource.name),
        TaggedResourceKind::Crawler => messages::crawler_not_found(&resource.name),
        TaggedResourceKind::Connection | TaggedResourceKind::Job => {
            format!("Entity not found for resource {resource_arn}")
        }
    }
}

/// The message a refusal carries for a `ResourceArn` that is not the ARN of a taggable
/// Glue resource ([`parse_tagged_resource`] answers `None`).
///
/// Derived from the request, like this crate's other reference refusals: it names the
/// member, the resource types the member accepts ([`TAGGED_RESOURCE_TYPES`]) and the
/// value that is none of them.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    let kinds: Vec<&str> = TAGGED_RESOURCE_TYPES
        .iter()
        .map(|(segment, _)| *segment)
        .collect();
    format!(
        "ResourceArn must be the ARN of a taggable Glue resource naming one of {}; got \
         {resource_arn:?}",
        kinds.join(", ")
    )
}

/// A wall-clock instant as the modelled timestamp type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGlueAwsglueState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGlueAwsglueState>)
}
