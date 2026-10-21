// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-timestreamwrite-timestream-20181101 service.
//!
//! The database/table/ingest control plane lives here: every response an operation
//! returns is projected from these structs, so a create is observable by the next
//! describe/list and a delete is observable by the next reader.
//!
//! `TableCount` on the wire `Database` is DERIVED from `DatabaseEntry::tables.len()`
//! and never stored separately, so it cannot drift from the tables that actually
//! exist.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    Database, Dimension, MagneticStoreWriteProperties, MeasureValue, RetentionProperties, Schema,
    Table, Tag,
};

/// The ARN namespace (and SigV4 signing name) Timestream Write uses, per the
/// service's `aws.api#service` trait (`arnNamespace: timestream`).
pub const ARN_NAMESPACE: &str = "timestream";

/// The region used when a request carries no SigV4 credential scope to read one
/// from. Matches the region the emulator's own generated AWS config declares.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `RetentionProperties.MemoryStoreRetentionPeriodInHours` applied when
/// `CreateTable` omits `RetentionProperties`.
///
/// Timestream's documented default for a new table is 6 hours of memory-store
/// retention (Developer Guide, "Storage"); the model bounds the member to 1..8766.
pub const DEFAULT_MEMORY_STORE_RETENTION_HOURS: i64 = 6;

/// `RetentionProperties.MagneticStoreRetentionPeriodInDays` applied when
/// `CreateTable` omits `RetentionProperties`.
///
/// Timestream's documented default for a new table is 73000 days of magnetic-store
/// retention (Developer Guide, "Storage"), which is also the model's upper bound.
pub const DEFAULT_MAGNETIC_STORE_RETENTION_DAYS: i64 = 73_000;

/// `Record.MeasureValueType` applied when a record (after `CommonAttributes` are
/// merged in) declares none. The model's own doc for the member says "Default type
/// is `DOUBLE`".
pub const DEFAULT_MEASURE_VALUE_TYPE: &str = "DOUBLE";

/// `Record.TimeUnit` applied when a record (after `CommonAttributes` are merged in)
/// declares none. The model's own doc for the member says "Default is
/// `MILLISECONDS`".
pub const DEFAULT_TIME_UNIT: &str = "MILLISECONDS";

/// `Record.Version` applied when a record declares none. The model's own doc for the
/// member says "Default value is `1`".
pub const DEFAULT_RECORD_VERSION: i64 = 1;

/// The smallest `Record.Version` Timestream accepts. The model's own doc for
/// `Record$Version` says it "must be `1` or greater, or you will receive a
/// `ValidationException` error".
pub const MIN_RECORD_VERSION: i64 = 1;

/// How many of the most recently ingested records each table retains.
///
/// `WriteRecords` is the only caller-driven growth site in this crate, and these
/// records are state no implemented operation reads back (Timestream's query surface
/// is a different service), so the buffer keeps a bounded recent window rather than
/// growing without limit. [`TableEntry::records_ingested`] counts every record ever
/// accepted and is never truncated, so the ingest counters stay exact no matter how
/// much has been written.
///
/// This is an emulator resource bound, not an AWS rule: the stored
/// `RetentionProperties` are recorded but not yet applied as an expiry (memory-store
/// retention runs to a year, so it is not a bound), which belongs with the query
/// surface that would read these records.
pub const RECORD_BUFFER_LIMIT: usize = 10_000;

/// The largest page `ListTables`/`ListDatabases` return, and the page size used when
/// the caller names no `MaxResults`: the upper bound the model's `PaginationLimit`
/// declares for both operations (range 1..20).
pub const MAX_PAGE_SIZE: i32 = 20;

/// The lifecycle state of a table, as the model's `TableStatus` enum names it.
///
/// Timestream answers `CreateTable` with a table that is already `ACTIVE`, so a
/// freshly created table needs no clock-owed transition; the other two variants
/// exist because the model declares them and a later operation (delete, restore)
/// reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableStatus {
    Active,
    Deleting,
    Restoring,
}

impl TableStatus {
    /// The wire value, one of the model's `TableStatus` enum values
    /// (`crate::ops::types::TABLE_STATUS_VALUES`).
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Restoring => "RESTORING",
        }
    }
}

/// One time-series data point accepted by `WriteRecords`, after `CommonAttributes`
/// have been merged into it and the modelled defaults applied.
#[derive(Debug, Clone)]
pub struct StoredRecord {
    pub dimensions: Vec<Dimension>,
    pub measure_name: Option<String>,
    pub measure_value: Option<String>,
    pub measure_value_type: String,
    pub measure_values: Vec<MeasureValue>,
    pub time: Option<String>,
    pub time_unit: String,
    pub version: i64,
    pub ingested_at: SystemTime,
}

/// A stored Timestream table.
#[derive(Debug, Clone)]
pub struct TableEntry {
    pub arn: String,
    pub table_name: String,
    pub database_name: String,
    pub status: TableStatus,
    pub retention_properties: RetentionProperties,
    pub magnetic_store_write_properties: Option<MagneticStoreWriteProperties>,
    pub schema: Option<Schema>,
    pub creation_time: SystemTime,
    pub last_updated_time: SystemTime,
    pub tags: Vec<Tag>,
    /// The most recent [`RECORD_BUFFER_LIMIT`] records accepted for this table.
    pub records: Vec<StoredRecord>,
    /// Every record ever accepted for this table, counted exactly — unaffected by the
    /// truncation [`RECORD_BUFFER_LIMIT`] applies to `records`.
    pub records_ingested: u64,
}

impl TableEntry {
    /// Accept `records` into this table, keeping only the most recent
    /// [`RECORD_BUFFER_LIMIT`] and raising [`Self::records_ingested`] by the number
    /// accepted.
    pub fn ingest(&mut self, records: Vec<StoredRecord>) {
        let accepted = u64::try_from(records.len()).unwrap_or(u64::MAX);
        self.records.extend(records);
        if self.records.len() > RECORD_BUFFER_LIMIT {
            let excess = self.records.len() - RECORD_BUFFER_LIMIT;
            self.records.drain(..excess);
        }
        self.records_ingested = self.records_ingested.saturating_add(accepted);
    }
    /// The wire `Table`, projected from what is stored.
    pub fn to_wire(&self) -> Table {
        Table {
            arn: Some(self.arn.clone()),
            table_name: Some(self.table_name.clone()),
            database_name: Some(self.database_name.clone()),
            table_status: Some(self.status.as_wire().to_string()),
            retention_properties: Some(self.retention_properties.clone()),
            creation_time: Some(as_date_time(self.creation_time)),
            last_updated_time: Some(as_date_time(self.last_updated_time)),
            magnetic_store_write_properties: self.magnetic_store_write_properties.clone(),
            schema: self.schema.clone(),
        }
    }
}

/// A stored Timestream database and the tables it contains.
#[derive(Debug, Clone)]
pub struct DatabaseEntry {
    pub arn: String,
    pub database_name: String,
    pub kms_key_id: String,
    pub creation_time: SystemTime,
    pub last_updated_time: SystemTime,
    pub tags: Vec<Tag>,
    /// Ordered by table name so `ListTables` paginates deterministically.
    pub tables: BTreeMap<String, TableEntry>,
}

impl DatabaseEntry {
    /// The wire `Database`, projected from what is stored. `TableCount` is derived
    /// from the tables actually held, never stored.
    pub fn to_wire(&self) -> Database {
        Database {
            arn: Some(self.arn.clone()),
            database_name: Some(self.database_name.clone()),
            table_count: i64::try_from(self.tables.len()).unwrap_or(i64::MAX),
            kms_key_id: Some(self.kms_key_id.clone()),
            creation_time: Some(as_date_time(self.creation_time)),
            last_updated_time: Some(as_date_time(self.last_updated_time)),
        }
    }
}

/// Per-service state for com-amazonaws-timestreamwrite-timestream-20181101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTimestreamwriteTimestream20181101State {
    /// Ordered by database name so `ListDatabases`/`ListTables` paginate
    /// deterministically.
    pub databases: BTreeMap<String, DatabaseEntry>,
}

impl ComAmazonawsTimestreamwriteTimestream20181101State {
    /// Every stored table, ordered by `(database name, table name)` — the order
    /// `ListTables` pages through when no `DatabaseName` narrows it.
    pub fn all_tables(&self) -> impl Iterator<Item = &TableEntry> {
        self.databases.values().flat_map(|db| db.tables.values())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTimestreamwriteTimestream20181101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTimestreamwriteTimestream20181101State>)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope. ARNs are regional, so they are built from the caller's own
/// region rather than from a baked-in literal.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:timestream:<region>:<account>:database/<DatabaseName>`.
pub fn database_arn(region: &str, database_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("database/{database_name}"))
}

/// `arn:aws:timestream:<region>:<account>:database/<DatabaseName>/table/<TableName>`.
pub fn table_arn(region: &str, database_name: &str, table_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("database/{database_name}/table/{table_name}"),
    )
}

/// The KMS key a database is encrypted with when the caller names none.
///
/// Timestream encrypts such a database with the Amazon Web Services managed key for
/// Timestream, which is one key per account and region — so this is that key's alias
/// ARN, not a per-call value.
pub fn default_kms_key_id(region: &str) -> String {
    ::engine::emulator_arn("kms", region, "alias/aws/timestream")
}

/// A `SystemTime` as the wire `Date` (an epoch timestamp).
pub fn as_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// Encode a pagination cursor — the key of the first item of the NEXT page — as an
/// opaque `NextToken`.
///
/// The token is opaque to the client (hex, not the key itself) but carries no state
/// of its own: paging resumes by seeking the stored, name-ordered map to this key, so
/// a token stays valid across inserts and deletes.
pub fn encode_next_token(cursor: &str) -> String {
    ::hex::encode(cursor.as_bytes())
}

/// Decode a `NextToken` produced by [`encode_next_token`]. `None` when the token was
/// not produced by this service (a caller must be refused, not served page one).
pub fn decode_next_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// The cursor key identifying a table across the whole service: `<database>/<table>`.
///
/// `/` cannot occur in either name (both are created through `ResourceCreateAPIName`,
/// `^[a-zA-Z0-9_.-]+$`), so the pair round-trips through [`decode_table_cursor`].
///
/// This joined form is the token's ENCODING only. It must never be compared as a
/// string to decide where a page resumes: `/` (0x2F) sorts above `-` (0x2D) and `.`
/// (0x2E), which both names admit, so `"a/t" > "a-b/t"` while the stored order is
/// `("a", "t") < ("a-b", "t")`. Compare the decoded PAIR instead, which is the order
/// the nested `BTreeMap`s iterate in.
pub fn table_cursor(database_name: &str, table_name: &str) -> String {
    format!("{database_name}/{table_name}")
}

/// Decode a `NextToken` into the `(database name, table name)` pair it names, so a
/// caller's page resumes on the same ordering the store iterates in. `None` when the
/// token was not produced by [`encode_next_token`] over a [`table_cursor`].
pub fn decode_table_cursor(token: &str) -> Option<(String, String)> {
    let cursor = decode_next_token(token)?;
    let (database_name, table_name) = cursor.split_once('/')?;
    Some((database_name.to_string(), table_name.to_string()))
}
