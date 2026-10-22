// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-glue-awsglue service.
//!
//! The Data Catalog is the service's primary resource tree: a catalog (keyed by
//! `CatalogId`, which defaults to the request's account id) holds databases, and a
//! database holds tables. Every handler reads and writes THIS store, so a
//! `CreateTable` in one request is visible to a `GetTable` in the next.
//!
//! Database and table names are folded to lowercase on the way in — the model says
//! so for both `DatabaseInput$Name` ("For Hive compatibility, this is folded to
//! lowercase when it is stored") and `TableInput$Name` — and every lookup folds the
//! requested name the same way, so the fold cannot make a resource unreachable.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    Column, Database, DatabaseIdentifier, DatabaseInput, FederatedDatabase, FederatedTable,
    PrincipalPermissions, StorageDescriptor, Table, TableIdentifier, TableInput,
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

/// Per-service state for com-amazonaws-glue-awsglue. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGlueAwsglueState {
    /// Every Data Catalog this emulator holds, keyed by `CatalogId`.
    pub catalogs: HashMap<String, Catalog>,
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

    /// How many tables the whole store holds. Bounded by the ceilings above, so this
    /// walk is over at most [`MAX_DATABASES_PER_CATALOG`] entries per catalog.
    pub fn stored_tables(&self) -> usize {
        self.catalogs
            .values()
            .map(|catalog| {
                catalog
                    .databases
                    .values()
                    .map(|database| database.tables.len())
                    .sum::<usize>()
            })
            .sum()
    }

    /// Whether the store as a whole can hold another table, i.e. is below
    /// [`MAX_STORED_TABLES`].
    pub fn can_store_table(&self) -> bool {
        self.stored_tables() < MAX_STORED_TABLES
    }
}

/// One Data Catalog: the databases stored under a single `CatalogId`.
#[derive(Debug, Default)]
pub struct Catalog {
    /// Databases keyed by their folded (lowercased) name.
    pub databases: HashMap<String, DatabaseEntry>,
}

impl Catalog {
    /// Whether this catalog can hold another database, i.e. is below
    /// [`MAX_DATABASES_PER_CATALOG`].
    pub fn can_store_database(&self) -> bool {
        self.databases.len() < MAX_DATABASES_PER_CATALOG
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
    pub create_table_default_permissions: Option<Vec<PrincipalPermissions>>,
    pub target_database: Option<DatabaseIdentifier>,
    pub federated_database: Option<FederatedDatabase>,
    pub catalog_id: String,
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
            create_table_default_permissions: input.create_table_default_permissions.clone(),
            target_database: input.target_database.clone(),
            federated_database: input.federated_database.clone(),
            catalog_id: catalog_id.to_string(),
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
            create_table_default_permissions: self.create_table_default_permissions.clone(),
            target_database: self.target_database.clone(),
            catalog_id: Some(self.catalog_id.clone()),
            federated_database: self.federated_database.clone(),
        }
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
    /// Server-set version counter, `"1"` for a freshly created table.
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

    /// Rebuild the modelled `Table` output shape from what is stored.
    pub fn to_table(&self) -> Table {
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
            storage_descriptor: self.storage_descriptor.clone(),
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
            view_definition: None,
            is_multi_dialect_view: None,
            is_materialized_view: None,
            iceberg_table_metadata: None,
            status: None,
            rls_properties: None,
        }
    }
}

/// The `VersionId` a table carries straight after `CreateTable`.
pub const FIRST_TABLE_VERSION_ID: &str = "1";

/// Fold a database or table name the way the catalog stores it: ASCII-lowercased.
/// Applied on both write and lookup, so a name is reachable by any casing the client
/// sends. The fold is ASCII because Hive compatibility is an ASCII rule, and because
/// ASCII folding is length-stable: full Unicode lowercasing can change a name's length
/// and so its modelled `length` constraint.
pub fn fold_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The principal fragment of the caller's identity, from the SigV4 credential scope.
///
/// The access key id is request text, so a value that is not shaped like one (empty,
/// over-long, or carrying anything but ASCII alphanumerics) is replaced by a fixed
/// `emulator` principal rather than being spliced into an ARN. `extract_akid` itself
/// performs no verification — the emulator verifies signatures at its boundary, and
/// with authentication off there is nothing to verify.
pub fn caller_principal(headers: &::http::HeaderMap) -> String {
    let akid = ::engine::extract_akid(headers);
    let shaped_like_an_access_key_id = !akid.is_empty()
        && akid.len() <= 128
        && akid.chars().all(|character| character.is_ascii_alphanumeric());
    if shaped_like_an_access_key_id {
        akid
    } else {
        "emulator".to_string()
    }
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
