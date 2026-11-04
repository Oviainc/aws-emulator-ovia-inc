// SPDX-License-Identifier: CC0-1.0

//! The PARTITION INDEXES of a Data Catalog table: the records a create registers on a table
//! and `GetPartitionIndexes` reports.
//!
//! The single home of this resource's records, reached by every handler that reads or writes
//! one, so no two handlers keep their own copy of a table's indexes. It is reached exactly as
//! [`crate::state::service_state`] reaches the catalog itself — through the SHARED engine, by
//! type ([`partition_index_store`]) — so the records one request writes are the records the
//! next request reads.
//!
//! The file sits with the rest of this service's state, beside
//! [`crate::state::catalog_encryption`]; it is DECLARED from `src/service.rs` (hence its
//! module path), which is the file this service owns and which is never regenerated. See the
//! declaration there for why.
//!
//! An index is a resource OF one table: "the partition indexes associated with a table"
//! (`GetPartitionIndexes`), addressed by catalog, database and table and then by its own
//! `IndexName`. A record set is therefore bound to the stored table it belongs to, including
//! the instant that table was created ([`TableIndexKey`]), and it goes when that table goes:
//! every operation that removes a table removes its records with it
//! ([`PartitionIndexStore::forget_tables`]), exactly as a table's partitions are dropped with
//! the table that holds them. So this store holds records for no more tables than the catalog
//! holds, which is what bounds it, and the table a later create puts under a recycled name is
//! a different table, with no indexes until something registers one on it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{Column, KeySchemaElement, PartitionIndex, PartitionIndexDescriptor};

/// The `PartitionIndexStatus` an index is in while the service is still building it:
/// "CREATING: The index is being created" (`PartitionIndexDescriptor$IndexStatus`). One of
/// the modelled values carried by [`crate::ops::types::PARTITION_INDEX_STATUS_VALUES`].
pub const INDEX_STATUS_CREATING: &str = "CREATING";

/// The `PartitionIndexStatus` the clock owes an index it has finished building: "ACTIVE: The
/// index creation succeeds" (`PartitionIndexDescriptor$IndexStatus`).
pub const INDEX_STATUS_ACTIVE: &str = "ACTIVE";

/// How long an index stays [`INDEX_STATUS_CREATING`] before the clock owes it
/// [`INDEX_STATUS_ACTIVE`].
///
/// CHOSEN, not documented: Glue publishes no duration for building a partition index, so this
/// is the emulator's own span — deliberately not zero, so a caller that reads
/// `GetPartitionIndexes` straight after registering an index still observes the index being
/// built. The due instant is stored on the record and compared with the instant of the call
/// that reads it; nothing sleeps.
pub const PARTITION_INDEX_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// The most partition indexes one table holds.
///
/// Source: the Glue documentation on partition indexes, which states that a maximum of three
/// partition indexes can be created on a table. A create that would take a table past it is
/// refused with the modelled `ResourceNumberLimitExceededException` rather than stored, so
/// this store holds no table the real service would not have.
pub const MAX_INDEXES_PER_TABLE: usize = 3;

/// Why a table cannot carry the partition indexes a request submitted, as the two different
/// failures the catalog answers them with.
///
/// The variants are the DISTINCTION the caller sees: indexes the table's own definition
/// cannot support are a bad request, while one index too many is a limit on the resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRefusal {
    /// The submitted indexes do not describe something this table can index — answered with
    /// `InvalidInputException`.
    Invalid(String),
    /// The table would hold more than [`MAX_INDEXES_PER_TABLE`] indexes — answered with
    /// `ResourceNumberLimitExceededException`.
    TooMany(String),
}

impl IndexRefusal {
    /// The refusal's message, as the error it is answered with carries it.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::TooMany(message) => message,
        }
    }
}

/// The refusal wordings this module answers with, each built from the REQUEST and the table's
/// own definition rather than from any one caller's values.
pub mod messages {
    use super::MAX_INDEXES_PER_TABLE;

    /// A key an index names that the table does not partition by: an index indexes "the name
    /// of a partition key" (`KeySchemaElement$Name`), so a column that is not one of the
    /// table's partition keys names nothing to index.
    pub fn key_is_not_a_partition_key(table_name: &str, index_name: &str, key: &str) -> String {
        format!(
            "Index key {key} of partition index {index_name} is not a partition key of table \
             {table_name}"
        )
    }

    /// One `IndexName` submitted twice in one request: an index is addressed by its name
    /// within its table, so two of one name name one index.
    pub fn index_named_twice(table_name: &str, index_name: &str) -> String {
        format!("Partition index {index_name} is named more than once for table {table_name}")
    }

    /// The same key named twice inside one index's `Keys`.
    pub fn key_named_twice(table_name: &str, index_name: &str, key: &str) -> String {
        format!(
            "Index key {key} is named more than once in partition index {index_name} of table \
             {table_name}"
        )
    }

    /// More indexes than a table holds.
    pub fn too_many_indexes(table_name: &str, would_hold: usize) -> String {
        format!(
            "Table {table_name} cannot have more than {MAX_INDEXES_PER_TABLE} partition indexes, \
             and {would_hold} were submitted"
        )
    }
}

/// The table one set of index records belongs to.
///
/// The CREATE INSTANT is part of the identity on purpose: a table is addressed by catalog,
/// database and name, but those name a table only while it is stored, so binding the records
/// to the incarnation that registered them is what keeps a table created under a recycled
/// name from reading another table's indexes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableIndexKey {
    /// The catalog holding the table, as the table itself records it.
    pub catalog_id: String,
    /// The table's database, as the table itself records it.
    pub database_name: String,
    /// The table's own stored name.
    pub table_name: String,
    /// `Table$CreateTime`: the instant the catalog stored this table, which a later table of
    /// the same name does not share.
    pub table_created_at: SystemTime,
}

impl TableIndexKey {
    /// The key of the records belonging to one STORED table, taken from the table itself so
    /// that a writer and a reader of the same table cannot disagree about its key.
    pub fn of(table: &crate::state::TableEntry) -> Self {
        Self {
            catalog_id: table.catalog_id.clone(),
            database_name: table.database_name.clone(),
            table_name: table.name.clone(),
            table_created_at: table.create_time,
        }
    }

    /// Whether this key addresses the same catalog, database and table name as `other`,
    /// whichever incarnation of that table each one belongs to.
    fn names_same_table(&self, other: &Self) -> bool {
        self.catalog_id == other.catalog_id
            && self.database_name == other.database_name
            && self.table_name == other.table_name
    }
}

/// ONE stored partition index: the index a create registered, with the keys it indexes and
/// the instant its build is due to be finished.
#[derive(Debug, Clone)]
pub struct PartitionIndexEntry {
    /// `PartitionIndex$IndexName` as the create sent it, which is also this entry's key
    /// within its table's records.
    pub index_name: String,
    /// The indexed keys, each resolved against the table's partition keys at create time, in
    /// the order the create named them: "a list of one or more keys, as `KeySchemaElement`
    /// structures, for the partition index" (`PartitionIndexDescriptor$Keys`).
    pub keys: Vec<KeySchemaElement>,
    /// The instant from which the clock owes this index [`INDEX_STATUS_ACTIVE`]; before it,
    /// the index is still [`INDEX_STATUS_CREATING`].
    pub active_at: SystemTime,
}

impl PartitionIndexEntry {
    /// The status this index is in at `at`: the status its create put it in while the due
    /// instant is still ahead, the status the clock owes it from the instant it is reached.
    pub fn status(&self, at: SystemTime) -> &'static str {
        if at < self.active_at {
            INDEX_STATUS_CREATING
        } else {
            INDEX_STATUS_ACTIVE
        }
    }

    /// This index as `GetPartitionIndexes` reports it.
    ///
    /// `BackfillErrors` are "errors that can occur when registering partition indexes for an
    /// existing table" (`PartitionIndexDescriptor$BackfillErrors`) — the partitions already
    /// stored that the index cannot cover. An index registered with its table is registered
    /// over no partitions at all, so there is no backfill to report an error from.
    pub fn to_descriptor(&self, at: SystemTime) -> PartitionIndexDescriptor {
        PartitionIndexDescriptor {
            index_name: self.index_name.clone(),
            keys: self.keys.clone(),
            index_status: self.status(at).to_string(),
            backfill_errors: None,
        }
    }
}

/// Every table's partition-index records, keyed by the table they belong to.
///
/// BOUNDED by the catalog itself: a record set is written by the create of one table and
/// removed by whatever removes that table ([`Self::forget_tables`], called from every
/// operation that drops a table, a database or a catalog), so the store holds records for no
/// more tables than the catalog holds tables — which the catalog's own store-wide ceiling
/// bounds ([`crate::state::MAX_STORED_TABLES`]).
///
/// Ordered by index name within a table, so a reader walks one table's indexes in one
/// deterministic order and a continuation token resumes against that same order.
#[derive(Debug, Default)]
pub struct PartitionIndexStore {
    tables: BTreeMap<TableIndexKey, BTreeMap<String, PartitionIndexEntry>>,
}

impl PartitionIndexStore {
    /// The indexes of one table as `GetPartitionIndexes` reports them at `at`, in `IndexName`
    /// order. A table nothing has registered an index on has none.
    pub fn descriptors(
        &self,
        table: &TableIndexKey,
        at: SystemTime,
    ) -> Vec<PartitionIndexDescriptor> {
        self.tables
            .get(table)
            .into_iter()
            .flat_map(BTreeMap::values)
            .map(|index| index.to_descriptor(at))
            .collect()
    }

    /// Record the indexes a create registered on ONE table.
    ///
    /// The records of any EARLIER table of the same name go with it. Those records are
    /// already removed by the delete that dropped that table ([`Self::forget_tables`]); this
    /// is the guard that keeps the invariant — at most one record set per catalog, database
    /// and table name — true of this store itself, whatever reached it before.
    pub fn record(&mut self, table: TableIndexKey, indexes: Vec<PartitionIndexEntry>) {
        self.tables.retain(|held, _| !held.names_same_table(&table));
        if indexes.is_empty() {
            return;
        }
        self.tables.insert(
            table,
            indexes
                .into_iter()
                .map(|index| (index.index_name.clone(), index))
                .collect(),
        );
    }

    /// Forget the records of every table in `tables`: the indexes of a table that is no
    /// longer stored.
    ///
    /// An index is a resource OF its table, so a delete that drops a table — on its own, in a
    /// batch, or as the cascade of dropping its database or its catalog — drops the table's
    /// indexes too, and this store keeps nothing for a table the catalog no longer holds.
    pub fn forget_tables<'a>(
        &mut self,
        tables: impl IntoIterator<Item = &'a crate::state::TableEntry>,
    ) {
        for table in tables {
            self.tables.remove(&TableIndexKey::of(table));
        }
    }

    /// Move the records of a renamed database's tables to the name those tables now record.
    ///
    /// A renamed database takes its tables with it, and each table records the database it
    /// belongs to ([`TableIndexKey`] is read off the stored table), so the records have to
    /// move with them: an index belongs to its table, and renaming the database around it
    /// changes neither the table nor the indexes registered on it.
    pub fn rename_database(&mut self, catalog_id: &str, from: &str, to: &str) {
        if from == to {
            return;
        }
        let moved: Vec<TableIndexKey> = self
            .tables
            .keys()
            .filter(|held| held.catalog_id == catalog_id && held.database_name == from)
            .cloned()
            .collect();
        for key in moved {
            if let Some(records) = self.tables.remove(&key) {
                self.tables.insert(
                    TableIndexKey {
                        database_name: to.to_string(),
                        ..key
                    },
                    records,
                );
            }
        }
    }
}

/// This service's partition-index records on the SHARED engine, or `None` when the engine
/// mounted under this service keeps no state at all.
///
/// Resolved by type, exactly as [`crate::state::service_state`] resolves the Data Catalog
/// itself, so every handler that touches an index reaches the one set of records.
pub fn partition_index_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<PartitionIndexStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<PartitionIndexStore>)
}

/// Resolve the partition indexes a create submitted against the partition keys of the table
/// it is registering them on, as the records to store — or the reason the table cannot carry
/// them.
///
/// RULE: an index indexes the table's PARTITION keys ("the name of a partition key",
/// `KeySchemaElement$Name`), so every key an index names has to be one of the table's own
/// partition keys, each index of a table is addressed by a name of its own, and a table takes
/// at most [`MAX_INDEXES_PER_TABLE`] of them. The TYPE the stored record carries for a key is
/// the type that partition key declares, so a read reports the table's own column type and
/// never one this code picked.
///
/// `registered_at` is the instant the create is served; the records it returns are due
/// [`INDEX_STATUS_ACTIVE`] one [`PARTITION_INDEX_SETTLES_AFTER`] after it.
///
/// BOUNDED by the table's own definition, whatever lists the request carries. The partition
/// keys are indexed by name ONCE, so resolving a key is a lookup and not a walk; and because
/// every key has to be a partition key this index has not named yet, the walk over one
/// index's keys stops at the first key that is not — at most one step past the number of
/// partition keys the table declares. With at most [`MAX_INDEXES_PER_TABLE`] indexes to
/// resolve, the work is linear in that definition and never the product of the two submitted
/// lists.
pub fn resolve_indexes(
    table_name: &str,
    indexes: &[PartitionIndex],
    partition_keys: &[Column],
    registered_at: SystemTime,
) -> Result<Vec<PartitionIndexEntry>, IndexRefusal> {
    if indexes.len() > MAX_INDEXES_PER_TABLE {
        return Err(IndexRefusal::TooMany(messages::too_many_indexes(
            table_name,
            indexes.len(),
        )));
    }

    // The table's partition keys by name: built once, from the definition about to be stored,
    // so no key resolution walks them. The FIRST column of a name wins, as a walk looking for
    // that name would have found.
    let mut partition_keys_by_name: HashMap<&str, &Column> = HashMap::new();
    for column in partition_keys {
        partition_keys_by_name
            .entry(column.name.as_str())
            .or_insert(column);
    }

    let mut resolved: Vec<PartitionIndexEntry> = Vec::with_capacity(indexes.len());
    for index in indexes {
        if resolved
            .iter()
            .any(|held| held.index_name == index.index_name)
        {
            return Err(IndexRefusal::Invalid(messages::index_named_twice(
                table_name,
                &index.index_name,
            )));
        }
        // Grown by what this index actually indexes, which is bounded by the table's own
        // partition keys — never reserved from the length of the submitted list.
        let mut keys: Vec<KeySchemaElement> = Vec::new();
        let mut named: HashSet<&str> = HashSet::new();
        for key in &index.keys {
            if !named.insert(key.as_str()) {
                return Err(IndexRefusal::Invalid(messages::key_named_twice(
                    table_name,
                    &index.index_name,
                    key,
                )));
            }
            let Some(partition_key) = partition_keys_by_name.get(key.as_str()) else {
                return Err(IndexRefusal::Invalid(
                    messages::key_is_not_a_partition_key(table_name, &index.index_name, key),
                ));
            };
            keys.push(KeySchemaElement {
                name: partition_key.name.clone(),
                // `Column$Type` is optional and `KeySchemaElement$Type` is a
                // `ColumnTypeString`, whose modelled length starts at zero: a partition key
                // that declares no type is indexed under none, rather than under a type
                // nothing in the table says it has.
                r#type: partition_key.r#type.clone().unwrap_or_default(),
            });
        }
        resolved.push(PartitionIndexEntry {
            index_name: index.index_name.clone(),
            keys,
            // Checked, as every other due instant in this crate is: an instant the clock
            // cannot represent leaves the index due its ACTIVE status straight away rather
            // than taking the process down.
            active_at: registered_at
                .checked_add(PARTITION_INDEX_SETTLES_AFTER)
                .unwrap_or(registered_at),
        });
    }
    Ok(resolved)
}
