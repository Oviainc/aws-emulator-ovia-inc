// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-quicksight-quicksight-20180401 service.
//!
//! Besides the state struct this module holds the few helpers every QuickSight
//! handler needs — ARN construction, request-id minting, the `NextToken` codec and
//! the model's enum catalogs — so the per-operation files in `src/ops/` share one
//! implementation instead of each growing its own.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ColumnGroup, ColumnLevelPermissionRule, DataPrepConfiguration, DataSet, DataSetSummary,
    DataSetUsageConfiguration, DataSource, DataSourceErrorInfo, DataSourceParameters,
    DatasetParameter, FieldFolder, InputColumn, LogicalTable, OutputColumn,
    PerformanceConfiguration, PhysicalTable, ResourcePermission, RowLevelPermissionDataSet,
    RowLevelPermissionTagConfiguration, SemanticModelConfiguration, SslProperties, Tag,
    VpcConnectionProperties,
};

/// Per-service state for com-amazonaws-quicksight-quicksight-20180401. A handler reaches it through [`service_state`].
///
/// Every map is keyed by the request's `AwsAccountId` first, so a resource created
/// under one account id is invisible under another — the account id is part of the
/// identity of every QuickSight resource, not a filter applied afterwards.
#[derive(Debug, Default)]
pub struct ComAmazonawsQuicksightQuicksight20180401State {
    /// Data sources by `(AwsAccountId, DataSourceId)`.
    pub data_sources: HashMap<(String, String), DataSourceRecord>,
    /// Data sets by `(AwsAccountId, DataSetId)`.
    pub data_sets: HashMap<(String, String), DataSetRecord>,
    /// Ingestions by `(AwsAccountId, DataSetId, IngestionId)`.
    pub ingestions: HashMap<(String, String, String), IngestionRecord>,
}

impl ComAmazonawsQuicksightQuicksight20180401State {
    /// One account's data sources, ordered by id — the order `ListDataSources` pages
    /// through.
    pub fn data_sources_in(&self, aws_account_id: &str) -> Vec<&DataSourceRecord> {
        let mut records: Vec<&DataSourceRecord> = self
            .data_sources
            .iter()
            .filter(|((account, _), _)| account == aws_account_id)
            .map(|(_, record)| record)
            .collect();
        records.sort_by(|left, right| left.data_source_id.cmp(&right.data_source_id));
        records
    }

    /// One account's data sets, ordered by id — the order `ListDataSets` pages through.
    pub fn data_sets_in(&self, aws_account_id: &str) -> Vec<&DataSetRecord> {
        let mut records: Vec<&DataSetRecord> = self
            .data_sets
            .iter()
            .filter(|((account, _), _)| account == aws_account_id)
            .map(|(_, record)| record)
            .collect();
        records.sort_by(|left, right| left.data_set_id.cmp(&right.data_set_id));
        records
    }

    /// How many data sources `aws_account_id` holds — what
    /// [`MAX_DATA_SOURCES_PER_ACCOUNT`] is checked against.
    pub fn data_source_count(&self, aws_account_id: &str) -> usize {
        self.data_sources
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }

    /// How many data sets `aws_account_id` holds — what [`MAX_DATA_SETS_PER_ACCOUNT`]
    /// is checked against.
    pub fn data_set_count(&self, aws_account_id: &str) -> usize {
        self.data_sets
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }

    /// The data source in `aws_account_id` whose ARN is `arn`, if any. A
    /// `PhysicalTable` names its data source by ARN, so resolving a submitted table
    /// means resolving that ARN inside the same account.
    ///
    /// Resolved through the key rather than by scanning: a QuickSight data source ARN
    /// ends in `datasource/<DataSourceId>`, so the id is read off the ARN and the
    /// candidate's own ARN is then compared in full — an ARN that differs anywhere
    /// else (another account, another region, another service) matches nothing.
    pub fn data_source_by_arn(&self, aws_account_id: &str, arn: &str) -> Option<&DataSourceRecord> {
        let data_source_id = arn.rsplit_once("datasource/").map(|(_, id)| id)?;
        self.data_sources
            .get(&(aws_account_id.to_string(), data_source_id.to_string()))
            .filter(|record| record.arn == arn)
    }
}

/// A stored data source.
///
/// `Credentials` are deliberately absent: the member is `@sensitive`, QuickSight
/// never returns it on any read, and nothing this emulator does needs it — so it is
/// dropped when the request is served rather than kept in memory.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    pub arn: String,
    pub data_source_id: String,
    pub name: String,
    pub r#type: String,
    pub status: String,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    pub data_source_parameters: Option<DataSourceParameters>,
    pub vpc_connection_properties: Option<VpcConnectionProperties>,
    pub ssl_properties: Option<SslProperties>,
    pub permissions: Vec<ResourcePermission>,
    pub tags: Vec<Tag>,
    pub error_info: Option<DataSourceErrorInfo>,
}

impl DataSourceRecord {
    /// The modelled `DataSource` structure this record describes.
    pub fn to_data_source(&self) -> DataSource {
        DataSource {
            arn: Some(self.arn.clone()),
            data_source_id: Some(self.data_source_id.clone()),
            name: Some(self.name.clone()),
            r#type: Some(self.r#type.clone()),
            status: Some(self.status.clone()),
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            data_source_parameters: self.data_source_parameters.clone(),
            // Set only by UpdateDataSource, which is still a stub.
            alternate_data_source_parameters: None,
            vpc_connection_properties: self.vpc_connection_properties.clone(),
            ssl_properties: self.ssl_properties.clone(),
            error_info: self.error_info.clone(),
            // Secret-based credentials are a separate feature of CreateDataSource's
            // `Credentials` member (`SecretArn`); nothing stores one yet.
            secret_arn: None,
            credential_status: None,
            last_credential_verified_at: None,
        }
    }
}

/// A stored data set.
#[derive(Debug, Clone)]
pub struct DataSetRecord {
    pub arn: String,
    pub data_set_id: String,
    pub name: String,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    pub import_mode: String,
    pub physical_table_map: HashMap<String, PhysicalTable>,
    pub logical_table_map: Option<HashMap<String, LogicalTable>>,
    pub output_columns: Vec<OutputColumn>,
    pub column_groups: Option<Vec<ColumnGroup>>,
    pub field_folders: Option<HashMap<String, FieldFolder>>,
    pub row_level_permission_data_set: Option<RowLevelPermissionDataSet>,
    pub row_level_permission_tag_configuration: Option<RowLevelPermissionTagConfiguration>,
    pub column_level_permission_rules: Option<Vec<ColumnLevelPermissionRule>>,
    pub data_set_usage_configuration: Option<DataSetUsageConfiguration>,
    pub dataset_parameters: Option<Vec<DatasetParameter>>,
    pub performance_configuration: Option<PerformanceConfiguration>,
    pub use_as: Option<String>,
    pub data_prep_configuration: Option<DataPrepConfiguration>,
    pub semantic_model_configuration: Option<SemanticModelConfiguration>,
    pub permissions: Vec<ResourcePermission>,
    pub tags: Vec<Tag>,
    /// SPICE bytes this data set holds. Nothing has been ingested into an emulated
    /// data set, so it stays 0 until an ingestion loads rows.
    pub consumed_spice_capacity_in_bytes: i64,
}

impl DataSetRecord {
    /// The modelled `DataSet` structure this record describes.
    pub fn to_data_set(&self) -> DataSet {
        DataSet {
            arn: Some(self.arn.clone()),
            data_set_id: Some(self.data_set_id.clone()),
            name: Some(self.name.clone()),
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            physical_table_map: Some(self.physical_table_map.clone()),
            logical_table_map: self.logical_table_map.clone(),
            output_columns: Some(self.output_columns.clone()),
            import_mode: Some(self.import_mode.clone()),
            consumed_spice_capacity_in_bytes: self.consumed_spice_capacity_in_bytes,
            column_groups: self.column_groups.clone(),
            field_folders: self.field_folders.clone(),
            row_level_permission_data_set: self.row_level_permission_data_set.clone(),
            row_level_permission_tag_configuration: self
                .row_level_permission_tag_configuration
                .clone(),
            column_level_permission_rules: self.column_level_permission_rules.clone(),
            data_set_usage_configuration: self.data_set_usage_configuration.clone(),
            dataset_parameters: self.dataset_parameters.clone(),
            performance_configuration: self.performance_configuration.clone(),
            use_as: self.use_as.clone(),
            data_prep_configuration: self.data_prep_configuration.clone(),
            semantic_model_configuration: self.semantic_model_configuration.clone(),
        }
    }

    /// The modelled `DataSetSummary` for this record, as `ListDataSets` reports it.
    pub fn to_summary(&self) -> DataSetSummary {
        DataSetSummary {
            arn: Some(self.arn.clone()),
            data_set_id: Some(self.data_set_id.clone()),
            name: Some(self.name.clone()),
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            import_mode: Some(self.import_mode.clone()),
            row_level_permission_data_set: self.row_level_permission_data_set.clone(),
            row_level_permission_data_set_map: None,
            row_level_permission_tag_configuration_applied: self
                .row_level_permission_tag_configuration
                .is_some(),
            column_level_permission_rules_applied: self
                .column_level_permission_rules
                .as_ref()
                .is_some_and(|rules| !rules.is_empty()),
            use_as: self.use_as.clone(),
        }
    }
}

/// A stored ingestion: the SPICE refresh `CreateDataSet` starts for an
/// `ImportMode=SPICE` data set, so the `IngestionId` it returns names a real object.
#[derive(Debug, Clone)]
pub struct IngestionRecord {
    pub arn: String,
    pub ingestion_id: String,
    /// The status the ingestion was created in. QuickSight starts a SPICE refresh in
    /// a non-terminal status, so [`IngestionRecord::status_at`] is what a reader
    /// reports.
    pub ingestion_status: String,
    pub created_time: DateTime,
    pub request_source: String,
    /// When the clock owes this ingestion its move to `COMPLETED`. Stored rather than
    /// waited on: a reader materialises the transition the clock has already made due.
    pub completes_at: SystemTime,
}

/// How long an emulated SPICE ingestion takes to complete. Chosen (QuickSight
/// publishes no duration): long enough that a caller acting straight after
/// `CreateDataSet` still observes the non-terminal status, short enough to be over by
/// the time anything polls.
pub const INGESTION_DURATION: Duration = Duration::from_millis(500);

impl IngestionRecord {
    /// This ingestion's status as of `now`: the stored non-terminal status until the
    /// completion the clock owes falls due, `COMPLETED` from then on.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now >= self.completes_at {
            INGESTION_STATUS_COMPLETED
        } else {
            &self.ingestion_status
        }
    }
}

/// `IngestionStatus` a freshly created SPICE ingestion carries (model enum
/// `com.amazonaws.quicksight#IngestionStatus`).
pub const INGESTION_STATUS_INITIALIZED: &str = "INITIALIZED";
/// The terminal `IngestionStatus` of an ingestion that loaded its rows.
pub const INGESTION_STATUS_COMPLETED: &str = "COMPLETED";
/// `IngestionRequestSource` for an ingestion a caller's own request started, as
/// opposed to one a refresh schedule started.
pub const INGESTION_REQUEST_SOURCE_MANUAL: &str = "MANUAL";

/// `ResourceStatus` of a resource whose creation succeeded (model enum
/// `com.amazonaws.quicksight#ResourceStatus`).
pub const RESOURCE_STATUS_CREATION_SUCCESSFUL: &str = "CREATION_SUCCESSFUL";

/// The import mode whose data set QuickSight loads into SPICE, and so the one whose
/// creation starts an ingestion.
pub const IMPORT_MODE_SPICE: &str = "SPICE";

/// The page size a list operation uses when the request names no `MaxResults`: the
/// model's own ceiling for the member (`range 1..100`), so an unpaginated caller sees
/// everything a single page can hold.
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// How many data sources one account may hold before `CreateDataSource` refuses with
/// the modelled `LimitExceededException`.
///
/// This is the emulator's own ceiling, of the same order as QuickSight's published
/// per-account data source quota. A caller that frees a slot with `DeleteDataSource`
/// can create again. It is a FIDELITY limit only: the account id is request-supplied,
/// so what bounds this process is [`MAX_DATA_SOURCES`].
pub const MAX_DATA_SOURCES_PER_ACCOUNT: usize = 1_000;

/// How many data sets one account may hold before `CreateDataSet` refuses with the
/// modelled `LimitExceededException`. Same standing as
/// [`MAX_DATA_SOURCES_PER_ACCOUNT`]: a fidelity limit, not the process bound.
pub const MAX_DATA_SETS_PER_ACCOUNT: usize = 1_000;

/// The TOTAL number of data sources this emulator holds, across every account.
///
/// This is the bound that holds for the process: both halves of the store key come
/// from the request — the account id is an `@httpLabel` the caller chooses — so a
/// per-account ceiling alone would be reset by varying the account id. A create loop
/// stops here however it varies its keys.
pub const MAX_DATA_SOURCES: usize = 10_000;

/// The TOTAL number of data sets this emulator holds, across every account — the
/// process bound, for the same reason as [`MAX_DATA_SOURCES`].
pub const MAX_DATA_SETS: usize = 10_000;

/// The TOTAL number of ingestions this emulator holds, across every account.
///
/// `CreateDataSet` is the only operation that inserts one, and it inserts at most one
/// per data set, so this ceiling is reached only if the data-set ceiling is; it is
/// checked explicitly all the same, so the bound on this map does not rest on
/// reasoning about another one.
pub const MAX_INGESTIONS: usize = 10_000;

/// The region of the resources a request addresses: the region of its SigV4
/// credential scope, which is the region the client signed for.
///
/// Falls back to `us-east-1` — the emulator's default region — for an unsigned or
/// unparseable scope, so an anonymous caller still gets a well-formed ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// A QuickSight ARN: `arn:aws:quicksight:<region>:<account>:<resource>`.
///
/// The account segment is the `AwsAccountId` the request names — every QuickSight
/// operation takes it as a required, pattern-validated `@httpLabel`, and it is the
/// account that owns the resource, so two accounts' resources never share an ARN. A
/// client signed with this emulator's own credentials presents
/// [`::engine::emulator_account_id`] there.
pub fn quicksight_arn(region: &str, aws_account_id: &str, resource: &str) -> String {
    format!("arn:aws:quicksight:{region}:{aws_account_id}:{resource}")
}

/// The ARN of a data source: `.../datasource/<DataSourceId>`.
pub fn data_source_arn(region: &str, aws_account_id: &str, data_source_id: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("datasource/{data_source_id}"))
}

/// The ARN of a data set: `.../dataset/<DataSetId>`.
pub fn data_set_arn(region: &str, aws_account_id: &str, data_set_id: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("dataset/{data_set_id}"))
}

/// The ARN of one data set's ingestion: `.../dataset/<DataSetId>/ingestion/<IngestionId>`.
pub fn ingestion_arn(
    region: &str,
    aws_account_id: &str,
    data_set_id: &str,
    ingestion_id: &str,
) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("dataset/{data_set_id}/ingestion/{ingestion_id}"),
    )
}

/// The source of every server-minted id in this service.
///
/// Process-wide rather than a field of the state struct on purpose: a reporting
/// operation also puts a `RequestId` in its response, and a read must not write to
/// the store it reports on.
static MINTED_IDS: AtomicU64 = AtomicU64::new(1);

/// A fresh id, unique for the life of the process.
fn next_minted_id() -> u64 {
    MINTED_IDS.fetch_add(1, Ordering::Relaxed)
}

/// `n` rendered as a well-formed RFC 4122 UUID string (version 4, variant 1) — the
/// shape AWS request and ingestion ids have. The value is the monotonic counter, not
/// randomness, so it is unique without being derived from the request.
fn uuid_shaped(n: u64) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// The `RequestId` to report for one response.
pub fn next_request_id() -> String {
    uuid_shaped(next_minted_id())
}

/// The `IngestionId` for a newly started ingestion. Matches the model's
/// `IngestionId` pattern (`^[a-zA-Z0-9-_]+$`, 1..128).
pub fn next_ingestion_id() -> String {
    uuid_shaped(next_minted_id())
}

/// `SystemTime::now` as the modelled timestamp type.
pub fn now_timestamp(now: SystemTime) -> DateTime {
    let millis = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// Prefix of this service's `NextToken`s, so a token from somewhere else is rejected
/// rather than silently read as a position.
const PAGE_TOKEN_PREFIX: &str = "qs1-";

/// A `NextToken` resuming after the id `after`.
///
/// The token carries the last id of the page just returned, so the next page is the
/// ids ordered after it: inserting or deleting another resource between pages cannot
/// make a caller skip or repeat an item the way an index-based token would.
pub fn encode_page_token(after: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", ::hex::encode(after))
}

/// The id a `NextToken` resumes after, or `None` when the token is not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    String::from_utf8(::hex::decode(body).ok()?).ok()
}

/// One page of `items` (ordered by the id `id_of` reads), and the `NextToken` to read
/// the page after it.
///
/// `max_results` is the page size the request asked for; the model constrains it to
/// 1..100 before a handler sees it, and an absent one means [`DEFAULT_MAX_RESULTS`].
pub fn page_of<'a, T>(
    items: &[&'a T],
    id_of: impl Fn(&T) -> &str,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<&'a T>, Option<String>) {
    let start = match after {
        // Resume *after* the recorded id. `partition_point` also does the right thing
        // when that id has since been deleted: the page continues from the next id in
        // order rather than restarting.
        Some(after) => items.partition_point(|item| id_of(item) <= after),
        None => 0,
    };
    let page_size = max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .filter(|requested| *requested > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS);
    let end = start.saturating_add(page_size).min(items.len());
    let page: Vec<&'a T> = items.get(start..end).unwrap_or_default().to_vec();
    let next_token = match page.last() {
        Some(last) if end < items.len() => Some(encode_page_token(id_of(last))),
        _ => None,
    };
    (page, next_token)
}

/// The ARN of the data source a `PhysicalTable` reads, whichever kind of table it is.
pub fn physical_table_data_source_arn(table: &PhysicalTable) -> &str {
    match table {
        PhysicalTable::RelationalTable(table) => &table.data_source_arn,
        PhysicalTable::CustomSql(table) => &table.data_source_arn,
        PhysicalTable::S3Source(table) => &table.data_source_arn,
        PhysicalTable::SaaSTable(table) => &table.data_source_arn,
        PhysicalTable::FileSource(table) => &table.data_source_arn,
    }
}

/// The columns a `PhysicalTable` presents, whichever kind of table it is.
pub fn physical_table_input_columns(table: &PhysicalTable) -> &[InputColumn] {
    match table {
        PhysicalTable::RelationalTable(table) => &table.input_columns,
        PhysicalTable::CustomSql(table) => table.columns.as_deref().unwrap_or(&[]),
        PhysicalTable::S3Source(table) => &table.input_columns,
        PhysicalTable::SaaSTable(table) => &table.input_columns,
        PhysicalTable::FileSource(table) => &table.input_columns,
    }
}

/// The columns a data set projects, derived from the tables it is built on.
///
/// A data set's output columns are the columns of its physical tables: taken in
/// physical-table-key order, each table's columns in the order the table declares
/// them, and de-duplicated by name (one column per name, as a projection has). The
/// transforms a `LogicalTableMap` can apply on top are stored and returned verbatim
/// but are not folded in here; that is `UpdateDataSet`'s and the transform
/// operations' own gap.
pub fn derive_output_columns(
    physical_table_map: &HashMap<String, PhysicalTable>,
) -> Vec<OutputColumn> {
    let mut table_keys: Vec<&String> = physical_table_map.keys().collect();
    table_keys.sort();

    let mut columns: Vec<OutputColumn> = Vec::new();
    // De-duplication is a hash lookup per column, so the work is linear in the total
    // number of columns the request carries rather than quadratic in it.
    let mut seen: HashSet<&str> = HashSet::new();
    for key in table_keys {
        let Some(table) = physical_table_map.get(key) else {
            continue;
        };
        for column in physical_table_input_columns(table) {
            if !seen.insert(column.name.as_str()) {
                continue;
            }
            columns.push(OutputColumn {
                name: Some(column.name.clone()),
                id: column.id.clone(),
                // `InputColumn` carries no description; QuickSight only has one once
                // a transform or UpdateDataSet sets it.
                description: None,
                r#type: Some(column.r#type.clone()),
                sub_type: column.sub_type.clone(),
            });
        }
    }
    columns
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsQuicksightQuicksight20180401State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsQuicksightQuicksight20180401State>)
}
